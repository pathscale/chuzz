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
mod receive;
mod sdp;
mod transport;
