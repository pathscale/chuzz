use std::sync::Arc;

use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
};

use super::{FrameLease, Frames, MAX_HANDLE};

struct FrameHost(Arc<Frames>);

impl Drop for FrameHost {
    fn drop(&mut self) {
        self.0.close();
    }
}

pub(crate) fn frame_registry(context: &mut Context) -> Arc<Frames> {
    if let Some(registry) = context.get_data::<FrameHost>() {
        return Arc::clone(&registry.0);
    }
    let frames = Arc::new(Frames::default());
    context.insert_data(FrameHost(Arc::clone(&frames)));
    frames
}

pub(crate) fn take_frame(context: &Context, handle: u64) -> Result<FrameLease, String> {
    context
        .get_data::<FrameHost>()
        .ok_or("native video registry is unavailable")?
        .0
        .take(handle)
}

fn acquire(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let mut numbers = [0_u64; 2];
    for (index, number) in numbers.iter_mut().enumerate() {
        let value = args
            .get(index)
            .cloned()
            .unwrap_or_default()
            .to_number(context)?;
        if !value.is_finite() || value.fract() != 0.0 || !(0.0..=MAX_HANDLE as f64).contains(&value)
        {
            return Err(JsNativeError::typ()
                .with_message("invalid video handle")
                .into());
        }
        *number = value as u64;
    }
    let descriptor = frame_registry(context)
        .acquire(numbers[0], numbers[1])
        .map_err(|error| JsNativeError::typ().with_message(error))?;
    let json = serde_json::to_string(&descriptor)
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
    Ok(JsString::from(json).into())
}

pub(crate) fn install_frame_bridge(context: &mut Context) {
    context
        .register_global_builtin_callable(
            JsString::from("__chuzzVideoAcquire"),
            2,
            NativeFunction::from_fn_ptr(acquire),
        )
        .expect("native video frame leases");
}
