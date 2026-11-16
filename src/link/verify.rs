//! The two certificate checks a link makes, in place of rustls' usual
//! CA-and-hostname ones. Handshake signatures are still verified normally,
//! so a peer must hold the key behind the certificate it shows.

use std::sync::{Arc, Mutex};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};

use super::Fingerprint;

/// The hub's check of an agent: its certificate must have the pinned
/// fingerprint. With no pin (the first look at a machine being added), any
/// certificate passes and its fingerprint is kept in [`PinnedServer::seen`]
/// for the person to confirm.
#[derive(Debug)]
pub struct PinnedServer {
    pin: Option<Fingerprint>,
    seen: Mutex<Option<Fingerprint>>,
    algs: WebPkiSupportedAlgorithms,
}

impl PinnedServer {
    pub fn pinned(pin: Fingerprint, provider: &CryptoProvider) -> Arc<Self> {
        Self::make(Some(pin), provider)
    }

    pub fn learning(provider: &CryptoProvider) -> Arc<Self> {
        Self::make(None, provider)
    }

    fn make(pin: Option<Fingerprint>, provider: &CryptoProvider) -> Arc<Self> {
        Arc::new(Self {
            pin,
            seen: Mutex::new(None),
            algs: provider.signature_verification_algorithms,
        })
    }

    /// The fingerprint the last handshake showed.
    pub fn seen(&self) -> Option<Fingerprint> {
        *self.seen.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl ServerCertVerifier for PinnedServer {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let got = Fingerprint::of(end_entity);
        *self.seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(got);
        match self.pin {
            Some(pin) if pin != got => Err(rustls::Error::General(format!(
                "the machine's key changed: expected {pin}, got {got}"
            ))),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

/// The agent's check of a caller at the TLS layer: a client certificate is
/// required, but any will do here. Which callers may do what is decided per
/// request from the certificate's fingerprint (see `machine::api`), since
/// the pairing route has to admit a hub the agent doesn't know yet.
#[derive(Debug)]
pub struct AnyClient {
    algs: WebPkiSupportedAlgorithms,
}

impl AnyClient {
    pub fn new(provider: Arc<CryptoProvider>) -> Self {
        Self {
            algs: provider.signature_verification_algorithms,
        }
    }
}

impl ClientCertVerifier for AnyClient {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}
