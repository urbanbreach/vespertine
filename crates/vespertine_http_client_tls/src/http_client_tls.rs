use std::sync::OnceLock;

use rustls::ClientConfig;
use rustls_platform_verifier::ConfigVerifierExt;

static TLS_CONFIG: OnceLock<rustls::ClientConfig> = OnceLock::new();

pub fn tls_config() -> ClientConfig {
    TLS_CONFIG
        .get_or_init(|| {
            // rustls uses the `aws_lc_rs` provider by default
            // This only errors if the default provider has already
            // been installed. We can ignore this `Result`.
            rustls::crypto::aws_lc_rs::default_provider()
                .install_default()
                .ok();

            ClientConfig::with_platform_verifier().unwrap_or_else(|error| {
                log::error!(
                    "failed to load platform TLS certificate verifier, falling back to the system trust store: {error}"
                );
                ClientConfig::builder()
                    .with_root_certificates(system_roots())
                    .with_no_client_auth()
            })
        })
        .clone()
}

/// The certificates of the system trust store. Unparsable entries and read errors are logged; with
/// no usable root every TLS connection fails certificate verification (no roots are bundled).
fn system_roots() -> rustls::RootCertStore {
    let native = rustls_native_certs::load_native_certs();
    for error in &native.errors {
        log::warn!("failed to read a system TLS root certificate: {error}");
    }
    let mut roots = rustls::RootCertStore::empty();
    let (added, ignored) = roots.add_parsable_certificates(native.certs);
    if ignored > 0 {
        log::warn!("ignored {ignored} unparsable system TLS root certificates");
    }
    if added == 0 {
        log::error!("the system trust store has no usable TLS root certificate");
    }
    roots
}
