use std::sync::Condvar;

use blitz_dom::node::RasterImageData;
use blitz_script::boa_engine::object::builtins::{JsArray, JsUint8ClampedArray};
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
};

use super::external::Source;
use super::*;

type ImageResult = Result<Arc<RasterImageData>, GpuError>;

#[derive(Default)]
pub(super) struct Completion {
    result: Mutex<Option<ImageResult>>,
    wake: Condvar,
}

impl Completion {
    pub(super) fn finish(&self, result: ImageResult) {
        let mut current = lock(&self.result);
        if current.is_none() {
            *current = Some(result);
            self.wake.notify_all();
        }
    }

    fn wait(&self) -> ImageResult {
        let (mut result, _) = self
            .wake
            .wait_timeout_while(lock(&self.result), DEADLINE, |result| result.is_none())
            .unwrap_or_else(|poison| poison.into_inner());
        result
            .take()
            .unwrap_or_else(|| Err(GpuError::internal("canvas readback timed out in chuzz")))
    }
}

#[derive(Clone)]
struct CanvasHost(Bridge);

fn argument(args: &[JsValue], index: usize) -> JsValue {
    args.get(index).cloned().unwrap_or_default()
}

fn read(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let bridge = context
        .get_data::<CanvasHost>()
        .ok_or_else(|| JsNativeError::typ().with_message("canvas host is unavailable"))?
        .0
        .clone();
    let source = argument(args, 0)
        .to_string(context)?
        .to_std_string_escaped();
    let source: Source = serde_json::from_str(&source)
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
    if !matches!(
        &source,
        // Texture: a WebGPU canvas read in the same task as its render(), before the frame is presented.
        Source::Canvas { .. } | Source::Element { .. } | Source::Bitmap { .. } | Source::Texture { .. }
    ) {
        return Err(JsNativeError::typ()
            .with_message("invalid synchronous canvas image source")
            .into());
    }
    let id = argument(args, 1).to_number(context)?;
    if !id.is_finite() || id.fract() != 0.0 || !(1.0..=9_007_199_254_740_991.0).contains(&id) {
        return Err(JsNativeError::typ().with_message("invalid canvas readback handle").into());
    }
    let id = id as u64;
    let incoming = {
        let mut state = bridge.state();
        if state.incoming.len() + state.in_flight.len() >= REQUEST_LIMIT
            || state.in_flight.contains_key(&id)
            || state.incoming.iter().any(|request| request.id() == id)
        {
            return Err(JsNativeError::typ()
                .with_message("canvas readback request limit reached in chuzz")
                .into());
        }
        std::mem::take(&mut state.incoming)
    };
    let snapshot = |marker: u64| {
        blitz_script::raster_image_for_attribute(
            context,
            "data-chuzz-gpu-source",
            &marker.to_string(),
        )
        .map(Arc::new)
    };
    for request in incoming {
        bridge.prepare_image_request(request, &snapshot);
    }
    let completion = Arc::new(Completion::default());
    bridge.prepare_image_request(
        Request::Readback {
            id,
            source,
            completion: Arc::clone(&completion),
        },
        snapshot,
    );

    // All preceding GPU and image work stays on the ordered Nagoya worker.
    // No DOM or Boa borrow crosses this wait, and no page jobs run reentrantly.
    match completion.wait() {
        Ok(image) => {
            // Boa's returned array is mutable page memory. Make one native
            // typed-array allocation here, sharing the Arc everywhere else.
            let data = JsUint8ClampedArray::from_iter(image.data.data().iter().copied(), context)?;
            Ok(JsArray::from_iter(
                [
                    JsValue::from(image.width),
                    JsValue::from(image.height),
                    data.into(),
                ],
                context,
            )
            .into())
        }
        Err(error) => Ok(JsArray::from_iter(
            [
                JsString::from(error.kind).into(),
                JsString::from(error.message).into(),
            ],
            context,
        )
        .into()),
    }
}

impl Bridge {
    pub(crate) fn install_canvas_readback(&self, document: &mut blitz_script::ScriptDocument) {
        super::canvas2d::install(document, self.clone());
        document.with_js_context(|context| {
            context.insert_data(CanvasHost(self.clone()));
            context
                .register_global_builtin_callable(
                    JsString::from("__chuzzCanvasReadback"),
                    2,
                    NativeFunction::from_fn_ptr(read),
                )
                .expect("canvas readback bridge");
        });
    }
}

