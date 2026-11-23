//! LAN discovery: small UDP announcements on the Flow port.
//!
//! Announcements are unauthenticated by nature, so they carry nothing sensitive (instance
//! id, display name, protocol version, TCP port) and are only ever used as an address hint:
//! nothing happens because of one until the pinned TLS session with that computer succeeds.
//!
//! Broadcasts never cross a router, so the announcement also goes, directed, to every
//! address we know for a paired computer (`directed`): a computer on another subnet hears
//! from us as long as one side knows the other's address. Only our own announcement, only
//! to paired computers, never in answer to a packet (no amplification or reflection).

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::PROTOCOL_VERSION;
use crate::util::{is_hex_lower, lock, printable};

pub use crate::wire::DISCOVERY_APP as APP;
pub const MAX_PACKET: usize = 512;
/// Directed destinations per round (paired peers' addresses).
pub const MAX_DIRECTED: usize = 32;

/// A well-formed announcement or query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub kind: String,
    pub v: u64,
    pub id: String,
    pub name: String,
    pub port: u16,
}

pub fn make_packet(kind: &str, ident: &str, name: &str, port: u16) -> Vec<u8> {
    let name: String = name.chars().take(64).collect();
    serde_json::to_vec(
        &json!({"app": APP, "v": PROTOCOL_VERSION, "type": kind, "id": ident,
                               "name": name, "port": port}),
    )
    .expect("packet serializes")
}

/// The announcement, or None if it is anything but a well-formed one. Packets from other
/// protocol versions are returned (with their `v`) so the UI can say "update Glisa
/// there"; their content is never trusted.
pub fn parse_packet(data: &[u8]) -> Option<Packet> {
    if data.is_empty() || data.len() > MAX_PACKET {
        return None;
    }
    let msg = crate::proto::parse_strict(data).ok()?;
    let m = msg.as_object()?;
    let fields = ["app", "v", "type", "id", "name", "port"];
    if m.len() != fields.len() || !fields.iter().all(|f| m.contains_key(*f)) || m["app"] != APP {
        return None;
    }
    let v = m["v"].as_u64().filter(|v| (1..=1000).contains(v))?;
    let kind = m["type"]
        .as_str()
        .filter(|t| *t == "announce" || *t == "query")?;
    let id = m["id"].as_str().filter(|s| is_hex_lower(s, 16))?;
    let name = m["name"].as_str()?;
    let n = name.chars().count();
    if n == 0 || n > 64 || !printable(name) {
        return None;
    }
    let port = m["port"].as_u64().filter(|p| (1..=65534).contains(p))?;
    Some(Packet {
        kind: kind.to_string(),
        v,
        id: id.to_string(),
        name: name.to_string(),
        port: port as u16,
    })
}

/// Broadcast address of every IPv4 interface that is up (not loopback).
pub fn broadcast_addresses() -> Vec<Ipv4Addr> {
    let mut out = os_broadcasts();
    out.retain(|a| !a.is_unspecified());
    out.dedup();
    if out.is_empty() {
        out.push(Ipv4Addr::BROADCAST);
    }
    out
}

#[cfg(unix)]
fn os_broadcasts() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills a list we free with freeifaddrs below.
    if unsafe { libc::getifaddrs(&mut ifap) } != 0 {
        return out;
    }
    let mut cur = ifap;
    while !cur.is_null() {
        // SAFETY: `cur` points into the list returned by getifaddrs.
        let ifa = unsafe { &*cur };
        cur = ifa.ifa_next;
        let flags = ifa.ifa_flags as libc::c_int;
        let wanted = libc::IFF_UP | libc::IFF_BROADCAST;
        if flags & wanted != wanted || flags & libc::IFF_LOOPBACK != 0 || ifa.ifa_addr.is_null() {
            continue;
        }
        // SAFETY: ifa_addr is a valid sockaddr.
        if i32::from(unsafe { (*ifa.ifa_addr).sa_family }) != libc::AF_INET {
            continue;
        }
        #[cfg(target_os = "linux")]
        let brd = ifa.ifa_ifu;
        #[cfg(not(target_os = "linux"))]
        let brd = ifa.ifa_dstaddr;
        if brd.is_null() {
            continue;
        }
        // SAFETY: for AF_INET interfaces with IFF_BROADCAST this is a sockaddr_in.
        let sin = unsafe { &*(brd as *const libc::sockaddr_in) };
        let addr = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
        if !out.contains(&addr) {
            out.push(addr);
        }
    }
    // SAFETY: frees the list from getifaddrs exactly once.
    unsafe { libc::freeifaddrs(ifap) };
    out
}

#[cfg(windows)]
fn os_broadcasts() -> Vec<Ipv4Addr> {
    crate::windows::broadcast_addresses()
}

#[cfg(not(any(unix, windows)))]
fn os_broadcasts() -> Vec<Ipv4Addr> {
    Vec::new()
}

pub type IdentityFn = Arc<dyn Fn() -> (String, String, u16) + Send + Sync>;
pub type SeenFn = Arc<dyn Fn(Packet, SocketAddr) + Send + Sync>;
pub type DirectedFn = Arc<dyn Fn() -> Vec<(String, u16)> + Send + Sync>;

struct Shared {
    identity: IdentityFn,
    on_seen: SeenFn,
    directed: Option<DirectedFn>,
    targets: Mutex<Option<Vec<(String, u16)>>>,
    port: u16,
    interval: Duration,
    sock: UdpSocket,
    stop: AtomicBool,
    wake: (Mutex<bool>, Condvar),
    last_reply: Mutex<Option<Instant>>,
}

/// Announce ourselves every `interval` and answer queries; report what we hear through
/// `on_seen(packet, address)`.
///
/// `targets` = explicit destinations instead of the broadcast addresses (`--flow-discovery`;
/// tests use loopback); None means the broadcast address of every interface on `port`.
/// `directed()` returns the paired computers' known addresses: they get the same
/// announcement too, whatever `targets` says (they are where the TCP link goes anyway).
pub struct Discovery {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl Discovery {
    pub fn start(
        identity: IdentityFn,
        on_seen: SeenFn,
        bind: Ipv4Addr,
        port: u16,
        targets: Option<Vec<(String, u16)>>,
        interval: Duration,
        directed: Option<DirectedFn>,
    ) -> std::io::Result<Self> {
        let s = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        #[cfg(unix)]
        s.set_reuse_address(true)?;
        if targets.is_none() {
            s.set_broadcast(true)?;
        }
        s.bind(&SocketAddr::V4(SocketAddrV4::new(bind, port)).into())?;
        let sock: UdpSocket = s.into();
        sock.set_read_timeout(Some(Duration::from_millis(500)))?;
        let shared = Arc::new(Shared {
            identity,
            on_seen,
            directed,
            targets: Mutex::new(targets),
            port,
            interval,
            sock,
            stop: AtomicBool::new(false),
            wake: (Mutex::new(false), Condvar::new()),
            last_reply: Mutex::new(None),
        });
        let mut threads = Vec::new();
        let rx = shared.clone();
        threads.push(
            std::thread::Builder::new()
                .name("flow-discovery-rx".into())
                .spawn(move || recv_loop(&rx))?,
        );
        let tx = shared.clone();
        threads.push(
            std::thread::Builder::new()
                .name("flow-discovery-tx".into())
                .spawn(move || announce_loop(&tx))?,
        );
        Ok(Self { shared, threads })
    }

    pub fn bound_port(&self) -> u16 {
        self.shared.sock.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    pub fn set_targets(&self, targets: Option<Vec<(String, u16)>>) {
        *lock(&self.shared.targets) = targets;
    }

    /// Ask everyone to announce themselves now (and announce ourselves).
    pub fn query(&self) {
        send(&self.shared, "query");
    }

    /// Where the next announcement goes.
    pub fn destinations(&self) -> Vec<(String, u16)> {
        destinations(&self.shared)
    }

    pub fn stop(mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        let (m, cv) = &self.shared.wake;
        *lock(m) = true;
        cv.notify_all();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

fn destinations(sh: &Shared) -> Vec<(String, u16)> {
    let mut out: Vec<(String, u16)> = match lock(&sh.targets).clone() {
        Some(t) => t,
        None => broadcast_addresses()
            .into_iter()
            .map(|a| (a.to_string(), sh.port))
            .collect(),
    };
    let extra = match &sh.directed {
        Some(f) => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f())) {
            Ok(v) => v,
            Err(_) => {
                log::error!("discovery: directed addresses");
                Vec::new()
            }
        },
        None => Vec::new(),
    };
    for dest in extra.into_iter().take(MAX_DIRECTED) {
        if !out.contains(&dest) {
            out.push(dest);
        }
    }
    out
}

fn send(sh: &Shared, kind: &str) {
    let (ident, name, tcp_port) = (sh.identity)();
    if ident.is_empty() {
        return;
    }
    let pkt = make_packet(kind, &ident, &name, tcp_port);
    for (host, port) in destinations(sh) {
        let Ok(ip) = host.parse::<Ipv4Addr>() else {
            continue;
        };
        if let Err(e) = sh.sock.send_to(&pkt, SocketAddrV4::new(ip, port)) {
            log::debug!("discovery: could not send to {host}:{port}: {e}");
        }
    }
}

fn announce_loop(sh: &Shared) {
    while !sh.stop.load(Ordering::SeqCst) {
        send(sh, "announce");
        let (m, cv) = &sh.wake;
        let mut woken = lock(m);
        if !*woken {
            woken = cv
                .wait_timeout(woken, sh.interval)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *woken = false;
    }
}

fn recv_loop(sh: &Shared) {
    let mut buf = [0u8; MAX_PACKET + 1];
    while !sh.stop.load(Ordering::SeqCst) {
        let (n, addr) = match sh.sock.recv_from(&mut buf) {
            Ok(x) => x,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(_) => {
                if sh.stop.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
        };
        let Some(pkt) = parse_packet(&buf[..n]) else {
            continue;
        };
        if pkt.id == (sh.identity)().0 {
            continue;
        }
        if pkt.kind == "query" {
            let mut last = lock(&sh.last_reply);
            if last.is_none_or(|t| t.elapsed() > Duration::from_secs(1)) {
                *last = Some(Instant::now()); // answer at most once a second
                drop(last);
                send(sh, "announce");
            }
        }
        let on_seen = sh.on_seen.clone();
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_seen(pkt, addr))).is_err() {
            log::error!("discovery handler");
        }
    }
}

/// The announcement as JSON (for tests and tools).
pub fn packet_json(p: &Packet) -> Value {
    json!({"app": APP, "v": p.v, "type": p.kind, "id": p.id, "name": p.name, "port": p.port})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        let good = make_packet("announce", "0123456789abcdef", "portátil", 27461);
        assert_eq!(parse_packet(&good).unwrap().name, "portátil");
        let ok: Value = serde_json::from_slice(&good).unwrap();
        for change in [
            json!({"id": "xyz"}),
            json!({"port": 0}),
            json!({"port": "1"}),
            json!({"name": ""}),
            json!({"name": "a\nb"}),
            json!({"type": "hack"}),
            json!({"app": "other"}),
            json!({"extra": 1}),
            json!({"v": true}),
        ] {
            let mut m = ok.as_object().unwrap().clone();
            m.extend(change.as_object().unwrap().clone());
            assert!(
                parse_packet(&serde_json::to_vec(&m).unwrap()).is_none(),
                "{change}"
            );
        }
        assert!(parse_packet(&[b'x'; 600]).is_none());
        assert!(parse_packet(b"\x00garbage").is_none());
        let keys: Vec<&String> = ok.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 6); // nothing sensitive
    }

    #[test]
    fn loopback_announcements() {
        let seen: Arc<Mutex<Vec<Packet>>> = Arc::default();
        let s = seen.clone();
        let d1 = Discovery::start(
            Arc::new(|| ("aaaaaaaaaaaaaaaa".into(), "a".into(), 1)),
            Arc::new(move |p, _| lock(&s).push(p)),
            Ipv4Addr::LOCALHOST,
            0,
            Some(vec![]),
            Duration::from_millis(100),
            None,
        )
        .unwrap();
        let p1 = d1.bound_port();
        let d2 = Discovery::start(
            Arc::new(|| ("bbbbbbbbbbbbbbbb".into(), "b".into(), 2)),
            Arc::new(|_, _| {}),
            Ipv4Addr::LOCALHOST,
            0,
            Some(vec![("127.0.0.1".into(), p1)]),
            Duration::from_millis(100),
            None,
        )
        .unwrap();
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end && !lock(&seen).iter().any(|p| p.id == "bbbbbbbbbbbbbbbb") {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            lock(&seen)
                .iter()
                .any(|p| p.id == "bbbbbbbbbbbbbbbb" && p.port == 2)
        );
        d1.stop();
        d2.stop();
    }

    #[test]
    fn destinations_deduplicated_and_bounded() {
        let many: Vec<(String, u16)> = (1..80).map(|i| (format!("10.0.0.{i}"), 27461)).collect();
        let d = Discovery::start(
            Arc::new(|| (String::new(), "a".into(), 1)),
            Arc::new(|_, _| {}),
            Ipv4Addr::LOCALHOST,
            0,
            Some(vec![("10.0.0.1".into(), 27461)]),
            Duration::from_secs(60),
            Some(Arc::new(move || {
                let mut v = vec![("10.0.0.1".to_string(), 27461)];
                v.extend(many.clone());
                v
            })),
        )
        .unwrap();
        let dests = d.destinations();
        assert_eq!(dests[0], ("10.0.0.1".to_string(), 27461));
        let set: std::collections::HashSet<_> = dests.iter().collect();
        assert_eq!(set.len(), dests.len());
        assert!(dests.len() <= 1 + 32);
        d.stop();
        assert!(!broadcast_addresses().is_empty());
    }
}
