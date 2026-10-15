use std::sync::Arc;

use blitz_script::boa_engine::object::builtins::{JsArray, JsUint8ClampedArray};
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
};

use super::super::Bridge;
use super::super::external::Source;
use super::{ImageResult, read_image};

#[derive(Clone)]
struct CanvasHost(Bridge);

fn argument(args: &[JsValue], index: usize) -> JsValue {
    args.get(index).cloned().unwrap_or_default()
}

/// Compatibility entry point retained for canvas2d and other sibling modules.
pub(crate) fn image(
    bridge: &Bridge,
    source: Source,
    id: u64,
    context: &mut Context,
) -> JsResult<ImageResult> {
    read_image(bridge, source, id, |marker| {
        blitz_script::raster_image_for_attribute(
            context,
            "data-chuzz-gpu-source",
            &marker.to_string(),
        )
        .map(Arc::new)
    })
    .map_err(|error| JsNativeError::typ().with_message(error.message).into())
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
    let id = argument(args, 1).to_number(context)?;
    if !id.is_finite() || id.fract() != 0.0 || !(1.0..=9_007_199_254_740_991.0).contains(&id) {
        return Err(JsNativeError::typ()
            .with_message("invalid canvas readback handle")
            .into());
    }
    match image(&bridge, source, id as u64, context)? {
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

pub(super) fn install(bridge: &Bridge, document: &mut blitz_script::ScriptDocument) {
    super::super::canvas2d::install(document, bridge.clone());
    document.with_js_context(|context| {
        bridge.state().media_frames = crate::video::frame_registry(context);
        context.insert_data(CanvasHost(bridge.clone()));
        context
            .register_global_builtin_callable(
                JsString::from("__chuzzCanvasReadback"),
                2,
                NativeFunction::from_fn_ptr(read),
            )
            .expect("canvas readback bridge");
    });
}
