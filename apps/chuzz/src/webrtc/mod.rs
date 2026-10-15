//! Synchronous WebRTC protocols with page-owned nagoya transports.

pub mod crypto;
mod encode;
mod gather;
mod host;
#[path = "webrtc_host_bridge.rs"]
pub mod native_host;
mod opus;
mod pcmu;
mod qa;
mod qa_script;
mod receive;
mod sdp;
mod transport;

pub(crate) fn install(document: &mut blitz_script::ScriptDocument) {
    qa::install(document);
    host::install(document);
}
