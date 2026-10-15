use std::sync::Arc;
use std::time::Instant;

use blitz_script::ScriptDocument;
use blitz_script::boa_engine::object::builtins::{AlignedVec, JsArrayBuffer};
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction, Source,
};

use super::pcm::Track;
use super::{Page, execute};

#[derive(Clone)]
struct PageHost(Arc<Page>);

fn native_error(message: String) -> blitz_script::boa_engine::JsError {
    JsNativeError::typ().with_message(message).into()
}

fn page(context: &Context) -> JsResult<Arc<Page>> {
    context
        .get_data::<PageHost>()
        .map(|host| Arc::clone(&host.0))
        .ok_or_else(|| native_error("InvalidStateError: audio host is unavailable".to_owned()))
}

/// Capture hosts register their shared PCM track, then attach the returned
/// identifier to the JS track through __chuzzAudioAttachTrack.
pub fn register_track(context: &Context, track: Arc<Track>) -> JsResult<u64> {
    page(context)?.register_track(track).map_err(native_error)
}

/// Lease a graph output for the media host without copying its PCM history.
pub(crate) fn registered_track(context: &Context, id: u64) -> JsResult<Arc<Track>> {
    page(context)?.track(id).map_err(native_error)
}

/// The same timestamp origin must be used for capture and decoded PCM.
pub fn media_epoch(context: &Context) -> JsResult<Instant> {
    Ok(page(context)?.epoch)
}

fn command(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let page = page(context)?;
    let message = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    if message.len() > 65_536 {
        return Err(native_error(
            "QuotaExceededError: audio command limit".to_owned(),
        ));
    }
    let command = serde_json::from_str(&message)
        .map_err(|error| native_error(format!("TypeError: invalid audio command: {error}")))?;
    let value = execute(&page, command).map_err(native_error)?;
    JsValue::from_json(&value, context)
}

fn integer(args: &[JsValue], index: usize, context: &mut Context) -> JsResult<u64> {
    let value = args
        .get(index)
        .cloned()
        .unwrap_or_default()
        .to_number(context)?;
    if !value.is_finite() || value <= 0.0 || value.fract() != 0.0 || value > 9_007_199_254_740_991.0
    {
        return Err(native_error(
            "TypeError: invalid audio identifier".to_owned(),
        ));
    }
    Ok(value as u64)
}

fn read(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let page = page(context)?;
    let context_id = integer(args, 0, context)?;
    let node = integer(args, 1, context)?;
    let kind = args
        .get(2)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let bytes = super::pcm::lock(&page.engine(context_id).map_err(native_error)?.graph)
        .analysis_bytes(node, &kind)
        .map_err(native_error)?;
    // This owned readback is constructed on the document thread. No Boa
    // allocation or borrow is retained by the render task.
    Ok(JsArrayBuffer::from_byte_block(AlignedVec::from_iter(0, bytes), context)?.into())
}

pub(crate) fn install(document: &mut ScriptDocument) {
    let epoch = document.with_js_context(|context| {
        crate::media::host(context)
            .map(|host| host.media_epoch())
            .unwrap_or_else(|_| Instant::now())
    });
    let page = Arc::new(Page {
        epoch,
        state: std::sync::Mutex::new(super::State::default()),
        events: std::sync::Mutex::new(std::collections::VecDeque::new()),
        reader: std::sync::Mutex::new(None),
    });
    document.with_js_context(|context| {
        context.insert_data(PageHost(Arc::clone(&page)));
        context
            .register_global_builtin_callable(
                JsString::from("__chuzzAudioCall"),
                1,
                NativeFunction::from_fn_ptr(command),
            )
            .expect("audio command bridge");
        context
            .register_global_builtin_callable(
                JsString::from("__chuzzAudioRead"),
                3,
                NativeFunction::from_fn_ptr(read),
            )
            .expect("audio analyser bridge");
        context
            .eval(Source::from_bytes(include_str!("../audio.ts")))
            .expect("Web Audio bootstrap");
    });
    document.add_poll_hook(move |document, task_context| {
        // Register before draining so an event racing the drain cannot lose
        // the document wakeup.
        if let Some(context) = task_context {
            *super::pcm::lock(&page.reader) = Some(context.waker().clone());
        }
        let events: Vec<_> = {
            let mut queue = super::pcm::lock(&page.events);
            let count = queue.len().min(64);
            let events = queue.drain(..count).collect();
            if !queue.is_empty()
                && let Some(reader) = super::pcm::lock(&page.reader).as_ref()
            {
                reader.wake_by_ref();
            }
            events
        };
        if events.is_empty() {
            return false;
        }
        document.with_js_context(|context| {
            let result = (|| -> JsResult<()> {
                let callback = context
                    .global_object()
                    .get(JsString::from("__chuzzAudioEnded"), context)?
                    .as_object()
                    .filter(|object| object.is_callable())
                    .ok_or_else(|| JsNativeError::typ().with_message("missing audio event hook"))?;
                for (context_id, node) in &events {
                    callback.call(
                        &JsValue::undefined(),
                        &[
                            JsValue::from(*context_id as f64),
                            JsValue::from(*node as f64),
                        ],
                        context,
                    )?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                eprintln!("Uncaught audio event handler error: {error}");
            }
        });
        true
    });
}
