use std::sync::Arc;

use blitz_script::ScriptDocument;
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
};
use serde_json::{Value, json};

use super::{AudioResources, Error, Host};
use super::{Store, trusted};

struct NativeHost(Arc<Host>);

impl Drop for NativeHost {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Obtain the page's native capture registry without passing samples through JS.
pub fn host(context: &Context) -> JsResult<Arc<Host>> {
    context
        .get_data::<NativeHost>()
        .map(|host| Arc::clone(&host.0))
        .ok_or_else(|| {
            JsNativeError::typ()
                .with_message("capture host is unavailable")
                .into()
        })
}

struct ContextAudio<'a>(&'a Context);

impl AudioResources for ContextAudio<'_> {
    fn track(&self, id: u64) -> Result<Arc<crate::audio::pcm::Track>, String> {
        crate::audio::registered_track(self.0, id).map_err(|error| error.to_string())
    }

    fn register(&self, track: Arc<crate::audio::pcm::Track>) -> Result<u64, String> {
        crate::audio::register_track(self.0, track).map_err(|error| error.to_string())
    }
}

fn control(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let text = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let audio = ContextAudio(context);
    let result = if text.len() > super::COMMAND_LIMIT {
        Err(Error::typ("media command exceeds 64 KiB"))
    } else {
        serde_json::from_str::<Value>(&text)
            .map_err(|_| Error::typ("invalid media command"))
            .and_then(|request| host.control(request, &audio))
    };
    let updates = host
        .updates()
        .unwrap_or(json!({"devicechange": false, "tracks": []}));
    let reply = match result {
        Ok(value) => json!({"ok": true, "value": value, "updates": updates}),
        Err(error) => json!({"ok": false, "error": error, "updates": updates}),
    };
    Ok(JsString::from(reply.to_string()).into())
}

pub(crate) fn install(document: &mut ScriptDocument) {
    let url = document.current_url().map(ToString::to_string);
    let host = Arc::new(Host {
        store: std::sync::Mutex::new(Store::default()),
        origin: std::time::Instant::now(),
        secure: trusted(url.as_deref()),
    });
    document.with_js_context(|context| {
        context.insert_data(NativeHost(Arc::clone(&host)));
        context
            .register_global_builtin_callable(
                JsString::from("__chuzzMediaControl"),
                1,
                NativeFunction::from_fn_ptr(control),
            )
            .expect("capture metadata bridge");
    });
    document.eval_named(include_str!("../media.ts"), "<chuzz-media>");
    document.add_poll_hook(move |document, _| {
        let Ok(updates) = host.updates() else {
            return false;
        };
        if updates["devicechange"] != true && updates["tracks"].as_array().is_none_or(Vec::is_empty)
        {
            return false;
        }
        document.eval(&format!("globalThis.__chuzzMediaUpdate({updates});"));
        true
    });
}
