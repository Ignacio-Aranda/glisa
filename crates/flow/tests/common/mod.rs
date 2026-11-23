//! Several Glisa "computers" in one process for Flow tests.
//!
//! Each [`Computer`] is a FlowService with its own temporary store and staging folder,
//! loopback-only TCP/UDP ports and a FakeBackend (fake edges, dry-run absolute pointer,
//! in-memory clipboard). A [`SimHub`] moves ONE simulated device between computers when it
//! receives set_host: it goes offline on the old computer and comes online on the computer
//! registered for that channel. Nothing touches the real desktop, the LAN or a device.

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use glisa_common::{DeviceKind, Event, FlowApi, FlowDevice, FlowHost};
use glisa_flow::backends::FakeBackend;
use glisa_flow::geometry::Rect;
use glisa_flow::{BackendChoice, FlowOptions, FlowService, NetConfig, free_ports};
use serde_json::{Value, json};

pub const LIFT: &str = "b031-lift";
pub const KEYS: &str = "b35b-keys";

pub fn wait_until(timeout: f64, mut pred: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs_f64(timeout);
    while Instant::now() < end {
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    pred()
}

pub fn sleep(s: f64) {
    std::thread::sleep(Duration::from_secs_f64(s));
}

/// The device service as Flow sees it.
#[derive(Default)]
pub struct SimHost {
    pub devices: Mutex<Vec<FlowDevice>>,
    pub hubs: Mutex<Vec<Arc<SimHub>>>,
    pub released: AtomicUsize,
    pub me: Mutex<Weak<SimHost>>,
}

impl SimHost {
    pub fn device(&self, id: &str) -> Option<FlowDevice> {
        self.devices
            .lock()
            .unwrap()
            .iter()
            .find(|d| d.id == id)
            .cloned()
    }

    pub fn set_device(&self, d: FlowDevice) {
        let mut devs = self.devices.lock().unwrap();
        devs.retain(|x| x.id != d.id);
        devs.push(d);
    }

    pub fn set_online(&self, id: &str, online: bool, current: u8) {
        for d in self.devices.lock().unwrap().iter_mut() {
            if d.id == id {
                d.online = online;
                d.current_host = current;
            }
        }
    }
}

impl FlowHost for SimHost {
    fn flow_devices(&self) -> Vec<FlowDevice> {
        self.devices.lock().unwrap().clone()
    }

    fn set_host(&self, device_id: &str, channel: u8) -> Result<(), String> {
        let hub = self
            .hubs
            .lock()
            .unwrap()
            .iter()
            .find(|h| h.id == device_id)
            .cloned();
        match hub {
            Some(h) => {
                h.switch(channel);
                Ok(())
            }
            None => Err("no such device".into()),
        }
    }

    fn release_all_input(&self) {
        self.released.fetch_add(1, Ordering::SeqCst);
    }
}

/// Channel → the computer registered for it.
pub type Computers = HashMap<u8, (Arc<SimHost>, Weak<FlowService>)>;

/// One simulated device paired with several computers (one per channel).
pub struct SimHub {
    pub id: String,
    pub template: FlowDevice,
    pub computers: Mutex<Computers>,
    pub moves: Mutex<Vec<u8>>,
    pub current: Mutex<u8>,
}

impl SimHub {
    pub fn moves(&self) -> Vec<u8> {
        self.moves.lock().unwrap().clone()
    }

    fn switch(&self, channel: u8) {
        let old = *self.current.lock().unwrap();
        self.moves.lock().unwrap().push(channel);
        *self.current.lock().unwrap() = channel;
        let computers = self.computers.lock().unwrap();
        if let Some((host, _)) = computers.get(&old) {
            host.set_online(&self.id, false, old);
        }
        if let Some((host, flow)) = computers.get(&channel) {
            let mut d = self.template.clone();
            d.online = true;
            d.current_host = channel;
            host.set_device(d);
            let flow = flow.clone();
            let id = self.id.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                if let Some(f) = flow.upgrade() {
                    f.device_online(&id);
                }
            });
        }
    }
}

pub struct Computer {
    pub name: String,
    pub tmp: tempfile::TempDir,
    pub port: u16,
    pub backend: Arc<FakeBackend>,
    pub host: Arc<SimHost>,
    pub flow: Arc<FlowService>,
    pub events: Arc<Mutex<Vec<Event>>>,
}

pub struct Opts {
    pub screens: Vec<Rect>,
    pub port: Option<u16>,
    pub targets: Vec<(String, u16)>,
    pub ctrl_available: bool,
    pub pairing_seconds: f64,
    pub discovery: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            screens: vec![[0, 0, 1920, 1080]],
            port: None,
            targets: Vec::new(),
            ctrl_available: true,
            pairing_seconds: 120.0,
            discovery: true,
        }
    }
}

impl Computer {
    pub fn new(name: &str, o: Opts) -> Self {
        let tmp = tempfile::Builder::new()
            .prefix("lo-flow-")
            .tempdir()
            .unwrap();
        let port = o
            .port
            .unwrap_or_else(|| free_ports(Ipv4Addr::LOCALHOST).unwrap());
        let backend = Arc::new(FakeBackend::with(&o.screens, o.ctrl_available, true, true));
        let host = Arc::new(SimHost::default());
        *host.me.lock().unwrap() = Arc::downgrade(&host);
        let events: Arc<Mutex<Vec<Event>>> = Arc::default();
        let ev = events.clone();
        let weak_host: Weak<dyn FlowHost> = Arc::downgrade(&(host.clone() as Arc<dyn FlowHost>));
        let flow = FlowService::new(
            weak_host,
            Arc::new(move |e| ev.lock().unwrap().push(e)),
            FlowOptions {
                store_dir: Some(tmp.path().join("flow")),
                staging_dir: Some(tmp.path().join("cache").join("flow-received")),
                net: Some(NetConfig {
                    bind: Ipv4Addr::LOCALHOST,
                    port,
                    udp_port: None,
                    targets: Some(o.targets.clone()),
                    discovery: o.discovery,
                    announce_interval: Duration::from_millis(300),
                }),
                backend: BackendChoice::Instance(backend.clone()),
                demo: false,
                hostname: Some(name.to_string()),
                pairing_seconds: o.pairing_seconds,
            },
        );
        Self {
            name: name.to_string(),
            tmp,
            port,
            backend,
            host,
            flow,
            events,
        }
    }

    pub fn id(&self) -> String {
        self.flow.store.id()
    }

    pub fn enable(self) -> Self {
        self.flow.set_enabled(true).unwrap();
        self
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.call_in(Some("es"), method, params)
    }

    pub fn call_in(
        &self,
        lang: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        glisa_i18n::with_language(lang, || self.flow.api(method, &params).map_err(|e| e.0))
    }

    pub fn pairing(&self) -> Value {
        self.flow.pairing_status()
    }

    pub fn pairing_state(&self) -> String {
        self.pairing()["state"].as_str().unwrap_or("").to_string()
    }

    /// Notices, in English (they are rendered in the language of whoever asks).
    pub fn events(&self) -> Vec<String> {
        self.events_in("en")
    }

    pub fn events_in(&self, lang: &str) -> Vec<String> {
        let r = self
            .call_in(Some(lang), "flow_events", json!({"since": 0}))
            .unwrap();
        r["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["text"].as_str().unwrap().to_string())
            .collect()
    }

    pub fn device(&self, id: &str) -> Option<FlowDevice> {
        self.host.device(id)
    }

    pub fn peer_state(&self, other: &Computer) -> Value {
        self.flow
            .peers_list()
            .into_iter()
            .find(|p| p["id"] == other.id().as_str())
            .unwrap_or(Value::Null)
    }

    pub fn last_transfer(&self) -> Value {
        self.flow
            .transfers_status()
            .last()
            .cloned()
            .unwrap_or(json!({"state": null, "done": 0, "active": false}))
    }

    pub fn staged(&self) -> Vec<String> {
        let root = &self.flow.staging.root;
        std::fs::read_dir(root)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n != ".spool")
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Drop for Computer {
    fn drop(&mut self) {
        self.flow.stop();
    }
}

pub fn two_computers_with(broadcast: bool, discovery: bool) -> (Computer, Computer) {
    let pa = free_ports(Ipv4Addr::LOCALHOST).unwrap();
    let mut pb = free_ports(Ipv4Addr::LOCALHOST).unwrap();
    while pb == pa || pb == pa + 1 || pb + 1 == pa {
        pb = free_ports(Ipv4Addr::LOCALHOST).unwrap();
    }
    let t = |p: u16| {
        if broadcast {
            vec![("127.0.0.1".to_string(), p)]
        } else {
            vec![]
        }
    };
    let a = Computer::new(
        "equipo-a",
        Opts {
            port: Some(pa),
            targets: t(pb),
            discovery,
            ..Default::default()
        },
    );
    let b = Computer::new(
        "equipo-b",
        Opts {
            screens: vec![[0, 0, 2560, 1440]],
            port: Some(pb),
            targets: t(pa),
            discovery,
            ..Default::default()
        },
    );
    (a.enable(), b.enable())
}

pub fn two_computers() -> (Computer, Computer) {
    two_computers_with(true, true)
}

/// The whole SAS pairing (b shows, a initiates). Returns (sas_a, sas_b).
pub fn pair_with(a: &Computer, b: &Computer, accept_a: bool, accept_b: bool) -> (String, String) {
    b.flow.pair_start().unwrap();
    a.flow
        .pair_request(None, Some(&format!("127.0.0.1:{}", b.port)))
        .unwrap();
    assert!(
        wait_until(8.0, || a.pairing_state() == "confirm"
            && b.pairing_state() == "confirm"),
        "{} / {}",
        a.pairing(),
        b.pairing()
    );
    let codes = (
        a.pairing()["sas"].as_str().unwrap().to_string(),
        b.pairing()["sas"].as_str().unwrap().to_string(),
    );
    a.flow.pair_confirm(accept_a).unwrap();
    b.flow.pair_confirm(accept_b).unwrap();
    let want = if accept_a && accept_b {
        "done"
    } else {
        "failed"
    };
    assert!(
        wait_until(8.0, || a.pairing_state() == want
            && b.pairing_state() == want),
        "{} / {}",
        a.pairing(),
        b.pairing()
    );
    codes
}

pub fn pair(a: &Computer, b: &Computer) -> (String, String) {
    pair_with(a, b, true, true)
}

/// b is to the right of a (and a to the left of b).
pub fn arrange(a: &Computer, b: &Computer) {
    a.flow.store.set_side(&b.id(), Some("right")).unwrap();
    b.flow.store.set_side(&a.id(), Some("left")).unwrap();
}

pub fn linked(a: &Computer, b: &Computer, timeout: f64) -> bool {
    let ok = |x: &Computer, other: &Computer| x.peer_state(other)["online"] == true;
    wait_until(timeout, || ok(a, b) && ok(b, a))
}

/// Paired, arranged and linked.
pub fn paired_pair() -> (Computer, Computer) {
    let (a, b) = two_computers();
    pair(&a, &b);
    arrange(&a, &b);
    assert!(linked(&a, &b, 6.0));
    (a, b)
}

fn device(id: &str, name: &str, kind: DeviceKind, names: &[&str]) -> FlowDevice {
    FlowDevice {
        id: id.to_string(),
        key: format!("{}:{}", id[..4].to_uppercase(), name),
        name: name.to_string(),
        kind,
        online: true,
        hosts_count: 3,
        current_host: 0,
        host_entries: names
            .iter()
            .enumerate()
            .map(|(i, n)| json!({"index": i, "paired": !n.is_empty(), "name": n}))
            .collect(),
    }
}

/// One simulated device paired with a on channel 1 and b on channel 2, connected to a.
pub fn shared_device_kind(
    a: &Computer,
    b: &Computer,
    id: &str,
    name: &str,
    kind: DeviceKind,
) -> Arc<SimHub> {
    let template = device(id, name, kind, &["equipo-a", "equipo-b", ""]);
    let hub = Arc::new(SimHub {
        id: id.to_string(),
        template: template.clone(),
        computers: Mutex::new(HashMap::from([
            (0, (a.host.clone(), Arc::downgrade(&a.flow))),
            (1, (b.host.clone(), Arc::downgrade(&b.flow))),
        ])),
        moves: Mutex::new(Vec::new()),
        current: Mutex::new(0),
    });
    a.host.hubs.lock().unwrap().push(hub.clone());
    b.host.hubs.lock().unwrap().push(hub.clone());
    a.host.set_device(template);
    hub
}

pub fn shared_device(a: &Computer, b: &Computer) -> Arc<SimHub> {
    shared_device_kind(a, b, LIFT, "LIFT", DeviceKind::Mouse)
}

/// `c`'s user copies these files in a file manager.
pub fn copy_files(c: &Computer, paths: &[std::path::PathBuf], text: &[u8]) {
    let uris: String = paths
        .iter()
        .map(|p| glisa_flow::transfer::uri_of(p) + "\r\n")
        .collect();
    let mut data = std::collections::BTreeMap::new();
    data.insert("text/uri-list".to_string(), uris.into_bytes());
    data.insert(
        "x-special/gnome-copied-files".to_string(),
        b"copy\n".to_vec(),
    );
    data.insert(
        "text/plain".to_string(),
        if text.is_empty() {
            b"paths".to_vec()
        } else {
            text.to_vec()
        },
    );
    c.backend.rich_clip.as_ref().unwrap().set(data);
}

pub fn sha(data: &[u8]) -> String {
    use std::fmt::Write;
    let d = ring::digest::digest(&ring::digest::SHA256, data);
    d.as_ref().iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}
