//! Helpers shared by the service's integration tests. Everything runs in temporary
//! directories with simulated devices and recorded input: nothing touches the real
//! settings, runtime directory, hardware or desktop.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glisa_hid::HidInfo;
use glisa_hidpp::sim::SimulatedMouse;
use glisa_hidpp::{ReportQueue, Transport, TransportClosed};
use glisa_service::{HidBackend, NoHid, Service, ServiceOptions};

pub fn wait_for(mut pred: impl FnMut() -> bool, secs: f64) -> bool {
    let end = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < end {
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    pred()
}

/// A fresh private temporary directory (short path: Unix socket names are limited).
pub struct TmpDir(pub PathBuf);

impl TmpDir {
    pub fn new(tag: &str) -> Self {
        use std::sync::atomic::AtomicU64;
        static N: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "lo-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        TmpDir(d)
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A service with its settings in `config`, recorded input, no IPC and no hardware.
pub fn service(config: PathBuf, simulate: bool) -> Arc<Service> {
    let s = Service::new(ServiceOptions {
        config_path: Some(config),
        simulate,
        dry_run_input: true,
        ipc: Some(String::new()),
        ..Default::default()
    })
    .unwrap();
    s.set_hid_backend(Arc::new(NoHid));
    s
}

pub fn call(
    s: &Service,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    s.handle(method, &params, Some("en")).map_err(|e| e.0)
}

/// Key events only (no SYN).
pub fn key_events(s: &Service) -> Vec<(u16, u16, i32)> {
    s.input_events().into_iter().filter(|e| e.0 != 0).collect()
}

/// A USB receiver node with simulated devices behind some indices; the other
/// indices answer "unknown device" at once (HID++ 1.0 error).
pub struct FakeReceiver {
    path: String,
    devices: HashMap<u8, Arc<SimulatedMouse>>,
    out: Arc<ReportQueue>,
    closed: AtomicBool,
}

impl FakeReceiver {
    pub fn new(path: &str, devices: Vec<(u8, Arc<SimulatedMouse>)>) -> Self {
        let out = Arc::new(ReportQueue::new());
        for (_, sim) in &devices {
            sim.set_output(out.clone()); // replies and notifications come out of this node
        }
        FakeReceiver {
            path: path.into(),
            devices: devices.into_iter().collect(),
            out,
            closed: AtomicBool::new(false),
        }
    }
}

impl Transport for FakeReceiver {
    fn path(&self) -> &str {
        &self.path
    }

    fn write(&self, data: &[u8]) -> Result<(), TransportClosed> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(TransportClosed(self.path.clone()));
        }
        match self.devices.get(&data[1]) {
            Some(sim) => sim.write(data),
            None => {
                self.out
                    .push(vec![0x10, data[1], 0x8F, data[2], data[3], 0x08, 0x00]);
                Ok(())
            }
        }
    }

    fn read(&self, timeout: Duration) -> Result<Option<Vec<u8>>, TransportClosed> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(TransportClosed(self.path.clone()));
        }
        let r = self.out.pop(timeout);
        if r.is_none() && self.closed.load(Ordering::SeqCst) {
            return Err(TransportClosed(self.path.clone()));
        }
        Ok(r)
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.out.wake();
    }
}

type Opener = Box<dyn Fn() -> Box<dyn Transport> + Send + Sync>;

/// Scans see `nodes`; opening a node calls its opener. `made` lists the opens.
#[derive(Default)]
pub struct FakeHid {
    pub nodes: Mutex<Vec<HidInfo>>,
    pub openers: Mutex<HashMap<String, Opener>>,
    pub made: Mutex<Vec<String>>,
}

impl FakeHid {
    pub fn set_nodes(&self, nodes: Vec<HidInfo>) {
        *self.nodes.lock().unwrap() = nodes;
    }

    pub fn on_open(&self, path: &str, f: impl Fn() -> Box<dyn Transport> + Send + Sync + 'static) {
        self.openers
            .lock()
            .unwrap()
            .insert(path.into(), Box::new(f));
    }
}

impl HidBackend for FakeHid {
    fn scan(&self) -> Vec<HidInfo> {
        self.nodes.lock().unwrap().clone()
    }

    fn open(&self, info: &HidInfo) -> io::Result<Box<dyn Transport>> {
        self.made.lock().unwrap().push(info.path.clone());
        match self.openers.lock().unwrap().get(&info.path) {
            Some(f) => Ok(f()),
            None => Err(io::Error::from(io::ErrorKind::PermissionDenied)),
        }
    }
}

pub fn node(path: &str, bus: u16, product: u16, name: &str) -> HidInfo {
    HidInfo {
        path: path.into(),
        bus,
        vendor: 0x046D,
        product,
        name: name.into(),
        has_short: true,
        has_long: true,
        extra_paths: Vec::new(),
    }
}

/// A transport that forwards to a shared simulated mouse (so tests keep a handle).
pub struct SimHandle(pub Arc<SimulatedMouse>);

impl Transport for SimHandle {
    fn path(&self) -> &str {
        self.0.path()
    }
    fn write(&self, data: &[u8]) -> Result<(), TransportClosed> {
        self.0.write(data)
    }
    fn read(&self, timeout: Duration) -> Result<Option<Vec<u8>>, TransportClosed> {
        self.0.read(timeout)
    }
    fn close(&self) {
        self.0.close()
    }
}
