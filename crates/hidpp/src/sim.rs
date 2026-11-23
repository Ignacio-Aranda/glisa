//! In-process simulation of HID++ 2.0 devices, as a [`Transport`].
//!
//! Used by the tests and by `glisa --simulate`, so the whole stack (service, IPC,
//! window) runs without hardware. Three profiles:
//!
//! - `lift`: a vertical mouse like the LIFT (no SmartShift);
//! - `mx`: an MX-style mouse with SmartShift, a gesture button and a wheel-mode button;
//! - `keys`: a keyboard (battery, host switching and identity only).
//!
//! A [`SimulatedMouse`] is its own transport. [`SimulatedMouse::connect`] gives a fresh
//! [`SimEndpoint`] (the device moved to another simulated computer) and [`SimHub`]
//! moves it between computers on "change host", for Flow's two-computer tests.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::constants::*;
use crate::transport::{ReportQueue, Transport, TransportClosed, lock};

/// `(cid, task, flags, pos, group, gmask, flags2)`
pub type ControlSpec = (u16, u16, u8, u8, u8, u8, u8);

pub const LIFT_CONTROLS: &[ControlSpec] = &[
    (0x0050, 0x0038, 0x01, 0, 1, 0x00, 0x00),
    (0x0051, 0x0039, 0x01, 0, 1, 0x00, 0x00),
    (0x0052, 0x003A, 0x71, 0, 3, 0x07, 0x01),
    (0x0053, 0x003C, 0x71, 0, 3, 0x07, 0x01),
    (0x0056, 0x003E, 0x71, 0, 3, 0x07, 0x01),
    (0x00FD, 0x0061, 0x70, 0, 0, 0x00, 0x01),
];

pub const MX_CONTROLS: &[ControlSpec] = &[
    (0x0050, 0x0038, 0x01, 0, 1, 0x00, 0x00),
    (0x0051, 0x0039, 0x01, 0, 1, 0x00, 0x00),
    (0x0052, 0x003A, 0x71, 0, 3, 0x07, 0x01),
    (0x0053, 0x003C, 0x71, 0, 3, 0x07, 0x01),
    (0x0056, 0x003E, 0x71, 0, 3, 0x07, 0x01),
    (0x00C3, 0x00A9, 0x70, 0, 0, 0x00, 0x01),
    (0x00C4, 0x00AD, 0x70, 0, 0, 0x00, 0x01),
];

pub struct Profile {
    pub id: &'static str,
    pub name: &'static str,
    pub pid: u16,
    /// (min, max, step)
    pub dpi: Option<(u16, u16, u16)>,
    pub controls: &'static [ControlSpec],
    pub smartshift: bool,
    pub battery: u8,
    /// 0x0005 device type (3 = mouse, 0 = keyboard).
    pub kind: u8,
}

pub const PROFILES: &[Profile] = &[
    Profile {
        id: "lift",
        name: "LIFT",
        pid: 0xB031,
        dpi: Some((400, 4000, 100)),
        controls: LIFT_CONTROLS,
        smartshift: false,
        battery: 78,
        kind: 3,
    },
    Profile {
        id: "mx",
        name: "Wireless Mouse MX-Sim",
        pid: 0xB034,
        dpi: Some((200, 8000, 50)),
        controls: MX_CONTROLS,
        smartshift: true,
        battery: 42,
        kind: 3,
    },
    Profile {
        id: "keys",
        name: "Wireless Keyboard K-Sim",
        pid: 0xB35B,
        dpi: None,
        controls: &[],
        smartshift: false,
        battery: 64,
        kind: 0,
    },
];

pub fn profile(id: &str) -> Option<&'static Profile> {
    PROFILES.iter().find(|p| p.id == id)
}

/// The simulated device's state; tests read and change it through
/// [`SimulatedMouse::state`].
pub struct SimState {
    pub features: Vec<u16>,
    pub kind: u8,
    pub dpi_min: u16,
    pub dpi_max: u16,
    pub dpi_step: u16,
    pub dpi: u16,
    pub battery: u8,
    pub wheel_mode: u8,
    /// SmartShift (mode, threshold, torque).
    pub ss: [u8; 3],
    pub host: u8,
    pub host_names: Vec<String>,
    pub controls: Vec<ControlSpec>,
    /// cid -> (reporting flags, remap)
    pub reporting: HashMap<u16, (u8, u16)>,
    pub pressed: Vec<u16>,
}

type HostChange = Arc<dyn Fn(u8) + Send + Sync>;

pub struct SimulatedMouse {
    pub profile: &'static str,
    path: String,
    pub name: &'static str,
    pub product_id: u16,
    pub devidx: u8,
    state: Mutex<SimState>,
    /// Where replies and notifications go (this transport, an endpoint, a receiver).
    out: Mutex<Arc<ReportQueue>>,
    own_out: Arc<ReportQueue>,
    closed: AtomicBool,
    asleep: AtomicBool,
    /// Every report written to the device.
    pub log: Mutex<Vec<Vec<u8>>>,
    on_host_change: Mutex<Option<HostChange>>,
    link: Mutex<Option<Arc<SimEndpoint>>>,
}

fn hostname() -> String {
    let raw = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_default();
    let name: String = raw.trim().chars().take(24).collect();
    if name.is_empty() {
        "linux".into()
    } else {
        name
    }
}

impl SimulatedMouse {
    /// A device of `profile` answering as `devidx` (0xFF direct, 1..=6 behind a
    /// receiver). Panics on an unknown profile (a programming error).
    pub fn new(profile_id: &str, devidx: u8) -> Arc<Self> {
        let p = profile(profile_id).unwrap_or_else(|| panic!("unknown profile {profile_id}"));
        let mut features = vec![
            f::ROOT,
            f::FEATURE_SET,
            f::DEVICE_FW_VERSION,
            f::DEVICE_NAME,
            f::UNIFIED_BATTERY,
            f::CHANGE_HOST,
            f::HOSTS_INFO,
            f::WIRELESS_DEVICE_STATUS,
        ];
        if !p.controls.is_empty() {
            features.push(f::REPROG_CONTROLS_V4);
        }
        if p.dpi.is_some() {
            features.extend([f::HIRES_WHEEL, f::ADJUSTABLE_DPI]);
        }
        if p.smartshift {
            features.push(f::SMART_SHIFT_ENHANCED);
        }
        let (dpi_min, dpi_max, dpi_step) = p.dpi.unwrap_or((0, 0, 0));
        let own_out = Arc::new(ReportQueue::new());
        Arc::new(SimulatedMouse {
            profile: p.id,
            path: format!("sim://{}", p.id),
            name: p.name,
            product_id: p.pid,
            devidx,
            state: Mutex::new(SimState {
                features,
                kind: p.kind,
                dpi_min,
                dpi_max,
                dpi_step,
                dpi: 1000,
                battery: p.battery,
                wheel_mode: 0,
                ss: [2, 30, 50],
                host: 0,
                host_names: vec![hostname(), "MacBook de trabajo".into(), String::new()],
                controls: p.controls.to_vec(),
                reporting: p.controls.iter().map(|c| (c.0, (0, 0))).collect(),
                pressed: Vec::new(),
            }),
            out: Mutex::new(own_out.clone()),
            own_out,
            closed: AtomicBool::new(false),
            asleep: AtomicBool::new(false),
            log: Mutex::new(Vec::new()),
            on_host_change: Mutex::new(None),
            link: Mutex::new(None),
        })
    }

    pub fn state(&self) -> MutexGuard<'_, SimState> {
        lock(&self.state)
    }

    /// Asleep: stays connected but answers nothing.
    pub fn set_asleep(&self, asleep: bool) {
        self.asleep.store(asleep, Ordering::SeqCst);
    }

    pub fn is_asleep(&self) -> bool {
        self.asleep.load(Ordering::SeqCst)
    }

    /// Sends replies and notifications to `queue` (e.g. a fake receiver's node).
    pub fn set_output(&self, queue: Arc<ReportQueue>) {
        *lock(&self.out) = queue;
    }

    pub fn set_on_host_change(&self, f: Option<HostChange>) {
        *lock(&self.on_host_change) = f;
    }

    /// The device goes away (sleep, out of range).
    pub fn disconnect(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.own_out.wake();
        if let Some(link) = lock(&self.link).as_ref() {
            link.close();
        }
    }

    /// A fresh connection of this device to another (simulated) computer. The previous
    /// connection closes, as when Easy-Switch changes host.
    pub fn connect(self: &Arc<Self>) -> Arc<SimEndpoint> {
        self.closed.store(true, Ordering::SeqCst);
        self.own_out.wake();
        let ep = Arc::new(SimEndpoint {
            sim: self.clone(),
            path: self.path.clone(),
            out: Arc::new(ReportQueue::new()),
            closed: AtomicBool::new(false),
        });
        let old = lock(&self.link).replace(ep.clone());
        if let Some(old) = old {
            old.close();
        }
        *lock(&self.out) = ep.out.clone();
        ep
    }

    fn emit(&self, report: Vec<u8>) {
        lock(&self.out).clone().push(report);
    }

    fn handle_write(&self, data: &[u8]) {
        lock(&self.log).push(data.to_vec());
        if self.is_asleep() {
            return;
        }
        let mut st = lock(&self.state);
        if let Some(reply) = self.handle(&mut st, data) {
            drop(st);
            self.emit(reply);
        }
    }

    // ---------------------------------------------------------- physical actions

    pub fn press(&self, cid: u16) {
        let mut st = lock(&self.state);
        let flags = st.reporting.get(&cid).map_or(0, |r| r.0);
        if flags & REPORT_DIVERT != 0 {
            st.pressed.push(cid);
            let r = self.buttons_report(&st);
            drop(st);
            self.emit(r);
        }
    }

    pub fn release(&self, cid: u16) {
        let mut st = lock(&self.state);
        if let Some(i) = st.pressed.iter().position(|&c| c == cid) {
            st.pressed.remove(i);
            let r = self.buttons_report(&st);
            drop(st);
            self.emit(r);
        }
    }

    pub fn move_by(&self, dx: i16, dy: i16) {
        let st = lock(&self.state);
        let raw = st
            .pressed
            .iter()
            .any(|c| st.reporting.get(c).is_some_and(|r| r.0 & REPORT_RAWXY != 0));
        if raw {
            let mut p = dx.to_be_bytes().to_vec();
            p.extend_from_slice(&dy.to_be_bytes());
            let r = self.notification(&st, f::REPROG_CONTROLS_V4, 1, &p);
            drop(st);
            self.emit(r);
        }
    }

    pub fn set_battery(&self, percent: u8, charging: bool) {
        let mut st = lock(&self.state);
        st.battery = percent;
        let p = battery_payload(percent, charging);
        let r = self.notification(&st, f::UNIFIED_BATTERY, 0, &p);
        drop(st);
        self.emit(r);
    }

    /// Sends a notification of `feature` (e.g. 0x1D4B function 0: "I reconnected").
    pub fn notify(&self, feature: u16, function: u8, payload: &[u8]) {
        let st = lock(&self.state);
        let r = self.notification(&st, feature, function, payload);
        drop(st);
        self.emit(r);
    }

    fn buttons_report(&self, st: &SimState) -> Vec<u8> {
        let mut cids = st.pressed.clone();
        cids.resize(4, 0);
        let p: Vec<u8> = cids.iter().take(4).flat_map(|c| c.to_be_bytes()).collect();
        self.notification(st, f::REPROG_CONTROLS_V4, 0, &p)
    }

    fn notification(&self, st: &SimState, feature: u16, function: u8, payload: &[u8]) -> Vec<u8> {
        let idx = st.features.iter().position(|&x| x == feature).unwrap_or(0) as u8;
        let mut r = vec![REPORT_LONG, self.devidx, idx, function << 4];
        r.extend(pad16(payload));
        r
    }

    // ------------------------------------------------------------------ protocol

    fn handle(&self, st: &mut SimState, req: &[u8]) -> Option<Vec<u8>> {
        if req.len() < 4 || req[1] != self.devidx {
            return None;
        }
        let reply = |payload: &[u8]| {
            let mut r = vec![REPORT_LONG, req[1], req[2], req[3]];
            r.extend(pad16(payload));
            Some(r)
        };
        let error = |code: u8| {
            let mut r = vec![REPORT_LONG, req[1], 0xFF, req[2], req[3], code];
            r.resize(20, 0);
            Some(r)
        };
        let (idx, func) = (req[2] as usize, req[3] >> 4);
        let p = |i: usize| req.get(4 + i).copied().unwrap_or(0);
        if idx >= st.features.len() {
            return error(6);
        }
        match st.features[idx] {
            f::ROOT => match func {
                0 => {
                    let fid = u16::from_be_bytes([p(0), p(1)]);
                    let i = st.features.iter().position(|&x| x == fid).unwrap_or(0);
                    return reply(&[i as u8, 0, 0]);
                }
                1 => return reply(&[4, 5, p(2)]),
                _ => {}
            },
            f::FEATURE_SET => match func {
                0 => return reply(&[(st.features.len() - 1) as u8]),
                1 => {
                    let fid = st.features.get(p(0) as usize).copied().unwrap_or(0);
                    let [hi, lo] = fid.to_be_bytes();
                    return reply(&[hi, lo, 0, 0]);
                }
                _ => {}
            },
            f::DEVICE_FW_VERSION => match func {
                0 => return reply(&[1]),
                1 => return reply(b"\x00RBM\x16\x01\x00\x17"),
                _ => {}
            },
            f::DEVICE_NAME => {
                let name = self.name.as_bytes();
                match func {
                    0 => return reply(&[name.len() as u8]),
                    1 => {
                        let start = (p(0) as usize).min(name.len());
                        let end = (start + 16).min(name.len());
                        return reply(&name[start..end]);
                    }
                    2 => return reply(&[st.kind]),
                    _ => {}
                }
            }
            f::UNIFIED_BATTERY => match func {
                0 => return reply(&[0x0F, 0x02]),
                1 => return reply(&battery_payload(st.battery, false)),
                _ => {}
            },
            f::CHANGE_HOST => match func {
                0 => return reply(&[3, st.host]),
                1 => {
                    let moved = p(0) != st.host;
                    st.host = p(0);
                    if moved {
                        if let Some(cb) = lock(&self.on_host_change).clone() {
                            let ch = p(0);
                            std::thread::spawn(move || cb(ch));
                        }
                    }
                    return None; // real devices switch away without answering
                }
                _ => {}
            },
            f::HOSTS_INFO => {
                let host_name = |i: u8| {
                    st.host_names
                        .get(i as usize)
                        .cloned()
                        .unwrap_or_default()
                        .into_bytes()
                };
                match func {
                    0 => return reply(&[0x01, 0, st.host_names.len() as u8, st.host]),
                    1 => {
                        let name = host_name(p(0));
                        return reply(&[
                            p(0),
                            u8::from(!name.is_empty()),
                            1,
                            0,
                            name.len() as u8,
                            24,
                        ]);
                    }
                    3 => {
                        let name = host_name(p(0));
                        let start = (p(1) as usize).min(name.len());
                        let end = (start + 14).min(name.len());
                        let mut out = vec![p(0), p(1)];
                        out.extend_from_slice(&name[start..end]);
                        return reply(&out);
                    }
                    _ => {}
                }
            }
            f::ADJUSTABLE_DPI => match func {
                0 => return reply(&[1]),
                1 => {
                    let mut out = vec![0];
                    for v in [st.dpi_min, 0xE000 | st.dpi_step, st.dpi_max] {
                        out.extend_from_slice(&v.to_be_bytes());
                    }
                    return reply(&out);
                }
                2 => {
                    let mut out = vec![0];
                    out.extend_from_slice(&st.dpi.to_be_bytes());
                    out.extend_from_slice(&1000u16.to_be_bytes());
                    return reply(&out);
                }
                3 => {
                    let dpi = u16::from_be_bytes([p(1), p(2)]);
                    if !(st.dpi_min..=st.dpi_max).contains(&dpi) {
                        return error(3);
                    }
                    st.dpi = dpi;
                    let mut out = vec![0];
                    out.extend_from_slice(&dpi.to_be_bytes());
                    return reply(&out);
                }
                _ => {}
            },
            f::HIRES_WHEEL => match func {
                0 => return reply(&[8, 0x08]),
                1 => return reply(&[st.wheel_mode]),
                2 => {
                    st.wheel_mode = p(0) & 0x07;
                    return reply(&[st.wheel_mode]);
                }
                _ => {}
            },
            f::SMART_SHIFT_ENHANCED => match func {
                0 => return reply(&[0x01, 0x64]),
                1 => return reply(&st.ss),
                2 => {
                    let or = |v: u8, old: u8| if v == 0 { old } else { v };
                    st.ss = [or(p(0), st.ss[0]), or(p(1), st.ss[1]), or(p(2), st.ss[2])];
                    return reply(&st.ss);
                }
                _ => {}
            },
            f::REPROG_CONTROLS_V4 => {
                match func {
                    0 => return reply(&[st.controls.len() as u8]),
                    1 => {
                        let Some(c) = st.controls.get(p(0) as usize).copied() else {
                            return error(2);
                        };
                        let mut out = c.0.to_be_bytes().to_vec();
                        out.extend_from_slice(&c.1.to_be_bytes());
                        out.extend_from_slice(&[c.2, c.3, c.4, c.5, c.6]);
                        return reply(&out);
                    }
                    _ => {}
                }
                let cid = u16::from_be_bytes([p(0), p(1)]);
                let Some(&(flags, remap)) = st.reporting.get(&cid) else {
                    return error(2);
                };
                match func {
                    2 => {
                        let mut out = cid.to_be_bytes().to_vec();
                        out.push(flags);
                        out.extend_from_slice(&remap.to_be_bytes());
                        out.push(0);
                        return reply(&out);
                    }
                    3 => {
                        let (mut flags, mut remap) = (flags, remap);
                        let fl = p(2);
                        if fl & REPORT_DVALID != 0 {
                            flags = (flags & !REPORT_DIVERT) | (fl & REPORT_DIVERT);
                        }
                        if fl & REPORT_RVALID != 0 {
                            flags = (flags & !REPORT_RAWXY) | (fl & REPORT_RAWXY);
                        }
                        let new_remap = u16::from_be_bytes([p(3), p(4)]);
                        if new_remap != 0 {
                            remap = if new_remap == cid { 0 } else { new_remap };
                        }
                        st.reporting.insert(cid, (flags, remap));
                        return reply(&[p(0), p(1), p(2), p(3), p(4)]);
                    }
                    _ => {}
                }
            }
            f::WIRELESS_DEVICE_STATUS => return reply(&[]),
            _ => {}
        }
        error(7)
    }
}

fn pad16(payload: &[u8]) -> Vec<u8> {
    let mut v: Vec<u8> = payload.iter().copied().take(16).collect();
    v.resize(16, 0);
    v
}

fn battery_payload(b: u8, charging: bool) -> [u8; 4] {
    let lvl = match b {
        81.. => 8,
        31..=80 => 4,
        11..=30 => 2,
        _ => 1,
    };
    [b, lvl, u8::from(charging), 0]
}

impl Transport for SimulatedMouse {
    fn path(&self) -> &str {
        &self.path
    }

    fn write(&self, data: &[u8]) -> Result<(), TransportClosed> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(TransportClosed(self.path.clone()));
        }
        self.handle_write(data);
        Ok(())
    }

    fn read(&self, timeout: Duration) -> Result<Option<Vec<u8>>, TransportClosed> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(TransportClosed(self.path.clone()));
        }
        let got = self.own_out.pop(timeout);
        if got.is_none() && self.closed.load(Ordering::SeqCst) {
            return Err(TransportClosed(self.path.clone()));
        }
        Ok(got)
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.own_out.wake();
    }
}

/// One connection between a [`SimulatedMouse`] and one simulated computer (made by
/// [`SimulatedMouse::connect`]).
pub struct SimEndpoint {
    sim: Arc<SimulatedMouse>,
    path: String,
    out: Arc<ReportQueue>,
    closed: AtomicBool,
}

impl SimEndpoint {
    pub fn sim(&self) -> &Arc<SimulatedMouse> {
        &self.sim
    }
}

impl Transport for SimEndpoint {
    fn path(&self) -> &str {
        &self.path
    }

    fn write(&self, data: &[u8]) -> Result<(), TransportClosed> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(TransportClosed(self.path.clone()));
        }
        self.sim.handle_write(data);
        Ok(())
    }

    fn read(&self, timeout: Duration) -> Result<Option<Vec<u8>>, TransportClosed> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(TransportClosed(self.path.clone()));
        }
        let got = self.out.pop(timeout);
        if got.is_none() && self.closed.load(Ordering::SeqCst) {
            return Err(TransportClosed(self.path.clone()));
        }
        Ok(got)
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.out.wake();
    }
}

/// Attaches a connection to one simulated computer.
pub type Attach = Arc<dyn Fn(Arc<SimEndpoint>) + Send + Sync>;

/// One physical device paired with several simulated computers on different
/// Easy-Switch channels: on "change host" it closes its connection to the current
/// computer and, after `delay` (the radio reconnecting), attaches to the computer
/// registered for that channel.
pub struct SimHub {
    pub sim: Arc<SimulatedMouse>,
    pub delay: Duration,
    computers: Mutex<HashMap<u8, Attach>>,
    pub moves: Mutex<Vec<u8>>,
}

impl SimHub {
    pub fn new(sim: Arc<SimulatedMouse>, delay: Duration) -> Arc<Self> {
        let hub = Arc::new(SimHub {
            sim: sim.clone(),
            delay,
            computers: Mutex::new(HashMap::new()),
            moves: Mutex::new(Vec::new()),
        });
        let weak = Arc::downgrade(&hub);
        sim.set_on_host_change(Some(Arc::new(move |channel| {
            if let Some(hub) = weak.upgrade() {
                hub.moved(channel);
            }
        })));
        hub
    }

    pub fn add(&self, channel: u8, attach: Attach) {
        lock(&self.computers).insert(channel, attach);
    }

    fn moved(&self, channel: u8) {
        lock(&self.moves).push(channel);
        let link = self.sim.connect(); // the old computer loses it now
        std::thread::sleep(self.delay);
        let attach = lock(&self.computers).get(&channel).cloned();
        if let Some(attach) = attach {
            attach(link);
        }
    }
}
