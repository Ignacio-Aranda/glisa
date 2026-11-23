//! Things every Glisa crate shares: app identity, per-OS paths, the API error and
//! event types, and the boundary between the device service and Flow.
//!
//! See docs/ARCHITECTURE.md.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub mod paths;

/// Product name shown to people. Rebranding changes this and the constants below.
pub const APP_NAME: &str = "Glisa";
/// Reverse-DNS identifier (bundle id, D-Bus prefix, autostart entry).
pub const APP_ID: &str = "io.github.ignacioaranda.glisa";
/// Directory and file stem used on disk ("~/.config/glisa", "glisa.sock").
pub const APP_DIR: &str = "glisa";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Longest name a user can give a device, in characters.
pub const DISPLAY_NAME_MAX: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Linux,
    Windows,
    Macos,
    Other,
}

pub const fn platform() -> Platform {
    if cfg!(target_os = "linux") {
        Platform::Linux
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Other
    }
}

/// An error answered to an API client. The message is already in the client's language
/// (it is built inside `i18n::with_language`). Wire form: `{"id": …, "error": message}`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ApiError(pub String);

impl ApiError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

pub type ApiResult = Result<serde_json::Value, ApiError>;

/// Something changed; clients re-fetch what they show. Topics: "devices", "device",
/// "status", "flow", "transfer". `data` is small (an id, a progress record…).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub topic: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

impl Event {
    pub fn new(topic: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            topic: topic.into(),
            data,
        }
    }
}

/// Receives events from the service and from Flow. Must not block.
pub type EventSink = std::sync::Arc<dyn Fn(Event) + Send + Sync>;

// ------------------------------------------------------------------ service <-> Flow

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Mouse,
    Keyboard,
    Trackball,
    Touchpad,
    Receiver,
    Other,
}

/// A configured device as Flow sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowDevice {
    /// Service id (the API's `id`).
    pub id: String,
    /// Stable config key (survives reconnections; the Python `md.key`).
    pub key: String,
    pub name: String,
    pub kind: DeviceKind,
    pub online: bool,
    /// Number of Easy-Switch channels (0 when the device has no change-host feature).
    pub hosts_count: u8,
    /// Channel the device is on now (0-based).
    pub current_host: u8,
    /// Names of the computers paired on each channel when the device reports them.
    pub host_entries: Vec<serde_json::Value>,
}

/// A Logitech HID node that just appeared, before its HID++ set-up (Flow's earliest
/// sign that an expected device arrived).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppearingDevice {
    /// OS path of the node (hidraw path, HID device path).
    pub path: String,
    pub vendor: u16,
    pub product: u16,
    pub is_receiver: bool,
    pub is_bluetooth: bool,
    /// When it appeared, milliseconds on a monotonic clock (`monotonic_ms`).
    pub appeared_ms: u64,
}

/// Implemented by the device service for Flow.
pub trait FlowHost: Send + Sync {
    fn flow_devices(&self) -> Vec<FlowDevice>;
    /// Switch a device to another Easy-Switch channel (HID++ 0x1814).
    fn set_host(&self, device_id: &str, channel: u8) -> Result<(), String>;
    /// Release every key and button the virtual input device holds.
    fn release_all_input(&self);
    /// Look for devices that just appeared right now instead of at the next periodic
    /// scan (Flow expects one after a crossing). Default: nothing.
    fn poll_arrivals(&self) {}
}

/// Implemented by `glisa_flow::FlowService` for the device service.
pub trait FlowApi: Send + Sync {
    /// Handles every `flow_*` method. Runs inside the caller's `i18n::with_language`.
    fn api(&self, method: &str, params: &serde_json::Value) -> ApiResult;
    fn device_appearing(&self, device: &AppearingDevice);
    fn device_online(&self, device_id: &str);
    /// The user removed a device (`forget`): drop what Flow keeps about it (its channel
    /// per peer). `key` is [`FlowDevice::key`]. Default: nothing.
    fn device_forgotten(&self, _key: &str) {}
    fn stop(&self);
}

/// Milliseconds on a process-wide monotonic clock.
pub fn monotonic_ms() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// A name typed by the user, made safe to show anywhere: no control or format characters
/// (newlines, bidi overrides…), runs of white space collapsed to one space, trimmed.
/// Empty means "no name". The caller checks the length against [`DISPLAY_NAME_MAX`].
pub fn clean_display_name(text: &str) -> String {
    let kept: String = text
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|&c| c == ' ' || c == '\u{200D}' || !is_control_or_format(c))
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Unicode general category C* (control, format, surrogate, private use, unassigned),
/// approximated without a Unicode database: controls, the format characters that matter
/// (bidi controls, zero-width, BOM, soft hyphen, tags) and private-use planes.
fn is_control_or_format(c: char) -> bool {
    let u = c as u32;
    c.is_control()
        || matches!(u,
            0x00AD | 0x061C | 0x070F | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E
            | 0x2060..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB | 0xE0000..=0xE007F
            | 0xE000..=0xF8FF | 0xF0000..=0x10FFFF)
}

/// `~/.config/glisa` and friends; see [`paths`].
pub fn config_dir() -> PathBuf {
    paths::config_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_are_cleaned() {
        assert_eq!(clean_display_name("  My\tmouse \n "), "My mouse");
        assert_eq!(clean_display_name("a\u{202E}b"), "ab");
        assert_eq!(clean_display_name("👩\u{200D}💻 desk"), "👩\u{200D}💻 desk");
        assert_eq!(clean_display_name("\u{0}\u{7}"), "");
    }
}
