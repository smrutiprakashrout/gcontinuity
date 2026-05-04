//! TLS certificate management.
//!
//! Loads an existing self-signed certificate from `{data_dir}/tls/` or
//! generates a new one if absent / malformed.  The compiled `ServerConfig`
//! uses `ClientAuth::NoClientCert` — Android authenticates through cert
//! pinning rather than mutual TLS, keeping the handshake simpler.

use anyhow::{Context, Result};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};
use rustls::ServerConfig;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use tokio::fs;

/// Compiled TLS identity ready for use by the WebSocket server.
pub struct TlsIdentity {
    /// rustls server configuration — clone the Arc to share across tasks.
    pub server_config: Arc<ServerConfig>,
    /// SHA-256 of the DER-encoded certificate — displayed to the user during
    /// the pairing ceremony so they can visually verify the connection.
    pub cert_sha256: [u8; 32],
}

/// Load from `{data_dir}/tls/{cert,key}.pem`, or generate and persist new
/// files when they are missing or cannot be parsed.
///
/// The `tls/` directory is created with `0o700` permissions so the private
/// key is not world-readable.
pub async fn load_or_generate(data_dir: &Path) -> Result<TlsIdentity> {
    let tls_dir = data_dir.join("tls");

    fs::create_dir_all(&tls_dir)
        .await
        .context("Failed to create tls/ directory")?;

    // Restrict permissions on the key directory — 0o700 so only the owner
    // can read/write.  The #[cfg] guard keeps the code portable to Windows CI.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tls_dir, std::fs::Permissions::from_mode(0o700))
            .context("Failed to chmod tls/ to 0o700")?;
    }

    let cert_path = tls_dir.join("cert.pem");
    let key_path = tls_dir.join("key.pem");

    if cert_path.exists() && key_path.exists() {
        match try_load(&cert_path, &key_path).await {
            Ok(id) => {
                tracing::debug!("Loaded existing TLS cert from {}", tls_dir.display());
                return Ok(id);
            }
            Err(e) => {
                tracing::warn!("Existing TLS cert invalid ({}); regenerating", e);
            }
        }
    }

    generate_and_save(&cert_path, &key_path).await
}

/// Try to parse existing PEM files into a `TlsIdentity`.
async fn try_load(cert_path: &Path, key_path: &Path) -> Result<TlsIdentity> {
    let cert_pem = fs::read_to_string(cert_path).await.context("read cert.pem")?;
    let key_pem = fs::read_to_string(key_path).await.context("read key.pem")?;
    build_identity(&cert_pem, &key_pem)
}

/// Generate a new self-signed certificate and persist it, then build an identity.
async fn generate_and_save(cert_path: &Path, key_path: &Path) -> Result<TlsIdentity> {
    let key_pair = KeyPair::generate().context("Failed to generate key pair")?;

    let mut params = CertificateParams::default();

    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "gcontinuity-daemon");
    params.distinguished_name = dn;

    // SAN: IP 0.0.0.0 — accepted on any local interface.
    params.subject_alt_names =
        vec![SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))];

    // 10-year validity — this is a long-lived daemon identity, not a CA.
    let not_before = time::OffsetDateTime::now_utc();
    let not_after = not_before + time::Duration::days(3650);
    params.not_before = not_before;
    params.not_after = not_after;

    let cert = params
        .self_signed(&key_pair)
        .context("Failed to self-sign certificate")?;

    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();

    fs::write(cert_path, &cert_pem)
        .await
        .context("Failed to write cert.pem")?;
    fs::write(key_path, &key_pem)
        .await
        .context("Failed to write key.pem")?;

    tracing::info!("Generated new TLS certificate at {}", cert_path.display());
    build_identity(&cert_pem, &key_pem)
}

/// Parse PEM strings into a fully initialised `TlsIdentity`.
fn build_identity(cert_pem: &str, key_pem: &str) -> Result<TlsIdentity> {
    let mut cert_reader = std::io::BufReader::new(cert_pem.as_bytes());
    let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls_pemfile::certs(&mut cert_reader)
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to parse cert PEM")?;

    anyhow::ensure!(!certs.is_empty(), "No certificates in cert.pem");

    // SHA-256 of the DER bytes of the first (leaf) certificate.
    let cert_sha256: [u8; 32] = {
        let mut h = Sha256::new();
        h.update(certs[0].as_ref());
        h.finalize().into()
    };

    let mut key_reader = std::io::BufReader::new(key_pem.as_bytes());
    let key = rustls_pemfile::private_key(&mut key_reader)
        .context("Failed to parse key PEM")?
        .context("No private key found in key.pem")?;

    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("Failed to build rustls ServerConfig")?;

    Ok(TlsIdentity {
        server_config: Arc::new(server_config),
        cert_sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Install ring provider once per test; ignore if another test already did.
    fn install_provider() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    #[tokio::test]
    async fn test_generates_when_missing() {
        install_provider();
        let dir = tempdir().unwrap();
        let id = load_or_generate(dir.path()).await.unwrap();
        assert_ne!(id.cert_sha256, [0u8; 32], "fingerprint must be non-zero");
        assert!(dir.path().join("tls/cert.pem").exists());
        assert!(dir.path().join("tls/key.pem").exists());
    }

    #[tokio::test]
    async fn test_loads_existing_gives_same_fingerprint() {
        install_provider();
        let dir = tempdir().unwrap();
        let id1 = load_or_generate(dir.path()).await.unwrap();
        let id2 = load_or_generate(dir.path()).await.unwrap();
        assert_eq!(id1.cert_sha256, id2.cert_sha256, "fingerprint must be stable");
    }

    #[tokio::test]
    async fn test_regenerates_on_corrupt_cert() {
        install_provider();
        let dir = tempdir().unwrap();
        let tls_dir = dir.path().join("tls");
        std::fs::create_dir_all(&tls_dir).unwrap();
        std::fs::write(tls_dir.join("cert.pem"), b"not a cert").unwrap();
        std::fs::write(tls_dir.join("key.pem"), b"not a key").unwrap();
        // Must succeed (regenerate) rather than propagate the parse error.
        let id = load_or_generate(dir.path()).await.unwrap();
        assert_ne!(id.cert_sha256, [0u8; 32]);
    }
}
