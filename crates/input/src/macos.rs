//! macOS: Quartz events (`CGEventPost`). Posting needs the Accessibility permission
//! (System Settings → Privacy & Security → Accessibility); without it macOS drops the
//! events silently, which the service reports in `status.permissions`.
//!
//! Modifier state is tracked here and set on every key event, since posted events do
//! not inherit it reliably. Media and brightness keys need NX system events and are not
//! sent (logged instead). Untested on hardware in this port: written against Apple's
//! documented C API.

use std::ffi::c_void;
use std::io;

use crate::Backend;
use crate::keys::{
    BTN_EXTRA, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, BTN_SIDE, KEY_ALT, KEY_ALTGR, KEY_CTRL, KEY_META,
    KEY_RIGHTCTRL, KEY_RIGHTMETA, KEY_RIGHTSHIFT, KEY_SHIFT,
};

type CGEventRef = *mut c_void;
type CGEventSourceRef = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

const K_CG_EVENT_SOURCE_STATE_HID_SYSTEM: i32 = 1;
const K_CG_HID_EVENT_TAP: u32 = 0;
const K_CG_SCROLL_EVENT_UNIT_LINE: u32 = 1;
const K_CG_MOUSE_EVENT_BUTTON_NUMBER: u32 = 3;

const LEFT_DOWN: u32 = 1;
const LEFT_UP: u32 = 2;
const RIGHT_DOWN: u32 = 3;
const RIGHT_UP: u32 = 4;
const OTHER_DOWN: u32 = 25;
const OTHER_UP: u32 = 26;

const FLAG_SHIFT: u64 = 0x0002_0000;
const FLAG_CONTROL: u64 = 0x0004_0000;
const FLAG_ALTERNATE: u64 = 0x0008_0000;
const FLAG_COMMAND: u64 = 0x0010_0000;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceCreate(state_id: i32) -> CGEventSourceRef;
    fn CGEventCreateKeyboardEvent(
        source: CGEventSourceRef,
        keycode: u16,
        key_down: bool,
    ) -> CGEventRef;
    fn CGEventSetFlags(event: CGEventRef, flags: u64);
    fn CGEventPost(tap: u32, event: CGEventRef);
    fn CGEventCreate(source: CGEventSourceRef) -> CGEventRef;
    fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
    fn CGEventCreateMouseEvent(
        source: CGEventSourceRef,
        mouse_type: u32,
        cursor: CGPoint,
        button: u32,
    ) -> CGEventRef;
    fn CGEventCreateScrollWheelEvent2(
        source: CGEventSourceRef,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> CGEventRef;
    fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

/// Evdev code -> macOS virtual key code (kVK_*).
fn keycode(code: u16) -> Option<u16> {
    Some(match code {
        30 => 0x00, // A
        31 => 0x01, // S
        32 => 0x02, // D
        33 => 0x03, // F
        35 => 0x04, // H
        34 => 0x05, // G
        44 => 0x06, // Z
        45 => 0x07, // X
        46 => 0x08, // C
        47 => 0x09, // V
        48 => 0x0B, // B
        16 => 0x0C, // Q
        17 => 0x0D, // W
        18 => 0x0E, // E
        19 => 0x0F, // R
        21 => 0x10, // Y
        20 => 0x11, // T
        2 => 0x12,  // 1
        3 => 0x13,  // 2
        4 => 0x14,  // 3
        5 => 0x15,  // 4
        7 => 0x16,  // 6
        6 => 0x17,  // 5
        13 => 0x18, // =
        10 => 0x19, // 9
        8 => 0x1A,  // 7
        12 => 0x1B, // -
        9 => 0x1C,  // 8
        11 => 0x1D, // 0
        27 => 0x1E, // ]
        24 => 0x1F, // O
        22 => 0x20, // U
        26 => 0x21, // [
        23 => 0x22, // I
        25 => 0x23, // P
        28 => 0x24, // Return
        38 => 0x25, // L
        36 => 0x26, // J
        40 => 0x27, // '
        37 => 0x28, // K
        39 => 0x29, // ;
        43 => 0x2A, // backslash
        51 => 0x2B, // ,
        53 => 0x2C, // /
        49 => 0x2D, // N
        50 => 0x2E, // M
        52 => 0x2F, // .
        15 => 0x30, // Tab
        57 => 0x31, // Space
        41 => 0x32, // `
        14 => 0x33, // Delete (backspace)
        1 => 0x35,  // Escape
        KEY_RIGHTMETA => 0x36,
        KEY_META => 0x37, // Command
        KEY_SHIFT => 0x38,
        58 => 0x39,      // Caps Lock
        KEY_ALT => 0x3A, // Option
        KEY_CTRL => 0x3B,
        KEY_RIGHTSHIFT => 0x3C,
        KEY_ALTGR => 0x3D, // right Option
        KEY_RIGHTCTRL => 0x3E,
        187 => 0x40, // F17
        115 => 0x48, // volume up
        114 => 0x49, // volume down
        113 => 0x4A, // mute
        188 => 0x4F, // F18
        189 => 0x50, // F19
        190 => 0x5A, // F20
        63 => 0x60,  // F5
        64 => 0x61,  // F6
        65 => 0x62,  // F7
        61 => 0x63,  // F3
        66 => 0x64,  // F8
        67 => 0x65,  // F9
        87 => 0x67,  // F11
        183 => 0x69, // F13
        186 => 0x6A, // F16
        184 => 0x6B, // F14
        68 => 0x6D,  // F10
        88 => 0x6F,  // F12
        185 => 0x71, // F15
        102 => 0x73, // Home
        104 => 0x74, // Page Up
        111 => 0x75, // Forward Delete
        62 => 0x76,  // F4
        107 => 0x77, // End
        60 => 0x78,  // F2
        109 => 0x79, // Page Down
        59 => 0x7A,  // F1
        105 => 0x7B, // Left
        106 => 0x7C, // Right
        108 => 0x7D, // Down
        103 => 0x7E, // Up
        _ => return None,
    })
}

fn modifier_flag(code: u16) -> u64 {
    match code {
        KEY_SHIFT | KEY_RIGHTSHIFT => FLAG_SHIFT,
        KEY_CTRL | KEY_RIGHTCTRL => FLAG_CONTROL,
        KEY_ALT | KEY_ALTGR => FLAG_ALTERNATE,
        KEY_META | KEY_RIGHTMETA => FLAG_COMMAND,
        _ => 0,
    }
}

pub struct CgEventBackend {
    source: CGEventSourceRef,
    /// Modifier keys held now (evdev codes).
    modifiers: Vec<u16>,
}

// SAFETY: the event source is a CoreFoundation object, which may be used from any
// thread; VirtualInput is used by one thread at a time (behind the service's lock).
unsafe impl Send for CgEventBackend {}

impl CgEventBackend {
    pub fn new() -> io::Result<Self> {
        // SAFETY: plain constructor; null is checked below.
        let source = unsafe { CGEventSourceCreate(K_CG_EVENT_SOURCE_STATE_HID_SYSTEM) };
        if source.is_null() {
            return Err(io::Error::other("CGEventSourceCreate failed"));
        }
        Ok(CgEventBackend {
            source,
            modifiers: Vec::new(),
        })
    }

    fn flags(&self) -> u64 {
        self.modifiers
            .iter()
            .fold(0, |acc, &c| acc | modifier_flag(c))
    }

    fn post(event: CGEventRef) -> io::Result<()> {
        if event.is_null() {
            return Err(io::Error::other("could not create the event"));
        }
        // SAFETY: event is a valid event we own; it is released after posting.
        unsafe {
            CGEventPost(K_CG_HID_EVENT_TAP, event);
            CFRelease(event as *const c_void);
        }
        Ok(())
    }

    fn cursor(&self) -> CGPoint {
        // SAFETY: CGEventCreate(source) returns a new event (or null), released here.
        unsafe {
            let e = CGEventCreate(self.source);
            if e.is_null() {
                return CGPoint { x: 0.0, y: 0.0 };
            }
            let p = CGEventGetLocation(e);
            CFRelease(e as *const c_void);
            p
        }
    }

    fn button(&self, code: u16, down: bool) -> io::Result<()> {
        let (kind, number) = match (code, down) {
            (BTN_LEFT, true) => (LEFT_DOWN, 0),
            (BTN_LEFT, false) => (LEFT_UP, 0),
            (BTN_RIGHT, true) => (RIGHT_DOWN, 1),
            (BTN_RIGHT, false) => (RIGHT_UP, 1),
            (BTN_MIDDLE, d) => (if d { OTHER_DOWN } else { OTHER_UP }, 2),
            (BTN_SIDE, d) => (if d { OTHER_DOWN } else { OTHER_UP }, 3),
            (BTN_EXTRA, d) => (if d { OTHER_DOWN } else { OTHER_UP }, 4),
            _ => return Ok(()),
        };
        let at = self.cursor();
        // SAFETY: creates a new event (checked by post) and sets one of its fields.
        let e = unsafe { CGEventCreateMouseEvent(self.source, kind, at, number) };
        if !e.is_null() {
            // SAFETY: e is a valid event.
            unsafe {
                CGEventSetIntegerValueField(e, K_CG_MOUSE_EVENT_BUTTON_NUMBER, i64::from(number))
            };
        }
        Self::post(e)
    }

    fn key(&mut self, code: u16, down: bool) -> io::Result<()> {
        if (BTN_LEFT..=BTN_EXTRA).contains(&code) {
            return self.button(code, down);
        }
        if modifier_flag(code) != 0 {
            if down {
                self.modifiers.push(code);
            } else if let Some(i) = self.modifiers.iter().rposition(|&c| c == code) {
                self.modifiers.remove(i);
            }
        }
        let Some(kc) = keycode(code) else {
            log::info!("virtual input: no macOS key for evdev code {code}");
            return Ok(());
        };
        // SAFETY: creates a new keyboard event (checked by post) and sets its flags.
        let e = unsafe { CGEventCreateKeyboardEvent(self.source, kc, down) };
        if !e.is_null() {
            // SAFETY: e is a valid event.
            unsafe { CGEventSetFlags(e, self.flags()) };
        }
        Self::post(e)
    }
}

impl Backend for CgEventBackend {
    fn press(&mut self, codes: &[u16]) -> io::Result<()> {
        for &c in codes {
            self.key(c, true)?;
        }
        Ok(())
    }

    fn release(&mut self, codes: &[u16]) -> io::Result<()> {
        for &c in codes {
            self.key(c, false)?;
        }
        Ok(())
    }

    fn scroll(&mut self, vertical: i32, horizontal: i32) -> io::Result<()> {
        // wheel1: positive scrolls up; wheel2: positive scrolls left.
        // SAFETY: creates a new event, checked and released by post.
        let e = unsafe {
            CGEventCreateScrollWheelEvent2(
                self.source,
                K_CG_SCROLL_EVENT_UNIT_LINE,
                2,
                vertical,
                -horizontal,
                0,
            )
        };
        Self::post(e)
    }
}

impl Drop for CgEventBackend {
    fn drop(&mut self) {
        // SAFETY: source was created by CGEventSourceCreate and is released once.
        unsafe { CFRelease(self.source as *const c_void) };
    }
}
