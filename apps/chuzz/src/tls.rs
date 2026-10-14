//! WSS uses the same ChaCha-only provider and certificate verification as HTTPS.

use std::sync::{Arc, OnceLock};
use rustls::{ClientConfig, RootCertStore, crypto::CryptoProvider};

pub(crate) fn crypto_provider() -> Arc<CryptoProvider> {
    blitz_net::chacha_provider()
}

pub(crate) fn client_config() -> Result<Arc<ClientConfig>, String> {
    static CONFIG: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
    CONFIG.get_or_init(build_config).clone()
}

pub(crate) fn prepare() -> Result<(), String> {
    blitz_net::prepare_native_tls()?;
    client_config().map(|_| ())
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
    #[cfg(feature = "sandbox2-witness")]
    if let Some(path) = std::env::var_os("SANDBOX2_TLS_CA_FILE") {
        use rustls::pki_types::{CertificateDer, pem::PemObject};
        for certificate in CertificateDer::pem_file_iter(path).map_err(|error| error.to_string())? {
            roots.add(certificate.map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        }
    }
    let mut config = ClientConfig::builder_with_provider(crypto_provider())
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|error| error.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(blitz_net::certificate_verifier(roots, RootCertStore::empty())?)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}
