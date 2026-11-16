//! How sooth machines trust each other. Every sooth has a key and a
//! self-signed certificate, made on first start and kept in `link/` beside
//! its config file. The hub and an agent talk over mutual TLS, and each side
//! accepts exactly the certificate it pinned when they were paired -- there's
//! no certificate authority and no host name check, only the fingerprint
//! (SHA-256 of the certificate).
//!
//! - [`Identity`]: this machine's key and certificate.
//! - [`verify`]: the rustls verifiers that pin a fingerprint (hub side) and
//!   take any client certificate for the API to look up (agent side).
//! - [`listener::TlsListener`]: the agent's TLS listener, which hands each
//!   connection's client fingerprint to the router.
//! - [`trust`]: the agent's pinned hubs and its one-time pairing code.

pub mod listener;
pub mod trust;
pub mod verify;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ClientConfig, ServerConfig};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The version of the agent API this build speaks. A hub refuses to pair
/// with an agent on a different one.
pub const API_VERSION: u32 = 1;

/// The crypto every link config uses, named explicitly: more than one rustls
/// provider is compiled in (via reqwest and self_update), so there is no
/// process default to fall back on.
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// The folder holding this machine's link files, beside its config file.
pub fn dir_beside(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("link")
}

/// SHA-256 of a certificate's DER bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    pub fn of(cert: &CertificateDer<'_>) -> Self {
        Self(Sha256::digest(cert.as_ref()).into())
    }

    /// Accepts the [`Display`](fmt::Display) form, or the same hex with any
    /// spacing or colons.
    pub fn parse(s: &str) -> Option<Self> {
        let hex: Vec<u8> = s
            .bytes()
            .filter(|b| !matches!(b, b' ' | b':' | b'-'))
            .collect();
        if hex.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, pair) in hex.chunks(2).enumerate() {
            let s = std::str::from_utf8(pair).ok()?;
            out[i] = u8::from_str_radix(s, 16).ok()?;
        }
        Some(Self(out))
    }
}

/// Sixteen groups of four hex digits -- long, but it's meant to be compared
/// by eye once, at pairing.
impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, pair) in self.0.chunks(2).enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{:02x}{:02x}", pair[0], pair[1])?;
        }
        Ok(())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({self})")
    }
}

impl Serialize for Fingerprint {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Fingerprint {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).ok_or_else(|| serde::de::Error::custom("not a certificate fingerprint"))
    }
}

/// This machine's key and self-signed certificate.
pub struct Identity {
    cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
    pub fingerprint: Fingerprint,
}

impl Identity {
    /// A fresh P-256 key and a certificate for it valid for a century: the
    /// pin is the trust, not the dates.
    pub fn generate() -> anyhow::Result<Self> {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let mut params = rcgen::CertificateParams::new(vec!["sooth".to_string()])?;
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "sooth");
        params.not_before = rcgen::date_time_ymd(2025, 1, 1);
        params.not_after = rcgen::date_time_ymd(2125, 1, 1);
        let cert = params.self_signed(&key)?;
        let cert = cert.der().clone();
        Ok(Self {
            fingerprint: Fingerprint::of(&cert),
            cert,
            key: PrivatePkcs8KeyDer::from(key.serialize_der()),
        })
    }

    /// Reads `cert.pem` and `key.pem` from `dir`, making them first if they
    /// don't exist yet. The key is written owner-only.
    pub fn load_or_create(dir: &Path) -> anyhow::Result<Self> {
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        if cert_path.exists() && key_path.exists() {
            let cert = CertificateDer::from_pem_file(&cert_path)
                .with_context(|| format!("reading {}", cert_path.display()))?;
            let key = PrivatePkcs8KeyDer::from_pem_file(&key_path)
                .with_context(|| format!("reading {}", key_path.display()))?;
            return Ok(Self {
                fingerprint: Fingerprint::of(&cert),
                cert,
                key,
            });
        }
        let identity = Self::generate()?;
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        write_private(
            &key_path,
            &pem("PRIVATE KEY", identity.key.secret_pkcs8_der()),
        )?;
        write_private(&cert_path, &pem("CERTIFICATE", identity.cert.as_ref()))?;
        tracing::info!(fingerprint = %identity.fingerprint, "made this machine's link key");
        Ok(identity)
    }

    fn key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(self.key.clone_key())
    }

    /// The agent's TLS config: this certificate, and a client certificate
    /// required from every caller (which one is the API's business).
    pub fn server_config(&self) -> anyhow::Result<ServerConfig> {
        let provider = provider();
        let verifier = Arc::new(verify::AnyClient::new(provider.clone()));
        Ok(ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .with_client_cert_verifier(verifier)
            .with_single_cert(vec![self.cert.clone()], self.key())?)
    }

    /// The hub's TLS config for one agent: present this certificate, and
    /// accept only a server certificate matching `server`.
    pub fn client_config(&self, server: Arc<verify::PinnedServer>) -> anyhow::Result<ClientConfig> {
        Ok(ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(server)
            .with_client_auth_cert(vec![self.cert.clone()], self.key())?)
    }
}

fn pem(label: &str, der: &[u8]) -> String {
    use std::fmt::Write;
    let b64 = base64_encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for line in b64.as_bytes().chunks(64) {
        let _ = writeln!(out, "{}", String::from_utf8_lossy(line));
    }
    let _ = writeln!(out, "-----END {label}-----");
    out
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Writes `contents` to `path` atomically, readable by the owner only.
pub(crate) fn write_private(path: &Path, contents: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&tmp)
        .with_context(|| format!("writing {}", path.display()))?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_print_and_parse_back() {
        let id = Identity::generate().unwrap();
        let shown = id.fingerprint.to_string();
        assert_eq!(shown.split(' ').count(), 16);
        assert_eq!(Fingerprint::parse(&shown), Some(id.fingerprint));
        assert_eq!(
            Fingerprint::parse(&shown.replace(' ', ":").to_uppercase()),
            Some(id.fingerprint)
        );
        assert_eq!(Fingerprint::parse("abcd"), None);
        assert_eq!(Fingerprint::parse(&"zz".repeat(32)), None);
    }

    #[test]
    fn an_identity_is_made_once_and_then_reused() {
        let dir = tempfile::tempdir().unwrap();
        let first = Identity::load_or_create(dir.path()).unwrap();
        let again = Identity::load_or_create(dir.path()).unwrap();
        assert_eq!(first.fingerprint, again.fingerprint);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("key.pem"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(first.server_config().is_ok());
    }

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }
}
