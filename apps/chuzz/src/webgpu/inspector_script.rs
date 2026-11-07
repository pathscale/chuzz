use serde::de::DeserializeOwned;

use crate::page_document::PageDocument;
use crate::script_host::{HostValue, to_json};

use super::*;

fn host_error(error: crate::script_host::HostError) -> DebugError {
    DebugError {
        code: error.name,
        message: error.message,
    }
}

fn decode<T: DeserializeOwned>(value: HostValue) -> Result<T, DebugError> {
    let value = to_json(&value).map_err(host_error)?;
    serde_json::from_value(value).map_err(|error| DebugError {
        code: "internal".to_owned(),
        message: format!("invalid native WebGPU diagnostic response: {error}"),
    })
}

fn invoke<T: DeserializeOwned>(
    document: &mut PageDocument,
    name: &str,
    args: &[HostValue],
) -> Result<T, DebugError> {
    let value = document
        .host_mut()
        .invoke_intrinsic(name, args)
        .map_err(host_error)?;
    decode(value)
}

pub(super) fn inspect_document(document: &mut PageDocument) -> Result<WebGpuReport, DebugError> {
    invoke(document, "__chuzzGpuInspect", &[])
}

pub(super) fn capture_document(
    document: &mut PageDocument,
    target: &str,
) -> Result<CapturedImage, DebugError> {
    invoke(
        document,
        "__chuzzGpuCapture",
        &[HostValue::String(target.to_owned().into())],
    )
}

pub(super) fn buffer_document(
    document: &mut PageDocument,
    target: &str,
    offset: u64,
    length: Option<u64>,
) -> Result<blitz_control_protocol::WebGpuBufferSnapshot, DebugError> {
    invoke(
        document,
        "__chuzzGpuCaptureBuffer",
        &[
            HostValue::String(target.to_owned().into()),
            HostValue::Number(offset as f64),
            length.map_or(HostValue::Undefined, |value| {
                HostValue::Number(value as f64)
            }),
        ],
    )
}

pub(super) fn buffer_document_reply(
    document: &mut PageDocument,
    target: &str,
    offset: u64,
    length: Option<u64>,
    reply: Arc<blitz_control_protocol::latest::Once<blitz_control_protocol::DebugResponse>>,
) {
    let response = match buffer_document(document, target, offset, length) {
        Ok(snapshot) => blitz_control_protocol::DebugResponse::WebGpuBuffer(snapshot),
        Err(error) => blitz_control_protocol::DebugResponse::Error(error),
    };
    reply.fill(response);
}
