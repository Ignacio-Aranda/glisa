//! FlowService: the Flow half of the Glisa service.
//!
//! * TLS 1.3 server on TCP `port` (mutual auth, pinned peers) and, only while the user has
//!   pairing open, the pairing-introduction listener on `port + 1`;
//! * one outgoing authenticated link per paired peer (requests: switch, clipboard, ping,
//!   unpair), see `link.rs`;
//! * pairing: certificate introduction → mutually authenticated session → commit/reveal
//!   nonces → 6-digit SAS confirmed by the user on BOTH computers (`pairing.rs`);
//! * the edge engine: edge events from the backend → dead zones, Ctrl, held buttons,
//!   cooldown, hysteresis → ask the peer → release held keys → set_host on the devices →
//!   clipboard; and on the other side: device arrives → place the cursor (`engine.rs`);
//! * copied files and images: a separate transfer connection per transfer (same TLS and
//!   pin), see `transfers.rs` and `crate::transfer`; received ones wait in a private
//!   staging folder until they are pasted;
//! * files dragged across the edge: the edge glow and catcher, the `drag` notice, and on
//!   the receiving side the pickup that lets the drag go on (`pickup.rs`, `crate::drag`);
//! * the `flow_*` API with the same methods, parameters, results and error texts as the
//!   Python version (`api.rs`), and events for the window (topics "flow" and "transfer").
//!
//! Threads: worker (edge engine), links (keep-alive / reconnect), server accept + one per
//! connection, discovery, one per outgoing transfer. Shared state is guarded by `st`;
//! the lock order is `st` → pairing → store, and no lock is held while calling the
//! network, the backend, the host or the event sink. Clipboard contents, nonces and SAS
//! values are never logged.

mod api;
mod engine;
mod link;
mod pairing;
mod pickup;
mod server;
mod transfers;

use std::collections::{HashMap, VecDeque};
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use glisa_common::{AppearingDevice, Event, EventSink, FlowHost};
use glisa_i18n::n_;
use serde_json::{Value, json};

use crate::backends::{EdgeBackend, FakeBackend, PickupNews, Sink};
use crate::discovery::{Discovery, Packet};
use crate::geometry::{Rect, clean_rects};
use crate::store::FlowStore;
use crate::text::{Text, plain};
use crate::tls::Identity;
use crate::transfer::{SendHooks, Staging};
use crate::util::lock;
use crate::{CAP_DRAG, CAPS, PORT, PROTOCOL_VERSION, SIDES};

pub use link::PeerLink;
pub use pairing::Pairing;
pub use pickup::{DRAG_WAIT, PICKUP_ARRIVAL, PICKUP_PRESS};
pub use server::Listener;
pub use transfers::{SendReason, TransferState};

pub const PING_EVERY: Duration = Duration::from_secs(10);
pub const SERVER_IDLE: Duration = Duration::from_secs(45);
pub const ARRIVAL_WINDOW: Duration = Duration::from_secs(15);
/// The pointer is only moved if the device became usable this recently: over Bluetooth
/// the HID++ set-up takes seconds, and moving the pointer after the user has started using
/// the mouse yanks it back to the edge.
pub const LATE_PLACEMENT: Duration = Duration::from_millis(1500);
pub const SWITCH_TIMEOUT: Duration = Duration::from_millis(800);
pub const MAX_CONNECTIONS: usize = 16;
pub const PAIR_ATTEMPTS: u32 = 3;
pub const PAIR_GAP: Duration = Duration::from_secs(2);
pub const DISCOVERED_TTL: Duration = Duration::from_secs(30);
/// Directed announcements: at most this many paired-peer addresses.
pub const ANNOUNCE_PEERS_MAX: usize = 32;
/// Look for received transfers older than a day this often.
pub const CLEANUP_EVERY: Duration = Duration::from_secs(600);

/// Why the other computer said no (protocol reason → msgid, translated when shown).
pub fn refused_text(reason: &str) -> Option<&'static str> {
    Some(match reason {
        "disabled" => n_!("Flow is turned off on {name}"),
        "busy" => n_!("{name} is busy"),
        "not_pairing" => n_!("{name} isn't ready to pair: open “Pair a computer” there too"),
        "rate_limited" => n_!("Too many attempts: wait a moment and open pairing again"),
        "version" => n_!("{name} uses a different version of Flow: update {app} on both computers"),
        "unknown_peer" => n_!("{name} doesn't recognize this computer: pair them again"),
        "rejected" => n_!("On {name}, the numbers were marked as not matching"),
        "expired" => n_!("Pairing has expired"),
        "mismatch" => n_!("The security check failed: pair again"),
        "protocol" => n_!("{name} gave an unexpected answer"),
        _ => return None,
    })
}

/// Where Flow listens and announces. Tests bind 127.0.0.1 and pass explicit loopback
/// discovery targets; the real thing uses the defaults.
#[derive(Debug, Clone)]
pub struct NetConfig {
    pub bind: Ipv4Addr,
    pub port: u16,
    /// UDP discovery port (None: the same as `port`).
    pub udp_port: Option<u16>,
    /// Discovery destinations instead of the broadcast addresses (None: broadcast).
    pub targets: Option<Vec<(String, u16)>>,
    /// Send and hear announcements at all.
    pub discovery: bool,
    pub announce_interval: Duration,
}

impl Default for NetConfig {
    fn default() -> Self {
        Self {
            bind: Ipv4Addr::UNSPECIFIED,
            port: PORT,
            udp_port: None,
            targets: None,
            discovery: true,
            announce_interval: Duration::from_secs(5),
        }
    }
}

impl NetConfig {
    pub fn udp(&self) -> u16 {
        self.udp_port.unwrap_or(self.port)
    }
}

/// Which desktop backend Flow uses.
#[derive(Clone, Default)]
pub enum BackendChoice {
    /// Detect the desktop (KWin, X11, Windows, macOS).
    #[default]
    Auto,
    /// A fake desktop (demo mode, tests), driven with `flow_simulate`.
    Fake,
    /// This backend instance (tests keep a handle to drive it).
    Instance(Arc<dyn EdgeBackend>),
}

/// How the shell builds the Flow service.
#[derive(Clone)]
pub struct FlowOptions {
    /// Identity, peers and options (default `<config>/flow`).
    pub store_dir: Option<PathBuf>,
    /// Received files (default `<cache>/flow-received`; demo: `<store>/received`).
    pub staging_dir: Option<PathBuf>,
    /// None: the defaults (demo: loopback on free ports, no discovery).
    pub net: Option<NetConfig>,
    pub backend: BackendChoice,
    /// Demo mode (`glisa --simulate`).
    pub demo: bool,
    /// This computer's name (default: the host name).
    pub hostname: Option<String>,
    pub pairing_seconds: f64,
}

impl Default for FlowOptions {
    fn default() -> Self {
        Self {
            store_dir: None,
            staging_dir: None,
            net: None,
            backend: BackendChoice::Auto,
            demo: false,
            hostname: None,
            pairing_seconds: crate::PAIRING_SECONDS,
        }
    }
}

/// A port p with TCP p, TCP p+1 and UDP p free (demo mode and tests).
pub fn free_ports(host: Ipv4Addr) -> std::io::Result<u16> {
    use std::net::{TcpListener, UdpSocket};
    for _ in 0..50 {
        let p = TcpListener::bind((host, 0))?.local_addr()?.port();
        if p > 65000 {
            continue;
        }
        let ok = (|| -> std::io::Result<()> {
            let _a = TcpListener::bind((host, p))?;
            let _b = TcpListener::bind((host, p + 1))?;
            let _c = UdpSocket::bind((host, p))?;
            Ok(())
        })();
        if ok.is_ok() {
            return Ok(p);
        }
    }
    Err(std::io::Error::other("no free ports"))
}

/// Is `ufw` active (the page shows the rule to add)?
pub fn ufw_active() -> bool {
    std::fs::read_to_string("/etc/ufw/ufw.conf")
        .map(|t| {
            t.lines()
                .any(|l| l.trim().eq_ignore_ascii_case("enabled=yes"))
        })
        .unwrap_or(false)
}

/// A notice for the user: kept as Text, rendered in the language of whoever reads it.
#[derive(Debug, Clone)]
pub(crate) struct Notice {
    seq: u64,
    msg: Text,
    kind: &'static str,
}

#[derive(Debug, Clone)]
pub(crate) struct Discovered {
    id: String,
    name: String,
    address: (String, u16),
    version: u64,
    seen: Instant,
}

/// An authenticated session a peer has open with us (ended when it is removed).
pub(crate) struct Served {
    id: u64,
    peer: String,
    sock: Arc<dyn crate::proto::RawSocket>,
    /// A peer channel (its `hello` was answered with our capabilities), not a transfer.
    channel: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct AtEdge {
    side: &'static str,
    x: f64,
    y: f64,
    rect: Rect,
}

/// The edge catcher is up (a button is held at an edge that leads to a peer).
#[derive(Debug, Clone)]
pub(crate) struct Catcher {
    side: &'static str,
    rect: Rect,
}

/// Files dragged onto the catcher, waiting for the crossing.
#[derive(Debug, Clone)]
pub(crate) struct FileDrag {
    side: &'static str,
    paths: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct Pending {
    peer: String,
    side: &'static str,
    t: f64,
    names: Vec<String>,
    until: Instant,
    since: Instant,
}

/// Shared state (Python's attributes guarded by `self.lock`).
pub(crate) struct State {
    links: HashMap<String, Arc<PeerLink>>,
    served: Vec<Served>,
    served_seq: u64,
    discovered: HashMap<String, Discovered>,
    pairing: Option<Arc<Pairing>>,
    error: Option<Text>,
    screens: Vec<Rect>,
    armed: HashMap<&'static str, bool>,
    at_edge: Option<AtEdge>,
    cooldown_until: Instant,
    pending: Option<Pending>,
    last_switch: Option<Value>,
    last_arrival: Option<Value>,
    clip_hash: HashMap<String, String>,
    gui_clip_offer: String,
    gui_clip_uris: Vec<String>,
    gui_clip_seq: u64,
    gui_clip_text: String,
    gui_clip_rich: Option<Value>,
    transfers: VecDeque<Arc<TransferState>>,
    sent_keys: HashMap<String, String>,
    last_cleanup: Option<Instant>,
    events: VecDeque<Notice>,
    event_seq: u64,
    event_last: HashMap<String, Instant>,
    /// When the last non-receiver Logitech node appeared (arrival timing).
    last_appearing: Option<Instant>,
    held_unknown_logged: bool,
    catcher: Option<Catcher>,
    file_drag: Option<FileDrag>,
    /// The edges glow while files are dragged anywhere, until then at the latest.
    glow_until: Option<Instant>,
    /// Files dragged here from another computer: the last notice, the pickup.
    incoming_drag: Option<pickup::IncomingDrag>,
    pickup: Option<pickup::Pickup>,
    pickup_cooldown: Option<Instant>,
}

impl State {
    fn new() -> Self {
        Self {
            links: HashMap::new(),
            served: Vec::new(),
            served_seq: 0,
            discovered: HashMap::new(),
            pairing: None,
            error: None,
            screens: Vec::new(),
            armed: SIDES.iter().map(|s| (*s, true)).collect(),
            at_edge: None,
            cooldown_until: Instant::now(),
            pending: None,
            last_switch: None,
            last_arrival: None,
            clip_hash: HashMap::new(),
            gui_clip_offer: String::new(),
            gui_clip_uris: Vec::new(),
            gui_clip_seq: 0,
            gui_clip_text: String::new(),
            gui_clip_rich: None,
            transfers: VecDeque::new(),
            sent_keys: HashMap::new(),
            last_cleanup: None,
            events: VecDeque::new(),
            event_seq: 0,
            event_last: HashMap::new(),
            last_appearing: None,
            held_unknown_logged: false,
            catcher: None,
            file_drag: None,
            glow_until: None,
            incoming_drag: None,
            pickup: None,
            pickup_cooldown: None,
        }
    }
}

/// Engine input.
pub(crate) enum Item {
    Edge(AtEdge),
    Leave(&'static str),
    Ctrl(bool),
    Online(String),
    Appearing,
    /// Files were dragged onto the catcher on this side.
    FileDrag(&'static str, Vec<PathBuf>),
    /// The files have been on the catcher (glowing) for [`engine::DRAG_DWELL`]: cross now.
    DragDwell(&'static str),
    /// The mouse button was let go while the catcher was up.
    Released,
    /// Files are being dragged somewhere on the desktop (true), or no longer (false).
    FilesDragging(bool),
    /// How the drag picked up here goes.
    Pickup(PickupNews),
    Stop,
}

/// Test hooks (behaviours the Python tests got by monkey-patching).
#[derive(Clone, Default)]
#[doc(hidden)]
pub struct TestHooks {
    /// Fields of an incoming `hello` this server refuses, as a Glisa from before them.
    pub refuse_hello_fields: Vec<&'static str>,
    /// Replaces the peer-channel server (a peer answering garbage).
    pub serve_peer: Option<Arc<dyn Fn(crate::proto::Conn) + Send + Sync>>,
    pub send: SendHooks,
    /// Lower cap on the number of files sent at once.
    pub max_files: Option<usize>,
    /// How long the pickup waits for the press (instead of [`PICKUP_PRESS`]).
    pub pickup_press_wait: Option<Duration>,
    /// How long the drag glow may last (instead of `engine::GLOW_TIMEOUT`).
    pub glow_timeout: Option<Duration>,
}

pub struct FlowService {
    me: Weak<FlowService>,
    host: Weak<dyn FlowHost>,
    events: EventSink,
    pub store: FlowStore,
    pub staging: Staging,
    net: NetConfig,
    backend_choice: BackendChoice,
    demo: bool,
    pairing_seconds: Mutex<f64>,
    life: Mutex<()>,
    running: AtomicBool,
    stop_flag: AtomicBool,
    /// An arrival watcher thread runs (see `engine::watch_arrival`).
    arrival_watch: AtomicBool,
    /// A thread watches the mouse buttons while the edge catcher is up.
    catcher_watch: AtomicBool,
    st: Mutex<State>,
    backend: RwLock<Option<Arc<dyn EdgeBackend>>>,
    backend_gen: AtomicU64,
    server: Mutex<Option<Listener>>,
    discovery: Mutex<Option<Discovery>>,
    queue: Mutex<Option<mpsc::Sender<Item>>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    identity: RwLock<Option<Arc<Identity>>>,
    hooks: Mutex<TestHooks>,
    fake: Mutex<Option<Arc<FakeBackend>>>,
}

impl FlowService {
    /// Builds the service (nothing starts yet: call [`FlowService::start`]).
    pub fn new(host: Weak<dyn FlowHost>, events: EventSink, opts: FlowOptions) -> Arc<FlowService> {
        let store_dir = opts
            .store_dir
            .clone()
            .unwrap_or_else(|| glisa_common::config_dir().join(crate::local::STORE_DIR));
        let store = FlowStore::new(&store_dir, opts.hostname.as_deref());
        let staging = opts.staging_dir.clone().unwrap_or_else(|| {
            if opts.demo {
                store_dir.join("received")
            } else {
                crate::transfer::default_staging_root()
            }
        });
        let net = opts.net.clone().unwrap_or_else(|| {
            if opts.demo {
                NetConfig {
                    bind: Ipv4Addr::LOCALHOST,
                    port: free_ports(Ipv4Addr::LOCALHOST).unwrap_or(PORT),
                    discovery: false,
                    ..NetConfig::default()
                }
            } else {
                NetConfig::default()
            }
        });
        let backend_choice = match opts.backend {
            BackendChoice::Auto if opts.demo => BackendChoice::Fake,
            other => other,
        };
        Arc::new_cyclic(|me| FlowService {
            me: me.clone(),
            host,
            events,
            store,
            staging: Staging::new(staging),
            net,
            backend_choice,
            demo: opts.demo,
            pairing_seconds: Mutex::new(opts.pairing_seconds),
            life: Mutex::new(()),
            running: AtomicBool::new(false),
            arrival_watch: AtomicBool::new(false),
            catcher_watch: AtomicBool::new(false),
            stop_flag: AtomicBool::new(false),
            st: Mutex::new(State::new()),
            backend: RwLock::new(None),
            backend_gen: AtomicU64::new(0),
            server: Mutex::new(None),
            discovery: Mutex::new(None),
            queue: Mutex::new(None),
            threads: Mutex::new(Vec::new()),
            identity: RwLock::new(None),
            hooks: Mutex::new(TestHooks::default()),
            fake: Mutex::new(None),
        })
    }

    fn arc(&self) -> Arc<FlowService> {
        self.me
            .upgrade()
            .expect("the service is alive while it runs")
    }

    pub(crate) fn host(&self) -> Option<Arc<dyn FlowHost>> {
        self.host.upgrade()
    }

    /// Starts Flow if the user left it on (as the Python service did when it was created).
    /// A start-up error is kept and shown by `flow_status`.
    pub fn start(&self) {
        if self.store.enabled() {
            if let Err(e) = self.start_now() {
                log::warn!("Flow could not start: {}", e.english());
                lock(&self.st).error = Some(e);
            }
        }
    }

    pub fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Tests: pretend Flow is off while its server still answers.
    #[doc(hidden)]
    pub fn set_running_flag(&self, on: bool) {
        self.running.store(on, Ordering::SeqCst);
    }

    #[doc(hidden)]
    pub fn hooks(&self) -> std::sync::MutexGuard<'_, TestHooks> {
        lock(&self.hooks)
    }

    #[doc(hidden)]
    pub fn set_pairing_seconds(&self, s: f64) {
        *lock(&self.pairing_seconds) = s;
    }

    fn make_backend(&self) -> Arc<dyn EdgeBackend> {
        match &self.backend_choice {
            BackendChoice::Instance(b) => b.clone(),
            BackendChoice::Fake => {
                let mut f = lock(&self.fake);
                f.get_or_insert_with(|| {
                    Arc::new(FakeBackend::new(&[crate::geometry::DEFAULT_SCREEN]))
                })
                .clone()
            }
            BackendChoice::Auto => crate::backends::make_backend(),
        }
    }

    /// Turns Flow on now (ports, discovery, links, the desktop backend).
    pub fn start_now(&self) -> Result<(), Text> {
        let _life = lock(&self.life);
        if self.running() {
            return Ok(());
        }
        self.store.ensure_identity().map_err(|e| e.0)?;
        self.store.reset_pin_cache();
        let id = Identity::load(&self.store.cert_path, &self.store.key_path).map_err(|e| {
            Text::new(n_!("Couldn't read this computer's identity: {error}"))
                .arg("error", e.to_string())
        })?;
        *self.identity.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(id));
        let me = self.me.clone();
        let server = Listener::start(
            self.net.bind,
            self.net.port,
            "flow-server",
            2.0,
            10,
            Arc::new(move |tcp, addr| {
                if let Some(svc) = me.upgrade() {
                    svc.serve(tcp, addr);
                }
            }),
        )
        .map_err(|e| {
            Text::new(n_!("Can't use TCP port {port}: {error}"))
                .arg("port", self.net.port as u64)
                .arg("error", e.to_string())
        })?;
        *lock(&self.server) = Some(server);
        lock(&self.st).error = None;
        self.stop_flag.store(false, Ordering::SeqCst);
        self.running.store(true, Ordering::SeqCst);
        if self.net.discovery {
            self.start_discovery();
        }
        let (tx, rx) = mpsc::channel();
        *lock(&self.queue) = Some(tx);
        let svc = self.arc();
        let worker = std::thread::Builder::new()
            .name("flow-engine".into())
            .spawn(move || svc.worker(rx))
            .expect("thread");
        let svc = self.arc();
        let links = std::thread::Builder::new()
            .name("flow-links".into())
            .spawn(move || svc.links_loop())
            .expect("thread");
        lock(&self.threads).extend([worker, links]);
        let backend = self.make_backend();
        let gen_ = self.backend_gen.fetch_add(1, Ordering::SeqCst) + 1;
        *self.backend.write().unwrap_or_else(|e| e.into_inner()) = Some(backend.clone());
        backend.start(Arc::new(ServiceSink {
            svc: self.me.clone(),
            gen_,
        }));
        self.update_drag_watch();
        self.cleanup_staging();
        log::info!("Flow on (TCP {})", self.port());
        drop(_life);
        self.emit_flow("state");
        Ok(())
    }

    fn start_discovery(&self) {
        let me = self.me.clone();
        let identity: crate::discovery::IdentityFn = {
            let me = me.clone();
            Arc::new(move || match me.upgrade() {
                Some(s) => (s.store.id(), s.store.hostname.clone(), s.port()),
                None => (String::new(), String::new(), 0),
            })
        };
        let seen: crate::discovery::SeenFn = {
            let me = me.clone();
            Arc::new(move |p, addr| {
                if let Some(s) = me.upgrade() {
                    s.seen(p, addr);
                }
            })
        };
        let directed: crate::discovery::DirectedFn = Arc::new(move || {
            me.upgrade()
                .map(|s| s.announce_targets())
                .unwrap_or_default()
        });
        match Discovery::start(
            identity,
            seen,
            self.net.bind,
            self.net.udp(),
            self.net.targets.clone(),
            self.net.announce_interval,
            Some(directed),
        ) {
            Ok(d) => *lock(&self.discovery) = Some(d),
            Err(e) => {
                lock(&self.st).error = Some(
                    Text::new(n_!("Can't use UDP port {port}: {error}"))
                        .arg("port", self.net.udp() as u64)
                        .arg("error", e.to_string()),
                );
            }
        }
    }

    pub fn stop(&self) {
        log::debug!("Flow: stopping (waiting for start/stop to finish)");
        let _life = lock(&self.life);
        if !self.running() {
            return;
        }
        log::debug!("Flow: stopping the engine and transfers");
        self.running.store(false, Ordering::SeqCst);
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(q) = lock(&self.queue).take() {
            let _ = q.send(Item::Stop);
        }
        let transfers: Vec<Arc<TransferState>> = lock(&self.st).transfers.iter().cloned().collect();
        for t in transfers {
            t.cancel.store(true, Ordering::SeqCst);
        }
        let backend = self
            .backend
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        self.backend_gen.fetch_add(1, Ordering::SeqCst);
        if let Some(b) = backend {
            log::debug!("Flow: stopping the desktop backend");
            b.stop();
        }
        log::debug!("Flow: stopping discovery, pairing, server and links");
        if let Some(d) = lock(&self.discovery).take() {
            d.stop();
        }
        let p = lock(&self.st).pairing.take();
        if let Some(p) = p {
            p.close();
        }
        if let Some(s) = lock(&self.server).take() {
            s.stop();
        }
        let served: Vec<Served> = std::mem::take(&mut lock(&self.st).served);
        for s in served {
            s.sock.shutdown(); // peers must see us go away now
        }
        let links: Vec<Arc<PeerLink>> = lock(&self.st).links.drain().map(|(_, l)| l).collect();
        for l in links {
            l.close();
        }
        let threads: Vec<JoinHandle<()>> = lock(&self.threads).drain(..).collect();
        for t in threads {
            let _ = t.join();
        }
        {
            let mut st = lock(&self.st);
            st.pending = None;
            st.at_edge = None;
            st.catcher = None;
            st.file_drag = None;
            st.glow_until = None;
        }
        self.drop_pickup();
        log::info!("Flow off");
        drop(_life);
        self.emit_flow("state");
    }

    pub fn set_enabled(&self, on: bool) -> Result<(), Text> {
        if on {
            self.start_now()?;
            self.store.set_enabled(true);
        } else {
            self.store.set_enabled(false);
            self.stop();
            let removed = self.staging.cleanup(0, &[]); // turning Flow off forgets what was received
            if removed > 0 {
                log::info!("Flow: {removed} received transfer(s) deleted");
            }
        }
        Ok(())
    }

    /// The TCP port the server listens on.
    pub fn port(&self) -> u16 {
        lock(&self.server)
            .as_ref()
            .map(|s| s.port)
            .unwrap_or(self.net.port)
    }

    pub(crate) fn identity(&self) -> Option<Arc<Identity>> {
        self.identity
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn backend(&self) -> Option<Arc<dyn EdgeBackend>> {
        self.backend
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The fake backend in use (demo mode, `BackendChoice::Fake`).
    pub fn fake_backend(&self) -> Option<Arc<FakeBackend>> {
        lock(&self.fake).clone()
    }

    pub(crate) fn post(&self, item: Item) {
        if self.running() {
            if let Some(q) = lock(&self.queue).as_ref() {
                let _ = q.send(item);
            }
        }
    }

    // ================================================================ events
    pub(crate) fn emit(&self, topic: &str, data: Value) {
        (self.events)(Event::new(topic, data));
    }

    pub(crate) fn emit_flow(&self, what: &str) {
        let seq = lock(&self.st).event_seq;
        self.emit("flow", json!({"what": what, "events_seq": seq}));
    }

    /// Queue a notice for the user (translated when a client reads it). `key` + `every`
    /// limit how often the same notice repeats.
    pub(crate) fn notice(
        &self,
        msg: Text,
        kind: &'static str,
        key: Option<String>,
        every: Duration,
    ) {
        {
            let mut st = lock(&self.st);
            if let Some(k) = key {
                if st.event_last.get(&k).is_some_and(|t| t.elapsed() < every) {
                    return;
                }
                st.event_last.insert(k, Instant::now());
            }
            st.event_seq += 1;
            let seq = st.event_seq;
            st.events.push_back(Notice { seq, msg, kind });
            while st.events.len() > 40 {
                st.events.pop_front();
            }
        }
        self.emit_flow("notice");
    }

    pub(crate) fn peers_changed(&self) {
        self.emit_flow("peers");
    }

    pub(crate) fn identity_mismatch(&self, pid: &str) {
        let name = self
            .store
            .peer(pid)
            .map(|p| plain(p.name))
            .unwrap_or_else(|| Text::new(n_!("the other computer")));
        log::warn!(
            "Flow: {} presents a certificate other than the paired one; connection refused",
            name.english()
        );
        self.notice(
            Text::new(n_!(
                "The identity of {name} doesn't match the one you paired, so it wasn't connected. If you reinstalled {app} there, remove it and pair it again"
            ))
            .arg("name", name)
            .arg("app", glisa_common::APP_NAME),
            "error",
            Some(format!("identity:{pid}")),
            Duration::from_secs(300),
        );
    }

    // ============================================================= discovery
    /// Where to send our announcement besides the broadcast addresses: every IPv4 address
    /// we know for a paired peer (typed, last reached, last announced). Only paired peers,
    /// only our own announcement, bounded.
    fn announce_targets(&self) -> Vec<(String, u16)> {
        let links: HashMap<String, Arc<PeerLink>> = lock(&self.st).links.clone();
        let mut out: Vec<(String, u16)> = Vec::new();
        for (pid, peer) in self.store.peers() {
            let hint = links.get(&pid).and_then(|l| l.hint());
            for addr in [peer.manual_address, peer.address, hint]
                .into_iter()
                .flatten()
            {
                if crate::address::is_ipv4(&addr.0) && !out.contains(&addr) {
                    out.push(addr);
                }
            }
        }
        out.truncate(ANNOUNCE_PEERS_MAX);
        out
    }

    pub(crate) fn seen(&self, pkt: Packet, addr: std::net::SocketAddr) {
        let info = Discovered {
            id: pkt.id.clone(),
            name: crate::store::clean_name(&pkt.name),
            address: (addr.ip().to_string(), pkt.port),
            version: pkt.v,
            seen: Instant::now(),
        };
        let (link, changed) = {
            let mut st = lock(&self.st);
            let mut changed = false;
            if st.discovered.len() < 64 || st.discovered.contains_key(&pkt.id) {
                let old = st.discovered.get(&pkt.id);
                changed = old.is_none_or(|o| o.name != info.name || o.address != info.address);
                st.discovered.insert(pkt.id.clone(), info.clone());
            }
            (st.links.get(&pkt.id).cloned(), changed)
        };
        if let Some(link) = link {
            if pkt.v == PROTOCOL_VERSION {
                link.set_hint(Some(info.address.clone())); // a hint only: the pinned TLS session decides
            }
        }
        if changed {
            self.emit_flow("discovered");
        }
    }

    /// Discovery packets as Python's `_seen` (tests).
    #[doc(hidden)]
    pub fn seen_for_tests(
        &self,
        kind: &str,
        id: &str,
        name: &str,
        port: u16,
        from: std::net::SocketAddr,
    ) {
        self.seen(
            Packet {
                kind: kind.into(),
                v: PROTOCOL_VERSION,
                id: id.into(),
                name: name.into(),
                port,
            },
            from,
        );
    }

    pub fn discovery_destinations(&self) -> Option<Vec<(String, u16)>> {
        lock(&self.discovery).as_ref().map(|d| d.destinations())
    }

    pub fn discovered_ids(&self) -> Vec<String> {
        lock(&self.st).discovered.keys().cloned().collect()
    }

    #[doc(hidden)]
    pub fn clear_discovered(&self) {
        lock(&self.st).discovered.clear();
    }

    /// What this computer announces in `hello.caps` now: `drag` only while files dragged
    /// across the edge are taken (the option, and copied files at all).
    pub fn caps(&self) -> Vec<&'static str> {
        let o = self.store.options();
        CAPS.iter()
            .copied()
            .filter(|c| *c != CAP_DRAG || (o.drag_files && o.files))
            .collect()
    }

    /// Our capabilities changed: end the peer channels peers have open with us, so they
    /// say `hello` again and hear the new ones (transfers under way are left alone).
    pub(crate) fn renew_peer_channels(&self) {
        let socks: Vec<_> = lock(&self.st)
            .served
            .iter()
            .filter(|s| s.channel)
            .map(|s| s.sock.clone())
            .collect();
        for s in socks {
            s.shutdown();
        }
    }

    pub fn link(&self, pid: &str) -> Option<Arc<PeerLink>> {
        lock(&self.st).links.get(pid).cloned()
    }

    pub(crate) fn screens(&self) -> Vec<Rect> {
        let s = lock(&self.st).screens.clone();
        if !s.is_empty() {
            return s;
        }
        self.backend().map(|b| b.screens()).unwrap_or_default()
    }

    // ================================================================ links
    fn links_loop(&self) {
        while !self.stop_flag.load(Ordering::SeqCst) {
            let peers = self.store.peer_ids();
            let (gone, current) = {
                let mut st = lock(&self.st);
                for pid in &peers {
                    if !st.links.contains_key(pid) {
                        st.links
                            .insert(pid.clone(), PeerLink::new(self.me.clone(), pid));
                    }
                }
                let gone: Vec<String> = st
                    .links
                    .keys()
                    .filter(|p| !peers.contains(p))
                    .cloned()
                    .collect();
                let gone: Vec<Arc<PeerLink>> =
                    gone.iter().filter_map(|p| st.links.remove(p)).collect();
                (gone, st.links.values().cloned().collect::<Vec<_>>())
            };
            for l in gone {
                l.close();
            }
            for l in current {
                if self.stop_flag.load(Ordering::SeqCst) {
                    break;
                }
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| l.maintain())).is_err()
                {
                    // one peer must not stop the loop for all
                    log::error!("Flow: link with {}", l.pid);
                    l.close();
                }
            }
            self.expire_pairing_window();
            let due = lock(&self.st)
                .last_cleanup
                .is_none_or(|t| t.elapsed() > CLEANUP_EVERY);
            if due {
                self.cleanup_staging();
            }
            for _ in 0..5 {
                if self.stop_flag.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// The sink a backend reports to; events of a backend that is no longer the current one
/// are dropped.
struct ServiceSink {
    svc: Weak<FlowService>,
    gen_: u64,
}

impl ServiceSink {
    fn current(&self) -> Option<Arc<FlowService>> {
        self.svc
            .upgrade()
            .filter(|s| s.backend_gen.load(Ordering::SeqCst) == self.gen_)
    }
}

impl Sink for ServiceSink {
    fn edge(&self, side: &'static str, x: f64, y: f64, rect: Rect) {
        if let Some(s) = self.current() {
            s.post(Item::Edge(AtEdge { side, x, y, rect }));
        }
    }
    fn leave(&self, side: &'static str) {
        if let Some(s) = self.current() {
            s.post(Item::Leave(side));
        }
    }
    fn ctrl(&self, held: bool) {
        if let Some(s) = self.current() {
            s.post(Item::Ctrl(held));
        }
    }
    fn screens(&self, rects: Vec<Rect>) {
        if let Some(s) = self.current() {
            lock(&s.st).screens = clean_rects(&rects);
        }
    }
    fn state(&self, state: &str, message: Option<Text>) {
        if let Some(s) = self.current() {
            log::info!(
                "Flow: edge detection {state} {}",
                message.as_ref().map(Text::english).unwrap_or_default()
            );
            s.emit_flow("backend");
        }
    }
    fn file_drag(&self, side: &'static str, paths: Vec<PathBuf>) {
        if let Some(s) = self.current() {
            s.post(Item::FileDrag(side, paths));
        }
    }
    fn drag_glow_wanted(&self) -> bool {
        self.current().is_some_and(|s| !s.glow_edges().is_empty())
    }
    fn files_dragging(&self, on: bool) {
        if let Some(s) = self.current() {
            s.post(Item::FilesDragging(on));
        }
    }
    fn pickup(&self, news: PickupNews) {
        if let Some(s) = self.current() {
            s.post(Item::Pickup(news));
        }
    }
}

impl glisa_common::FlowApi for FlowService {
    fn api(&self, method: &str, params: &Value) -> glisa_common::ApiResult {
        self.handle(method, params)
    }

    fn device_appearing(&self, d: &AppearingDevice) {
        self.appearing(d);
    }

    fn device_online(&self, device_id: &str) {
        self.online(device_id);
    }

    fn device_forgotten(&self, key: &str) {
        if self.store.forget_device(key) {
            self.emit("flow", json!({"what": "channels"}));
        }
    }

    fn stop(&self) {
        FlowService::stop(self);
    }
}
