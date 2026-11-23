//! Flow security between two Rust computers: TLS 1.3 only, unknown and changed
//! certificates, replayed / reordered / oversized / malformed frames, removed peers losing
//! their sessions, no session tickets.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use glisa_flow::proto::{self, Conn, FRAME_MAX, encode};
use glisa_flow::tls::{self, Identity, PinVerifier};
use rustls::client::ClientSessionStore;
use serde_json::json;

fn identity(c: &Computer) -> Identity {
    Identity::load(&c.flow.store.cert_path, &c.flow.store.key_path).unwrap()
}

fn stranger() -> (tempfile::TempDir, Identity) {
    let d = tempfile::tempdir().unwrap();
    let (c, k) = (d.path().join("cert.pem"), d.path().join("key.pem"));
    glisa_flow::crypto::generate_identity(&c, &k).unwrap();
    let id = Identity::load(&c, &k).unwrap();
    (d, id)
}

/// An authenticated session to b with `id` (a's key: a stolen laptop, say).
fn session(b: &Computer, id: &Identity) -> Result<Conn, tls::ConnectError> {
    let cfg = tls::client_config(id, &b.flow.store.pin()).unwrap();
    tls::connect(
        "127.0.0.1",
        b.port,
        cfg,
        Duration::from_secs(3),
        proto::PEER_SERVER,
        proto::PEER_CLIENT,
    )
}

fn hello(a: &Computer, b: &Computer) -> Conn {
    let mut c = session(b, &identity(a)).unwrap();
    c.send("hello", json!({"id": a.id(), "name": "equipo-a"}))
        .unwrap();
    assert_eq!(
        c.recv_msg(Some(Duration::from_secs(3))).unwrap()["type"],
        "hello"
    );
    c
}

/// Connect + hello: refused (TLS 1.3 reports a refused client certificate on the first read).
fn refused(b: &Computer, id: &Identity, pid: &str) {
    if let Ok(mut c) = session(b, id) {
        let _ = c.send("hello", json!({"id": pid, "name": "x"}));
        assert!(c.recv_msg(Some(Duration::from_secs(2))).is_err());
    }
}

fn expect_closed(mut c: Conn) {
    let m = c.recv_msg(Some(Duration::from_secs(3))).unwrap();
    assert_eq!(
        (m["type"].as_str(), m["reason"].as_str()),
        (Some("error"), Some("protocol"))
    );
    assert!(c.recv_msg(Some(Duration::from_secs(3))).is_err());
}

fn openssl() -> bool {
    std::process::Command::new("openssl")
        .arg("version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn tls12_is_refused_and_openssl_speaks_tls13() {
    if !openssl() {
        eprintln!("openssl not available: skipped");
        return;
    }
    let (a, b) = paired_pair();
    let run = |version: &str| {
        let out = std::process::Command::new("openssl")
            .args([
                "s_client",
                "-connect",
                &format!("127.0.0.1:{}", b.port),
                version,
                "-brief",
            ])
            .arg("-cert")
            .arg(&a.flow.store.cert_path)
            .arg("-key")
            .arg(&a.flow.store.key_path)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
    };
    let out = run("-tls1_2");
    assert!(!out.contains("CONNECTION ESTABLISHED"), "{out}");
    let out = run("-tls1_3");
    assert!(
        out.contains("CONNECTION ESTABLISHED") && out.contains("TLSv1.3"),
        "{out}"
    );
}

#[test]
fn unknown_certificate_refused() {
    let (_a, b) = paired_pair();
    let (_d, x) = stranger();
    refused(&b, &x, "0123456789abcdef");
    // no client certificate at all
    let cfg = rustls::ClientConfig::builder_with_provider(tls::provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(PinVerifier::new([b.flow.store.pin()]))
        .with_no_client_auth();
    if let Ok(mut c) = tls::connect(
        "127.0.0.1",
        b.port,
        Arc::new(cfg),
        Duration::from_secs(3),
        proto::PEER_SERVER,
        proto::PEER_CLIENT,
    ) {
        let _ = c.send("hello", json!({"id": "0123456789abcdef", "name": "x"}));
        assert!(c.recv_msg(Some(Duration::from_secs(2))).is_err());
    }
}

#[test]
fn pin_mismatch_is_a_hard_failure() {
    let (a, b) = paired_pair();
    b.flow.stop();
    for f in ["cert.pem", "key.pem"] {
        std::fs::remove_file(b.flow.store.dir.join(f)).unwrap(); // b's key changes behind a's back
    }
    b.flow.store.ensure_identity().unwrap();
    b.flow.start_now().unwrap();
    a.flow.link(&b.id()).unwrap().close();
    let hub = shared_device(&a, &b);
    a.backend.hit("right", 0.5);
    assert!(
        wait_until(8.0, || a
            .events_in("es")
            .iter()
            .any(|t| t.contains("identidad"))),
        "{:?}",
        a.events_in("es")
    );
    assert!(wait_until(8.0, || a.peer_state(&b)["state"] == "identity"));
    assert!(hub.moves().is_empty()); // the mouse stays here
}

#[test]
fn replayed_and_reordered_messages() {
    let (a, b) = paired_pair();
    let mut c = hello(&a, &b);
    c.send_raw(&encode(&json!({"v": 1, "type": "ping", "seq": 1})))
        .unwrap(); // replay of seq 1
    expect_closed(c);
    let mut c = hello(&a, &b);
    c.send_raw(&encode(&json!({"v": 1, "type": "ping", "seq": 3})))
        .unwrap(); // skips 2
    expect_closed(c);
    let mut c = hello(&a, &b);
    c.send("ping", json!({})).unwrap();
    assert_eq!(
        c.recv_msg(Some(Duration::from_secs(3))).unwrap()["type"],
        "pong"
    ); // in order: fine
}

#[test]
fn oversized_and_malformed() {
    let (a, b) = paired_pair();
    let mut c = hello(&a, &b);
    c.send_raw(&encode(
        &json!({"v": 1, "type": "ping", "seq": 2, "pad": "x".repeat(9000)}),
    ))
    .unwrap();
    expect_closed(c);
    let mut c = hello(&a, &b);
    c.send_raw(&((FRAME_MAX + 1) as u32).to_be_bytes()).unwrap();
    expect_closed(c);
    let mut c = hello(&a, &b);
    let body = b"{not json";
    c.send_raw(&[&(body.len() as u32).to_be_bytes()[..], body].concat())
        .unwrap();
    expect_closed(c);
    let mut c = hello(&a, &b);
    c.send_raw(&encode(
        &json!({"v": 1, "type": "pair_commit", "seq": 2, "commit": "0".repeat(64)}),
    ))
    .unwrap(); // wrong channel
    expect_closed(c);
}

#[test]
fn remove_peer_deletes_pin_on_both() {
    let (a, b) = paired_pair();
    assert_eq!(
        a.call("flow_remove_peer", json!({"peer": b.id()})).unwrap(),
        true
    );
    assert!(a.flow.store.peer(&b.id()).is_none());
    assert!(
        !std::fs::read_to_string(&a.flow.store.file)
            .unwrap()
            .contains(&b.flow.store.pin())
    );
    assert!(wait_until(5.0, || b.flow.store.peer(&a.id()).is_none()));
    refused(&b, &identity(&a), &a.id()); // b no longer accepts a's certificate
}

#[test]
fn reset_identity_unpairs_everyone() {
    let (a, b) = paired_pair();
    let old = a.id();
    let st = a.call("flow_reset_identity", json!({})).unwrap();
    assert_ne!(st["id"], old.as_str());
    assert_eq!(st["peers"], json!([]));
    assert!(wait_until(5.0, || b.flow.store.peer(&old).is_none()));
}

#[test]
fn removed_peer_loses_its_open_session() {
    // Removing a peer must end what it already has open (removal is the remedy for a
    // stolen laptop).
    let (a, b) = paired_pair();
    let mut c = hello(&a, &b);
    c.send("clipboard", json!({"text": "antes"})).unwrap();
    assert_eq!(
        c.recv_msg(Some(Duration::from_secs(3))).unwrap()["type"],
        "ack"
    );
    assert_eq!(b.backend.clip().as_deref(), Some("antes"));
    assert_eq!(
        b.call("flow_remove_peer", json!({"peer": a.id()})).unwrap(),
        true
    );
    let injected = c
        .send("clipboard", json!({"text": "inyectado"}))
        .ok()
        .and_then(|_| c.recv_msg(Some(Duration::from_secs(3))).ok())
        .is_some_and(|m| m["type"] == "ack");
    assert!(!injected);
    sleep(0.2);
    assert_eq!(b.backend.clip().as_deref(), Some("antes"));
}

#[test]
fn intro_port_refuses_a_big_frame_before_reading_it() {
    let (_a, b) = two_computers();
    b.flow.pair_start().unwrap();
    let mut c = tls::connect(
        "127.0.0.1",
        b.port + 1,
        tls::intro_client_config().unwrap(),
        Duration::from_secs(3),
        proto::INTRO_SERVER,
        proto::INTRO_CLIENT,
    )
    .unwrap();
    c.send_raw(&(6u32 << 20).to_be_bytes()).unwrap(); // announce 6 MiB, send nothing
    let t0 = Instant::now();
    match c.recv(Some(Duration::from_secs(3))) {
        Err(proto::Error::Idle) => {
            panic!("the introduction listener is still waiting for the 6 MiB body")
        }
        Err(_) => {}
        Ok(m) => panic!("{m:?}"),
    }
    assert!(t0.elapsed() < Duration::from_secs(2));
}

#[test]
fn links_survive_a_peer_answering_garbage() {
    let (a, b) = paired_pair();
    let hits = Arc::new(Mutex::new(0));
    let h2 = hits.clone();
    b.flow.hooks().serve_peer = Some(Arc::new(move |mut conn: Conn| {
        conn.set_types(proto::PEER_CLIENT, proto::PEER_SERVER, false);
        let _ = conn.recv(Some(Duration::from_secs(3)));
        let body = [vec![b'['; 300_000], vec![b']'; 300_000]].concat();
        let _ = conn.send_raw(&[&(body.len() as u32).to_be_bytes()[..], &body].concat());
        *h2.lock().unwrap() += 1;
        std::thread::sleep(Duration::from_millis(200));
    }));
    a.flow.link(&b.id()).unwrap().close(); // a reconnects and meets the new b
    assert!(wait_until(8.0, || *hits.lock().unwrap() >= 1));
    sleep(1.0);
    b.flow.hooks().serve_peer = None;
    assert!(linked(&a, &b, 20.0)); // the links thread is still alive and reconnects
}

/// Records whether a server ever sends a session ticket.
#[derive(Debug, Default)]
struct TicketSpy(Mutex<usize>);

impl ClientSessionStore for TicketSpy {
    fn set_kx_hint(&self, _: rustls::pki_types::ServerName<'static>, _: rustls::NamedGroup) {}
    fn kx_hint(&self, _: &rustls::pki_types::ServerName<'_>) -> Option<rustls::NamedGroup> {
        None
    }
    fn set_tls12_session(
        &self,
        _: rustls::pki_types::ServerName<'static>,
        _: rustls::client::Tls12ClientSessionValue,
    ) {
    }
    fn tls12_session(
        &self,
        _: &rustls::pki_types::ServerName<'_>,
    ) -> Option<rustls::client::Tls12ClientSessionValue> {
        None
    }
    fn remove_tls12_session(&self, _: &rustls::pki_types::ServerName<'static>) {}
    fn insert_tls13_ticket(
        &self,
        _: rustls::pki_types::ServerName<'static>,
        _: rustls::client::Tls13ClientSessionValue,
    ) {
        *self.0.lock().unwrap() += 1;
    }
    fn take_tls13_ticket(
        &self,
        _: &rustls::pki_types::ServerName<'static>,
    ) -> Option<rustls::client::Tls13ClientSessionValue> {
        None
    }
}

#[test]
fn servers_issue_no_session_tickets() {
    let (a, b) = paired_pair();
    let spy = Arc::new(TicketSpy::default());
    let id = identity(&a);
    let mut cfg = rustls::ClientConfig::builder_with_provider(tls::provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(PinVerifier::new([b.flow.store.pin()]))
        .with_client_auth_cert(vec![id.cert.clone()], id.key.clone_key())
        .unwrap();
    cfg.resumption = rustls::client::Resumption::store(spy.clone());
    let mut c = tls::connect(
        "127.0.0.1",
        b.port,
        Arc::new(cfg),
        Duration::from_secs(3),
        proto::PEER_SERVER,
        proto::PEER_CLIENT,
    )
    .unwrap();
    c.send("hello", json!({"id": a.id(), "name": "equipo-a"}))
        .unwrap();
    assert_eq!(
        c.recv_msg(Some(Duration::from_secs(3))).unwrap()["type"],
        "hello"
    ); // any ticket comes before it
    assert_eq!(*spy.0.lock().unwrap(), 0);
    // the pairing introduction too
    b.flow.pair_start().unwrap();
    let mut cfg = rustls::ClientConfig::builder_with_provider(tls::provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(PinVerifier::any())
        .with_no_client_auth();
    cfg.resumption = rustls::client::Resumption::store(spy.clone());
    let mut c = tls::connect(
        "127.0.0.1",
        b.port + 1,
        Arc::new(cfg),
        Duration::from_secs(3),
        proto::INTRO_SERVER,
        proto::INTRO_CLIENT,
    )
    .unwrap();
    c.send(
        "pair_intro",
        json!({"id": a.id(), "name": "a", "cert": id.pem, "port": a.port}),
    )
    .unwrap();
    let _ = c.recv_msg(Some(Duration::from_secs(3)));
    assert_eq!(*spy.0.lock().unwrap(), 0);
}
