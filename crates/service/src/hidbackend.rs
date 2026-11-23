//! Where the service finds and opens HID nodes: the real OS ([`SystemHid`]) or, in
//! tests, a fake (scans and opens are injected, as the Python tests patched `scan` and
//! `HidrawTransport`).

use std::io;

use glisa_hid::HidInfo;
use glisa_hidpp::Transport;

pub trait HidBackend: Send + Sync {
    fn scan(&self) -> Vec<HidInfo>;
    fn open(&self, info: &HidInfo) -> io::Result<Box<dyn Transport>>;
}

/// The OS: `glisa_hid::scan` / `open`.
pub struct SystemHid;

impl HidBackend for SystemHid {
    fn scan(&self) -> Vec<HidInfo> {
        glisa_hid::scan()
    }

    fn open(&self, info: &HidInfo) -> io::Result<Box<dyn Transport>> {
        glisa_hid::open(info)
    }
}

/// No hardware at all (tests that start the service's threads).
pub struct NoHid;

impl HidBackend for NoHid {
    fn scan(&self) -> Vec<HidInfo> {
        Vec::new()
    }

    fn open(&self, _info: &HidInfo) -> io::Result<Box<dyn Transport>> {
        Err(io::Error::from(io::ErrorKind::NotFound))
    }
}
