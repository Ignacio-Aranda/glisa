//! Linux: discovery through sysfs and I/O on `/dev/hidrawN` (no libudev).

use std::ffi::CString;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use glisa_hidpp::constants::LOGITECH_VENDOR_ID;
use glisa_hidpp::{Transport, TransportClosed};

use crate::{HidInfo, parse_report_ids};

pub const SYS_HIDRAW: &str = "/sys/class/hidraw";

/// Report descriptors are at most 4 KiB (HID_MAX_DESCRIPTOR_SIZE).
const MAX_DESCRIPTOR: u64 = 4096;
const MAX_UEVENT: u64 = 16 * 1024;

fn read_limited(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let f = std::fs::File::open(path).ok()?;
    let mut out = Vec::new();
    f.take(limit).read_to_end(&mut out).ok()?;
    Some(out)
}

/// Logitech HID++ nodes under `root` (normally `/sys/class/hidraw`), sorted by name.
pub fn scan_sysfs(root: &Path) -> Vec<HidInfo> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut nodes: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("hidraw"))
        .collect();
    nodes.sort();
    let mut out = Vec::new();
    for node in nodes {
        let dev = root.join(&node).join("device");
        let uevent = read_limited(&dev.join("uevent"), MAX_UEVENT).unwrap_or_default();
        let uevent = String::from_utf8_lossy(&uevent);
        let prop = |key: &str| {
            uevent
                .lines()
                .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
                .map(str::to_string)
        };
        let Some((bus, vendor, product)) = prop("HID_ID").and_then(|id| parse_hid_id(&id)) else {
            continue;
        };
        if vendor != LOGITECH_VENDOR_ID {
            continue;
        }
        let Some(desc) = read_limited(&dev.join("report_descriptor"), MAX_DESCRIPTOR) else {
            continue;
        };
        let (has_short, has_long) = parse_report_ids(&desc);
        if !(has_short || has_long) {
            continue;
        }
        out.push(HidInfo {
            path: format!("/dev/{node}"),
            bus,
            vendor,
            product,
            name: prop("HID_NAME").unwrap_or_default(),
            has_short,
            has_long,
            extra_paths: Vec::new(),
        });
    }
    out
}

/// `0005:0000046D:0000B031` -> (bus, vendor, product)
fn parse_hid_id(id: &str) -> Option<(u16, u16, u16)> {
    let mut parts = id
        .split(':')
        .map(|p| u32::from_str_radix(p.trim(), 16).ok());
    let bus = parts.next()??;
    let vendor = parts.next()??;
    let product = parts.next()??;
    if parts.next().is_some() {
        return None;
    }
    Some((
        u16::try_from(bus).ok()?,
        u16::try_from(vendor).ok()?,
        u16::try_from(product).ok()?,
    ))
}

/// A `/dev/hidrawN` node. Reads poll the node and a wake-up pipe, so [`Transport::close`]
/// stops a blocked reader at once; the descriptors are closed when the transport is
/// dropped (never under a thread still polling them).
pub struct HidrawTransport {
    path: String,
    fd: OwnedFd,
    wake_r: OwnedFd,
    wake_w: OwnedFd,
    closed: AtomicBool,
    wlock: Mutex<()>,
    short: bool,
    long: bool,
}

fn last_error() -> io::Error {
    io::Error::last_os_error()
}

impl HidrawTransport {
    pub fn open(path: &str, supports_short: bool, supports_long: bool) -> io::Result<Self> {
        let cpath = CString::new(path).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        // SAFETY: cpath is a valid NUL-terminated string; open has no other preconditions.
        let fd = unsafe {
            libc::open(
                cpath.as_ptr(),
                libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(last_error());
        }
        // SAFETY: fd was just returned by open and is owned by nobody else.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut pipe = [0; 2];
        // SAFETY: pipe points to two writable c_ints.
        if unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
            return Err(last_error());
        }
        // SAFETY: both descriptors were just created by pipe2 and are owned by us.
        let (wake_r, wake_w) =
            unsafe { (OwnedFd::from_raw_fd(pipe[0]), OwnedFd::from_raw_fd(pipe[1])) };
        Ok(HidrawTransport {
            path: path.to_string(),
            fd,
            wake_r,
            wake_w,
            closed: AtomicBool::new(false),
            wlock: Mutex::new(()),
            short: supports_short,
            long: supports_long,
        })
    }

    fn closed_err(&self, why: impl std::fmt::Display) -> TransportClosed {
        TransportClosed(format!("{}: {why}", self.path))
    }
}

impl Transport for HidrawTransport {
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
        let _g = self.wlock.lock().unwrap_or_else(|e| e.into_inner());
        let mut tries = 0;
        loop {
            // SAFETY: data is a valid buffer of data.len() bytes; fd is open.
            let n = unsafe { libc::write(self.fd.as_raw_fd(), data.as_ptr().cast(), data.len()) };
            if n >= 0 {
                return Ok(());
            }
            let err = last_error();
            match err.raw_os_error() {
                Some(libc::EAGAIN) | Some(libc::EINTR) if tries < 50 => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(2));
                }
                _ => return Err(self.closed_err(err)),
            }
        }
    }

    fn read(&self, timeout: Duration) -> Result<Option<Vec<u8>>, TransportClosed> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(TransportClosed(self.path.clone()));
        }
        let mut fds = [
            libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.wake_r.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let ms = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
        // SAFETY: fds is an array of two valid pollfd structures.
        let r = unsafe { libc::poll(fds.as_mut_ptr(), 2, ms) };
        if r < 0 {
            let err = last_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                return Ok(None);
            }
            return Err(self.closed_err(err));
        }
        if self.closed.load(Ordering::SeqCst) || fds[1].revents != 0 {
            return Err(TransportClosed(self.path.clone()));
        }
        if fds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(self.closed_err("device gone"));
        }
        if r == 0 || fds[0].revents & libc::POLLIN == 0 {
            return Ok(None);
        }
        let mut buf = [0u8; 64];
        // SAFETY: buf is a writable buffer of 64 bytes; fd is open.
        let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            let err = last_error();
            return match err.raw_os_error() {
                Some(libc::EAGAIN) | Some(libc::EINTR) => Ok(None),
                _ => Err(self.closed_err(err)),
            };
        }
        if n == 0 {
            return Err(TransportClosed(self.path.clone()));
        }
        Ok(Some(buf[..n as usize].to_vec()))
    }

    fn close(&self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            // SAFETY: writes one byte from a valid buffer to our own pipe.
            unsafe { libc::write(self.wake_w.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lo-hid-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn node(root: &Path, name: &str, uevent: &str, desc: &[u8]) {
        let dev = root.join(name).join("device");
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(dev.join("uevent"), uevent).unwrap();
        std::fs::write(dev.join("report_descriptor"), desc).unwrap();
    }

    #[test]
    fn scans_a_fake_sysfs() {
        let root = tmpdir("sysfs");
        let hidpp = [0x06, 0x00, 0xFF, 0x85, 0x10, 0x85, 0x11];
        node(
            &root,
            "hidraw3",
            "DRIVER=hid-logitech-hidpp-device\nHID_ID=0005:0000046D:0000B031\nHID_NAME=LIFT\n",
            &hidpp,
        );
        node(
            &root,
            "hidraw1",
            "HID_ID=0003:0000046D:0000C52B\nHID_NAME=Logitech USB Receiver\n",
            &hidpp,
        );
        // Not Logitech, and a Logitech node without HID++ reports.
        node(&root, "hidraw0", "HID_ID=0003:00001234:00005678\n", &hidpp);
        node(
            &root,
            "hidraw2",
            "HID_ID=0003:0000046D:0000C077\n",
            &[0x05, 0x01, 0x85, 0x11],
        );
        node(&root, "hidraw9", "garbage", &hidpp);
        let found = scan_sysfs(&root);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].path, "/dev/hidraw1");
        assert!(found[0].is_receiver() && !found[0].is_bluetooth());
        assert_eq!(found[1].path, "/dev/hidraw3");
        assert_eq!(found[1].name, "LIFT");
        assert!(found[1].is_bluetooth() && found[1].has_short && found[1].has_long);
        assert_eq!(found[1].product, 0xB031);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hid_ids() {
        assert_eq!(
            parse_hid_id("0005:0000046D:0000B031"),
            Some((5, 0x046D, 0xB031))
        );
        assert_eq!(parse_hid_id("0005:0000046D"), None);
        assert_eq!(parse_hid_id("x:y:z"), None);
    }

    #[test]
    fn close_wakes_a_blocked_reader() {
        // A FIFO stands in for a hidraw node: nothing to read until we close.
        let dir = tmpdir("fifo");
        let path = dir.join("node");
        let c = CString::new(path.to_str().unwrap()).unwrap();
        // SAFETY: c is a valid path string.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let t =
            std::sync::Arc::new(HidrawTransport::open(path.to_str().unwrap(), true, true).unwrap());
        let t2 = t.clone();
        let reader = std::thread::spawn(move || t2.read(Duration::from_secs(10)));
        std::thread::sleep(Duration::from_millis(100));
        let t0 = std::time::Instant::now();
        t.close();
        let r = reader.join().unwrap();
        assert!(r.is_err());
        assert!(t0.elapsed() < Duration::from_secs(2));
        assert!(t.write(&[0x11]).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
