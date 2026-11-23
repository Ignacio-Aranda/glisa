//! Pairing (docs/FLOW.md §7): short authentication string with commitment, as in ZRTP
//! (RFC 6189) / Bluetooth numeric comparison.
//!
//! 1. Introduction: initiator → responder:`port+1` (server-authenticated TLS), `pair_intro
//!    {id, name, cert, port}` → `pair_intro_ok {id, name, cert}`; the initiator checks the
//!    answered certificate is the one used in that handshake.
//! 2. Mutual TLS 1.3 session initiator → responder:`port` with exactly those two
//!    certificates; both pins are checked.
//! 3. `pair_hello` both ways (ids must match the introduction).
//! 4. Both commitments before either nonce: `pair_commit` ×2, then `pair_reveal` ×2.
//! 5. SAS = 6 digits of SHA-256(pins ‖ nonces), shown on both computers.
//! 6. Each side sends `pair_confirm {accept}`; a computer stores the peer only after it has
//!    sent its own "yes" and received the other's.
//!
//! One attempt at a time; 3 attempts per window, ≥ 2 s apart; the window lasts 120 s; the
//! mutual session must start within 15 s of the introduction.

use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glisa_i18n::tr;
use serde_json::{Value, json};

use super::{FlowService, PAIR_ATTEMPTS, PAIR_GAP, PeerLink, refused_text};
use crate::crypto::{commitment, new_nonce, pin_of_der, pin_of_pem, sas};
use crate::proto::{self, Conn, Msg, RawSocket};
use crate::store::{Addr, clean_name};
use crate::text::{Text, plain};
use crate::tls::{self, ConnectError};
use crate::util::{hex, lock, unhex};
use crate::{PROTOCOL_VERSION, pair_port};

#[derive(Debug, Clone)]
pub(crate) struct PairState {
    pub expires: Instant,
    /// waiting | connecting | confirm | done | failed | expired
    pub state: &'static str,
    pub attempts: u32,
    pub last_attempt: Option<Instant>,
    pub role: Option<&'static str>,
    pub peer_id: String,
    pub peer_name: String,
    pub peer_cert: String,
    pub peer_pin: String,
    pub address: Option<Addr>,
    pub sas: Option<String>,
    pub local: Option<bool>,
    pub remote: Option<bool>,
    pub reason: String,
    pub session_deadline: Instant,
    pub session_taken: bool,
}

impl PairState {
    fn reset(&mut self, role: Option<&'static str>) {
        self.role = role;
        self.peer_id.clear();
        self.peer_name.clear();
        self.peer_cert.clear();
        self.peer_pin.clear();
        self.address = None;
        self.sas = None;
        self.local = None;
        self.remote = None;
        self.reason.clear();
        self.session_deadline = Instant::now();
        self.session_taken = false;
    }

    fn expired(&self) -> bool {
        Instant::now() >= self.expires
    }

    fn busy(&self) -> bool {
        matches!(self.state, "connecting" | "confirm")
    }
}

/// One pairing window.
pub struct Pairing {
    pub(crate) m: Mutex<PairState>,
    listener: Mutex<Option<super::Listener>>,
    conn: Mutex<Option<Arc<dyn RawSocket>>>,
}

impl Pairing {
    fn new(seconds: f64) -> Arc<Self> {
        let now = Instant::now();
        let mut s = PairState {
            expires: now + Duration::from_secs_f64(seconds.max(0.0)),
            state: "waiting",
            attempts: 0,
            last_attempt: None,
            role: None,
            peer_id: String::new(),
            peer_name: String::new(),
            peer_cert: String::new(),
            peer_pin: String::new(),
            address: None,
            sas: None,
            local: None,
            remote: None,
            reason: String::new(),
            session_deadline: now,
            session_taken: false,
        };
        s.reset(None);
        Arc::new(Self {
            m: Mutex::new(s),
            listener: Mutex::new(None),
            conn: Mutex::new(None),
        })
    }

    pub(crate) fn close(&self) {
        if let Some(l) = lock(&self.listener).take() {
            l.stop();
        }
        if let Some(c) = lock(&self.conn).take() {
            c.shutdown();
        }
    }

    fn set_conn(&self, c: &Conn) {
        *lock(&self.conn) = Some(c.shutdown_handle());
    }

    pub fn has_listener(&self) -> bool {
        lock(&self.listener).is_some()
    }

    /// Tests: attempts used in this window.
    #[doc(hidden)]
    pub fn set_attempts(&self, n: u32) {
        lock(&self.m).attempts = n;
    }
}

/// The other computer said no (or the security check failed).
struct Refused(String);

impl Refused {
    fn new(reason: &str) -> Self {
        Refused(if proto::REASONS.contains(&reason) {
            reason.to_string()
        } else {
            "protocol".into()
        })
    }
}

enum PairErr {
    Refused(String),
    Mismatch,
    Protocol,
}

impl From<Refused> for PairErr {
    fn from(r: Refused) -> Self {
        PairErr::Refused(r.0)
    }
}

impl From<proto::Error> for PairErr {
    fn from(_: proto::Error) -> Self {
        PairErr::Protocol
    }
}

fn reason_of(m: &Msg) -> String {
    m.get("reason")
        .and_then(Value::as_str)
        .unwrap_or("protocol")
        .to_string()
}

fn expect(conn: &mut Conn, type_: &str, timeout: Duration) -> Result<Msg, PairErr> {
    let m = conn.recv_msg(Some(timeout))?;
    if m["type"] == "pair_refused" || m["type"] == "error" {
        return Err(Refused::new(&reason_of(&m)).into());
    }
    if m["type"] != type_ {
        return Err(PairErr::Protocol);
    }
    Ok(m)
}

impl FlowService {
    fn current_pairing(&self) -> Option<Arc<Pairing>> {
        lock(&self.st).pairing.clone()
    }

    fn is_current(&self, p: &Arc<Pairing>) -> bool {
        lock(&self.st)
            .pairing
            .as_ref()
            .is_some_and(|c| Arc::ptr_eq(c, p))
    }

    /// The pairing in progress (tests).
    pub fn pairing(&self) -> Option<Arc<Pairing>> {
        self.current_pairing()
    }

    /// Pin of the computer being paired right now, trusted for that one session only.
    pub(crate) fn pairing_candidate_pin(&self) -> Option<String> {
        let p = self.current_pairing()?;
        let s = lock(&p.m);
        (s.role == Some("responder") && s.state == "connecting" && !s.peer_cert.is_empty())
            .then(|| s.peer_pin.clone())
    }

    pub(crate) fn expire_pairing_window(&self) {
        let Some(p) = self.current_pairing() else {
            return;
        };
        let listener = {
            let mut s = lock(&p.m);
            if s.expired() && s.state == "waiting" {
                s.state = "expired";
                s.reason = "expired".into();
                lock(&p.listener).take()
            } else {
                None
            }
        };
        if let Some(l) = listener {
            l.stop();
            self.emit_flow("pairing");
        }
    }

    pub fn pair_start(&self) -> Result<Value, String> {
        if !self.running() {
            return Err(tr!("Turn on Flow to pair computers"));
        }
        if let Some(old) = self.current_pairing() {
            if lock(&old.m).busy() {
                return Err(tr!("Pairing is already in progress"));
            }
            old.close();
        }
        let p = Pairing::new(*lock(&self.pairing_seconds));
        let me = self.me.clone();
        let port = pair_port(self.port());
        let listener = super::Listener::start(
            self.net.bind,
            port,
            "flow-pair",
            1.0,
            3,
            Arc::new(move |tcp, addr| {
                if let Some(s) = me.upgrade() {
                    s.serve_intro(tcp, addr);
                }
            }),
        )
        .map_err(|e| tr!("Can't use TCP port {port}: {error}", port = port, error = e))?;
        *lock(&p.listener) = Some(listener);
        lock(&self.st).pairing = Some(p);
        self.emit_flow("pairing");
        Ok(self.pairing_status())
    }

    pub fn pair_cancel(&self) {
        let p = lock(&self.st).pairing.take();
        if let Some(p) = p {
            {
                let mut s = lock(&p.m);
                if s.state == "confirm" {
                    s.local = Some(false);
                }
            }
            p.close();
            self.emit_flow("pairing");
        }
    }

    fn pair_fail(&self, p: &Arc<Pairing>, reason: &str, conn: Option<&mut Conn>, tell: bool) {
        if let Some(c) = conn {
            if tell {
                let _ = c.send("pair_refused", json!({"reason": reason}));
            }
            c.close();
        }
        {
            let mut s = lock(&p.m);
            if !matches!(s.state, "done" | "failed" | "expired") {
                s.state = if reason == "expired" {
                    "expired"
                } else {
                    "failed"
                };
                s.reason = reason.to_string();
            }
            s.sas = None;
        }
        log::info!("Flow: pairing cancelled ({reason})");
        self.emit_flow("pairing");
    }

    pub fn pair_confirm(&self, accept: bool) -> Result<Value, String> {
        let p = self.current_pairing();
        match p {
            Some(p) if lock(&p.m).state == "confirm" => lock(&p.m).local = Some(accept),
            _ => return Err(tr!("No pairing is waiting for confirmation")),
        }
        Ok(self.pairing_status())
    }

    pub fn pair_request(&self, peer: Option<&str>, address: Option<&str>) -> Result<Value, String> {
        if !self.running() {
            return Err(tr!("Turn on Flow to pair computers"));
        }
        let (target, name) = if let Some(peer) = peer {
            let d = lock(&self.st).discovered.get(peer).cloned();
            let Some(d) = d else {
                return Err(tr!("That computer can't be found on the network"));
            };
            if d.version != PROTOCOL_VERSION {
                return Err(glisa_i18n::format(
                    &glisa_i18n::gettext(refused_text("version").expect("known")),
                    &[("name", &d.name), ("app", &glisa_common::APP_NAME)],
                ));
            }
            (d.address.clone(), d.name.clone())
        } else if let Some(address) = address {
            match crate::address::parse_address(address)? {
                Some(t) => {
                    let name = t.0.clone();
                    (t, name)
                }
                None => return Err(tr!("Say which computer to pair with")),
            }
        } else {
            return Err(tr!("Say which computer to pair with"));
        };
        let p = {
            let mut st = lock(&self.st);
            let mut p = st.pairing.clone();
            if let Some(cur) = &p {
                if lock(&cur.m).busy() {
                    return Err(tr!("Pairing is already in progress"));
                }
            }
            let fresh = p.as_ref().is_none_or(|c| {
                let s = lock(&c.m);
                s.expired() || matches!(s.state, "done" | "expired")
            });
            if fresh {
                if let Some(old) = p.take() {
                    old.close();
                }
                let n = Pairing::new(*lock(&self.pairing_seconds));
                st.pairing = Some(n.clone());
                p = Some(n);
            }
            let p = p.expect("a pairing");
            {
                let mut s = lock(&p.m);
                if s.attempts >= PAIR_ATTEMPTS {
                    return Err(tr!(
                        "Too many attempts: wait a moment and open pairing again"
                    ));
                }
                s.attempts += 1;
                s.last_attempt = Some(Instant::now());
                s.reset(Some("initiator"));
                s.state = "connecting";
                s.peer_name = clean_name(&name);
                s.address = Some(target.clone());
            }
            p
        };
        let svc = self.arc();
        let p2 = p.clone();
        std::thread::Builder::new()
            .name("flow-pair-init".into())
            .spawn(move || svc.pair_initiator(p2, target))
            .map_err(|e| e.to_string())?;
        self.emit_flow("pairing");
        Ok(self.pairing_status())
    }

    // ---- responder: introduction listener (port + 1)
    fn serve_intro(&self, tcp: TcpStream, addr: SocketAddr) {
        let Some(id) = self.identity() else { return };
        let Ok(cfg) = tls::intro_server_config(&id) else {
            return;
        };
        let Ok(mut conn) = tls::accept(tcp, cfg, proto::INTRO_CLIENT, proto::INTRO_SERVER) else {
            return;
        };
        if let Ok(m) = conn.recv_msg(Some(Duration::from_secs(5))) {
            let reason = self.accept_intro(&m, addr);
            if reason.is_empty() {
                let _ = conn.send(
                    "pair_intro_ok",
                    json!({"id": self.store.id(), "name": self.store.hostname, "cert": id.pem}),
                );
            } else {
                let _ = conn.send("pair_refused", json!({"reason": reason}));
            }
        }
        conn.close();
    }

    fn accept_intro(&self, m: &Msg, addr: SocketAddr) -> &'static str {
        let Ok(pin) = pin_of_pem(m["cert"].as_str().unwrap_or("")) else {
            return "protocol";
        };
        if pin == self.store.pin() || m["id"] == self.store.id().as_str() {
            return "protocol";
        }
        let Some(p) = self.current_pairing() else {
            return "not_pairing";
        };
        {
            let mut s = lock(&p.m);
            if s.expired() || matches!(s.state, "done" | "expired") {
                return "not_pairing";
            }
            if s.busy() {
                return "busy";
            }
            if s.attempts >= PAIR_ATTEMPTS || s.last_attempt.is_some_and(|t| t.elapsed() < PAIR_GAP)
            {
                return "rate_limited";
            }
            s.attempts += 1;
            s.last_attempt = Some(Instant::now());
            s.reset(Some("responder"));
            s.state = "connecting";
            s.peer_id = m["id"].as_str().unwrap_or("").to_string();
            s.peer_name = clean_name(m["name"].as_str().unwrap_or(""));
            s.peer_cert = m["cert"].as_str().unwrap_or("").to_string();
            s.peer_pin = pin;
            s.address = Some((
                addr.ip().to_string(),
                m["port"].as_u64().unwrap_or(0) as u16,
            ));
            s.session_deadline = Instant::now() + Duration::from_secs(15);
        }
        let svc = self.arc();
        let p2 = p.clone();
        let _ = std::thread::Builder::new()
            .name("flow-pair-timeout".into())
            .spawn(move || {
                std::thread::sleep(Duration::from_millis(15_200));
                let stale = {
                    let s = lock(&p2.m);
                    s.state == "connecting" && !s.session_taken
                };
                if stale {
                    svc.pair_fail(&p2, "expired", None, false);
                }
            });
        self.emit_flow("pairing");
        ""
    }

    pub(crate) fn take_pair_session(&self, pin: &str) -> Option<Arc<Pairing>> {
        let p = self.current_pairing()?;
        let mut s = lock(&p.m);
        if s.role == Some("responder")
            && s.state == "connecting"
            && !s.session_taken
            && s.peer_pin == pin
            && Instant::now() < s.session_deadline
            && !s.expired()
        {
            s.session_taken = true;
            drop(s);
            return Some(p);
        }
        None
    }

    pub(crate) fn pair_responder(&self, mut conn: Conn, p: Arc<Pairing>) {
        p.set_conn(&conn);
        let (peer_id, peer_pin) = {
            let s = lock(&p.m);
            (s.peer_id.clone(), s.peer_pin.clone())
        };
        let result = (|| -> Result<Option<String>, PairErr> {
            let m = expect(&mut conn, "pair_hello", Duration::from_secs(5))?;
            if m["id"] != peer_id.as_str() {
                return Err(PairErr::Protocol); // identity differs from the introduced one
            }
            conn.send(
                "pair_hello",
                json!({"id": self.store.id(), "name": self.store.hostname}),
            )?;
            let c_i = expect(&mut conn, "pair_commit", Duration::from_secs(10))?["commit"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let n_r = new_nonce();
            conn.send(
                "pair_commit",
                json!({"commit": commitment(crate::wire::ROLE_RESPONDER, &self.store.pin(), &n_r)}),
            )?;
            let reveal = expect(&mut conn, "pair_reveal", Duration::from_secs(10))?;
            let n_i = unhex(reveal["nonce"].as_str().unwrap_or("")).ok_or(PairErr::Protocol)?;
            if commitment(crate::wire::ROLE_INITIATOR, &peer_pin, &n_i) != c_i {
                return Err(PairErr::Mismatch);
            }
            conn.send("pair_reveal", json!({"nonce": hex(&n_r)}))?;
            Ok(Some(sas(&peer_pin, &self.store.pin(), &n_i, &n_r)))
        })();
        match result {
            Ok(Some(code)) => self.pair_decide(conn, p, code),
            Ok(None) => {}
            Err(PairErr::Mismatch) => self.pair_fail(&p, "mismatch", Some(&mut conn), true),
            Err(PairErr::Refused(r)) => self.pair_fail(&p, &r, Some(&mut conn), false),
            Err(PairErr::Protocol) => self.pair_fail(&p, "protocol", Some(&mut conn), false),
        }
    }

    // ---- initiator
    fn pair_initiator(&self, p: Arc<Pairing>, target: Addr) {
        let (host, port) = target;
        let mut conn: Option<Conn> = None;
        let result = (|| -> Result<Option<String>, PairErr> {
            let id = self.identity().ok_or(PairErr::Protocol)?;
            let cfg = tls::intro_client_config().map_err(|_| PairErr::Protocol)?;
            let mut intro = tls::connect(
                &host,
                pair_port(port),
                cfg,
                Duration::from_secs(5),
                proto::INTRO_SERVER,
                proto::INTRO_CLIENT,
            )
            .map_err(|_| PairErr::Refused("not_pairing".into()))?;
            let server_pin = intro
                .peer_cert
                .as_deref()
                .and_then(|c| pin_of_der(c).ok())
                .ok_or(PairErr::Protocol)?;
            intro.send(
                "pair_intro",
                json!({"id": self.store.id(), "name": self.store.hostname, "cert": id.pem, "port": self.port()}),
            )?;
            let m = intro.recv_msg(Some(Duration::from_secs(5)));
            intro.close();
            let m = m?;
            if m["type"] == "pair_refused" {
                return Err(Refused::new(&reason_of(&m)).into());
            }
            let cert = m["cert"].as_str().unwrap_or("").to_string();
            let peer_pin = pin_of_pem(&cert).map_err(|_| PairErr::Protocol)?;
            if peer_pin != server_pin
                || peer_pin == self.store.pin()
                || m["id"] == self.store.id().as_str()
            {
                return Err(PairErr::Refused("mismatch".into()));
            }
            let peer_id = m["id"].as_str().unwrap_or("").to_string();
            {
                if !self.is_current(&p) {
                    return Ok(None);
                }
                let mut s = lock(&p.m);
                s.peer_id = peer_id.clone();
                s.peer_name = clean_name(m["name"].as_str().unwrap_or(""));
                s.peer_cert = cert.clone();
                s.peer_pin = peer_pin.clone();
            }
            let cfg = tls::client_config(&id, &peer_pin).map_err(|_| PairErr::Protocol)?;
            let c = match tls::connect(
                &host,
                port,
                cfg,
                Duration::from_secs(5),
                proto::PAIR,
                proto::PAIR,
            ) {
                Ok(c) => c,
                Err(ConnectError::Mismatch) => return Err(PairErr::Refused("mismatch".into())),
                Err(ConnectError::Io(_)) => return Err(PairErr::Protocol),
            };
            p.set_conn(&c);
            let c = conn.insert(c);
            if c.peer_cert
                .as_deref()
                .and_then(|d| pin_of_der(d).ok())
                .as_deref()
                != Some(peer_pin.as_str())
            {
                return Err(PairErr::Refused("mismatch".into()));
            }
            c.send(
                "pair_hello",
                json!({"id": self.store.id(), "name": self.store.hostname}),
            )?;
            if expect(c, "pair_hello", Duration::from_secs(5))?["id"] != peer_id.as_str() {
                return Err(PairErr::Refused("mismatch".into()));
            }
            let n_i = new_nonce();
            c.send(
                "pair_commit",
                json!({"commit": commitment(crate::wire::ROLE_INITIATOR, &self.store.pin(), &n_i)}),
            )?;
            let c_r = expect(c, "pair_commit", Duration::from_secs(10))?["commit"]
                .as_str()
                .unwrap_or("")
                .to_string();
            c.send("pair_reveal", json!({"nonce": hex(&n_i)}))?;
            let reveal = expect(c, "pair_reveal", Duration::from_secs(10))?;
            let n_r = unhex(reveal["nonce"].as_str().unwrap_or("")).ok_or(PairErr::Protocol)?;
            if commitment(crate::wire::ROLE_RESPONDER, &peer_pin, &n_r) != c_r {
                return Err(PairErr::Mismatch);
            }
            Ok(Some(sas(&self.store.pin(), &peer_pin, &n_i, &n_r)))
        })();
        match result {
            Ok(Some(code)) => {
                let c = conn.take().expect("pairing session");
                self.pair_decide(c, p, code);
            }
            Ok(None) => {
                if let Some(mut c) = conn {
                    c.close();
                }
            }
            Err(PairErr::Mismatch) => self.pair_fail(&p, "mismatch", conn.as_mut(), true),
            Err(PairErr::Refused(r)) => self.pair_fail(&p, &r, conn.as_mut(), false),
            Err(PairErr::Protocol) => self.pair_fail(&p, "protocol", conn.as_mut(), false),
        }
    }

    /// Both users compare the SAS; the pin is stored only if BOTH accept.
    fn pair_decide(&self, mut conn: Conn, p: Arc<Pairing>, code: String) {
        if !self.is_current(&p) {
            conn.close();
            return;
        }
        {
            let mut s = lock(&p.m);
            s.sas = Some(code);
            s.state = "confirm";
        }
        self.emit_flow("pairing");
        let mut sent = false;
        loop {
            if !self.is_current(&p) {
                // cancelled here
                self.pair_fail(&p, "rejected", Some(&mut conn), true);
                return;
            }
            if lock(&p.m).expired() {
                self.pair_fail(&p, "expired", Some(&mut conn), true);
                return;
            }
            let (local, remote) = {
                let s = lock(&p.m);
                (s.local, s.remote)
            };
            if let Some(accept) = local {
                if !sent {
                    if conn
                        .send("pair_confirm", json!({"accept": accept}))
                        .is_err()
                    {
                        self.pair_fail(&p, "protocol", Some(&mut conn), false);
                        return;
                    }
                    sent = true;
                    if !accept {
                        self.pair_fail(&p, "rejected", Some(&mut conn), false);
                        return;
                    }
                }
            }
            if remote.is_none() {
                match conn.recv_msg(Some(Duration::from_millis(200))) {
                    Err(proto::Error::Idle) => continue,
                    Err(_) => {
                        self.pair_fail(&p, "protocol", Some(&mut conn), false);
                        return;
                    }
                    Ok(m) => {
                        if m["type"] == "pair_confirm" {
                            let accept = m["accept"].as_bool().unwrap_or(false);
                            lock(&p.m).remote = Some(accept);
                            self.emit_flow("pairing");
                            if !accept {
                                self.pair_fail(&p, "rejected", Some(&mut conn), false);
                                return;
                            }
                        } else if m["type"] == "pair_refused" || m["type"] == "error" {
                            let r = Refused::new(&reason_of(&m)).0;
                            self.pair_fail(&p, &r, Some(&mut conn), false);
                            return;
                        } else {
                            self.pair_fail(&p, "protocol", Some(&mut conn), false);
                            return;
                        }
                    }
                }
            } else if local.is_none() {
                std::thread::sleep(Duration::from_millis(50));
            }
            let s = lock(&p.m).clone();
            if sent && s.local == Some(true) && s.remote == Some(true) {
                // our "yes" has gone out and theirs has arrived
                self.pair_finish(&p);
                conn.close();
                return;
            }
        }
    }

    fn pair_finish(&self, p: &Arc<Pairing>) {
        let s = lock(&p.m).clone();
        if let Err(e) =
            self.store
                .add_peer(&s.peer_id, &s.peer_name, &s.peer_cert, s.address.clone())
        {
            log::warn!("Flow: couldn't store the paired computer: {e}");
        }
        let link = {
            let mut st = lock(&self.st);
            {
                let mut ps = lock(&p.m);
                ps.state = "done";
                ps.sas = None;
            }
            let old = st.links.remove(&s.peer_id);
            let link = PeerLink::new(self.me.clone(), &s.peer_id);
            if let Some(addr) = &s.address {
                link.set_hint(Some(addr.clone()));
            }
            st.links.insert(s.peer_id.clone(), link);
            old
        };
        if let Some(old) = link {
            old.close();
        }
        if let Some(l) = lock(&p.listener).take() {
            std::thread::spawn(move || l.stop());
        }
        log::info!("Flow: paired with {}", s.peer_name);
        self.notice(
            Text::new(glisa_i18n::n_!("Paired with {name}"))
                .arg("name", plain(s.peer_name.clone())),
            "ok",
            None,
            Duration::ZERO,
        );
        self.peers_changed();
        self.emit_flow("pairing");
    }

    pub fn pairing_status(&self) -> Value {
        let Some(p) = self.current_pairing() else {
            return Value::Null;
        };
        let mut s = lock(&p.m);
        if s.expired() && matches!(s.state, "waiting" | "failed") {
            s.state = "expired";
            s.reason = "expired".into();
        }
        let reason = s.reason.clone();
        let mut text = match refused_text(&reason) {
            Some(msgid) => glisa_i18n::gettext(msgid).into_owned(),
            None if !reason.is_empty() => tr!("Pairing didn't complete"),
            None => String::new(),
        };
        if reason == "rejected" && s.local == Some(false) {
            text = tr!("You said the numbers don't match. Nothing was saved");
        }
        let other = if s.peer_name.is_empty() {
            tr!("The other computer")
        } else {
            s.peer_name.clone()
        };
        let message =
            glisa_i18n::format(&text, &[("name", &other), ("app", &glisa_common::APP_NAME)]);
        json!({
            "state": s.state, "role": s.role,
            "expires_in": s.expires.saturating_duration_since(Instant::now()).as_secs(),
            "peer": if s.peer_name.is_empty() { Value::Null } else { json!({"id": s.peer_id, "name": s.peer_name}) },
            "sas": if s.state == "confirm" { json!(s.sas) } else { Value::Null },
            "local": s.local, "remote": s.remote,
            "reason": reason, "message": message,
            "attempts_left": PAIR_ATTEMPTS.saturating_sub(s.attempts),
        })
    }

    // ============================================================ peers
    pub fn remove_peer(&self, pid: &str) -> Result<bool, String> {
        if self.store.peer(pid).is_none() {
            return Err(tr!("That computer isn't paired"));
        }
        if let Some(link) = self.link(pid) {
            if self.running() {
                let _ = link.request("unpair", Duration::from_secs(1), json!({}));
            }
        }
        let link = lock(&self.st).links.remove(pid);
        if let Some(l) = link {
            l.close();
        }
        {
            let mut st = lock(&self.st);
            st.clip_hash.remove(pid);
            st.sent_keys.remove(pid);
        }
        let removed = self.store.remove_peer(pid);
        self.drop_served(pid);
        self.peers_changed();
        Ok(removed)
    }

    pub(crate) fn remote_unpair(&self, pid: &str) {
        let name = self.store.peer(pid).map(|p| p.name).unwrap_or_default();
        let link = lock(&self.st).links.remove(pid);
        if let Some(l) = link {
            std::thread::spawn(move || l.close());
        }
        self.store.remove_peer(pid);
        self.drop_served(pid);
        self.notice(
            Text::new(glisa_i18n::n_!(
                "{name} is no longer paired with this computer"
            ))
            .arg("name", plain(name)),
            "info",
            None,
            Duration::ZERO,
        );
        self.peers_changed();
    }

    pub fn reset_identity(&self) -> Result<(), Text> {
        let was = self.running.load(Ordering::SeqCst);
        for pid in self.store.peer_ids() {
            let _ = self.remove_peer(&pid);
        }
        if was {
            self.stop();
        }
        self.store.regenerate_identity().map_err(|e| e.0)?;
        if was {
            self.start_now()?;
        }
        Ok(())
    }
}
