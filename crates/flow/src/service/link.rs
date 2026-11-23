//! Our outgoing authenticated connection to one paired peer. One request at a time
//! (request → answer), serialised by `req`, which also holds the live connection.
//!
//! Where to connect: the address the user typed, the last one that worked (or that the
//! peer reached us from), the last discovery announcement — in that order. Whenever that
//! list changes the link retries at once instead of waiting out its back-off.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use super::{FlowService, PING_EVERY};
use crate::CAPS;
use crate::proto::{self, Conn, Msg};
use crate::store::{Addr, Peer};
use crate::tls::{self, ConnectError};
use crate::util::{TimedGuard, TimedMutex, lock};

/// What `hello` says, newest first. An older peer answers `error {protocol}` to a field it
/// doesn't know (`caps` came after `port`): say hello again with the next variant.
const HELLO_VARIANTS: [&[&str]; 3] = [&["port", "caps"], &["port"], &[]];

#[derive(Debug)]
pub struct PeerUnavailable(pub String);

impl std::fmt::Display for PeerUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub(crate) struct LinkState {
    pub online: bool,
    /// Where the live connection goes.
    pub addr: Option<Addr>,
    /// Last discovery announcement (unauthenticated).
    pub hint: Option<Addr>,
    pub last_ok: Instant,
    pub next_try: Instant,
    pub backoff: Duration,
    pub down_since: Instant,
    pub tried: Vec<Addr>,
    pub identity_error: bool,
    pub error: String,
    /// What the peer said it supports (hello.caps).
    pub caps: HashSet<String>,
}

enum Handshake {
    Ok(Conn, Msg),
    Mismatch,
    OldPeer,
    Failed,
}

pub struct PeerLink {
    svc: Weak<FlowService>,
    pub pid: String,
    req: TimedMutex<Option<Conn>>,
    pub(crate) st: Mutex<LinkState>,
}

impl PeerLink {
    pub(crate) fn new(svc: Weak<FlowService>, pid: &str) -> Arc<Self> {
        let now = Instant::now();
        Arc::new(Self {
            svc,
            pid: pid.to_string(),
            req: TimedMutex::new(None),
            st: Mutex::new(LinkState {
                online: false,
                addr: None,
                hint: None,
                last_ok: now,
                next_try: now,
                backoff: Duration::from_secs(1),
                down_since: now,
                tried: Vec::new(),
                identity_error: false,
                error: String::new(),
                caps: HashSet::new(),
            }),
        })
    }

    pub fn online(&self) -> bool {
        lock(&self.st).online
    }

    pub fn addr(&self) -> Option<Addr> {
        lock(&self.st).addr.clone()
    }

    pub fn hint(&self) -> Option<Addr> {
        lock(&self.st).hint.clone()
    }

    pub fn set_hint(&self, hint: Option<Addr>) {
        lock(&self.st).hint = hint;
    }

    pub fn caps(&self) -> HashSet<String> {
        lock(&self.st).caps.clone()
    }

    /// Tests: as with a Glisa from before `caps`.
    #[doc(hidden)]
    pub fn set_caps(&self, caps: HashSet<String>) {
        lock(&self.st).caps = caps;
    }

    pub fn identity_error(&self) -> bool {
        lock(&self.st).identity_error
    }

    /// Where to look for the peer, in order, with where each address came from.
    pub fn candidates(&self, peer: Option<&Peer>) -> Vec<(Addr, &'static str)> {
        let hint = self.hint();
        let mut out: Vec<(Addr, &'static str)> = Vec::new();
        let (manual, learned) = match peer {
            Some(p) => (p.manual_address.clone(), p.address.clone()),
            None => (None, None),
        };
        for (addr, source) in [
            (manual, "manual"),
            (learned, "learned"),
            (hint, "discovered"),
        ] {
            if let Some(a) = addr {
                if !out.iter().any(|(x, _)| *x == a) {
                    out.push((a, source));
                }
            }
        }
        out
    }

    fn addresses(&self, svc: &FlowService) -> Vec<Addr> {
        self.candidates(svc.store.peer(&self.pid).as_ref())
            .into_iter()
            .map(|(a, _)| a)
            .collect()
    }

    /// Try again at the next round (a new address to try).
    pub fn retry_now(&self) {
        let mut st = lock(&self.st);
        st.next_try = Instant::now();
        st.backoff = Duration::from_secs(1);
    }

    fn connect(
        &self,
        slot: &mut TimedGuard<'_, Option<Conn>>,
        timeout: Duration,
    ) -> Result<(), PeerUnavailable> {
        let svc = self
            .svc
            .upgrade()
            .ok_or_else(|| PeerUnavailable("stopped".into()))?;
        let peer = svc
            .store
            .peer(&self.pid)
            .ok_or_else(|| PeerUnavailable("not paired".into()))?;
        let addrs = self.addresses(&svc);
        if addrs.is_empty() {
            return Err(PeerUnavailable("unknown address".into()));
        }
        let mut mismatch = false;
        for addr in addrs {
            // `port` in hello tells the peer where we listen, `caps` what we support. A
            // peer from before those fields answers `error {protocol}`: say hello again
            // with fewer of them.
            let mut result = Handshake::Failed;
            for variant in HELLO_VARIANTS {
                result = self.handshake(&svc, &addr, &peer, timeout, variant);
                if !matches!(result, Handshake::OldPeer) {
                    break;
                }
            }
            match result {
                Handshake::Mismatch => mismatch = true,
                Handshake::Ok(conn, hello) => {
                    let caps: HashSet<String> = hello
                        .get("caps")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .filter(|c| CAPS.contains(c))
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                    svc.store.set_address(&self.pid, &addr.0, addr.1);
                    if let Some(name) = hello.get("name").and_then(Value::as_str) {
                        svc.store.set_peer_name(&self.pid, name);
                    }
                    **slot = Some(conn);
                    {
                        let mut st = lock(&self.st);
                        st.online = true;
                        st.addr = Some(addr);
                        st.last_ok = Instant::now();
                        st.backoff = Duration::from_secs(1);
                        st.error.clear();
                        st.identity_error = false;
                        st.caps = caps;
                    }
                    svc.peers_changed();
                    return Ok(());
                }
                _ => {}
            }
        }
        if mismatch {
            lock(&self.st).identity_error = true;
            svc.identity_mismatch(&self.pid);
        }
        let error = lock(&self.st).error.clone();
        Err(PeerUnavailable(if error.is_empty() {
            "no answer".into()
        } else {
            error
        }))
    }

    fn handshake(
        &self,
        svc: &FlowService,
        addr: &Addr,
        peer: &Peer,
        timeout: Duration,
        variant: &[&str],
    ) -> Handshake {
        let set_error = |e: String| lock(&self.st).error = e;
        let Some(id) = svc.identity() else {
            set_error("stopped".into());
            return Handshake::Failed;
        };
        let cfg = match tls::client_config(&id, &peer.pin) {
            Ok(c) => c,
            Err(e) => {
                set_error(e.to_string());
                return Handshake::Failed;
            }
        };
        let mut conn = match tls::connect(
            &addr.0,
            addr.1,
            cfg,
            timeout,
            proto::PEER_SERVER,
            proto::PEER_CLIENT,
        ) {
            Ok(c) => c,
            Err(ConnectError::Mismatch) => return Handshake::Mismatch, // not the pinned certificate
            Err(ConnectError::Io(e)) => {
                set_error(e.to_string());
                return Handshake::Failed;
            }
        };
        if conn
            .peer_cert
            .as_deref()
            .and_then(|c| crate::crypto::pin_of_der(c).ok())
            .as_deref()
            != Some(peer.pin.as_str())
        {
            return Handshake::Mismatch;
        }
        let mut fields = Map::new();
        fields.insert("id".into(), json!(svc.store.id()));
        fields.insert("name".into(), json!(svc.store.hostname));
        if variant.contains(&"port") {
            fields.insert("port".into(), json!(svc.port()));
        }
        if variant.contains(&"caps") {
            fields.insert("caps".into(), json!(svc.caps()));
        }
        let result = conn
            .send("hello", Value::Object(fields))
            .and_then(|_| conn.recv_msg(Some(Duration::from_secs(5))));
        match result {
            Ok(m) => {
                if !variant.is_empty() && m["type"] == "error" && m["reason"] == "protocol" {
                    return Handshake::OldPeer;
                }
                if m["type"] != "hello" || m["id"] != self.pid.as_str() {
                    set_error("unexpected hello".into());
                    return Handshake::Failed;
                }
                Handshake::Ok(conn, m)
            }
            Err(e) => {
                set_error(e.to_string());
                Handshake::Failed
            }
        }
    }

    /// One request and its answer (`re` must match). Connects first if needed.
    pub fn request(
        &self,
        type_: &str,
        timeout: Duration,
        fields: Value,
    ) -> Result<Msg, PeerUnavailable> {
        let Some(mut g) = self.req.lock_for(timeout) else {
            return Err(PeerUnavailable("busy".into()));
        };
        let connect_timeout = (timeout + Duration::from_secs(1)).min(Duration::from_secs(2));
        let mut reused = g.is_some();
        if !reused {
            self.connect(&mut g, connect_timeout)?;
        }
        let result = loop {
            let conn = g.as_mut().expect("connected");
            let result = conn.send(type_, fields.clone()).and_then(|seq| {
                let m = conn.recv_msg(Some(timeout))?;
                if m.get("re").and_then(Value::as_u64) != Some(seq) {
                    return Err(proto::Error::Protocol("mismatched answer".into()));
                }
                Ok(m)
            });
            // The peer closed the connection we kept (it restarted, or it ended its peer
            // channels to announce new capabilities): connect again and ask once more.
            // Every request is safe to repeat; nothing was answered on the old connection.
            let closed = matches!(&result, Err(proto::Error::Io(e)) if matches!(e.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::NotConnected));
            if reused && closed {
                reused = false;
                self.drop_conn(&mut g);
                if self.connect(&mut g, connect_timeout).is_ok() {
                    continue;
                }
            }
            break result;
        };
        match result {
            Ok(m) => {
                lock(&self.st).last_ok = Instant::now();
                Ok(m)
            }
            Err(e) => {
                self.drop_conn(&mut g);
                Err(PeerUnavailable(match e {
                    proto::Error::Idle => "no answer".into(),
                    e => e.to_string(),
                }))
            }
        }
    }

    pub(crate) fn maintain(&self) {
        let (online, last_ok) = {
            let st = lock(&self.st);
            (st.online, st.last_ok)
        };
        let Some(svc) = self.svc.upgrade() else {
            return;
        };
        if !online {
            let addrs = self.addresses(&svc);
            {
                let mut st = lock(&self.st);
                if addrs != st.tried {
                    // a new address (typed, learned or announced): try it now
                    st.tried = addrs.clone();
                    st.next_try = Instant::now();
                    st.backoff = Duration::from_secs(1);
                }
                if Instant::now() < st.next_try || addrs.is_empty() {
                    return;
                }
            }
            let Some(mut g) = self.req.try_lock() else {
                return;
            };
            if g.is_some() {
                return;
            }
            if self.connect(&mut g, Duration::from_secs(2)).is_err() {
                let mut st = lock(&self.st);
                st.next_try = Instant::now() + st.backoff;
                st.backoff = (st.backoff * 2).min(Duration::from_secs(15));
            }
        } else if last_ok.elapsed() > PING_EVERY
            && self
                .request("ping", Duration::from_secs(3), json!({}))
                .is_err()
        {
            lock(&self.st).next_try = Instant::now() + Duration::from_secs(1);
        }
    }

    fn drop_conn(&self, slot: &mut TimedGuard<'_, Option<Conn>>) {
        if let Some(mut c) = slot.take() {
            c.close();
            {
                let mut st = lock(&self.st);
                st.online = false;
                st.addr = None;
                st.down_since = Instant::now();
            }
            if let Some(svc) = self.svc.upgrade() {
                svc.peers_changed();
            }
        }
    }

    pub fn close(&self) {
        let mut g = self.req.lock();
        self.drop_conn(&mut g);
    }
}
