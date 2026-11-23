//! Persistent Flow state under `<config>/flow/` (directory 0700, every file 0600): this
//! computer's identity, paired peers with their pins, layout, channel mapping and options.
//!
//! Same files and format as the Python version (`flow.json`, `cert.pem`, `key.pem`), so a
//! user's pairings carry over. Entries whose pin does not match their certificate, and
//! anything else that does not have the expected shape, are dropped on load.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use glisa_i18n::tr;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::SIDES;
use crate::address::valid_host;
use crate::crypto::{self, IdentityError, generate_identity, pin_of_pem};
use crate::util::{lock, printable, random_hex, unix_time};

pub type Addr = (String, u16);

/// The user's Flow options (same names, types, defaults and limits as the Python version).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Options {
    /// Switch only while Ctrl is held.
    pub require_ctrl: bool,
    /// Send/receive clipboard text when switching.
    pub clipboard: bool,
    /// Send/receive copied files, folders and images when switching.
    pub files: bool,
    /// Files dragged to the edge go along (glow, edge catcher, `drag` notice, pickup). Added by
    /// Glisa: a store without it (older, or made by the Python version) gets the default.
    pub drag_files: bool,
    /// Keyboards follow the mouse.
    pub keyboard: bool,
    /// Nothing switches again this soon after a switch.
    pub cooldown_ms: i64,
    /// Dead zone at both ends of an edge.
    pub corner_px: i64,
    /// Copied files: at most this many MiB per transfer (sent or received).
    pub max_transfer_mb: i64,
    /// A copied image: at most this many MiB.
    pub max_image_mb: i64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            require_ctrl: false,
            clipboard: true,
            files: true,
            drag_files: true,
            keyboard: true,
            cooldown_ms: 800,
            corner_px: 48,
            max_transfer_mb: 2048,
            max_image_mb: 50,
        }
    }
}

const BOOL_OPTIONS: [&str; 5] = [
    "require_ctrl",
    "clipboard",
    "files",
    "drag_files",
    "keyboard",
];
const INT_OPTIONS: [(&str, i64, i64); 4] = [
    ("cooldown_ms", 200, 5000),
    ("corner_px", 0, 400),
    ("max_transfer_mb", 1, 102_400),
    ("max_image_mb", 1, 200),
];

impl Options {
    fn bool_mut(&mut self, k: &str) -> Option<&mut bool> {
        Some(match k {
            "require_ctrl" => &mut self.require_ctrl,
            "clipboard" => &mut self.clipboard,
            "files" => &mut self.files,
            "drag_files" => &mut self.drag_files,
            "keyboard" => &mut self.keyboard,
            _ => return None,
        })
    }

    fn int_mut(&mut self, k: &str) -> Option<&mut i64> {
        Some(match k {
            "cooldown_ms" => &mut self.cooldown_ms,
            "corner_px" => &mut self.corner_px,
            "max_transfer_mb" => &mut self.max_transfer_mb,
            "max_image_mb" => &mut self.max_image_mb,
            _ => return None,
        })
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).expect("options serialize")
    }

    /// Bytes per transfer of files.
    pub fn max_transfer_bytes(&self) -> u64 {
        (self.max_transfer_mb.max(0) as u64) << 20
    }

    /// Bytes per image.
    pub fn max_image_bytes(&self) -> u64 {
        (self.max_image_mb.max(0) as u64) << 20
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Peer {
    pub name: String,
    pub pin: String,
    pub cert: String,
    /// Where the pinned peer was last reached (or reached us from).
    pub address: Option<Addr>,
    /// The address the user typed (never overwritten by a learned one).
    pub manual_address: Option<Addr>,
    pub paired_at: f64,
}

impl Peer {
    fn to_json(&self) -> Value {
        let addr = |a: &Option<Addr>| {
            a.as_ref()
                .map(|(h, p)| json!([h, p]))
                .unwrap_or(Value::Null)
        };
        json!({"name": self.name, "pin": self.pin, "cert": self.cert,
               "address": addr(&self.address), "manual_address": addr(&self.manual_address),
               "paired_at": self.paired_at})
    }
}

#[derive(Debug, Clone, Default)]
struct Data {
    id: String,
    enabled: bool,
    options: Options,
    peers: BTreeMap<String, Peer>,
    layout: BTreeMap<String, String>,
    channels: BTreeMap<String, BTreeMap<String, u8>>,
}

impl Data {
    fn to_json(&self) -> Value {
        let peers: Map<String, Value> = self
            .peers
            .iter()
            .map(|(k, p)| (k.clone(), p.to_json()))
            .collect();
        json!({"version": 1, "id": self.id, "enabled": self.enabled, "options": self.options.to_json(),
               "peers": peers, "layout": self.layout, "channels": self.channels})
    }
}

/// A computer name from the network, safe to show: printable, no markup characters (labels
/// and desktop notifications may render HTML, which would let anyone on the LAN restyle
/// the pairing dialog), ≤ 64 characters.
pub fn clean_name(name: &str) -> String {
    let kept: String = name
        .chars()
        .filter(|c| printable(&c.to_string()) && !"<>&".contains(*c))
        .collect();
    let out: String = kept.trim().chars().take(64).collect();
    if out.is_empty() {
        "linux".to_string()
    } else {
        out
    }
}

fn is_id(s: &str) -> bool {
    crate::util::is_hex_lower(s, 16)
}

/// [host, port] from flow.json, or None if it is anything else.
fn stored_address(v: Option<&Value>) -> Option<Addr> {
    let a = v?.as_array()?;
    if a.len() != 2 {
        return None;
    }
    let host = a[0].as_str()?;
    let port = a[1].as_u64()?;
    if !valid_host(host) || !(1..=65535).contains(&port) || !a[1].is_u64() {
        return None;
    }
    Some((host.to_string(), port as u16))
}

pub struct FlowStore {
    pub dir: PathBuf,
    pub file: PathBuf,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub hostname: String,
    data: Mutex<Data>,
    pin: Mutex<Option<String>>,
}

impl FlowStore {
    pub fn new(directory: &Path, hostname: Option<&str>) -> Self {
        let host = hostname
            .map(str::to_string)
            .unwrap_or_else(crate::util::hostname);
        let s = Self {
            dir: directory.to_path_buf(),
            file: directory.join("flow.json"),
            cert_path: directory.join("cert.pem"),
            key_path: directory.join("key.pem"),
            hostname: clean_name(&host),
            data: Mutex::new(Data::default()),
            pin: Mutex::new(None),
        };
        s.load();
        s
    }

    // ------------------------------------------------------------ files
    pub fn load(&self) {
        let Ok(text) = fs::read(&self.file) else {
            return;
        };
        let Ok(Value::Object(loaded)) = serde_json::from_slice::<Value>(&text) else {
            return;
        };
        let mut d = Data::default();
        if let Some(id) = loaded
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| is_id(s))
        {
            d.id = id.to_string();
        }
        d.enabled = match loaded.get("enabled") {
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
            Some(Value::String(s)) => !s.is_empty(),
            _ => false,
        };
        if let Some(Value::Object(opts)) = loaded.get("options") {
            for k in BOOL_OPTIONS {
                if let Some(Value::Bool(b)) = opts.get(k) {
                    *d.options.bool_mut(k).expect("known option") = *b;
                }
            }
            for (k, _, _) in INT_OPTIONS {
                if let Some(Value::Number(n)) = opts.get(k) {
                    if let Some(i) = n.as_i64() {
                        *d.options.int_mut(k).expect("known option") = i;
                    }
                }
            }
        }
        if let Some(Value::Object(peers)) = loaded.get("peers") {
            for (pid, p) in peers {
                let Some(p) = p.as_object() else { continue };
                let Some(cert) = p.get("cert").and_then(Value::as_str) else {
                    continue;
                };
                if !is_id(pid) {
                    continue;
                }
                let Ok(pin) = pin_of_pem(cert) else { continue };
                if p.get("pin").and_then(Value::as_str) != Some(pin.as_str()) {
                    continue; // tampered or corrupt entry: drop it
                }
                let name = match p.get("name") {
                    Some(Value::String(s)) => s.clone(),
                    Some(v) if !v.is_null() => v.to_string(),
                    _ => String::new(),
                };
                d.peers.insert(
                    pid.clone(),
                    Peer {
                        name: clean_name(&name),
                        pin,
                        cert: cert.to_string(),
                        address: stored_address(p.get("address")),
                        manual_address: stored_address(p.get("manual_address")),
                        paired_at: p.get("paired_at").and_then(Value::as_f64).unwrap_or(0.0),
                    },
                );
            }
        }
        if let Some(Value::Object(layout)) = loaded.get("layout") {
            for (pid, side) in layout {
                if let Some(side) = side.as_str().filter(|s| SIDES.contains(s)) {
                    if d.peers.contains_key(pid) {
                        d.layout.insert(pid.clone(), side.to_string());
                    }
                }
            }
        }
        if let Some(Value::Object(channels)) = loaded.get("channels") {
            for (dev, m) in channels {
                let Some(m) = m.as_object() else { continue };
                let chosen: BTreeMap<String, u8> = m
                    .iter()
                    .filter(|(pid, _)| d.peers.contains_key(*pid))
                    .filter_map(|(pid, ch)| {
                        let ch = ch.as_u64().filter(|c| *c < 8)?;
                        Some((pid.clone(), ch as u8))
                    })
                    .collect();
                d.channels.insert(dev.clone(), chosen);
            }
        }
        *lock(&self.data) = d;
    }

    fn save_locked(&self, d: &Data) -> std::io::Result<()> {
        crypto::private_dir(&self.dir)?;
        let text = serde_json::to_string_pretty(&d.to_json()).expect("store serializes");
        let tmp = self.dir.join(format!(".flow.{}", random_hex(6)));
        let result = crypto::write_private(&tmp, text.as_bytes())
            .and_then(|()| fs::rename(&tmp, &self.file));
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }

    pub fn save(&self) -> std::io::Result<()> {
        let d = lock(&self.data);
        self.save_locked(&d)
    }

    fn save_or_log(&self, d: &Data) {
        if let Err(e) = self.save_locked(d) {
            log::warn!("Flow: couldn't save {}: {e}", self.file.display());
        }
    }

    // --------------------------------------------------------- identity
    pub fn id(&self) -> String {
        lock(&self.data).id.clone()
    }

    /// Create id + certificate the first time.
    pub fn ensure_identity(&self) -> Result<(), IdentityError> {
        let mut d = lock(&self.data);
        crypto::private_dir(&self.dir).map_err(|e| crypto::io_error(&e))?;
        let mut changed = false;
        if d.id.is_empty() {
            d.id = random_hex(8);
            changed = true;
        }
        if !(self.cert_path.exists() && self.key_path.exists()) {
            generate_identity(&self.cert_path, &self.key_path)?;
            *lock(&self.pin) = None;
        }
        #[cfg(unix)]
        for p in [&self.cert_path, &self.key_path] {
            use std::os::unix::fs::PermissionsExt;
            let meta = fs::metadata(p).map_err(|e| crypto::io_error(&e))?;
            if meta.permissions().mode() & 0o077 != 0 {
                fs::set_permissions(p, fs::Permissions::from_mode(0o600))
                    .map_err(|e| crypto::io_error(&e))?;
            }
        }
        if changed || !self.file.exists() {
            self.save_locked(&d).map_err(|e| crypto::io_error(&e))?;
        }
        Ok(())
    }

    pub fn cert_pem(&self) -> std::io::Result<String> {
        fs::read_to_string(&self.cert_path)
    }

    /// SHA-256 pin of this computer's certificate ("" if there is none yet).
    pub fn pin(&self) -> String {
        let mut p = lock(&self.pin);
        if p.is_none() {
            *p = self.cert_pem().ok().and_then(|pem| pin_of_pem(&pem).ok());
        }
        p.clone().unwrap_or_default()
    }

    /// Forget the cached pin (the certificate file changed).
    pub fn reset_pin_cache(&self) {
        *lock(&self.pin) = None;
    }

    /// New id and key pair. Every pairing is forgotten (their pins would no longer match).
    pub fn regenerate_identity(&self) -> Result<(), IdentityError> {
        {
            let mut d = lock(&self.data);
            let _ = fs::remove_file(&self.cert_path);
            let _ = fs::remove_file(&self.key_path);
            *lock(&self.pin) = None;
            d.id.clear();
            d.peers.clear();
            d.layout.clear();
            d.channels.clear();
        }
        self.ensure_identity()?;
        self.save().map_err(|e| crypto::io_error(&e))
    }

    // ------------------------------------------------------------ peers
    pub fn peers(&self) -> BTreeMap<String, Peer> {
        lock(&self.data).peers.clone()
    }

    pub fn peer(&self, pid: &str) -> Option<Peer> {
        lock(&self.data).peers.get(pid).cloned()
    }

    pub fn peer_by_pin(&self, pin: &str) -> Option<String> {
        lock(&self.data)
            .peers
            .iter()
            .find(|(_, p)| p.pin == pin)
            .map(|(k, _)| k.clone())
    }

    pub fn peer_ids(&self) -> Vec<String> {
        lock(&self.data).peers.keys().cloned().collect()
    }

    pub fn add_peer(
        &self,
        pid: &str,
        name: &str,
        cert: &str,
        address: Option<Addr>,
    ) -> Result<(), String> {
        let pin = pin_of_pem(cert).map_err(|e| e.to_string())?;
        let mut d = lock(&self.data);
        // one entry per identity: a re-pair under a new id replaces the old one
        let old: Vec<String> = d
            .peers
            .iter()
            .filter(|(k, p)| p.pin == pin && *k != pid)
            .map(|(k, _)| k.clone())
            .collect();
        for k in old {
            Self::remove_locked(&mut d, &k);
        }
        let manual = d.peers.get(pid).and_then(|p| p.manual_address.clone()); // pairing again keeps a typed address
        d.peers.insert(
            pid.to_string(),
            Peer {
                name: clean_name(name),
                pin,
                cert: cert.to_string(),
                address,
                manual_address: manual,
                paired_at: unix_time(),
            },
        );
        self.save_locked(&d).map_err(|e| e.to_string())
    }

    fn remove_locked(d: &mut Data, pid: &str) -> bool {
        let existed = d.peers.remove(pid).is_some();
        d.layout.remove(pid);
        for m in d.channels.values_mut() {
            m.remove(pid);
        }
        existed
    }

    /// Forget a peer: its pin, certificate, layout and channel choices.
    pub fn remove_peer(&self, pid: &str) -> bool {
        let mut d = lock(&self.data);
        let existed = Self::remove_locked(&mut d, pid);
        self.save_or_log(&d);
        existed
    }

    /// The address where the pinned peer was last reached (or reached us from). True if it
    /// changed.
    pub fn set_address(&self, pid: &str, host: &str, port: u16) -> bool {
        let mut d = lock(&self.data);
        let Some(p) = d.peers.get_mut(pid) else {
            return false;
        };
        let want = Some((host.to_string(), port));
        if p.address == want {
            return false;
        }
        p.address = want;
        self.save_or_log(&d);
        true
    }

    /// The address the user typed for a peer (None: find it automatically).
    pub fn set_manual_address(&self, pid: &str, addr: Option<Addr>) -> Result<bool, String> {
        if let Some((h, p)) = &addr {
            if !valid_host(h) || *p == 0 {
                return Err(tr!("Invalid address"));
            }
        }
        let mut d = lock(&self.data);
        let Some(p) = d.peers.get_mut(pid) else {
            return Err(tr!("That computer isn't paired"));
        };
        if p.manual_address == addr {
            return Ok(false);
        }
        p.manual_address = addr;
        self.save_locked(&d).map_err(|e| e.to_string())?;
        Ok(true)
    }

    pub fn set_peer_name(&self, pid: &str, name: &str) {
        let mut d = lock(&self.data);
        let clean = clean_name(name);
        if let Some(p) = d.peers.get_mut(pid) {
            if p.name != clean {
                p.name = clean;
                self.save_or_log(&d);
            }
        }
    }

    // ------------------------------------------------- layout / options
    pub fn layout(&self) -> BTreeMap<String, String> {
        lock(&self.data).layout.clone()
    }

    pub fn set_side(&self, pid: &str, side: Option<&str>) -> Result<(), String> {
        let mut d = lock(&self.data);
        if !d.peers.contains_key(pid) {
            return Err(tr!("That computer isn't paired"));
        }
        match side {
            None => {
                d.layout.remove(pid);
            }
            Some(side) => {
                if !SIDES.contains(&side) {
                    return Err(tr!("Invalid position"));
                }
                let others: Vec<String> = d
                    .layout
                    .iter()
                    .filter(|(o, s)| s.as_str() == side && o.as_str() != pid)
                    .map(|(o, _)| o.clone())
                    .collect();
                for other in others {
                    // one peer per side: the one there takes our old place (or none)
                    match d.layout.get(pid).cloned() {
                        Some(old) => {
                            d.layout.insert(other, old);
                        }
                        None => {
                            d.layout.remove(&other);
                        }
                    }
                }
                d.layout.insert(pid.to_string(), side.to_string());
            }
        }
        self.save_locked(&d).map_err(|e| e.to_string())
    }

    pub fn peer_at(&self, side: &str) -> Option<String> {
        lock(&self.data)
            .layout
            .iter()
            .find(|(_, s)| s.as_str() == side)
            .map(|(p, _)| p.clone())
    }

    pub fn options(&self) -> Options {
        lock(&self.data).options.clone()
    }

    /// Sets options from `{name: value}`; whole numbers are clamped to their limits.
    pub fn set_options(&self, values: &Map<String, Value>) -> Result<(), String> {
        let mut d = lock(&self.data);
        let mut opts = d.options.clone();
        for (k, v) in values {
            if let Some(slot) = opts.bool_mut(k) {
                let Some(b) = v.as_bool() else {
                    return Err(tr!("{option} must be true or false", option = k));
                };
                *slot = b;
            } else if let Some(slot) = opts.int_mut(k) {
                let (_, lo, hi) = INT_OPTIONS
                    .iter()
                    .find(|(n, _, _)| n == k)
                    .copied()
                    .expect("limits");
                let n = match v {
                    Value::Number(n) if n.is_i64() || n.is_u64() => n.as_i64().unwrap_or(i64::MAX),
                    _ => return Err(tr!("{option} must be a whole number", option = k)),
                };
                *slot = n.clamp(lo, hi);
            } else {
                return Err(tr!("Unknown option: {option}", option = k));
            }
        }
        d.options = opts;
        self.save_locked(&d).map_err(|e| e.to_string())
    }

    pub fn enabled(&self) -> bool {
        lock(&self.data).enabled
    }

    pub fn set_enabled(&self, on: bool) {
        let mut d = lock(&self.data);
        d.enabled = on;
        self.save_or_log(&d);
    }

    // ----------------------------------------------------------- channels
    pub fn channel(&self, device_key: &str, pid: &str) -> Option<u8> {
        lock(&self.data)
            .channels
            .get(device_key)
            .and_then(|m| m.get(pid))
            .copied()
    }

    pub fn set_channel(
        &self,
        device_key: &str,
        pid: &str,
        channel: Option<u8>,
    ) -> Result<(), String> {
        let mut d = lock(&self.data);
        if !d.peers.contains_key(pid) {
            return Err(tr!("That computer isn't paired"));
        }
        let m = d.channels.entry(device_key.to_string()).or_default();
        match channel {
            None => {
                m.remove(pid);
            }
            Some(c) => {
                m.insert(pid.to_string(), c);
            }
        }
        self.save_locked(&d).map_err(|e| e.to_string())
    }

    /// Drop the channel choices of a device (it was removed from Glisa).
    pub fn forget_device(&self, device_key: &str) -> bool {
        let mut d = lock(&self.data);
        if d.channels.remove(device_key).is_none() {
            return false;
        }
        self.save_or_log(&d);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer_cert(dir: &Path) -> String {
        let (c, k) = (dir.join("peer-cert.pem"), dir.join("peer-key.pem"));
        generate_identity(&c, &k).unwrap();
        fs::read_to_string(c).unwrap()
    }

    #[test]
    fn permissions_tamper_and_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("flow");
        let s = FlowStore::new(&dir, Some("yo"));
        s.ensure_identity().unwrap();
        let cert = peer_cert(tmp.path());
        let pid = "0123456789abcdef";
        s.add_peer(pid, "otro", &cert, Some(("10.0.0.2".into(), 27461)))
            .unwrap();
        s.set_side(pid, Some("left")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for name in ["flow.json", "cert.pem", "key.pem"] {
                assert_eq!(
                    fs::metadata(dir.join(name)).unwrap().permissions().mode() & 0o777,
                    0o600,
                    "{name}"
                );
            }
            assert_eq!(
                fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let mut data: Value = serde_json::from_str(&fs::read_to_string(&s.file).unwrap()).unwrap();
        data["peers"][pid]["pin"] = json!("00".repeat(32)); // tampered pin
        fs::write(&s.file, data.to_string()).unwrap();
        assert!(FlowStore::new(&dir, Some("yo")).peers().is_empty());
        s.save().unwrap();
        assert!(s.remove_peer(pid));
        assert!(!fs::read_to_string(&s.file).unwrap().contains(pid)); // pin, layout, all gone
    }

    #[test]
    fn manual_address() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("flow");
        let s = FlowStore::new(&dir, Some("yo"));
        s.ensure_identity().unwrap();
        let cert = peer_cert(tmp.path());
        let pid = "0123456789abcdef";
        s.add_peer(pid, "otro", &cert, Some(("192.168.1.110".into(), 27461)))
            .unwrap();
        let typed = Some(("192.168.2.20".to_string(), 27461));
        assert!(s.set_manual_address(pid, typed.clone()).unwrap());
        assert!(!s.set_manual_address(pid, typed.clone()).unwrap());
        assert!(s.set_address(pid, "192.168.2.20", 27461));
        let p = FlowStore::new(&dir, Some("yo")).peer(pid).unwrap();
        assert_eq!(
            (p.address.clone(), p.manual_address.clone()),
            (typed.clone(), typed.clone())
        );
        s.add_peer(pid, "otro", &cert, Some(("10.0.0.9".into(), 27461)))
            .unwrap(); // typed one stays
        assert_eq!(s.peer(pid).unwrap().manual_address, typed);
        assert!(
            s.set_manual_address(pid, Some(("bad host".into(), 1)))
                .is_err()
        );
        assert!(
            s.set_manual_address("fedcba9876543210", Some(("10.0.0.1".into(), 1)))
                .is_err()
        );
        assert!(s.set_manual_address(pid, None).unwrap());
        assert_eq!(
            FlowStore::new(&dir, Some("yo"))
                .peer(pid)
                .unwrap()
                .manual_address,
            None
        );
        let mut data: Value = serde_json::from_str(&fs::read_to_string(&s.file).unwrap()).unwrap();
        data["peers"][pid]["manual_address"] = json!(["<b>x</b>", 99999]);
        data["peers"][pid]["address"] = json!("10.0.0.1:1");
        fs::write(&s.file, data.to_string()).unwrap();
        let p = FlowStore::new(&dir, Some("yo")).peer(pid).unwrap();
        assert_eq!((p.address, p.manual_address), (None, None));
    }

    #[test]
    fn python_store_format_loads() {
        // The shape the Python version writes (json.dump of FlowStore.data).
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("flow");
        fs::create_dir_all(&dir).unwrap();
        let cert = peer_cert(tmp.path());
        let pin = pin_of_pem(&cert).unwrap();
        let data = json!({"version": 1, "id": "aaaaaaaaaaaaaaaa", "enabled": true,
            "options": {"require_ctrl": true, "clipboard": false, "files": true, "keyboard": true,
                        "cooldown_ms": 900, "corner_px": 30.5, "max_transfer_mb": 10, "max_image_mb": 5},
            "peers": {"0123456789abcdef": {"name": "nuc", "pin": pin, "cert": cert,
                       "address": ["192.168.1.5", 27461], "manual_address": null, "paired_at": 1763.5}},
            "layout": {"0123456789abcdef": "right", "ffffffffffffffff": "left"},
            "channels": {"B031:LIFT": {"0123456789abcdef": 1, "ffffffffffffffff": 0}}});
        fs::write(
            dir.join("flow.json"),
            serde_json::to_string_pretty(&data).unwrap(),
        )
        .unwrap();
        let s = FlowStore::new(&dir, Some("yo"));
        assert_eq!(s.id(), "aaaaaaaaaaaaaaaa");
        assert!(s.enabled());
        let o = s.options();
        assert!(o.require_ctrl && !o.clipboard);
        assert!(o.drag_files); // a key the Python version never wrote: the default
        assert_eq!(
            (o.cooldown_ms, o.corner_px, o.max_transfer_mb),
            (900, 48, 10)
        ); // a float is dropped
        assert_eq!(
            s.peer("0123456789abcdef").unwrap().address,
            Some(("192.168.1.5".into(), 27461))
        );
        assert_eq!(s.layout().len(), 1);
        assert_eq!(s.channel("B031:LIFT", "0123456789abcdef"), Some(1));
        assert_eq!(s.channel("B031:LIFT", "ffffffffffffffff"), None);
        assert_eq!(s.peer_at("right").as_deref(), Some("0123456789abcdef"));
    }

    #[test]
    fn options_and_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let s = FlowStore::new(&tmp.path().join("flow"), Some("yo"));
        s.ensure_identity().unwrap();
        let set = |v: Value| s.set_options(v.as_object().unwrap());
        assert!(set(json!({"require_ctrl": "sí"})).is_err());
        assert!(set(json!({"cooldown_ms": 1.5})).is_err());
        assert!(set(json!({"cooldown_ms": true})).is_err());
        assert!(set(json!({"nope": 1})).is_err());
        set(json!({"cooldown_ms": 10, "corner_px": 9999})).unwrap();
        assert_eq!((s.options().cooldown_ms, s.options().corner_px), (200, 400));
        // dragging files across the edge: on by default, a switch like the others, kept
        assert!(s.options().drag_files);
        assert!(set(json!({"drag_files": 1})).is_err());
        set(json!({"drag_files": false})).unwrap();
        assert!(!s.options().drag_files);
        assert_eq!(s.options().to_json()["drag_files"], false);
        let again = FlowStore::new(&tmp.path().join("flow"), Some("yo"));
        assert!(!again.options().drag_files);
        assert_eq!(
            clean_name("<p style=\"x\">482 913</p>"),
            "p style=\"x\"482 913/p"
        );
        assert_eq!(clean_name("  \u{202e}\n "), "linux");
        assert_eq!(clean_name("portátil-ana"), "portátil-ana");
    }
}
