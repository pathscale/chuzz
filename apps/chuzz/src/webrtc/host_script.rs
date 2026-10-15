use std::sync::Arc;

use blitz_script::ScriptDocument;
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
};
use serde_json::{Value, json};

use crate::audio::pcm::Track;
use crate::webrtc::host::{self, NativeHost, Page, Resources};

struct ContextResources<'a>(&'a Context);

impl Resources for ContextResources<'_> {
    fn media_epoch(&self) -> Result<std::time::Instant, String> {
        crate::audio::media_epoch(self.0).map_err(|error| error.to_string())
    }

    fn media(&self) -> Result<Arc<crate::media::Host>, String> {
        crate::media::host(self.0).map_err(|error| error.to_string())
    }

    fn register_audio(&self, track: Arc<Track>) -> Result<u64, String> {
        crate::audio::register_track(self.0, track).map_err(|error| error.to_string())
    }

    fn audio(&self, id: u64) -> Result<Arc<Track>, String> {
        crate::audio::registered_track(self.0, id).map_err(|error| error.to_string())
    }
}

fn control(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let page = context
        .get_data::<NativeHost>()
        .map(|host| Arc::clone(&host.0))
        .ok_or_else(|| JsNativeError::typ().with_message("peer host is unavailable"))?;
    let text = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let result = if text.len() > 192 * 1024 {
        Err("OperationError: peer command exceeds 192 KiB".into())
    } else {
        serde_json::from_str::<Value>(&text)
            .map_err(|_| "TypeError: invalid peer command".to_owned())
            .and_then(|data| host::execute(&page, data, &ContextResources(context)))
    };
    let reply = match result {
        Ok(value) => json!({"ok": true, "value": value}),
        Err(error) => json!({"ok": false, "error": error}),
    };
    Ok(JsString::from(reply.to_string()).into())
}

pub(crate) fn install(document: &mut ScriptDocument) {
    let page = Arc::new(Page {
        state: std::sync::Mutex::new(Default::default()),
    });
    document.with_js_context(|context| {
        context.insert_data(NativeHost(Arc::clone(&page)));
        context
            .register_global_builtin_callable(
                JsString::from("__chuzzPeerControl"),
                1,
                NativeFunction::from_fn_ptr(control),
            )
            .expect("peer metadata bridge");
    });
    document.eval_named(include_str!("../webrtc.ts"), "<chuzz-webrtc>");
    document.add_poll_hook(move |document, task_context| {
        let events: Vec<_> = {
            let mut state = host::lock(&page.state);
            if let Some(context) = task_context {
                state.reader = Some(context.waker().clone());
            }
            let count = state.events.len().min(64);
            let events = state.events.drain(..count).collect();
            if !state.events.is_empty()
                && let Some(reader) = &state.reader
            {
                reader.wake_by_ref();
            }
            events
        };
        if events.is_empty() {
            return false;
        }
        document.eval(&format!("globalThis.__chuzzPeerEvents({});", json!(events)));
        true
    });
}
