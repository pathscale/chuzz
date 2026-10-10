//! WSS TLS policy. HTTPS uses the same provider and native-root policy in Blitz.

use std::sync::{Arc, OnceLock};

use rustls::{ClientConfig, RootCertStore, crypto::CryptoProvider};

pub(crate) fn crypto_provider() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    Arc::clone(PROVIDER.get_or_init(|| Arc::new(ps_rustls_rustcrypto::provider())))
}

pub(crate) fn client_config() -> Result<Arc<ClientConfig>, String> {
    static CONFIG: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
    CONFIG.get_or_init(build_config).clone()
}

fn build_config() -> Result<Arc<ClientConfig>, String> {
    let native = rustls_native_certs::load_native_certs();
    let mut roots = RootCertStore::empty();
    let (accepted, rejected) = roots.add_parsable_certificates(native.certs);
    if accepted == 0 {
        return Err(format!(
            "no usable native TLS trust anchors: {} loading errors, {rejected} rejected certificates",
            native.errors.len(),
        ));
    }
    if !native.errors.is_empty() || rejected != 0 {
        eprintln!(
            "TLS trust store: {} loading errors, {rejected} rejected certificates",
            native.errors.len(),
        );
    }
    let mut config = ClientConfig::builder_with_provider(crypto_provider())
        .with_safe_default_protocol_versions()
        .map_err(|error| error.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

