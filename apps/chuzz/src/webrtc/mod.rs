//! Synchronous WebRTC protocol foundations.

pub mod crypto;
mod qa;

pub(crate) fn install(document: &mut blitz_script::ScriptDocument) {
    qa::install(document);
}

