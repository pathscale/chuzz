use blitz_script::ScriptDocument;
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
};

fn probe(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let json = super::qa::run_fixture_json()
        .map_err(|error| JsNativeError::error().with_message(error))?;
    Ok(JsString::from(json).into())
}

pub(super) fn install(document: &mut ScriptDocument) {
    document.with_js_context(|context| {
        context
            .register_global_builtin_callable(
                JsString::from("__chuzzWebrtcCryptoProbe"),
                0,
                NativeFunction::from_fn_ptr(probe),
            )
            .expect("WebRTC crypto native fixture bridge");
    });
}
