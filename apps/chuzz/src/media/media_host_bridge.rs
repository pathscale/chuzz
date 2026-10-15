use std::sync::Arc;
use std::task::Waker;
use std::time::Instant;

use serde_json::{Value, json};

use crate::script_host::{
    ChuzzScriptHost, HostCallbackId, HostError, HostInstallContext, HostObject, HostValue,
    from_json, string_arg,
};

const STATE_KEY: &str = "chuzz.media";

struct NativeHost(Arc<super::Host>);

impl Drop for NativeHost {
    fn drop(&mut self) {
        self.0.close();
    }
}

fn host_error(name: &str, message: impl Into<String>) -> HostError {
    HostError::new(name, message)
}

fn native_host(host: &dyn ChuzzScriptHost) -> Result<Arc<super::Host>, HostError> {
    let object = host
        .state(STATE_KEY)
        .ok_or_else(|| host_error("InvalidStateError", "capture host is unavailable"))?;
    object
        .downcast_ref::<NativeHost>()
        .map(|native| Arc::clone(&native.0))
        .ok_or_else(|| host_error("InvalidStateError", "capture host state is invalid"))
}

pub fn capture_host(host: &dyn ChuzzScriptHost) -> Result<Arc<super::Host>, HostError> {
    native_host(host)
}

fn callback_id(value: Option<&HostValue>) -> Result<HostCallbackId, HostError> {
    let Some(HostValue::Object(object)) = value else {
        return Err(host_error("TypeError", "a media callback root is required"));
    };
    object
        .downcast_ref::<HostCallbackId>()
        .cloned()
        .ok_or_else(|| host_error("TypeError", "invalid media callback root"))
}

fn replace_callback(
    host: &mut dyn ChuzzScriptHost,
    key: &'static str,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    let id = callback_id(args.first())?;
    let previous = host.state(key);
    host.set_state(key, HostObject::new(id))?;
    if let Some(previous) = previous
        && let Some(previous) = previous.downcast_ref::<HostCallbackId>()
    {
        host.release_callback(previous.clone())?;
    }
    Ok(HostValue::Undefined)
}

fn control(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let native = native_host(host)?;
    let text = string_arg(args, 0)?;
    let resources = NativeAudioResources { host };
    let result = if text.len() > super::COMMAND_LIMIT {
        Err(super::Error::typ("media command exceeds 64 KiB"))
    } else {
        serde_json::from_str::<Value>(&text)
            .map_err(|_| super::Error::typ("invalid media command"))
            .and_then(|request| native.control(request, &resources))
    };
    let updates = native
        .updates()
        .unwrap_or_else(|_| json!({"devicechange": false, "tracks": []}));
    let reply = match result {
        Ok(value) => json!({"ok": true, "value": value, "updates": updates}),
        Err(error) => json!({"ok": false, "error": error, "updates": updates}),
    };
    Ok(HostValue::String(reply.to_string().into()))
}

struct NativeAudioResources<'a> {
    host: &'a dyn ChuzzScriptHost,
}

impl super::AudioResources for NativeAudioResources<'_> {
    fn track(&self, id: u64) -> Result<Arc<crate::audio::pcm::Track>, String> {
        crate::audio::native_host::registered_track(self.host, id).map_err(|error| error.message)
    }

    fn register(&self, track: Arc<crate::audio::pcm::Track>) -> Result<u64, String> {
        crate::audio::native_host::register_track(self.host, track).map_err(|error| error.message)
    }
}

fn dispatch_update(host: &mut dyn ChuzzScriptHost, updates: &Value) -> Result<(), HostError> {
    let Some(object) = host.state("chuzz.media.callback") else {
        return Ok(());
    };
    let Some(callback) = object.downcast_ref::<HostCallbackId>() else {
        return Err(host_error(
            "InvalidStateError",
            "media callback state is invalid",
        ));
    };
    host.dispatch_callback(callback.clone(), &[from_json(updates.clone())])?;
    Ok(())
}

pub fn poll(host: &mut dyn ChuzzScriptHost, _waker: Option<&Waker>) -> Result<bool, HostError> {
    let native = native_host(host)?;
    let updates = native
        .updates()
        .map_err(|error| host_error(error.name, error.message))?;
    let changed = updates["devicechange"] == true
        || updates["tracks"]
            .as_array()
            .is_some_and(|tracks| !tracks.is_empty());
    if changed {
        dispatch_update(host, &updates)?;
    }
    Ok(changed)
}

pub fn install(
    host: &mut dyn ChuzzScriptHost,
    config: &HostInstallContext,
) -> Result<(), HostError> {
    let url = config.base.as_ref().map(ToString::to_string);
    let native = Arc::new(super::Host {
        store: std::sync::Mutex::new(super::Store::default()),
        origin: Instant::now(),
        secure: super::trusted(url.as_deref()),
    });
    host.set_state(STATE_KEY, HostObject::new(NativeHost(native)))?;
    host.register_intrinsic("__chuzzMediaControl", 1, control)?;
    host.register_intrinsic("__chuzzMediaSubscribe", 1, |host, args| {
        replace_callback(host, "chuzz.media.callback", args)
    })?;
    Ok(())
}
