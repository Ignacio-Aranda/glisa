//! Windows and macOS: HID through the `hidapi` crate.
//!
//! Windows lists every top-level collection as its own path (`…&col01#…`, `…&col02#…`),
//! so collections of one interface are grouped by their path without the `&colNN`
//! part. macOS lists one entry per collection with the same path. Usage pages:
//! receivers 0xFF00 (usage 1: short reports, usage 2: long reports), Bluetooth devices
//! 0xFF43 (usage 0x0202: long reports).

use std::collections::BTreeMap;
use std::ffi::CString;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use glisa_hidpp::constants::LOGITECH_VENDOR_ID;
use glisa_hidpp::{Transport, TransportClosed};
use hidapi::{BusType, HidApi, HidDevice};

use crate::{BUS_BLUETOOTH, BUS_USB, HidInfo};

fn api() -> &'static Mutex<Option<HidApi>> {
    static API: OnceLock<Mutex<Option<HidApi>>> = OnceLock::new();
    API.get_or_init(|| Mutex::new(None))
}

fn with_api<R>(f: impl FnOnce(&mut HidApi) -> R) -> io::Result<R> {
    let mut guard = api().lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let api = HidApi::new().map_err(io::Error::other)?;
        // A seized device stops moving the pointer: always open shared.
        #[cfg(target_os = "macos")]
        api.set_open_exclusive(false);
        *guard = Some(api);
    }
    Ok(f(guard.as_mut().expect("initialised above")))
}

/// Group key: the path without Windows' `&colNN` collection suffix.
fn group_key(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    if let Some(i) = lower.find("&col") {
        let rest = &lower[i + 4..];
        let digits = rest.chars().take_while(|c| c.is_ascii_hexdigit()).count();
        return format!("{}{}", &lower[..i], &rest[digits..]);
    }
    lower
}

#[derive(Default)]
struct Group {
    short: Option<String>,
    long: Option<String>,
    bus: u16,
    product: u16,
    name: String,
}

pub fn scan() -> Vec<HidInfo> {
    let entries = with_api(|api| {
        if api.reset_devices().is_err() || api.add_devices(LOGITECH_VENDOR_ID, 0).is_err() {
            return Vec::new();
        }
        api.device_list()
            .map(|d| {
                (
                    d.path().to_string_lossy().into_owned(),
                    d.vendor_id(),
                    d.product_id(),
                    d.usage_page(),
                    d.usage(),
                    d.bus_type(),
                    d.product_string().unwrap_or_default().to_string(),
                )
            })
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for (path, vendor, product, page, usage, bus, name) in entries {
        if vendor != LOGITECH_VENDOR_ID || page < 0xFF00 {
            continue;
        }
        let g = groups.entry(group_key(&path)).or_default();
        match (page, usage) {
            (0xFF00, 0x0001) => g.short = Some(path),
            (0xFF00, 0x0002) | (0xFF43, 0x0202) => g.long = Some(path),
            _ => continue,
        }
        g.product = product;
        g.bus = if matches!(bus, BusType::Bluetooth) || page == 0xFF43 {
            BUS_BLUETOOTH
        } else {
            BUS_USB
        };
        if g.name.is_empty() {
            g.name = name;
        }
    }
    groups
        .into_values()
        .filter_map(|g| {
            let main = g.long.clone().or_else(|| g.short.clone())?;
            let extra_paths = match (&g.long, &g.short) {
                (Some(_), Some(short)) if *short != main => vec![short.clone()],
                _ => Vec::new(),
            };
            Some(HidInfo {
                path: main,
                bus: g.bus,
                vendor: LOGITECH_VENDOR_ID,
                product: g.product,
                name: g.name,
                has_short: g.short.is_some(),
                has_long: g.long.is_some(),
                extra_paths,
            })
        })
        .collect()
}

fn open_path(path: &str) -> io::Result<HidDevice> {
    let c = CString::new(path).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    with_api(|api| api.open_path(&c))?.map_err(|e| {
        let text = e.to_string();
        let lower = text.to_ascii_lowercase();
        // macOS without Input Monitoring: kIOReturnNotPermitted; Windows: access denied.
        if lower.contains("permit") || lower.contains("denied") || lower.contains("0xe00002e2") {
            io::Error::new(io::ErrorKind::PermissionDenied, text)
        } else {
            io::Error::other(text)
        }
    })
}

/// One HID++ interface opened through hidapi: a handle to read and one to write (two
/// handles, so a blocked read never delays a request), plus readers for the other
/// collections of a receiver.
pub struct HidapiTransport {
    path: String,
    reader: Mutex<HidDevice>,
    writer: Mutex<HidDevice>,
    extra: Vec<Mutex<HidDevice>>,
    closed: AtomicBool,
    short: bool,
    long: bool,
}

pub fn open(info: &HidInfo) -> io::Result<Box<dyn Transport>> {
    let reader = open_path(&info.path)?;
    let writer = open_path(&info.path)?;
    let extra = info
        .extra_paths
        .iter()
        .filter_map(|p| open_path(p).ok())
        .map(Mutex::new)
        .collect();
    Ok(Box::new(HidapiTransport {
        path: info.path.clone(),
        reader: Mutex::new(reader),
        writer: Mutex::new(writer),
        extra,
        closed: AtomicBool::new(false),
        // A receiver's short collection is only read: requests go out as long reports.
        short: info.has_short && !info.has_long,
        long: info.has_long,
    }))
}

impl HidapiTransport {
    fn gone(&self, why: impl std::fmt::Display) -> TransportClosed {
        TransportClosed(format!("{}: {why}", self.path))
    }
}

impl Transport for HidapiTransport {
    fn path(&self) -> &str {
        &self.path
    }

    fn supports_short(&self) -> bool {
        self.short
    }

    fn supports_long(&self) -> bool {
        self.long
    }

    fn write(&self, data: &[u8]) -> Result<(), TransportClosed> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(TransportClosed(self.path.clone()));
        }
        let dev = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        dev.write(data).map(|_| ()).map_err(|e| self.gone(e))
    }

    fn read(&self, timeout: Duration) -> Result<Option<Vec<u8>>, TransportClosed> {
        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; 64];
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return Err(TransportClosed(self.path.clone()));
            }
            for extra in &self.extra {
                let dev = extra.lock().unwrap_or_else(|e| e.into_inner());
                match dev.read_timeout(&mut buf, 0) {
                    Ok(n) if n > 0 => return Ok(Some(buf[..n].to_vec())),
                    Ok(_) => {}
                    Err(e) => return Err(self.gone(e)),
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            // Short slices: close() is noticed quickly and the extra readers get turns.
            let slice = left.min(Duration::from_millis(if self.extra.is_empty() {
                100
            } else {
                10
            }));
            let n = {
                let dev = self.reader.lock().unwrap_or_else(|e| e.into_inner());
                dev.read_timeout(&mut buf, slice.as_millis() as i32)
                    .map_err(|e| self.gone(e))?
            };
            if n > 0 {
                return Ok(Some(buf[..n].to_vec()));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_collections_group_together() {
        let a = r"\\?\HID#VID_046D&PID_C52B&MI_02&Col01#7&1a2b&0&0000#{4d1e55b2}";
        let b = r"\\?\HID#VID_046D&PID_C52B&MI_02&Col02#7&1a2b&0&0001#{4d1e55b2}";
        assert_eq!(group_key(a).replace("0000", "0001"), group_key(b));
        assert_eq!(group_key("DevSrvsID:4294969423"), "devsrvsid:4294969423");
    }
}
