//! The device service: finds devices (scan + hot-plug), applies their settings, watches
//! them come and go, runs button actions and answers the JSON API.
//!
//! Threads: one loop (scan every 2 s, liveness of devices behind receivers, battery),
//! one HID++ reader per node (notifications run there and must not block), one probe
//! per new node, the IPC server. Locks are never held across a HID++ request when a
//! reader thread could need them.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use glisa_common::{
    AppearingDevice, Event, EventSink, FlowApi, FlowDevice, FlowHost, monotonic_ms,
};
use glisa_hid::HidInfo;
use glisa_hidpp::sim::{SimEndpoint, SimulatedMouse};
use glisa_hidpp::{Channel, Device, Error as HidppError, ListenerId, Transport, f};
use glisa_input::VirtualInput;
use serde_json::{Value, json};

use crate::config::Config;
use crate::device::ManagedDevice;
use crate::executor::{Executor, Special};
use crate::fsutil::error_text;
use crate::hidbackend::{HidBackend, SystemHid};

pub const SCAN_INTERVAL: Duration = Duration::from_secs(2);
pub const BATTERY_INTERVAL: Duration = Duration::from_secs(120);
pub const ALIVE_INTERVAL: Duration = Duration::from_secs(6);
/// The `device` request re-reads a battery older than this.
pub const BATTERY_FRESH: Duration = Duration::from_secs(20);
const INPUT_RETRY: Duration = Duration::from_secs(30);

/// How the service is built (the Tauri shell and the tests fill it).
pub struct ServiceOptions {
    /// Settings file; `None`: `config.json` in the platform's config directory.
    pub config_path: Option<PathBuf>,
    /// Add the simulated devices (`lift`, `mx`, `keys`) on `start`.
    pub simulate: bool,
    /// Record input events instead of creating a virtual device.
    pub dry_run_input: bool,
    /// IPC endpoint (Unix socket path, Windows pipe name); `None`: the default;
    /// `Some("")`: no IPC server.
    pub ipc: Option<String>,
    /// Receives "devices", "device" and "status" events.
    pub events: EventSink,
}

impl Default for ServiceOptions {
    fn default() -> Self {
        ServiceOptions {
            config_path: None,
            simulate: false,
            dry_run_input: false,
            ipc: None,
            events: Arc::new(|_| {}),
        }
    }
}

/// A removed device that is still attached: it is added again only once it
/// reconnects.
pub(crate) struct Slot {
    source: String,
    path: String,
    dev: Arc<Device>,
    channel: Arc<Channel>,
    bus: Option<&'static str>,
    st: Mutex<SlotState>,
}

struct SlotState {
    away: bool,
    misses: u32,
    checked: Option<Instant>,
    watch_id: Option<u64>,
    closed_id: Option<ListenerId>,
}

pub struct Service {
    pub config: Config,
    simulate: bool,
    dry_run_input: bool,
    ipc_endpoint: Option<String>,
    events: EventSink,
    devices: Mutex<Vec<Arc<ManagedDevice>>>,
    channels: Mutex<HashMap<String, Arc<Channel>>>,
    sims: Mutex<HashMap<String, Arc<SimulatedMouse>>>,
    denied: Mutex<Vec<String>>,
    opened_any: AtomicBool,
    forgotten: Mutex<Vec<Arc<Slot>>>,
    executor: Mutex<Executor>,
    input_error: Mutex<String>,
    last_input_retry: Mutex<Option<Instant>>,
    stopped: Mutex<bool>,
    wake: Condvar,
    scan_lock: Mutex<()>,
    pub(crate) api_lock: Mutex<()>,
    flow: RwLock<Option<Arc<dyn FlowApi>>>,
    hid: RwLock<Arc<dyn HidBackend>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    ipc_server: Mutex<Option<crate::ipc::Server>>,
    me: Weak<Service>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Name of our virtual keyboard/mouse ("Glisa virtual input").
fn vinput_name() -> String {
    format!("{} virtual input", glisa_common::APP_NAME)
}

fn input_error_text(e: &io::Error) -> String {
    if cfg!(target_os = "linux") {
        format!("/dev/uinput: {}", error_text(e))
    } else {
        error_text(e)
    }
}

impl Service {
    pub fn new(opts: ServiceOptions) -> io::Result<Arc<Service>> {
        let config = Config::new(opts.config_path.clone());
        let (vinput, input_error) = match VirtualInput::open(&vinput_name(), opts.dry_run_input) {
            Ok(v) => (v, String::new()),
            Err(e) => {
                log::warn!("no virtual input device ({e}): shortcuts will not work");
                (VirtualInput::dry_run(), input_error_text(&e))
            }
        };
        Ok(Arc::new_cyclic(|me| Service {
            config,
            simulate: opts.simulate,
            dry_run_input: opts.dry_run_input,
            ipc_endpoint: opts.ipc,
            events: opts.events,
            devices: Mutex::new(Vec::new()),
            channels: Mutex::new(HashMap::new()),
            sims: Mutex::new(HashMap::new()),
            denied: Mutex::new(Vec::new()),
            opened_any: AtomicBool::new(false),
            forgotten: Mutex::new(Vec::new()),
            executor: Mutex::new(Executor::new(vinput)),
            input_error: Mutex::new(input_error),
            last_input_retry: Mutex::new(None),
            stopped: Mutex::new(false),
            wake: Condvar::new(),
            scan_lock: Mutex::new(()),
            api_lock: Mutex::new(()),
            flow: RwLock::new(None),
            hid: RwLock::new(Arc::new(SystemHid)),
            threads: Mutex::new(Vec::new()),
            ipc_server: Mutex::new(None),
            me: me.clone(),
        }))
    }

    fn arc(&self) -> Arc<Service> {
        self.me.upgrade().expect("the service is alive while used")
    }

    /// Starts the simulated devices (with `simulate`), the scan/hot-plug loop and the
    /// IPC server. An IPC error (another service already listening) is returned; the
    /// rest keeps running.
    pub fn start(self: &Arc<Self>) -> io::Result<()> {
        if self.simulate {
            for p in ["lift", "mx", "keys"] {
                self.add_simulated(p);
            }
        }
        let me = Arc::downgrade(self);
        let handle = std::thread::Builder::new()
            .name(format!("{}-scan", glisa_common::APP_DIR))
            .spawn(move || {
                while let Some(s) = me.upgrade() {
                    if s.is_stopped() {
                        break;
                    }
                    s.loop_once();
                    let wait = s.wait_stop(SCAN_INTERVAL);
                    drop(s);
                    if wait {
                        break;
                    }
                }
            })?;
        lock(&self.threads).push(handle);
        let endpoint = match &self.ipc_endpoint {
            Some(e) if e.is_empty() => return Ok(()),
            Some(e) => e.clone(),
            None => crate::ipc::default_endpoint()?,
        };
        let server = crate::ipc::serve(Arc::downgrade(self), &endpoint)?;
        log::info!(
            "{} {} listening on {endpoint}",
            glisa_common::APP_NAME,
            glisa_common::VERSION
        );
        *lock(&self.ipc_server) = Some(server);
        Ok(())
    }

    /// Stops everything: loop, IPC, Flow, device channels; releases held input.
    pub fn stop(&self) {
        {
            let mut s = lock(&self.stopped);
            if *s {
                return;
            }
            *s = true;
        }
        self.wake.notify_all();
        if let Some(mut server) = lock(&self.ipc_server).take() {
            server.stop();
        }
        if let Some(flow) = self.flow() {
            flow.stop();
        }
        let threads: Vec<JoinHandle<()>> = lock(&self.threads).drain(..).collect();
        for t in threads {
            if t.thread().id() != std::thread::current().id() {
                let _ = t.join();
            }
        }
        let channels: Vec<Arc<Channel>> = lock(&self.channels).drain().map(|(_, c)| c).collect();
        for ch in channels {
            ch.close();
        }
        self.release_all_input_now();
        lock(&self.executor).vinput.close();
    }

    pub fn is_stopped(&self) -> bool {
        *lock(&self.stopped)
    }

    /// Waits up to `d` or until stopped; true when stopped.
    fn wait_stop(&self, d: Duration) -> bool {
        let g = lock(&self.stopped);
        let (g, _) = self
            .wake
            .wait_timeout_while(g, d, |s| !*s)
            .unwrap_or_else(|e| e.into_inner());
        *g
    }

    /// Replaces how nodes are found and opened (tests).
    pub fn set_hid_backend(&self, backend: Arc<dyn HidBackend>) {
        *self.hid.write().unwrap_or_else(|e| e.into_inner()) = backend;
    }

    fn hid(&self) -> Arc<dyn HidBackend> {
        self.hid.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn simulate(&self) -> bool {
        self.simulate
    }

    // ------------------------------------------------------------------ events

    pub(crate) fn emit(&self, topic: &str, data: Value) {
        let sink = self.events.clone();
        let event = Event::new(topic, data);
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink(event))).is_err() {
            log::warn!("event subscriber panicked");
        }
    }

    pub(crate) fn emit_device(&self, id: &str) {
        self.emit("device", json!({"id": id}));
    }

    pub(crate) fn emit_devices(&self) {
        self.emit("devices", json!({}));
    }

    // ------------------------------------------------------------------- Flow

    /// Routes `flow_*` to Flow and tells it about devices arriving.
    pub fn set_flow(&self, flow: Arc<dyn FlowApi>) {
        *self.flow.write().unwrap_or_else(|e| e.into_inner()) = Some(flow);
    }

    pub(crate) fn flow(&self) -> Option<Arc<dyn FlowApi>> {
        self.flow.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn device_appearing(&self, info: &HidInfo, appeared_ms: u64) {
        if let Some(flow) = self.flow() {
            flow.device_appearing(&AppearingDevice {
                path: info.path.clone(),
                vendor: info.vendor,
                product: info.product,
                is_receiver: info.is_receiver(),
                is_bluetooth: info.is_bluetooth(),
                appeared_ms,
            });
        }
    }

    /// A device appeared or came back (Flow may be waiting for it).
    pub(crate) fn device_online(&self, md: &Arc<ManagedDevice>) {
        if let Some(flow) = self.flow() {
            flow.device_online(&md.id);
        }
    }

    /// Looks for a device on its way here right now (Flow): a Bluetooth node
    /// appearing, a receiver device answering again.
    pub fn poll_arrivals(&self) {
        if self.simulate {
            return;
        }
        self.scan_once();
        for md in self.devices() {
            if !md.online()
                && md.source.contains('#')
                && !md.dev.channel().is_closed()
                && md.check_alive()
            {
                self.emit_devices();
            }
        }
    }

    // ------------------------------------------------------------- devices

    pub fn devices(&self) -> Vec<Arc<ManagedDevice>> {
        lock(&self.devices).clone()
    }

    pub fn device(&self, id: &str) -> Option<Arc<ManagedDevice>> {
        lock(&self.devices).iter().find(|d| d.id == id).cloned()
    }

    pub fn denied(&self) -> Vec<String> {
        lock(&self.denied).clone()
    }

    pub fn input_error(&self) -> String {
        lock(&self.input_error).clone()
    }

    pub(crate) fn opened_any(&self) -> bool {
        self.opened_any.load(Ordering::SeqCst)
    }

    pub(crate) fn dry_run_input(&self) -> bool {
        self.dry_run_input
    }

    /// The node's channel, if open (tests, diagnostics).
    pub fn channel(&self, path: &str) -> Option<Arc<Channel>> {
        lock(&self.channels).get(path).cloned()
    }

    pub fn channel_paths(&self) -> Vec<String> {
        lock(&self.channels).keys().cloned().collect()
    }

    pub(crate) fn sim(&self, source: &str) -> Option<Arc<SimulatedMouse>> {
        lock(&self.sims).get(source).cloned()
    }

    /// Input events recorded so far (dry-run input only).
    pub fn input_events(&self) -> Vec<glisa_input::Event> {
        lock(&self.executor).vinput.events().to_vec()
    }

    /// A simulated device of `profile` ("lift", "mx", "keys"), connected now.
    pub fn add_simulated(&self, profile: &str) -> Arc<SimulatedMouse> {
        let sim = SimulatedMouse::new(profile, 0xFF);
        if let Err(e) = self.attach_simulated(&sim) {
            log::warn!("could not add the simulated {profile}: {e}");
        }
        sim
    }

    pub fn attach_simulated(
        &self,
        sim: &Arc<SimulatedMouse>,
    ) -> Result<Arc<ManagedDevice>, HidppError> {
        self.attach_sim(sim.clone(), sim.clone())
    }

    /// A simulated device that moved here from another simulated computer.
    pub fn attach_sim_endpoint(
        &self,
        ep: &Arc<SimEndpoint>,
    ) -> Result<Arc<ManagedDevice>, HidppError> {
        self.attach_sim(ep.clone(), ep.sim().clone())
    }

    fn attach_sim(
        &self,
        transport: Arc<dyn Transport>,
        sim: Arc<SimulatedMouse>,
    ) -> Result<Arc<ManagedDevice>, HidppError> {
        let path = transport.path().to_string();
        let ch = Channel::new(transport);
        lock(&self.channels).insert(path.clone(), ch.clone());
        lock(&self.sims).insert(path.clone(), sim.clone());
        let dev = Device::new(ch.clone(), 0xFF, &path, sim.product_id);
        self.register(dev, &path, &ch, None, None)
    }

    /// Registers the device at `devidx` of an open channel as `source` (tests: a
    /// device behind a receiver is `"<path>#<index>"`).
    pub fn attach_channel_device(
        &self,
        ch: &Arc<Channel>,
        devidx: u8,
        source: &str,
        product_id: u16,
    ) -> Result<Arc<ManagedDevice>, HidppError> {
        let dev = Device::new(ch.clone(), devidx, ch.path(), product_id);
        self.register(dev, source, ch, None, None)
    }

    fn register(
        &self,
        dev: Arc<Device>,
        source: &str,
        ch: &Arc<Channel>,
        appeared_ms: Option<u64>,
        bus: Option<&'static str>,
    ) -> Result<Arc<ManagedDevice>, HidppError> {
        let md = ManagedDevice::new(&self.arc(), dev, source)?;
        // When it became usable (its node appeared): Flow must not move the pointer
        // long after the user started using the mouse.
        md.set_appeared_ms(appeared_ms.unwrap_or_else(monotonic_ms));
        md.set_bus(bus);
        let (old, stale) = {
            let mut devices = lock(&self.devices);
            let old = match devices.iter().position(|d| d.id == md.id) {
                Some(i) => Some(std::mem::replace(&mut devices[i], md.clone())),
                None => {
                    devices.push(md.clone());
                    None
                }
            };
            let mut forgotten = lock(&self.forgotten);
            let stale = forgotten
                .iter()
                .position(|s| s.source == source)
                .map(|i| forgotten.remove(i));
            (old, stale)
        };
        if let Some(old) = old.filter(|o| !Arc::ptr_eq(o, &md)) {
            old.detach();
        }
        if let Some(stale) = stale {
            self.unwatch(&stale);
        }
        md.apply();
        let me = self.me.clone();
        let weak = Arc::downgrade(&md);
        let id = ch.add_closed_listener(Arc::new(move || {
            if let (Some(s), Some(md)) = (me.upgrade(), weak.upgrade()) {
                s.gone(&md);
            }
        }));
        *lock(&md.closed_cb) = Some(id);
        log::info!("device connected: {} ({source})", md.name);
        self.device_online(&md);
        self.emit_devices();
        self.emit("status", json!({}));
        Ok(md)
    }

    /// Its channel closed: the device is offline until its node comes back.
    fn gone(&self, md: &Arc<ManagedDevice>) {
        log::info!("device disconnected: {}", md.name);
        md.set_online(false);
        self.release_all_input_now();
        let path = md.source.split('#').next().unwrap_or("").to_string();
        let ch = {
            let mut channels = lock(&self.channels);
            match channels.get(&path) {
                Some(c) if Arc::ptr_eq(c, md.dev.channel()) => channels.remove(&path),
                _ => None,
            }
        };
        drop(ch);
        self.emit_devices();
        self.emit_device(&md.id);
    }

    // ----------------------------------------------------------- removing

    /// Removes a device from Glisa: its stored settings go, its factory behaviour
    /// is restored when it is here, and it leaves the list. If it is still connected it
    /// is only added again (with default settings) once it reconnects.
    pub fn forget(&self, md: &Arc<ManagedDevice>) {
        {
            let mut devices = lock(&self.devices);
            if let Some(i) = devices.iter().position(|d| Arc::ptr_eq(d, md)) {
                devices.remove(i);
            }
        }
        md.detach();
        self.release_all_input_now();
        if let Err(e) = self.config.forget_device(&md.key) {
            log::warn!("{}: could not save the settings: {e}", md.name);
        }
        // Flow's channel choices for the device go with it.
        if let Some(flow) = self.flow() {
            flow.device_forgotten(&md.key);
        }
        if md.online() {
            md.factory_reset();
        }
        self.watch(md);
        log::info!("device removed: {} ({})", md.name, md.source);
        self.emit_devices();
        self.emit("status", json!({}));
    }

    fn watch(&self, md: &Arc<ManagedDevice>) {
        let ch = md.dev.channel().clone();
        if let Some(id) = lock(&md.closed_cb).take() {
            ch.remove_closed_listener(id);
        }
        if ch.is_closed() {
            md.dev.detach(); // already gone: a scan finds it when it comes back
            return;
        }
        let slot = Arc::new(Slot {
            source: md.source.clone(),
            path: md.source.split('#').next().unwrap_or("").to_string(),
            dev: md.dev.clone(),
            channel: ch.clone(),
            bus: md.bus(),
            st: Mutex::new(SlotState {
                away: !md.online(),
                misses: 0,
                checked: None,
                watch_id: None,
                closed_id: None,
            }),
        });
        self.arm(&slot);
        let previous = {
            let mut forgotten = lock(&self.forgotten);
            let prev = forgotten
                .iter()
                .position(|s| s.source == slot.source)
                .map(|i| forgotten.remove(i));
            forgotten.push(slot.clone());
            prev
        };
        if let Some(p) = previous.filter(|p| !Arc::ptr_eq(p, &slot)) {
            self.unwatch(&p);
        }
        if ch.is_closed() {
            self.slot_closed(&slot);
        }
    }

    /// Listens for the removed device reconnecting, and for its node closing.
    fn arm(&self, slot: &Arc<Slot>) {
        let me = self.me.clone();
        let weak = Arc::downgrade(slot);
        let watch_id = slot.dev.add_listener(Arc::new(move |feature, function, _| {
            if feature == f::WIRELESS_DEVICE_STATUS && function == 0 {
                if let (Some(s), Some(slot)) = (me.upgrade(), weak.upgrade()) {
                    std::thread::spawn(move || s.readd(&slot, Duration::from_millis(300)));
                }
            }
        }));
        let me = self.me.clone();
        let weak = Arc::downgrade(slot);
        let closed_id = slot.channel.add_closed_listener(Arc::new(move || {
            if let (Some(s), Some(slot)) = (me.upgrade(), weak.upgrade()) {
                s.slot_closed(&slot);
            }
        }));
        let mut st = lock(&slot.st);
        st.watch_id = Some(watch_id);
        st.closed_id = Some(closed_id);
    }

    fn slot_closed(&self, slot: &Arc<Slot>) {
        {
            let mut forgotten = lock(&self.forgotten);
            if let Some(i) = forgotten.iter().position(|s| Arc::ptr_eq(s, slot)) {
                forgotten.remove(i);
            }
        }
        let mut channels = lock(&self.channels);
        if channels
            .get(&slot.path)
            .is_some_and(|c| Arc::ptr_eq(c, &slot.channel))
        {
            channels.remove(&slot.path);
        }
    }

    fn unwatch(&self, slot: &Slot) {
        let (watch, closed) = {
            let mut st = lock(&slot.st);
            (st.watch_id.take(), st.closed_id.take())
        };
        if let Some(id) = watch {
            slot.dev.remove_listener(id);
        }
        if let Some(id) = closed {
            slot.channel.remove_closed_listener(id);
        }
    }

    /// A removed device reconnected: add it again, with default settings.
    fn readd(&self, slot: &Arc<Slot>, settle: Duration) {
        if !settle.is_zero() {
            std::thread::sleep(settle); // let it finish waking up
        }
        {
            let mut forgotten = lock(&self.forgotten);
            match forgotten.iter().position(|s| Arc::ptr_eq(s, slot)) {
                Some(i) => {
                    forgotten.remove(i);
                }
                None => return, // already handled (or replaced)
            }
        }
        self.unwatch(slot);
        if slot.channel.is_closed() {
            return;
        }
        if let Err(e) = self.register(
            slot.dev.clone(),
            &slot.source,
            &slot.channel,
            None,
            slot.bus,
        ) {
            log::warn!(
                "could not add {} again ({e}); waiting for its next connection",
                slot.source
            );
            {
                let mut st = lock(&slot.st);
                st.away = true;
                st.misses = 0;
            }
            self.arm(slot);
            let mut forgotten = lock(&self.forgotten);
            if !forgotten.iter().any(|s| s.source == slot.source) {
                forgotten.push(slot.clone());
            }
        }
    }

    /// Sources of removed devices still attached (waiting to reconnect).
    pub fn forgotten(&self) -> Vec<String> {
        self.prune_forgotten();
        lock(&self.forgotten)
            .iter()
            .map(|s| s.source.clone())
            .collect()
    }

    /// Drops the watch of removed devices whose channel closed (a channel closed by us
    /// does not call its closed listeners).
    fn prune_forgotten(&self) {
        let dead: Vec<Arc<Slot>> = {
            let mut forgotten = lock(&self.forgotten);
            let (dead, alive): (Vec<_>, Vec<_>) =
                forgotten.drain(..).partition(|s| s.channel.is_closed());
            *forgotten = alive;
            dead
        };
        for slot in dead {
            self.unwatch(&slot);
        }
    }

    /// Behind a receiver a device keeps its node, so its reconnection is probed: one
    /// that stopped answering and answers again is back. Slots checked less than
    /// `every` ago are skipped.
    pub fn check_forgotten(&self, every: Duration) {
        self.prune_forgotten();
        let slots: Vec<Arc<Slot>> = lock(&self.forgotten)
            .iter()
            .filter(|s| s.source.contains('#'))
            .cloned()
            .collect();
        for slot in slots {
            if slot.channel.is_closed() {
                continue;
            }
            {
                let mut st = lock(&slot.st);
                if st.checked.is_some_and(|t| t.elapsed() < every) {
                    continue;
                }
                st.checked = Some(Instant::now());
            }
            if slot.dev.ping(Duration::from_millis(600)).is_none() {
                let mut st = lock(&slot.st);
                st.misses += 1;
                if st.misses >= 2 {
                    st.away = true;
                }
            } else {
                let away = {
                    let mut st = lock(&slot.st);
                    st.misses = 0;
                    st.away
                };
                if away {
                    self.readd(&slot, Duration::ZERO);
                }
            }
        }
    }

    // ---------------------------------------------------------------- scan

    /// Opens Logitech nodes that appeared and closes those that went away.
    pub fn scan_once(&self) {
        let _scan = lock(&self.scan_lock);
        let hid = self.hid();
        let found = hid.scan();
        let gone: Vec<Arc<Channel>> = {
            let mut channels = lock(&self.channels);
            let paths: Vec<String> = channels
                .keys()
                .filter(|p| !p.starts_with("sim://") && !found.iter().any(|i| &i.path == *p))
                .cloned()
                .collect();
            paths.iter().filter_map(|p| channels.remove(p)).collect()
        };
        for ch in gone {
            ch.close();
        }
        self.prune_forgotten();
        let mut denied = Vec::new();
        for info in found {
            if lock(&self.channels).contains_key(&info.path) {
                continue;
            }
            match hid.open(&info) {
                Ok(t) => {
                    let ch = Channel::new(Arc::from(t));
                    lock(&self.channels).insert(info.path.clone(), ch.clone());
                    self.opened_any.store(true, Ordering::SeqCst);
                    let appeared = monotonic_ms();
                    // The OS creates the input device together with this node, so the
                    // mouse already moves the pointer now: tell Flow before the (slow,
                    // over Bluetooth) HID++ set-up.
                    self.device_appearing(&info, appeared);
                    let me = self.me.clone();
                    std::thread::spawn(move || {
                        if let Some(s) = me.upgrade() {
                            s.probe(&info, &ch, appeared);
                        }
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => denied.push(info.path),
                Err(e) => log::debug!("cannot open {}: {e}", info.path),
            }
        }
        let changed = {
            let mut d = lock(&self.denied);
            let changed = *d != denied;
            *d = denied;
            changed
        };
        if changed {
            self.emit("status", json!({}));
        }
    }

    fn probe(&self, info: &HidInfo, ch: &Arc<Channel>, appeared: u64) {
        let indices: Vec<u8> = if info.is_receiver() {
            (1..=6).collect()
        } else {
            vec![0xFF]
        };
        for idx in indices {
            let dev = Device::new(ch.clone(), idx, &info.path, info.product);
            match dev.ping(Duration::from_secs(1)) {
                Some((major, _)) if major >= 2 => {}
                _ => {
                    dev.detach();
                    continue;
                }
            }
            let source = if info.is_receiver() {
                format!("{}#{idx}", info.path)
            } else {
                info.path.clone()
            };
            let bus = if info.is_bluetooth() {
                "bluetooth"
            } else {
                "usb"
            };
            let appeared = (!info.is_receiver()).then_some(appeared);
            if let Err(e) = self.register(dev, &source, ch, appeared, Some(bus)) {
                log::error!("error initialising {source}: {e}");
            }
        }
    }

    /// One pass of the background loop.
    pub fn loop_once(&self) {
        self.scan_once();
        for md in self.devices() {
            if md.source.contains('#')
                && !md.dev.channel().is_closed()
                && md.alive_due(ALIVE_INTERVAL)
                && md.check_alive()
            {
                self.emit_devices();
                self.emit_device(&md.id);
            }
            if md.online()
                && md.battery_age().is_none_or(|a| a > BATTERY_INTERVAL)
                && md.refresh_battery()
            {
                self.emit_device(&md.id);
            }
        }
        self.check_forgotten(ALIVE_INTERVAL);
        self.retry_input(false);
    }

    /// Tries to create the virtual input device again after it failed (the user may
    /// have installed the permissions since). `now`: don't wait for the retry delay.
    pub fn retry_input(&self, now: bool) -> bool {
        if self.dry_run_input || lock(&self.input_error).is_empty() {
            return false;
        }
        {
            let mut last = lock(&self.last_input_retry);
            if !now && last.is_some_and(|t| t.elapsed() < INPUT_RETRY) {
                return false;
            }
            *last = Some(Instant::now());
        }
        match VirtualInput::open(&vinput_name(), false) {
            Ok(v) => {
                let mut ex = lock(&self.executor);
                ex.release_all();
                ex.vinput = v;
                lock(&self.input_error).clear();
                log::info!("virtual input device created");
                self.emit("status", json!({}));
                true
            }
            Err(e) => {
                *lock(&self.input_error) = input_error_text(&e);
                false
            }
        }
    }

    // ------------------------------------------------------------- actions

    pub(crate) fn on_buttons(&self, md: &Arc<ManagedDevice>, pressed: Vec<u16>) {
        let cfg = md.cfg();
        let empty = serde_json::Map::new();
        let buttons = cfg
            .get("buttons")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let action_of = |cid: u16| {
            buttons
                .get(&format!("{cid:#06x}"))
                .filter(|a| crate::actions::truthy(Some(a)))
                .cloned()
                .unwrap_or_else(|| json!({"type": "disabled"}))
        };
        let prev = std::mem::replace(&mut md.state().pressed, pressed.clone());
        let mut specials = Vec::new();
        {
            let mut ex = lock(&self.executor);
            for cid in pressed.iter().filter(|c| !prev.contains(c)) {
                specials.extend(ex.pressed(*cid, &action_of(*cid)));
            }
            for cid in prev.iter().filter(|c| !pressed.contains(c)) {
                specials.extend(ex.released(*cid, &action_of(*cid)));
            }
        }
        for s in specials {
            match s {
                Special::DpiCycle => {
                    let md = md.clone();
                    std::thread::spawn(move || md.cycle_dpi());
                }
            }
        }
    }

    pub(crate) fn on_moved(&self, dx: i32, dy: i32) {
        lock(&self.executor).moved(dx, dy);
    }

    pub(crate) fn release_all_input_now(&self) {
        lock(&self.executor).release_all();
    }
}

impl FlowHost for Service {
    fn flow_devices(&self) -> Vec<FlowDevice> {
        self.devices().iter().map(|md| md.flow_device()).collect()
    }

    fn set_host(&self, device_id: &str, channel: u8) -> Result<(), String> {
        let md = self
            .device(device_id)
            .ok_or_else(|| glisa_i18n::tr!("Device not found"))?;
        md.set_host(i64::from(channel)).map_err(|e| match e {
            crate::device::DevError::Invalid(m) => m,
            other => format!("{other:?}"),
        })
    }

    fn release_all_input(&self) {
        self.release_all_input_now();
    }

    fn poll_arrivals(&self) {
        Service::poll_arrivals(self);
    }
}
