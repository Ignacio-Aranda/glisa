//! One device Glisa manages: identity, cached state, settings pushed to it, events from it
//! and its API snapshot.
//!
//! Locking: `ops` serialises HID++ sequences (Python's per-device RLock); `st` guards
//! the cached state and is never held across a HID++ request, because the channel's
//! reader thread takes it to record battery notifications.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use glisa_common::{DISPLAY_NAME_MAX, DeviceKind, FlowDevice, clean_display_name};
use glisa_hidpp::constants::feature_label;
use glisa_hidpp::features::{self as fe, Battery, Control, DpiInfo, HostEntry};
use glisa_hidpp::{Device, Error as HidppError, ListenerId, f};
use glisa_i18n::{n_, tr};
use serde_json::{Map, Value, json};

use crate::actions::{self, truthy};
use crate::models::{self, Model};
use crate::service::Service;

/// Name of a device that does not report one. Kept in English internally (it is part
/// of the settings key and Flow compares names between computers); shown translated.
pub const UNNAMED_DEVICE: &str = n_!("Logitech device");

const DEFAULT_DPI_PRESETS: [i64; 3] = [800, 1600, 2400];

/// Why a device request failed.
#[derive(Debug)]
pub enum DevError {
    /// The device refused or did not answer.
    Hidpp(HidppError),
    /// A message for the user, already in their language.
    Invalid(String),
    /// The setting needs a feature the device lacks.
    Unsupported,
    /// The settings file could not be written.
    Save(io::Error),
}

impl From<HidppError> for DevError {
    fn from(e: HidppError) -> Self {
        DevError::Hidpp(e)
    }
}

impl From<io::Error> for DevError {
    fn from(e: io::Error) -> Self {
        DevError::Save(e)
    }
}

pub(crate) struct DevState {
    pub firmware: Vec<String>,
    pub battery: Option<Battery>,
    pub controls: Vec<Control>,
    pub dpi: Option<DpiInfo>,
    pub wheel: Option<fe::WheelInfo>,
    pub smartshift: Option<fe::SmartShiftInfo>,
    pub hosts: (u8, u8),
    pub host_entries: Vec<HostEntry>,
    pub last_battery: Option<Instant>,
    pub last_alive: Option<Instant>,
    /// Found on the device before we touched it: what "reset" goes back to.
    pub initial_wheel_hires: Option<bool>,
    pub initial_ss: Option<(bool, u8)>,
    pub misses: u32,
    pub pressed: Vec<u16>,
    pub bus: Option<&'static str>,
    pub appeared_ms: u64,
}

pub struct ManagedDevice {
    service: Weak<Service>,
    pub dev: Arc<Device>,
    /// hidraw path (+ `#index` behind a receiver) or `sim://…`.
    pub source: String,
    /// The name the device reports (settings key; Flow matches devices by it).
    pub name: String,
    /// "mouse", "keyboard", "trackball"… (0x0005).
    pub kind: String,
    /// Settings key: `"B031:LIFT"`.
    pub key: String,
    /// API id: `"b031-lift"`.
    pub id: String,
    /// Feature ids the device reports, sorted.
    pub features: Vec<u16>,
    model: Option<&'static Model>,
    online: AtomicBool,
    ops: Mutex<()>,
    st: Mutex<DevState>,
    listener: Mutex<Option<u64>>,
    pub(crate) closed_cb: Mutex<Option<ListenerId>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Python's `int()` of a JSON value (numbers, booleans, numeric text).
pub(crate) fn int_of(v: &Value) -> Option<i64> {
    match v {
        Value::Bool(b) => Some(i64::from(*b)),
        Value::Number(n) => n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.is_finite())
                .map(|f| f.trunc() as i64)
        }),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn button_key(cid: u16) -> String {
    format!("{cid:#06x}")
}

/// Keeps the first error of a best-effort read, for the debug log.
fn attempt<T>(name: &str, what: &str, r: Result<T, HidppError>) -> Option<T> {
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            log::debug!("{name}: {what} failed: {e}");
            None
        }
    }
}

impl ManagedDevice {
    /// Reads the device's identity and state (several HID++ requests).
    pub(crate) fn new(
        service: &Arc<Service>,
        dev: Arc<Device>,
        source: &str,
    ) -> Result<Arc<Self>, HidppError> {
        let has_name = dev.has(f::DEVICE_NAME);
        let name = if has_name {
            fe::device_name(&dev)?
        } else {
            UNNAMED_DEVICE.to_string()
        };
        let kind = if has_name {
            fe::device_type(&dev).to_string()
        } else {
            "unknown".to_string()
        };
        let key = format!("{:04X}:{name}", dev.product_id);
        let id = key.replace(' ', "_").replace(':', "-").to_lowercase();
        let mut features: Vec<u16> = if dev.has(f::FEATURE_SET) {
            dev.enumerate_features()?
                .into_iter()
                .map(|(fid, _)| fid)
                .collect()
        } else {
            Vec::new()
        };
        features.sort_unstable();
        features.dedup();
        let mut pids = Vec::new();
        if features.contains(&f::DEVICE_FW_VERSION) {
            if let Ok(ids) = fe::device_ids(&dev) {
                pids = ids.all();
            }
        }
        if !source.contains('#') {
            pids.push(dev.product_id); // direct: Bluetooth or USB product id
        }
        let model = models::lookup(&pids, &name);
        let md = Arc::new(ManagedDevice {
            service: Arc::downgrade(service),
            dev: dev.clone(),
            source: source.to_string(),
            name,
            kind,
            key,
            id,
            features,
            model,
            online: AtomicBool::new(true),
            ops: Mutex::new(()),
            st: Mutex::new(DevState {
                firmware: Vec::new(),
                battery: None,
                controls: Vec::new(),
                dpi: None,
                wheel: None,
                smartshift: None,
                hosts: (0, 0),
                host_entries: Vec::new(),
                last_battery: None,
                last_alive: None,
                initial_wheel_hires: None,
                initial_ss: None,
                misses: 0,
                pressed: Vec::new(),
                bus: None,
                appeared_ms: glisa_common::monotonic_ms(),
            }),
            listener: Mutex::new(None),
            closed_cb: Mutex::new(None),
        });
        md.refresh();
        {
            let mut st = lock(&md.st);
            st.initial_wheel_hires = st.wheel.as_ref().map(|w| w.hires);
            st.initial_ss = st.smartshift.as_ref().map(|s| (s.ratchet, s.threshold));
        }
        let weak = Arc::downgrade(&md);
        let id = dev.add_listener(Arc::new(move |feature, function, data| {
            if let Some(md) = weak.upgrade() {
                md.on_event(feature, function, data);
            }
        }));
        *lock(&md.listener) = Some(id);
        Ok(md)
    }

    pub(crate) fn state(&self) -> MutexGuard<'_, DevState> {
        lock(&self.st)
    }

    pub fn has(&self, feature: u16) -> bool {
        self.features.binary_search(&feature).is_ok()
    }

    pub fn online(&self) -> bool {
        self.online.load(Ordering::SeqCst)
    }

    pub(crate) fn set_online(&self, online: bool) {
        self.online.store(online, Ordering::SeqCst);
    }

    pub fn model(&self) -> Option<&'static Model> {
        self.model
    }

    /// The name the device reports, in this thread's language when it has none.
    pub fn model_name(&self) -> String {
        if self.name == UNNAMED_DEVICE {
            tr!(UNNAMED_DEVICE)
        } else {
            self.name.clone()
        }
    }

    /// The name the user gave it ("" when none).
    pub fn custom_name(&self) -> String {
        self.service
            .upgrade()
            .map(|s| s.config.display_name(&self.key))
            .unwrap_or_default()
    }

    /// The name to show: the user's, else the model's.
    pub fn display_name(&self) -> String {
        let custom = self.custom_name();
        if custom.is_empty() {
            self.model_name()
        } else {
            custom
        }
    }

    pub fn hosts(&self) -> (u8, u8) {
        lock(&self.st).hosts
    }

    pub fn dpi(&self) -> Option<DpiInfo> {
        lock(&self.st).dpi.clone()
    }

    pub fn appeared_ms(&self) -> u64 {
        lock(&self.st).appeared_ms
    }

    pub(crate) fn set_appeared_ms(&self, ms: u64) {
        lock(&self.st).appeared_ms = ms;
    }

    pub(crate) fn bus(&self) -> Option<&'static str> {
        lock(&self.st).bus
    }

    pub(crate) fn set_bus(&self, bus: Option<&'static str>) {
        lock(&self.st).bus = bus;
    }

    // ------------------------------------------------------------------- reading

    /// Re-reads everything (firmware, battery, controls, DPI, wheel, hosts).
    pub fn refresh(&self) {
        let _ops = lock(&self.ops);
        self.refresh_locked();
    }

    fn refresh_locked(&self) {
        let d = &self.dev;
        let n = &self.name;
        if self.has(f::DEVICE_FW_VERSION) {
            let fw = attempt(n, "firmware", fe::firmware(d)).unwrap_or_default();
            lock(&self.st).firmware = fw;
        }
        self.refresh_battery();
        if self.has(f::REPROG_CONTROLS_V4) {
            let c = attempt(n, "controls", fe::controls(d)).unwrap_or_default();
            lock(&self.st).controls = c;
        }
        if self.has(f::ADJUSTABLE_DPI) {
            let v = attempt(n, "dpi", fe::dpi_info(d, 0));
            lock(&self.st).dpi = v;
        }
        if self.has(f::HIRES_WHEEL) {
            let v = attempt(n, "wheel", fe::wheel_info(d));
            lock(&self.st).wheel = v;
        }
        if self.has(f::SMART_SHIFT) || self.has(f::SMART_SHIFT_ENHANCED) {
            let v = attempt(n, "smartshift", fe::smartshift_info(d)).flatten();
            lock(&self.st).smartshift = v;
        }
        if self.has(f::CHANGE_HOST) {
            let hosts = attempt(n, "hosts", fe::host_info(d)).unwrap_or((0, 0));
            let entries = fe::host_entries(d, hosts.0);
            let mut st = lock(&self.st);
            st.hosts = hosts;
            st.host_entries = entries;
        }
    }

    /// Re-reads the battery; true when it changed.
    pub fn refresh_battery(&self) -> bool {
        let b = attempt(&self.name, "battery", fe::battery(&self.dev)).flatten();
        let mut st = lock(&self.st);
        st.last_battery = Some(Instant::now());
        match b {
            Some(b) if st.battery.as_ref() != Some(&b) => {
                st.battery = Some(b);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn battery_age(&self) -> Option<Duration> {
        lock(&self.st).last_battery.map(|t| t.elapsed())
    }

    // ------------------------------------------------------------------ applying

    /// The device's stored settings.
    pub fn cfg(&self) -> Map<String, Value> {
        self.service
            .upgrade()
            .map(|s| s.config.device_copy(&self.key))
            .unwrap_or_else(|| {
                json!({"buttons": {}})
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
            })
    }

    /// Pushes every stored setting to the device (idempotent).
    pub fn apply(&self) {
        let cfg = self.cfg();
        let _ops = lock(&self.ops);
        self.apply_locked(&cfg);
    }

    fn apply_locked(&self, cfg: &Map<String, Value>) {
        let d = &self.dev;
        let (dpi, wheel, ss) = {
            let st = lock(&self.st);
            (st.dpi.is_some(), st.wheel.clone(), st.smartshift.clone())
        };
        if dpi && truthy(cfg.get("dpi")) {
            if let Some(v) = cfg.get("dpi").and_then(int_of) {
                let v = self.clamp_dpi(v);
                if attempt(&self.name, "set dpi", fe::set_dpi(d, v as u16, 0)).is_some() {
                    if let Some(info) = lock(&self.st).dpi.as_mut() {
                        info.current = v as u16;
                    }
                }
            }
        }
        if let Some(w) = wheel {
            if cfg.contains_key("wheel_hires") || cfg.contains_key("wheel_inverted") {
                let hires = cfg.get("wheel_hires").map_or(w.hires, |v| truthy(Some(v)));
                let inv = cfg
                    .get("wheel_inverted")
                    .map_or(w.inverted, |v| truthy(Some(v)));
                attempt(
                    &self.name,
                    "set wheel",
                    fe::set_wheel_mode(d, hires, inv, false),
                );
                if let Some(w) = lock(&self.st).wheel.as_mut() {
                    w.hires = hires;
                    w.inverted = inv;
                }
            }
        }
        if let Some(s) = ss {
            if cfg.contains_key("ss_ratchet") || cfg.contains_key("ss_threshold") {
                let ratchet = cfg.get("ss_ratchet").map_or(s.ratchet, |v| truthy(Some(v)));
                let thr = cfg
                    .get("ss_threshold")
                    .and_then(int_of)
                    .unwrap_or(i64::from(s.threshold));
                attempt(
                    &self.name,
                    "set smartshift",
                    fe::set_smartshift(d, ratchet, thr, s.torque),
                );
                if let Some(s) = lock(&self.st).smartshift.as_mut() {
                    s.ratchet = ratchet;
                    s.threshold = thr.clamp(1, 255) as u8;
                }
            }
        }
        self.apply_buttons_locked(cfg, false);
    }

    /// Diverts / remaps each control as its stored action needs.
    pub(crate) fn apply_buttons_locked(&self, cfg: &Map<String, Value>, force: bool) {
        let empty = Map::new();
        let buttons = cfg
            .get("buttons")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let controls = lock(&self.st).controls.clone();
        for c in controls {
            if !c.divertable() && !c.reprogrammable() {
                continue;
            }
            let action = buttons.get(&button_key(c.cid));
            let divert = c.divertable() && actions::needs_divert(action, &c.group_cids);
            let raw = divert
                && c.supports_raw_xy()
                && action.and_then(actions::action_type) == Some("gesture");
            let remap = if c.reprogrammable() {
                actions::native_remap(action, &c.group_cids)
            } else {
                0
            };
            if !force && (divert, raw, remap) == (c.diverted, c.raw_xy, c.remap) {
                continue;
            }
            if let Err(e) = fe::set_control_reporting(&self.dev, c.cid, divert, raw, remap) {
                log::warn!("{}: could not configure {}: {e}", self.name, c.name);
                continue;
            }
            let mut st = lock(&self.st);
            if let Some(sc) = st.controls.iter_mut().find(|x| x.cid == c.cid) {
                sc.diverted = divert;
                sc.raw_xy = raw;
                sc.remap = remap;
            }
        }
    }

    fn clamp_dpi(&self, value: i64) -> i64 {
        let st = lock(&self.st);
        match st.dpi.as_ref().filter(|d| !d.choices.is_empty()) {
            None => value,
            Some(d) => d
                .choices
                .iter()
                .map(|&c| i64::from(c))
                .min_by_key(|c| (c - value).abs())
                .unwrap_or(value),
        }
    }

    // -------------------------------------------------------------------- events

    fn on_event(self: &Arc<Self>, feature: u16, function: u8, data: &[u8]) {
        let Some(service) = self.service.upgrade() else {
            return;
        };
        match (feature, function) {
            (f::REPROG_CONTROLS_V4, 0) => {
                service.on_buttons(self, fe::parse_diverted_buttons(&data[..8.min(data.len())]))
            }
            (f::REPROG_CONTROLS_V4, 1) => {
                let (dx, dy) = fe::parse_raw_xy(data);
                service.on_moved(i32::from(dx), i32::from(dy));
            }
            (f::UNIFIED_BATTERY | f::BATTERY_STATUS, _) => {
                if let Some(b) = fe::parse_battery_event(feature, data) {
                    lock(&self.st).battery = Some(b);
                    service.emit_device(&self.id);
                }
            }
            (f::WIRELESS_DEVICE_STATUS, 0) => {
                // Woke up / reconnected: it lost our volatile settings.
                log::info!("{} reconnected, applying settings again", self.name);
                let md = self.clone();
                std::thread::spawn(move || md.reapply());
                service.device_online(self);
            }
            _ => {}
        }
    }

    fn reapply(&self) {
        std::thread::sleep(Duration::from_millis(300));
        let cfg = self.cfg();
        let _ops = lock(&self.ops);
        self.apply_locked(&cfg);
        self.apply_buttons_locked(&cfg, true);
    }

    /// Next DPI preset (the "Change DPI" action).
    pub fn cycle_dpi(&self) {
        let Some(cur) = lock(&self.st).dpi.as_ref().map(|d| i64::from(d.current)) else {
            return;
        };
        let cfg = self.cfg();
        let mut presets: Vec<i64> = cfg
            .get("dpi_presets")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty())
            .map(|a| a.iter().filter_map(int_of).collect())
            .unwrap_or_else(|| DEFAULT_DPI_PRESETS.to_vec());
        if presets.is_empty() {
            presets = DEFAULT_DPI_PRESETS.to_vec();
        }
        let presets: Vec<i64> = presets.into_iter().map(|p| self.clamp_dpi(p)).collect();
        let next = presets
            .iter()
            .copied()
            .find(|&p| p > cur)
            .unwrap_or(presets[0]);
        if let Err(e) = self.set("dpi", &json!(next)) {
            log::warn!("{}: could not change the DPI: {e:?}", self.name);
        }
        if let Some(s) = self.service.upgrade() {
            s.emit_device(&self.id);
        }
    }

    // ----------------------------------------------------------------------- API

    /// Changes one setting on the device and stores it; returns the value stored.
    pub fn set(&self, key: &str, value: &Value) -> Result<Value, DevError> {
        let invalid = || DevError::Invalid(tr!("Invalid parameters"));
        let stored = match key {
            "dpi" => {
                let v = int_of(value).ok_or_else(invalid)?;
                if lock(&self.st).dpi.is_none() {
                    return Err(DevError::Unsupported);
                }
                let v = self.clamp_dpi(v);
                let v16 = u16::try_from(v).map_err(|_| invalid())?;
                let _ops = lock(&self.ops);
                fe::set_dpi(&self.dev, v16, 0)?;
                if let Some(d) = lock(&self.st).dpi.as_mut() {
                    d.current = v16;
                }
                json!(v)
            }
            "dpi_presets" => {
                let items = value.as_array().ok_or_else(invalid)?;
                let mut v: Vec<i64> = Vec::new();
                for item in items.iter().take(64) {
                    v.push(self.clamp_dpi(int_of(item).ok_or_else(invalid)?));
                }
                v.sort_unstable();
                v.dedup();
                v.truncate(5);
                json!(v)
            }
            "wheel_hires" | "wheel_inverted" => {
                let on = truthy(Some(value));
                let Some(w) = lock(&self.st).wheel.clone() else {
                    return Err(DevError::Unsupported);
                };
                let hires = if key == "wheel_hires" { on } else { w.hires };
                let inv = if key == "wheel_inverted" {
                    on
                } else {
                    w.inverted
                };
                let _ops = lock(&self.ops);
                fe::set_wheel_mode(&self.dev, hires, inv, false)?;
                if let Some(w) = lock(&self.st).wheel.as_mut() {
                    w.hires = hires;
                    w.inverted = inv;
                }
                json!(on)
            }
            "ss_ratchet" | "ss_threshold" => {
                let Some(ss) = lock(&self.st).smartshift.clone() else {
                    return Err(DevError::Unsupported);
                };
                let (ratchet, thr, out) = if key == "ss_ratchet" {
                    let r = truthy(Some(value));
                    (r, i64::from(ss.threshold), json!(r))
                } else {
                    let t = int_of(value).ok_or_else(invalid)?;
                    (ss.ratchet, t, json!(t))
                };
                let _ops = lock(&self.ops);
                fe::set_smartshift(&self.dev, ratchet, thr, ss.torque)?;
                if let Some(s) = lock(&self.st).smartshift.as_mut() {
                    s.ratchet = ratchet;
                    s.threshold = thr.clamp(1, 255) as u8;
                }
                out
            }
            // Stored only; the desktop side applies them.
            "pointer_speed" | "natural_scroll" => value.clone(),
            _ => return Err(DevError::Invalid(tr!("Unknown setting: {key}", key = key))),
        };
        let mut m = Map::new();
        m.insert(key.to_string(), stored.clone());
        if let Some(s) = self.service.upgrade() {
            s.config.update_device(&self.key, m)?;
        }
        Ok(stored)
    }

    /// What control `cid` does (`None`: the hardware default).
    pub fn set_button(&self, cid: u16, action: Option<&Value>) -> Result<(), DevError> {
        let action = action.filter(|a| !a.is_null());
        if let Some(a) = action.filter(|a| truthy(Some(a))) {
            actions::validate(a).map_err(DevError::Invalid)?;
        } else if action.is_some_and(|a| !a.is_object()) {
            return Err(DevError::Invalid(tr!("Invalid parameters")));
        }
        if !lock(&self.st).controls.iter().any(|c| c.cid == cid) {
            return Err(DevError::Invalid(tr!("Unknown control")));
        }
        let service = self.service.upgrade().ok_or(DevError::Unsupported)?;
        service.config.set_button(&self.key, cid, action)?;
        let cfg = self.cfg();
        let _ops = lock(&self.ops);
        self.apply_buttons_locked(&cfg, false);
        Ok(())
    }

    /// Switches to Easy-Switch channel `host` (0-based).
    pub fn set_host(&self, host: i64) -> Result<(), DevError> {
        let count = i64::from(self.hosts().0);
        if !(0..count).contains(&host) {
            return Err(DevError::Invalid(tr!("Invalid channel")));
        }
        fe::set_host(&self.dev, host as u8);
        Ok(())
    }

    /// Gives the device the name the user chose (`""` or null: the model's again).
    /// Returns the name now shown.
    pub fn rename(&self, name: &Value) -> Result<String, DevError> {
        let raw = match name {
            Value::Null => "",
            Value::String(s) => s.as_str(),
            _ => return Err(DevError::Invalid(tr!("The name has to be text"))),
        };
        let mut clean = clean_display_name(raw);
        if clean.chars().count() > DISPLAY_NAME_MAX {
            return Err(DevError::Invalid(tr!(
                "The name can't be longer than {n} characters",
                n = DISPLAY_NAME_MAX
            )));
        }
        if clean == self.name || clean == self.model_name() {
            clean.clear(); // its own name: nothing to remember
        }
        if let Some(s) = self.service.upgrade() {
            s.config
                .set_display_name(&self.key, Some(clean.as_str()).filter(|c| !c.is_empty()))?;
        }
        if clean.is_empty() {
            log::info!("{}: display name reset", self.name);
        } else {
            log::info!("{}: display name {clean:?}", self.name);
        }
        Ok(self.display_name())
    }

    /// Stops reacting to the device (it was removed from Glisa).
    pub fn detach(&self) {
        if let Some(id) = lock(&self.listener).take() {
            self.dev.remove_listener(id);
        }
    }

    /// Buttons, DPI, wheel and SmartShift back to how the device was found (stored
    /// settings are cleared by the caller).
    pub fn factory_reset(&self) {
        let _ops = lock(&self.ops);
        self.apply_buttons_locked(&Map::new(), true);
        let (dpi, wheel, ss, init_hires, init_ss) = {
            let st = lock(&self.st);
            (
                st.dpi.clone(),
                st.wheel.clone(),
                st.smartshift.clone(),
                st.initial_wheel_hires,
                st.initial_ss,
            )
        };
        let n = &self.name;
        if let Some(d) = dpi {
            attempt(n, "reset dpi", fe::set_dpi(&self.dev, d.default, 0));
        }
        if let Some(w) = wheel {
            let hires = init_hires.unwrap_or(w.hires);
            attempt(
                n,
                "reset wheel",
                fe::set_wheel_mode(&self.dev, hires, false, false),
            );
        }
        if let (Some(s), Some((ratchet, thr))) = (ss, init_ss) {
            attempt(
                n,
                "reset smartshift",
                fe::set_smartshift(&self.dev, ratchet, i64::from(thr), s.torque),
            );
        }
        self.refresh_locked();
    }

    /// Devices behind a receiver keep their node while asleep or on another computer,
    /// so liveness has to be probed. Returns true when the online state changed.
    pub fn check_alive(self: &Arc<Self>) -> bool {
        let service = self.service.upgrade();
        if self.dev.ping(Duration::from_millis(600)).is_some() {
            lock(&self.st).misses = 0;
            if !self.online() {
                log::info!("{} is available again", self.name);
                self.set_online(true);
                self.set_appeared_ms(glisa_common::monotonic_ms());
                if let Some(s) = &service {
                    s.device_online(self);
                }
                self.refresh();
                let cfg = self.cfg();
                let _ops = lock(&self.ops);
                self.apply_locked(&cfg);
                self.apply_buttons_locked(&cfg, true);
                return true;
            }
        } else {
            let misses = {
                let mut st = lock(&self.st);
                st.misses += 1;
                st.misses
            };
            if self.online() && misses >= 2 {
                log::info!("{} is not responding", self.name);
                self.set_online(false);
                if let Some(s) = &service {
                    s.release_all_input_now();
                }
                return true;
            }
        }
        false
    }

    pub(crate) fn alive_due(&self, every: Duration) -> bool {
        let mut st = lock(&self.st);
        if st.last_alive.is_some_and(|t| t.elapsed() <= every) {
            return false;
        }
        st.last_alive = Some(Instant::now());
        true
    }

    /// bluetooth | receiver | usb
    pub fn connection(&self) -> &'static str {
        if self.source.contains('#') {
            "receiver"
        } else if self.source.starts_with("sim://") {
            "bluetooth"
        } else {
            self.bus().unwrap_or("bluetooth")
        }
    }

    pub fn family(&self) -> &'static str {
        let cids: Vec<u16> = lock(&self.st).controls.iter().map(|c| c.cid).collect();
        models::family(self.model, &self.kind, &self.name, &self.features, &cids)
    }

    /// The API's device snapshot (`full`: everything the device page shows).
    pub fn snapshot(&self, full: bool) -> Value {
        let battery = lock(&self.st).battery.clone();
        let mut out = json!({
            "id": self.id,
            "name": self.display_name(),
            "model": self.model_name(),
            "kind": self.kind,
            "online": self.online(),
            "product_id": format!("{:04X}", self.dev.product_id),
            "source": self.source,
            "connection": self.connection(),
            "battery": battery.map(|b| json!({
                "percent": b.percent, "level": b.level, "charging": b.charging, "status": b.status,
            })),
            "family": self.family(),
            "photo": models::photo(self.model),
        });
        if !full {
            return out;
        }
        let cfg = self.cfg();
        let empty = Map::new();
        let buttons = cfg
            .get("buttons")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let presets = cfg
            .get("dpi_presets")
            .filter(|v| truthy(Some(v)))
            .cloned()
            .unwrap_or_else(|| json!(DEFAULT_DPI_PRESETS));
        let st = lock(&self.st);
        let extra = json!({
            "firmware": st.firmware,
            "features": self.features.iter().map(|f| feature_label(*f)).collect::<Vec<_>>(),
            "hosts": {
                "count": st.hosts.0,
                "current": st.hosts.1,
                "entries": st.host_entries.iter().map(|e| json!({
                    "index": e.index, "paired": e.paired, "name": e.name,
                })).collect::<Vec<_>>(),
            },
            "dpi": st.dpi.as_ref().map(|d| json!({
                "current": d.current, "default": d.default, "min": d.minimum, "max": d.maximum,
                "step": d.step.unwrap_or(50),
                "choices": d.choices.iter().take(200).collect::<Vec<_>>(),
                "presets": presets,
            })),
            "wheel": st.wheel.as_ref().map(|w| json!({
                "hires": w.hires, "inverted": w.inverted, "has_invert": w.has_invert,
            })),
            "smartshift": st.smartshift.as_ref().map(|s| json!({
                "ratchet": s.ratchet, "threshold": s.threshold, "enhanced": s.enhanced,
            })),
            "controls": st.controls.iter()
                .filter(|c| c.divertable() || c.reprogrammable())
                .map(|c| json!({
                    "cid": c.cid,
                    "name": fe::control_name(c.cid),
                    "divertable": c.divertable(),
                    "reprogrammable": c.reprogrammable(),
                    "gestures": c.supports_raw_xy(),
                    "action": buttons.get(&button_key(c.cid))
                        .filter(|a| truthy(Some(a)))
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "default"})),
                }))
                .collect::<Vec<_>>(),
            "settings": cfg.iter()
                .filter(|(k, _)| k.as_str() != "buttons" && k.as_str() != "display_name")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Map<String, Value>>(),
        });
        drop(st);
        if let (Value::Object(o), Value::Object(e)) = (&mut out, extra) {
            o.extend(e);
        }
        out
    }

    /// This device as Flow sees it.
    pub fn flow_device(&self) -> FlowDevice {
        let st = lock(&self.st);
        let kind = match self.kind.as_str() {
            "mouse" => DeviceKind::Mouse,
            "keyboard" => DeviceKind::Keyboard,
            "trackball" => DeviceKind::Trackball,
            "trackpad" => DeviceKind::Touchpad,
            "receiver" => DeviceKind::Receiver,
            _ => DeviceKind::Other,
        };
        FlowDevice {
            id: self.id.clone(),
            key: self.key.clone(),
            name: self.name.clone(),
            kind,
            online: self.online(),
            hosts_count: st.hosts.0,
            current_host: st.hosts.1,
            host_entries: st
                .host_entries
                .iter()
                .map(|e| json!({"index": e.index, "paired": e.paired, "name": e.name}))
                .collect(),
        }
    }
}
