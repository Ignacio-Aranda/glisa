//! Finds Logitech HID++ nodes and opens them as [`Transport`]s.
//!
//! - **Linux**: our own code. sysfs (`/sys/class/hidraw/*/device/{uevent,
//!   report_descriptor}`) for discovery, `/dev/hidrawN` for I/O. No libudev.
//! - **Windows and macOS**: the `hidapi` crate. On Windows each top-level collection is
//!   its own device path: Bluetooth HID++ is usage page 0xFF43 / usage 0x0202, and a
//!   receiver has a short (0xFF00/0x0001) and a long (0xFF00/0x0002) collection. We open
//!   the long one for writing and reading, and also read the short one. On macOS every
//!   collection shares one path; devices are opened in shared (non-exclusive) mode.
//!
//! Hot-plug is a rescan: the service calls [`scan`] every 2 s.

use std::io;

pub use glisa_hidpp::Transport;
use glisa_hidpp::constants::LOGITECH_VENDOR_ID;

pub mod descriptor;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(any(windows, target_os = "macos"))]
mod native;

pub use descriptor::parse_report_ids;

/// Linux bus numbers (also used on the other OSes for the same meaning).
pub const BUS_USB: u16 = 0x03;
pub const BUS_BLUETOOTH: u16 = 0x05;

/// Known receivers (USB product ids) and their kind.
pub const RECEIVERS: &[(u16, &str)] = &[
    (0xC52B, "Unifying"),
    (0xC532, "Unifying"),
    (0xC534, "Nano"),
    (0xC539, "Lightspeed"),
    (0xC53A, "Lightspeed"),
    (0xC53F, "Lightspeed"),
    (0xC547, "Lightspeed"),
    (0xC548, "Bolt"),
];

pub fn receiver_kind(product: u16) -> Option<&'static str> {
    RECEIVERS
        .iter()
        .find(|(p, _)| *p == product)
        .map(|(_, k)| *k)
}

/// A Logitech HID node that speaks HID++.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidInfo {
    /// `/dev/hidrawN`, or the hidapi path of the collection we write to.
    pub path: String,
    pub bus: u16,
    pub vendor: u16,
    pub product: u16,
    pub name: String,
    pub has_short: bool,
    pub has_long: bool,
    /// Other paths of the same interface to read from (Windows: a receiver's short
    /// collection). Empty on Linux.
    pub extra_paths: Vec<String>,
}

impl HidInfo {
    pub fn is_receiver(&self) -> bool {
        self.bus == BUS_USB && receiver_kind(self.product).is_some()
    }

    pub fn is_bluetooth(&self) -> bool {
        self.bus == BUS_BLUETOOTH
    }

    pub fn is_logitech(&self) -> bool {
        self.vendor == LOGITECH_VENDOR_ID
    }
}

/// Every Logitech HID++ node present now.
pub fn scan() -> Vec<HidInfo> {
    #[cfg(target_os = "linux")]
    {
        linux::scan_sysfs(std::path::Path::new(linux::SYS_HIDRAW))
    }
    #[cfg(any(windows, target_os = "macos"))]
    {
        native::scan()
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        Vec::new()
    }
}

/// Opens a node found by [`scan`]. `ErrorKind::PermissionDenied` means the user has no
/// access (Linux: the udev rule is missing; macOS: Input Monitoring is not granted).
pub fn open(info: &HidInfo) -> io::Result<Box<dyn Transport>> {
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(linux::HidrawTransport::open(
            &info.path,
            info.has_short,
            info.has_long,
        )?))
    }
    #[cfg(any(windows, target_os = "macos"))]
    {
        native::open(info)
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = info;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "HID is not supported on this OS",
        ))
    }
}
