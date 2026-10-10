//! Explicit RustCrypto selection for every WebRTC connection.
//!
//! The host owns time, transport, cancellation and the returned configuration.
//! This does not install or depend on a process-global default provider.

use std::sync::Arc;

use str0m::RtcConfig;
use str0m::config::{CryptoProvider, DtlsVersion};

pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(ps_str0m_rust_crypto::default_provider())
}

pub fn config() -> RtcConfig {
    RtcConfig::new()
        .set_crypto_provider(provider())
        .set_dtls_version(DtlsVersion::Dtls12)
}

