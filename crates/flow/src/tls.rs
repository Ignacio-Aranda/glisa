//! TLS for Flow (docs/FLOW.md §7), with rustls and the ring provider:
//!
//! * TLS 1.3 only (1.2 is refused), forward-secret key exchange, no session tickets and no
//!   resumption (the server issues none and keeps no session cache; the client offers none);
//! * both sides authenticate with their per-installation ECDSA P-256 certificate; hostnames
//!   are irrelevant: a custom verifier accepts the peer only if the SHA-256 of its
//!   certificate's SubjectPublicKeyInfo is one of the expected pins (and the certificate is
//!   within its validity period), and the handshake signature is checked against that key;
//! * the pairing introduction is server-authenticated only and accepts any certificate
//!   (it carries public certificates that the SAS comparison then authenticates), but the
//!   server still proves it holds the key of the certificate it presents.

use std::collections::HashSet;
use std::io;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    CertificateError, ClientConfig, ClientConnection, DigitallySignedStruct, DistinguishedName,
    Error, ServerConfig, ServerConnection, SignatureScheme, StreamOwned,
};

use crate::crypto::{pin_of_der, validity};
use crate::proto::{Conn, HANDSHAKE_TIMEOUT, RawSocket};

pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// This computer's identity, loaded from `cert.pem` / `key.pem`.
pub struct Identity {
    pub cert: CertificateDer<'static>,
    pub key: PrivateKeyDer<'static>,
    pub pin: String,
    pub pem: String,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("pin", &self.pin)
            .finish_non_exhaustive()
    }
}

impl Identity {
    pub fn load(cert_path: &std::path::Path, key_path: &std::path::Path) -> io::Result<Self> {
        let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_string());
        let pem = std::fs::read_to_string(cert_path)?;
        let cert = CertificateDer::from_pem_slice(pem.as_bytes())
            .map_err(|_| bad("invalid certificate"))?;
        let key_pem = std::fs::read(key_path)?;
        let key =
            PrivateKeyDer::from_pem_slice(&key_pem).map_err(|_| bad("invalid private key"))?;
        provider()
            .key_provider
            .load_private_key(key.clone_key())
            .map_err(|_| bad("unsupported private key"))?;
        let pin = pin_of_der(&cert).map_err(bad)?;
        Ok(Self {
            cert,
            key,
            pin,
            pem,
        })
    }
}

/// Accepts a peer whose certificate's SPKI pin is in `pins` (`any`: whatever certificate,
/// still proving possession of its key).
#[derive(Debug)]
pub struct PinVerifier {
    pins: HashSet<String>,
    any: bool,
    algs: WebPkiSupportedAlgorithms,
}

impl PinVerifier {
    pub fn new(pins: impl IntoIterator<Item = String>) -> Arc<Self> {
        Arc::new(Self {
            pins: pins.into_iter().collect(),
            any: false,
            algs: provider().signature_verification_algorithms,
        })
    }

    pub fn any() -> Arc<Self> {
        Arc::new(Self {
            pins: HashSet::new(),
            any: true,
            algs: provider().signature_verification_algorithms,
        })
    }

    fn check(&self, end_entity: &CertificateDer<'_>, now: UnixTime) -> Result<(), Error> {
        if self.any {
            return Ok(());
        }
        let pin = pin_of_der(end_entity)
            .map_err(|_| Error::InvalidCertificate(CertificateError::BadEncoding))?;
        if !self.pins.contains(&pin) {
            return Err(Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ));
        }
        let (not_before, not_after) = validity(end_entity)
            .map_err(|_| Error::InvalidCertificate(CertificateError::BadEncoding))?;
        let now = now.as_secs() as i64;
        if now < not_before {
            return Err(Error::InvalidCertificate(CertificateError::NotValidYet));
        }
        if now > not_after {
            return Err(Error::InvalidCertificate(CertificateError::Expired));
        }
        Ok(())
    }
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        self.check(end_entity, now)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::PeerIncompatible(
            rustls::PeerIncompatible::Tls12NotOffered,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

impl ClientCertVerifier for PinVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        self.check(end_entity, now)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::PeerIncompatible(
            rustls::PeerIncompatible::Tls12NotOffered,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

fn tls_error(e: Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

/// Peer channel / pairing session server: TLS 1.3, a client certificate required and
/// accepted only if its pin is in `trusted_pins` (pins are checked again by the caller).
pub fn server_config(
    id: &Identity,
    trusted_pins: impl IntoIterator<Item = String>,
) -> io::Result<Arc<ServerConfig>> {
    let cfg = ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(tls_error)?
        .with_client_cert_verifier(PinVerifier::new(trusted_pins))
        .with_single_cert(vec![id.cert.clone()], id.key.clone_key())
        .map_err(tls_error)?;
    Ok(Arc::new(no_resumption_server(cfg)))
}

/// Pairing introduction only: exchanges public certificates, which the SAS comparison
/// then authenticates. Carries nothing else.
pub fn intro_server_config(id: &Identity) -> io::Result<Arc<ServerConfig>> {
    let cfg = ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(tls_error)?
        .with_no_client_auth()
        .with_single_cert(vec![id.cert.clone()], id.key.clone_key())
        .map_err(tls_error)?;
    Ok(Arc::new(no_resumption_server(cfg)))
}

fn no_resumption_server(mut cfg: ServerConfig) -> ServerConfig {
    cfg.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    cfg.send_tls13_tickets = 0;
    cfg
}

/// Connect to one known peer: TLS 1.3, our certificate, only that peer's pin accepted.
pub fn client_config(id: &Identity, peer_pin: &str) -> io::Result<Arc<ClientConfig>> {
    let mut cfg = ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(tls_error)?
        .dangerous()
        .with_custom_certificate_verifier(PinVerifier::new([peer_pin.to_string()]))
        .with_client_auth_cert(vec![id.cert.clone()], id.key.clone_key())
        .map_err(tls_error)?;
    cfg.resumption = rustls::client::Resumption::disabled();
    Ok(Arc::new(cfg))
}

/// The pairing introduction's client: no client certificate, any server certificate.
pub fn intro_client_config() -> io::Result<Arc<ClientConfig>> {
    let mut cfg = ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(tls_error)?
        .dangerous()
        .with_custom_certificate_verifier(PinVerifier::any())
        .with_no_client_auth();
    cfg.resumption = rustls::client::Resumption::disabled();
    Ok(Arc::new(cfg))
}

/// Why a connection could not be made.
#[derive(Debug)]
pub enum ConnectError {
    /// The peer answered with a certificate that is not the pinned one.
    Mismatch,
    Io(io::Error),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::Mismatch => f.write_str("the certificate doesn't match"),
            ConnectError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for ConnectError {
    fn from(e: io::Error) -> Self {
        if is_cert_error(&e) {
            ConnectError::Mismatch
        } else {
            ConnectError::Io(e)
        }
    }
}

fn is_cert_error(e: &io::Error) -> bool {
    e.get_ref()
        .and_then(|inner| inner.downcast_ref::<Error>())
        .is_some_and(|e| matches!(e, Error::InvalidCertificate(_)))
}

fn set_deadline(sock: &TcpStream, deadline: Instant) -> io::Result<()> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "TLS handshake timed out",
        ));
    }
    sock.set_read_timeout(Some(left))?;
    sock.set_write_timeout(Some(left))
}

fn tcp_connect(host: &str, port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let addrs: Vec<SocketAddr> = (host, port).to_socket_addrs()?.collect();
    let mut last = io::Error::new(io::ErrorKind::NotFound, "no address");
    for a in addrs {
        match TcpStream::connect_timeout(&a, timeout) {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// TCP + TLS 1.3 handshake (5 s) to `host:port`. The connection's `peer_cert` is the
/// server's certificate.
pub fn connect(
    host: &str,
    port: u16,
    cfg: Arc<ClientConfig>,
    timeout: Duration,
    recv_types: &'static [&'static str],
    send_types: &'static [&'static str],
) -> Result<Conn, ConnectError> {
    let mut tcp = tcp_connect(host, port, timeout)?;
    let name = match host.parse::<std::net::IpAddr>() {
        Ok(ip) => ServerName::IpAddress(ip.into()),
        Err(_) => ServerName::try_from(host.to_string()).unwrap_or(ServerName::IpAddress(
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST).into(),
        )),
    };
    let mut tls = ClientConnection::new(cfg, name).map_err(tls_error)?;
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    while tls.is_handshaking() {
        set_deadline(&tcp, deadline)?;
        tls.complete_io(&mut tcp)?;
    }
    while tls.wants_write() {
        set_deadline(&tcp, deadline)?;
        tls.write_tls(&mut tcp)?;
    }
    let peer = tls
        .peer_certificates()
        .and_then(|c| c.first())
        .map(|c| c.as_ref().to_vec());
    finish(tcp, StreamParts::Client(tls), peer, recv_types, send_types)
}

/// TLS 1.3 handshake (5 s) on an accepted connection. The connection's `peer_cert` is the
/// client's certificate (None on the introduction port).
pub fn accept(
    mut tcp: TcpStream,
    cfg: Arc<ServerConfig>,
    recv_types: &'static [&'static str],
    send_types: &'static [&'static str],
) -> Result<Conn, ConnectError> {
    let _ = tcp.set_nodelay(true);
    let mut tls = ServerConnection::new(cfg).map_err(tls_error)?;
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    while tls.is_handshaking() {
        set_deadline(&tcp, deadline)?;
        tls.complete_io(&mut tcp)?;
    }
    while tls.wants_write() {
        set_deadline(&tcp, deadline)?;
        tls.write_tls(&mut tcp)?;
    }
    let peer = tls
        .peer_certificates()
        .and_then(|c| c.first())
        .map(|c| c.as_ref().to_vec());
    finish(tcp, StreamParts::Server(tls), peer, recv_types, send_types)
}

enum StreamParts {
    Client(ClientConnection),
    Server(ServerConnection),
}

fn finish(
    tcp: TcpStream,
    parts: StreamParts,
    peer: Option<Vec<u8>>,
    recv_types: &'static [&'static str],
    send_types: &'static [&'static str],
) -> Result<Conn, ConnectError> {
    let raw: Arc<dyn RawSocket> = Arc::new(tcp.try_clone()?);
    let io: Box<dyn crate::proto::Stream> = match parts {
        StreamParts::Client(c) => Box::new(StreamOwned::new(c, tcp)),
        StreamParts::Server(s) => Box::new(StreamOwned::new(s, tcp)),
    };
    let mut conn = Conn::new(io, raw, recv_types, send_types, false);
    conn.peer_cert = peer;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn identity(dir: &std::path::Path, tag: &str) -> Identity {
        let (c, k) = (
            dir.join(format!("{tag}-cert.pem")),
            dir.join(format!("{tag}-key.pem")),
        );
        crate::crypto::generate_identity(&c, &k).unwrap();
        Identity::load(&c, &k).unwrap()
    }

    fn serve_once(
        cfg: Arc<ServerConfig>,
    ) -> (u16, std::thread::JoinHandle<Result<Vec<u8>, String>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut c = accept(s, cfg, crate::proto::PEER_CLIENT, crate::proto::PEER_SERVER)
                .map_err(|e| e.to_string())?;
            let m = c
                .recv_msg(Some(Duration::from_secs(3)))
                .map_err(|e| e.to_string())?;
            c.send("pong", serde_json::json!({"re": m["seq"]}))
                .map_err(|e| e.to_string())?;
            Ok(c.peer_cert.clone().unwrap_or_default())
        });
        (port, t)
    }

    #[test]
    fn mutual_pinned_handshake() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (identity(d.path(), "a"), identity(d.path(), "b"));
        let (port, t) = serve_once(server_config(&b, [a.pin.clone()]).unwrap());
        let mut c = connect(
            "127.0.0.1",
            port,
            client_config(&a, &b.pin).unwrap(),
            Duration::from_secs(3),
            crate::proto::PEER_SERVER,
            crate::proto::PEER_CLIENT,
        )
        .unwrap();
        assert_eq!(pin_of_der(c.peer_cert.as_ref().unwrap()).unwrap(), b.pin);
        c.send("ping", serde_json::json!({})).unwrap();
        assert_eq!(
            c.recv_msg(Some(Duration::from_secs(3))).unwrap()["type"],
            "pong"
        );
        assert_eq!(pin_of_der(&t.join().unwrap().unwrap()).unwrap(), a.pin);
    }

    #[test]
    fn wrong_pins_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let (a, b, x) = (
            identity(d.path(), "a"),
            identity(d.path(), "b"),
            identity(d.path(), "x"),
        );
        // the client expects b, a stranger answers: a hard "mismatch"
        let (port, _t) = serve_once(server_config(&x, [a.pin.clone()]).unwrap());
        let r = connect(
            "127.0.0.1",
            port,
            client_config(&a, &b.pin).unwrap(),
            Duration::from_secs(3),
            crate::proto::PEER_SERVER,
            crate::proto::PEER_CLIENT,
        );
        assert!(matches!(r, Err(ConnectError::Mismatch)), "{r:?}");
        // the server doesn't know the client: refused (TLS 1.3 reports it on the first read)
        let (port, t) = serve_once(server_config(&b, [a.pin.clone()]).unwrap());
        let r = connect(
            "127.0.0.1",
            port,
            client_config(&x, &b.pin).unwrap(),
            Duration::from_secs(3),
            crate::proto::PEER_SERVER,
            crate::proto::PEER_CLIENT,
        );
        if let Ok(mut c) = r {
            let _ = c.send("ping", serde_json::json!({}));
            assert!(c.recv_msg(Some(Duration::from_secs(3))).is_err());
        }
        assert!(t.join().unwrap().is_err());
    }
}
