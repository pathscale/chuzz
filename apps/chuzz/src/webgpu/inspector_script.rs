use super::*;

fn inspector_bridge(document: &mut blitz_script::ScriptDocument) -> Option<Bridge> {
    document.with_js_context(|context| {
        context
            .get_data::<InspectorHost>()
            .map(|host| host.0.clone())
    })
}

fn unsupported() -> DebugError {
    DebugError {
        code: "unsupported".to_owned(),
        message: "the document has no page WebGPU inspector".to_owned(),
    }
}

pub(super) fn install(bridge: &Bridge, document: &mut blitz_script::ScriptDocument) {
    document.with_js_context(|context| {
        context.insert_data(InspectorHost(bridge.clone()));
    });
}

pub(super) fn inspect_document(
    document: &mut blitz_script::ScriptDocument,
) -> Result<WebGpuReport, DebugError> {
    inspector_bridge(document)
        .map(|bridge| bridge.inspect())
        .ok_or_else(unsupported)
}

pub(super) fn capture_document(
    document: &mut blitz_script::ScriptDocument,
    target: &str,
) -> Result<CapturedImage, DebugError> {
    let bridge = inspector_bridge(document).ok_or_else(unsupported)?;
    // Commands still sitting in `incoming` have to be ordered ahead of the copy.
    bridge.prepare_images(document);
    bridge.capture_and_wait(target)
}

pub(super) fn buffer_document(
    document: &mut blitz_script::ScriptDocument,
    target: &str,
    offset: u64,
    length: Option<u64>,
) -> Result<blitz_control_protocol::WebGpuBufferSnapshot, DebugError> {
    let bridge = inspector_bridge(document).ok_or_else(unsupported)?;
    bridge.inspection_session().renew();
    // Drain incoming page writes ahead of the diagnostics request. The worker
    // fills a native slot and this document turn immediately returns pending.
    bridge.prepare_images(document);
    bridge.sample_buffer(target, offset, length)
}

pub(super) fn buffer_document_reply(
    document: &mut blitz_script::ScriptDocument,
    target: &str,
    offset: u64,
    length: Option<u64>,
    reply: Arc<blitz_control_protocol::latest::Once<blitz_control_protocol::DebugResponse>>,
) {
    let Some(bridge) = inspector_bridge(document) else {
        reply.fill(blitz_control_protocol::DebugResponse::Error(unsupported()));
        return;
    };
    bridge.inspection_session().renew();
    bridge.prepare_images(document);
    bridge.dispatch_buffer(target, offset, length, reply);
}
