//! The authenticated port (peer channels and transfer connections) and the listeners.
//!
//! Every connection must pass a mutual TLS 1.3 handshake whose client certificate is
//! pinned (a paired peer, or the candidate of a pairing in progress). The first message
//! says what the connection is for: `hello` (a peer channel) or `transfer` (copied files or
//! an image). Every message on a served session is checked against the current pins, so
//! removing a peer ends what it has open.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{Map, Value, json};

use super::{FlowService, MAX_CONNECTIONS, SERVER_IDLE, Served};
use crate::proto::{self, Conn, RateLimiter};
use crate::util::{lock, wait_readable};
use crate::{PORT, tls};

pub type Handler = Arc<dyn Fn(TcpStream, SocketAddr) + Send + Sync>;

/// A TCP listener with a connection limit and a per-address rate limit; one thread per
/// connection.
pub struct Listener {
    pub port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Listener {
    pub fn start(
        bind: Ipv4Addr,
        port: u16,
        name: &str,
        rate: f64,
        burst: u32,
        handler: Handler,
    ) -> std::io::Result<Self> {
        let s = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )?;
        #[cfg(unix)]
        s.set_reuse_address(true)?;
        s.bind(&SocketAddr::V4(SocketAddrV4::new(bind, port)).into())?;
        s.listen(8)?;
        let listener: TcpListener = s.into();
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let tname = name.to_string();
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let start = std::time::Instant::now();
                let limiter =
                    RateLimiter::new(rate, burst, Box::new(move || start.elapsed().as_secs_f64()));
                let active = Arc::new(AtomicUsize::new(0));
                while !stop2.load(Ordering::SeqCst) {
                    if !wait_readable(&listener, Duration::from_millis(250)) {
                        continue;
                    }
                    if stop2.load(Ordering::SeqCst) {
                        break;
                    }
                    let (raw, addr) = match listener.accept() {
                        Ok(x) => x,
                        Err(_) => {
                            std::thread::sleep(Duration::from_millis(100));
                            continue;
                        }
                    };
                    let _ = raw.set_nonblocking(false);
                    let busy = active.load(Ordering::SeqCst) >= MAX_CONNECTIONS;
                    if busy || !limiter.allow(&addr.ip().to_string()) {
                        log::info!(
                            "Flow: connection from {} refused (connection limit)",
                            addr.ip()
                        );
                        drop(raw);
                        continue;
                    }
                    active.fetch_add(1, Ordering::SeqCst);
                    let (handler, active) = (handler.clone(), active.clone());
                    let spawned = std::thread::Builder::new()
                        .name(format!("{tname}-conn"))
                        .spawn(move || {
                            struct Done(Arc<AtomicUsize>);
                            impl Drop for Done {
                                fn drop(&mut self) {
                                    self.0.fetch_sub(1, Ordering::SeqCst);
                                }
                            }
                            let _done = Done(active);
                            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                handler(raw, addr)
                            }))
                            .is_err()
                            {
                                log::error!("Flow: error serving {}", addr.ip());
                            }
                        });
                    if spawned.is_err() {
                        log::warn!("Flow: couldn't start a thread for a connection");
                    }
                }
            })?;
        Ok(Self {
            port,
            stop,
            thread: Some(thread),
        })
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl FlowService {
    /// TLS server configuration with the pins trusted right now: the paired peers plus,
    /// for one pairing attempt only, the introduced candidate.
    fn server_config(&self) -> Option<Arc<rustls::ServerConfig>> {
        let id = self.identity()?;
        let mut pins: Vec<String> = self.store.peers().into_values().map(|p| p.pin).collect();
        if let Some(candidate) = self.pairing_candidate_pin() {
            pins.push(candidate);
        }
        tls::server_config(&id, pins).ok()
    }

    pub(crate) fn serve(&self, tcp: TcpStream, addr: SocketAddr) {
        let Some(cfg) = self.server_config() else {
            return;
        };
        let mut conn = match tls::accept(tcp, cfg, proto::FIRST, proto::FIRST_ANSWERS) {
            Ok(c) => c,
            Err(e) => {
                log::info!("Flow: connection from {} refused ({e})", addr.ip());
                return;
            }
        };
        let Some(pin) = conn
            .peer_cert
            .as_deref()
            .and_then(|c| crate::crypto::pin_of_der(c).ok())
        else {
            return;
        };
        if let Some(p) = self.take_pair_session(&pin) {
            conn.set_types(proto::PAIR, proto::PAIR, false);
            self.pair_responder(conn, p);
            return;
        }
        let Some(pid) = self.store.peer_by_pin(&pin) else {
            return; // trusted a moment ago, removed since
        };
        let evil = lock(&self.hooks).serve_peer.clone();
        if let Some(f) = evil {
            f(conn);
            return;
        }
        self.serve_peer(conn, &pid, &pin, Some(addr));
    }

    fn register_served(&self, conn: &Conn, pid: &str) -> Option<u64> {
        let mut st = lock(&self.st);
        if !self.running() {
            return None;
        }
        st.served_seq += 1;
        let id = st.served_seq;
        st.served.push(Served {
            id,
            peer: pid.to_string(),
            sock: conn.shutdown_handle(),
            channel: false,
        });
        Some(id)
    }

    /// End the sessions a peer has open with us (they are not re-checked until their next
    /// message, which may never come).
    pub(crate) fn drop_served(&self, pid: &str) {
        let socks: Vec<_> = lock(&self.st)
            .served
            .iter()
            .filter(|s| s.peer == pid)
            .map(|s| s.sock.clone())
            .collect();
        for s in socks {
            s.shutdown();
        }
    }

    /// The pinned peer `pid` just reached us from `host` and listens on `port`: remember it
    /// as the peer's address, so our own link (each side opens its own) finds it on another
    /// subnet too. Only ever called after the TLS pin and the hello id matched. A link that
    /// is up stays where it is: the address that works for us wins.
    fn learn_address(&self, pid: &str, host: &str, port: u16) {
        let link = self.link(pid);
        if let Some(l) = &link {
            if l.online() && l.addr() != Some((host.to_string(), port)) {
                return;
            }
        }
        if self.store.set_address(pid, host, port) {
            log::info!(
                "Flow: {} reached this computer from {}; using that address",
                self.store
                    .peer(pid)
                    .map(|p| p.name)
                    .unwrap_or_else(|| pid.to_string()),
                crate::address::format_address(Some(&(host.to_string(), port)))
            );
        }
        if let Some(l) = link {
            if !l.online() {
                l.retry_now();
            }
        }
    }

    pub(crate) fn serve_peer(
        &self,
        mut conn: Conn,
        pid: &str,
        pin: &str,
        addr: Option<SocketAddr>,
    ) {
        // The first message says what the connection is for: `hello` (the peer channel) or
        // `transfer` (copied files or an image).
        conn.set_types(proto::FIRST, proto::FIRST_ANSWERS, false);
        let Some(served) = self.register_served(&conn, pid) else {
            return;
        };
        let result = self.serve_peer_inner(&mut conn, pid, pin, addr, served);
        if let Err(proto::Error::Protocol(e)) = result {
            log::info!("Flow: {pid} broke the protocol: {e}");
            let _ = conn.send("error", json!({"reason": "protocol"}));
        }
        lock(&self.st).served.retain(|s| s.id != served);
        conn.close();
    }

    fn serve_peer_inner(
        &self,
        conn: &mut Conn,
        pid: &str,
        pin: &str,
        addr: Option<SocketAddr>,
        served: u64,
    ) -> Result<(), proto::Error> {
        let m = conn.recv_msg(Some(Duration::from_secs(5)))?;
        if m["id"] != pid {
            conn.send("error", json!({"reason": "unknown_peer"}))?;
            return Ok(());
        }
        if m["type"] == "transfer" {
            self.serve_transfer(conn, pid, pin, m);
            return Ok(());
        }
        let refused = lock(&self.hooks).refuse_hello_fields.clone();
        if refused.iter().any(|f| m.contains_key(*f)) {
            return Err(proto::Error::Protocol("invalid fields in hello".into())); // as an older peer
        }
        conn.set_types(proto::PEER_CLIENT, proto::PEER_SERVER, false);
        let mut hello = Map::new();
        hello.insert("id".into(), json!(self.store.id()));
        hello.insert("name".into(), json!(self.store.hostname));
        if m.contains_key("caps") {
            hello.insert("caps".into(), json!(self.caps())); // only to a peer that knows the field
        }
        if let Some(s) = lock(&self.st).served.iter_mut().find(|s| s.id == served) {
            s.channel = true;
        }
        conn.send("hello", Value::Object(hello))?;
        if let Some(name) = m["name"].as_str() {
            self.store.set_peer_name(pid, name);
        }
        if let Some(a) = addr {
            let port = m
                .get("port")
                .and_then(Value::as_u64)
                .map(|p| p as u16)
                .unwrap_or(PORT); // older peers don't send it
            self.learn_address(pid, &a.ip().to_string(), port);
        }
        while self.running() {
            let m = match conn.recv_msg(Some(SERVER_IDLE)) {
                Err(proto::Error::Idle) => return Ok(()),
                other => other?,
            };
            if self.store.peer_by_pin(pin).as_deref() != Some(pid) {
                // Removed (or re-paired with another key) while this session was open:
                // what it authenticated no longer grants anything.
                conn.send("error", json!({"reason": "unknown_peer"}))?;
                return Ok(());
            }
            let seq = m["seq"].clone();
            match m["type"].as_str().unwrap_or("") {
                "ping" => {
                    conn.send("pong", json!({"re": seq}))?;
                }
                "switch" => match self.incoming_switch(pid, &m) {
                    Ok(()) => {
                        conn.send("switch_ok", json!({"re": seq}))?;
                    }
                    Err(reason) => {
                        conn.send("switch_refused", json!({"re": seq, "reason": reason}))?;
                    }
                },
                "clipboard" => {
                    self.incoming_clipboard(pid, m["text"].as_str().unwrap_or(""));
                    conn.send("ack", json!({"re": seq}))?;
                }
                "drag" => {
                    conn.send("ack", json!({"re": seq}))?; // first: the sender is about to switch
                    self.incoming_drag(pid, &m);
                }
                "unpair" => {
                    conn.send("ack", json!({"re": seq}))?;
                    self.remote_unpair(pid);
                    return Ok(());
                }
                t => return Err(proto::Error::Protocol(format!("{t} out of place"))),
            }
        }
        Ok(())
    }
}
