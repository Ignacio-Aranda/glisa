//! High-level wrappers for the HID++ 2.0 features Glisa uses.
//!
//! Answers are at least 16 bytes long (see [`crate::Channel::request`]), so the parsers
//! index them directly.

use std::time::Duration;

use glisa_i18n::tr;

use crate::constants::*;
use crate::device::Device;
use crate::error::{Error, Result};

fn be16(r: &[u8], i: usize) -> u16 {
    u16::from_be_bytes([r[i], r[i + 1]])
}

// ------------------------------------------------------------------ identity

/// The name the device reports (0x0005).
pub fn device_name(dev: &Device) -> Result<String> {
    let length = dev.call(f::DEVICE_NAME, 0, &[])?[0] as usize;
    let mut name: Vec<u8> = Vec::new();
    while name.len() < length {
        let chunk = dev.call(f::DEVICE_NAME, 1, &[name.len() as u8])?;
        let take = (length - name.len()).min(chunk.len());
        let chunk = &chunk[..take];
        if chunk.iter().all(|&b| b == 0) {
            break;
        }
        name.extend_from_slice(chunk);
    }
    while name.last() == Some(&0) {
        name.pop();
    }
    Ok(String::from_utf8_lossy(&name).into_owned())
}

/// "keyboard", "mouse", "trackball"… (0x0005 function 2); "unknown" when not known.
pub fn device_type(dev: &Device) -> &'static str {
    match dev.call(f::DEVICE_NAME, 2, &[]) {
        Ok(r) => device_type_name(r[0]),
        Err(_) => "unknown",
    }
}

pub fn device_type_name(code: u8) -> &'static str {
    match code {
        0 => "keyboard",
        1 => "remote",
        2 => "numpad",
        3 => "mouse",
        4 => "trackpad",
        5 => "trackball",
        6 => "presenter",
        7 => "receiver",
        _ => "unknown",
    }
}

/// Firmware entities, e.g. `["FW RBM16.01.B0017"]` (0x0003).
pub fn firmware(dev: &Device) -> Result<Vec<String>> {
    let count = dev.call(f::DEVICE_FW_VERSION, 0, &[])?[0];
    let mut out = Vec::new();
    for i in 0..count {
        let r = dev.call(f::DEVICE_FW_VERSION, 1, &[i])?;
        let kind = match r[0] {
            0 => "FW".to_string(),
            1 => "Bootloader".to_string(),
            2 => "HW".to_string(),
            n => format!("#{n}"),
        };
        let prefix: String = r[1..4]
            .iter()
            .map(|&b| if b.is_ascii() { b as char } else { '\u{FFFD}' })
            .collect::<String>()
            .trim_matches('\0')
            .to_string();
        out.push(format!(
            "{kind} {prefix}{:02X}.{:02X}.B{:02X}{:02X}",
            r[4], r[5], r[6], r[7]
        ));
    }
    Ok(out)
}

/// Product ids the device reports for each transport (0x0003 function 0), which name
/// the model whatever way it is connected.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceIds {
    pub unit_id: u32,
    /// Bluetooth (classic) product id.
    pub btid: Option<u16>,
    /// Bluetooth Low Energy product id.
    pub btleid: Option<u16>,
    /// Wireless product id behind a Unifying/Lightspeed receiver.
    pub wpid: Option<u16>,
    /// USB (cable) product id.
    pub usbid: Option<u16>,
}

impl DeviceIds {
    pub fn all(&self) -> Vec<u16> {
        [self.btid, self.btleid, self.wpid, self.usbid]
            .into_iter()
            .flatten()
            .collect()
    }
}

pub fn device_ids(dev: &Device) -> Result<DeviceIds> {
    let r = dev.call(f::DEVICE_FW_VERSION, 0, &[])?;
    let unit_id = u32::from_be_bytes([r[1], r[2], r[3], r[4]]);
    let transport = r[6];
    let mut ids = DeviceIds {
        unit_id,
        ..DeviceIds::default()
    };
    let mut offset = 7;
    for (bit, slot) in [
        (0x01, &mut ids.btid),
        (0x02, &mut ids.btleid),
        (0x04, &mut ids.wpid),
        (0x08, &mut ids.usbid),
    ] {
        if transport & bit != 0 && offset + 1 < 13 {
            let pid = be16(&r, offset);
            *slot = (pid != 0).then_some(pid);
            offset += 2;
        }
    }
    Ok(ids)
}

// -------------------------------------------------------------------- battery

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Battery {
    pub percent: Option<u8>,
    /// critical / low / good / full / unknown
    pub level: &'static str,
    pub charging: bool,
    /// Protocol state, English: discharging / charging / charging slow / full / error.
    pub status: &'static str,
}

fn unified_battery_parse(r: &[u8]) -> Battery {
    let lvl = r[1];
    let level = if lvl & 8 != 0 {
        "full"
    } else if lvl & 4 != 0 {
        "good"
    } else if lvl & 2 != 0 {
        "low"
    } else if lvl & 1 != 0 {
        "critical"
    } else {
        "unknown"
    };
    let status = match r[2] {
        0 => "discharging",
        1 => "charging",
        2 => "charging slow",
        3 => "full",
        4 => "error",
        _ => "unknown",
    };
    Battery {
        percent: (r[0] != 0).then_some(r[0]),
        level,
        charging: matches!(r[2], 1 | 2),
        status,
    }
}

/// Battery state from 0x1004 (unified) or 0x1000; `None` without either.
pub fn battery(dev: &Device) -> Result<Option<Battery>> {
    if dev.has(f::UNIFIED_BATTERY) {
        return Ok(Some(unified_battery_parse(&dev.call(
            f::UNIFIED_BATTERY,
            1,
            &[],
        )?)));
    }
    if dev.has(f::BATTERY_STATUS) {
        let r = dev.call(f::BATTERY_STATUS, 0, &[])?;
        let status = match r[2] {
            0 => "discharging",
            1 | 2 => "charging",
            3 => "full",
            _ => "unknown",
        };
        let level = match r[0] {
            81.. => "full",
            31..=80 => "good",
            11..=30 => "low",
            _ => "critical",
        };
        return Ok(Some(Battery {
            percent: Some(r[0]),
            level,
            charging: matches!(r[2], 1 | 2),
            status,
        }));
    }
    Ok(None)
}

/// Battery notification payload (0x1004 function 0).
pub fn parse_battery_event(feature_id: u16, data: &[u8]) -> Option<Battery> {
    (feature_id == f::UNIFIED_BATTERY && data.len() >= 3).then(|| unified_battery_parse(data))
}

// ------------------------------------------------------------------------ DPI

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DpiInfo {
    pub current: u16,
    pub default: u16,
    pub choices: Vec<u16>,
    pub minimum: u16,
    pub maximum: u16,
    pub step: Option<u16>,
}

/// The DPI values a sensor accepts, and the current one (0x2201).
pub fn dpi_info(dev: &Device, sensor: u8) -> Result<DpiInfo> {
    // Answer: sensor, then up to 7 big-endian u16 (0-terminated). A value 0xE000|step
    // between two DPIs means "every `step` from the previous to the next".
    let r = dev.call(f::ADJUSTABLE_DPI, 1, &[sensor])?;
    let raw: Vec<u16> = (0..7)
        .map(|k| be16(&r, 1 + 2 * k))
        .take_while(|&v| v != 0)
        .collect();
    let mut choices: Vec<u16> = Vec::new();
    let mut step = None;
    let mut i = 0;
    while i < raw.len() {
        let v = raw[i];
        if v >> 13 == 0b111 && !choices.is_empty() && i + 1 < raw.len() {
            let s = v & 0x1FFF;
            let hi = raw[i + 1];
            if s > 0 {
                step = Some(s);
                let mut next = u32::from(*choices.last().unwrap_or(&0)) + u32::from(s);
                while next <= u32::from(hi) && choices.len() < 4096 {
                    choices.push(next as u16);
                    next += u32::from(s);
                }
            }
            i += 2;
            continue;
        }
        choices.push(v);
        i += 1;
    }
    if choices.is_empty() {
        return Err(Error::Malformed("no DPI values"));
    }
    let r = dev.call(f::ADJUSTABLE_DPI, 2, &[sensor])?;
    let current = be16(&r, 1);
    let mut default = be16(&r, 3);
    if default == 0 {
        default = current;
    }
    let minimum = *choices.iter().min().unwrap_or(&0);
    let maximum = *choices.iter().max().unwrap_or(&0);
    Ok(DpiInfo {
        current,
        default,
        choices,
        minimum,
        maximum,
        step,
    })
}

/// Sets the DPI; returns what the device says it uses now.
pub fn set_dpi(dev: &Device, dpi: u16, sensor: u8) -> Result<u16> {
    let [hi, lo] = dpi.to_be_bytes();
    let r = dev.call(f::ADJUSTABLE_DPI, 3, &[sensor, hi, lo])?;
    Ok(match be16(&r, 1) {
        0 => dpi,
        v => v,
    })
}

// ---------------------------------------------------------------------- wheel

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WheelInfo {
    pub multiplier: u8,
    pub has_invert: bool,
    pub has_ratchet_switch: bool,
    pub hires: bool,
    pub inverted: bool,
    pub diverted: bool,
}

pub fn wheel_info(dev: &Device) -> Result<WheelInfo> {
    let cap = dev.call(f::HIRES_WHEEL, 0, &[])?;
    let mode = dev.call(f::HIRES_WHEEL, 1, &[])?[0];
    Ok(WheelInfo {
        multiplier: cap[0],
        has_invert: cap[1] & 0x08 != 0,
        has_ratchet_switch: cap[1] & 0x04 != 0,
        hires: mode & 0x02 != 0,
        inverted: mode & 0x04 != 0,
        diverted: mode & 0x01 != 0,
    })
}

pub fn set_wheel_mode(dev: &Device, hires: bool, inverted: bool, diverted: bool) -> Result<()> {
    let mode = u8::from(diverted) | (u8::from(hires) << 1) | (u8::from(inverted) << 2);
    dev.call(f::HIRES_WHEEL, 2, &[mode]).map(|_| ())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmartShiftInfo {
    pub enhanced: bool,
    /// True = ratchet ("clicky"), false = free spin.
    pub ratchet: bool,
    /// 1..=255 auto-disengage; 255 = always ratchet.
    pub threshold: u8,
    pub torque: Option<u8>,
}

pub fn smartshift_info(dev: &Device) -> Result<Option<SmartShiftInfo>> {
    if dev.has(f::SMART_SHIFT_ENHANCED) {
        let r = dev.call(f::SMART_SHIFT_ENHANCED, 1, &[])?;
        return Ok(Some(SmartShiftInfo {
            enhanced: true,
            ratchet: r[0] == 2,
            threshold: r[1],
            torque: Some(r[2]),
        }));
    }
    if dev.has(f::SMART_SHIFT) {
        let r = dev.call(f::SMART_SHIFT, 0, &[])?;
        return Ok(Some(SmartShiftInfo {
            enhanced: false,
            ratchet: r[0] == 2,
            threshold: r[1],
            torque: None,
        }));
    }
    Ok(None)
}

pub fn set_smartshift(
    dev: &Device,
    ratchet: bool,
    threshold: i64,
    torque: Option<u8>,
) -> Result<()> {
    let mode = if ratchet { 2 } else { 1 };
    let threshold = threshold.clamp(1, 255) as u8;
    if dev.has(f::SMART_SHIFT_ENHANCED) {
        dev.call(
            f::SMART_SHIFT_ENHANCED,
            2,
            &[mode, threshold, torque.unwrap_or(0)],
        )
        .map(|_| ())
    } else {
        dev.call(f::SMART_SHIFT, 1, &[mode, threshold]).map(|_| ())
    }
}

// ---------------------------------------------------------------- change host

/// `(channel count, current channel 0-based)` (0x1814).
pub fn host_info(dev: &Device) -> Result<(u8, u8)> {
    let r = dev.call(f::CHANGE_HOST, 0, &[])?;
    Ok((r[0], r[1]))
}

/// Switches to another Easy-Switch channel. The device switches immediately and
/// usually never answers, so nothing is reported.
pub fn set_host(dev: &Device, host: u8) {
    let _ = dev.call_timeout(f::CHANGE_HOST, 1, &[host], Duration::from_millis(300));
}

/// A computer paired on one channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEntry {
    pub index: u8,
    /// None: unknown.
    pub paired: Option<bool>,
    pub name: String,
}

/// One entry per channel from 0x1815; best effort (never fails).
pub fn host_entries(dev: &Device, count: u8) -> Vec<HostEntry> {
    let mut entries: Vec<HostEntry> = (0..count)
        .map(|i| HostEntry {
            index: i,
            paired: None,
            name: String::new(),
        })
        .collect();
    if !dev.has(f::HOSTS_INFO) {
        return entries;
    }
    let _ = (|| -> Result<()> {
        let info = dev.call(f::HOSTS_INFO, 0, &[])?;
        let can_names = info[0] & 0x01 != 0;
        let reported = info[2];
        let n = if count == 0 { reported } else { count };
        let n = n.min(if reported == 0 { count } else { reported });
        for i in 0..n {
            let h = dev.call(f::HOSTS_INFO, 1, &[i])?;
            let Some(entry) = entries.get_mut(i as usize) else {
                break;
            };
            entry.paired = Some(h[1] != 0);
            let name_len = h[4] as usize;
            let mut name: Vec<u8> = Vec::new();
            while can_names && name.len() < name_len {
                let piece = dev.call(f::HOSTS_INFO, 3, &[i, name.len() as u8])?;
                let piece = &piece[2..16];
                let take = (name_len - name.len()).min(piece.len());
                if take == 0 {
                    break;
                }
                name.extend_from_slice(&piece[..take]);
            }
            entry.name = String::from_utf8_lossy(&name)
                .trim_matches(|c| c == '\0' || c == ' ')
                .to_string();
        }
        Ok(())
    })();
    entries
}

// ---------------------------------------------------------- reprog controls

/// Name of a control for people, in this thread's language.
pub fn control_name(cid: u16) -> String {
    match control_name_en(cid) {
        Some(name) => tr!(name),
        None => tr!("Control {cid}", cid = format!("{cid:#06x}")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Control {
    pub cid: u16,
    pub task: u16,
    pub flags: u8,
    pub pos: u8,
    pub group: u8,
    pub gmask: u8,
    pub flags2: u8,
    /// English (logs); people see [`control_name`].
    pub name: String,
    /// Control it is natively remapped to (0: itself).
    pub remap: u16,
    pub diverted: bool,
    pub raw_xy: bool,
    /// Controls it may be remapped to.
    pub group_cids: Vec<u16>,
}

impl Control {
    pub fn reprogrammable(&self) -> bool {
        self.flags & CID_FLAG_REPROG != 0
    }
    pub fn divertable(&self) -> bool {
        self.flags & CID_FLAG_DIVERTABLE != 0
    }
    pub fn supports_raw_xy(&self) -> bool {
        self.flags2 & CID_FLAG2_RAWXY != 0
    }
}

/// Every reprogrammable control with its current reporting (0x1B04).
pub fn controls(dev: &Device) -> Result<Vec<Control>> {
    let count = dev.call(f::REPROG_CONTROLS_V4, 0, &[])?[0];
    let mut out: Vec<Control> = Vec::with_capacity(count as usize);
    for i in 0..count {
        let r = dev.call(f::REPROG_CONTROLS_V4, 1, &[i])?;
        let cid = be16(&r, 0);
        let mut c = Control {
            cid,
            task: be16(&r, 2),
            flags: r[4],
            pos: r[5],
            group: r[6],
            gmask: r[7],
            flags2: r[8],
            name: control_name_en(cid)
                .map(str::to_string)
                .unwrap_or_else(|| format!("Control {cid:#06x}")),
            remap: 0,
            diverted: false,
            raw_xy: false,
            group_cids: Vec::new(),
        };
        let [hi, lo] = cid.to_be_bytes();
        if let Ok(rep) = dev.call(f::REPROG_CONTROLS_V4, 2, &[hi, lo]) {
            c.diverted = rep[2] & REPORT_DIVERT != 0;
            c.raw_xy = rep[2] & REPORT_RAWXY != 0;
            let remap = be16(&rep, 3);
            c.remap = if remap == cid { 0 } else { remap };
        }
        out.push(c);
    }
    // gmask: bit n means "may be remapped to controls of group n+1".
    let groups: Vec<(u8, u16)> = out.iter().map(|c| (c.group, c.cid)).collect();
    for c in &mut out {
        c.group_cids = groups
            .iter()
            .filter(|(g, cid)| *g != 0 && *g <= 8 && c.gmask & (1 << (g - 1)) != 0 && *cid != c.cid)
            .map(|(_, cid)| *cid)
            .collect();
    }
    Ok(out)
}

pub fn set_control_reporting(
    dev: &Device,
    cid: u16,
    divert: bool,
    raw_xy: bool,
    remap: u16,
) -> Result<()> {
    let mut flags = REPORT_DVALID | REPORT_RVALID;
    if divert {
        flags |= REPORT_DIVERT;
    }
    if raw_xy {
        flags |= REPORT_RAWXY;
    }
    let remap = if remap == 0 { cid } else { remap };
    let [hi, lo] = cid.to_be_bytes();
    let [rhi, rlo] = remap.to_be_bytes();
    dev.call(f::REPROG_CONTROLS_V4, 3, &[hi, lo, flags, rhi, rlo])
        .map(|_| ())
}

/// Diverted controls held down now (0x1B04 notification 0).
pub fn parse_diverted_buttons(data: &[u8]) -> Vec<u16> {
    data.chunks_exact(2)
        .take(4)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .filter(|&c| c != 0)
        .collect()
}

/// Raw movement while a raw-XY control is held (0x1B04 notification 1).
pub fn parse_raw_xy(data: &[u8]) -> (i16, i16) {
    if data.len() < 4 {
        return (0, 0);
    }
    (
        i16::from_be_bytes([data[0], data[1]]),
        i16::from_be_bytes([data[2], data[3]]),
    )
}
