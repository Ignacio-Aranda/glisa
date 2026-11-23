//! HID++ 2.0 constants: report ids, feature ids, control ids, error names, flag bits.
//!
//! Values come from the publicly documented HID++ 2.0 protocol and from the open-source
//! Linux ecosystem (Solaar, logiops, the kernel's hid-logitech-hidpp).

use glisa_i18n::n_;

pub const LOGITECH_VENDOR_ID: u16 = 0x046D;

/// Short report: 7 bytes.
pub const REPORT_SHORT: u8 = 0x10;
/// Long report: 20 bytes (preferred everywhere).
pub const REPORT_LONG: u8 = 0x11;
/// Very long report: 64 bytes (only ever received).
pub const REPORT_VERY_LONG: u8 = 0x12;

/// Total length (report id included) of a HID++ report.
pub const fn report_len(report_id: u8) -> Option<usize> {
    match report_id {
        REPORT_SHORT => Some(7),
        REPORT_LONG => Some(20),
        REPORT_VERY_LONG => Some(64),
        _ => None,
    }
}

/// Bluetooth and wired devices answer as index 0xFF; receivers use 1..=6.
pub const DEVICE_INDEX_DIRECT: u8 = 0xFF;

/// Our HID++ software id (non-zero; notifications use 0). The Python implementation uses
/// 0x0A: a different value lets both talk to the same device side by side, since every
/// process reading a hidraw node sees every answer and matches its own by this id.
pub const SW_ID: u8 = 0x0B;

/// Feature ids.
pub mod f {
    pub const ROOT: u16 = 0x0000;
    pub const FEATURE_SET: u16 = 0x0001;
    pub const DEVICE_FW_VERSION: u16 = 0x0003;
    pub const DEVICE_NAME: u16 = 0x0005;
    pub const RESET: u16 = 0x0020;
    pub const BATTERY_STATUS: u16 = 0x1000;
    pub const BATTERY_VOLTAGE: u16 = 0x1001;
    pub const UNIFIED_BATTERY: u16 = 0x1004;
    pub const CHANGE_HOST: u16 = 0x1814;
    pub const HOSTS_INFO: u16 = 0x1815;
    pub const REPROG_CONTROLS_V4: u16 = 0x1B04;
    pub const WIRELESS_DEVICE_STATUS: u16 = 0x1D4B;
    pub const SMART_SHIFT: u16 = 0x2110;
    pub const SMART_SHIFT_ENHANCED: u16 = 0x2111;
    pub const HIRES_WHEEL: u16 = 0x2121;
    pub const THUMB_WHEEL: u16 = 0x2150;
    pub const ADJUSTABLE_DPI: u16 = 0x2201;
    pub const POINTER_SPEED: u16 = 0x2205;
}

const FEATURE_NAMES: &[(u16, &str)] = &[
    (f::ROOT, "ROOT"),
    (f::FEATURE_SET, "FEATURE_SET"),
    (f::DEVICE_FW_VERSION, "DEVICE_FW_VERSION"),
    (f::DEVICE_NAME, "DEVICE_NAME"),
    (f::RESET, "RESET"),
    (f::BATTERY_STATUS, "BATTERY_STATUS"),
    (f::BATTERY_VOLTAGE, "BATTERY_VOLTAGE"),
    (f::UNIFIED_BATTERY, "UNIFIED_BATTERY"),
    (f::CHANGE_HOST, "CHANGE_HOST"),
    (f::HOSTS_INFO, "HOSTS_INFO"),
    (f::REPROG_CONTROLS_V4, "REPROG_CONTROLS_V4"),
    (f::WIRELESS_DEVICE_STATUS, "WIRELESS_DEVICE_STATUS"),
    (f::SMART_SHIFT, "SMART_SHIFT"),
    (f::SMART_SHIFT_ENHANCED, "SMART_SHIFT_ENHANCED"),
    (f::HIRES_WHEEL, "HIRES_WHEEL"),
    (f::THUMB_WHEEL, "THUMB_WHEEL"),
    (f::ADJUSTABLE_DPI, "ADJUSTABLE_DPI"),
    (f::POINTER_SPEED, "POINTER_SPEED"),
];

/// Symbolic name of a feature we know ("ADJUSTABLE_DPI").
pub fn feature_name(id: u16) -> Option<&'static str> {
    FEATURE_NAMES
        .iter()
        .find(|(i, _)| *i == id)
        .map(|(_, n)| *n)
}

/// Name for the API's feature list: the symbolic name, else `0x1d4b` (as Python's
/// `f"{id:#06x}"`).
pub fn feature_label(id: u16) -> String {
    feature_name(id)
        .map(str::to_string)
        .unwrap_or_else(|| format!("{id:#06x}"))
}

/// HID++ 2.0 error names (protocol terms carried by errors; not translated).
pub fn error_20_name(code: u8) -> &'static str {
    match code {
        0 => "no error",
        1 => "unknown",
        2 => "invalid argument",
        3 => "out of range",
        4 => "hardware error",
        5 => "logitech internal",
        6 => "invalid feature index",
        7 => "invalid function id",
        8 => "busy",
        9 => "unsupported",
        _ => "?",
    }
}

/// HID++ 1.0 error names (receivers).
pub fn error_10_name(code: u8) -> &'static str {
    match code {
        0x01 => "invalid sub id",
        0x02 => "invalid address",
        0x03 => "invalid value",
        0x04 => "connection failed",
        0x05 => "too many devices",
        0x06 => "already exists",
        0x07 => "busy",
        0x08 => "unknown device",
        0x09 => "resource error",
        0x0A => "request unavailable",
        0x0B => "invalid param value",
        0x0C => "wrong pin code",
        _ => "?",
    }
}

/// Reprogrammable control ids the mice commonly expose, with English names marked for
/// translation (whoever shows them translates them with `tr!`).
pub const CONTROL_NAMES: &[(u16, &str)] = &[
    (0x0050, n_!("Left button")),
    (0x0051, n_!("Right button")),
    (0x0052, n_!("Middle button")),
    (0x0053, n_!("Back button")),
    (0x0056, n_!("Forward button")),
    (0x0059, n_!("Scroll left button")),
    (0x005B, n_!("Scroll right button")),
    (0x005D, n_!("Top button")),
    (0x00C3, n_!("Gesture button")),
    (0x00C4, n_!("Wheel mode button")),
    (0x00D7, n_!("Switch computer button")),
    (0x00FD, n_!("DPI button")),
    (0x01A0, n_!("Action button")),
];

/// English name of a control we know.
pub fn control_name_en(cid: u16) -> Option<&'static str> {
    CONTROL_NAMES
        .iter()
        .find(|(c, _)| *c == cid)
        .map(|(_, n)| *n)
}

// Bits in getCidInfo flags (byte 4).
pub const CID_FLAG_MOUSE: u8 = 0x01;
pub const CID_FLAG_REPROG: u8 = 0x10;
pub const CID_FLAG_DIVERTABLE: u8 = 0x20;
pub const CID_FLAG_PERSIST: u8 = 0x40;
pub const CID_FLAG_VIRTUAL: u8 = 0x80;
// Additional flags (byte 8).
pub const CID_FLAG2_RAWXY: u8 = 0x01;

// Bits for setCidReporting.
pub const REPORT_DIVERT: u8 = 0x01;
pub const REPORT_DVALID: u8 = 0x02;
pub const REPORT_PERSIST: u8 = 0x04;
pub const REPORT_PVALID: u8 = 0x08;
pub const REPORT_RAWXY: u8 = 0x10;
pub const REPORT_RVALID: u8 = 0x20;
