//! Windows: `SendInput` with virtual-key codes.
//!
//! Each press, release or scroll is one `SendInput` call, so it is not interleaved with
//! other input. Keys Windows has no virtual key for (brightness, mic mute, zoom) are
//! skipped with a log line.

use std::io;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, SendInput,
};

use crate::Backend;
use crate::keys::{BTN_EXTRA, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, BTN_SIDE};

const WHEEL_DELTA: i32 = 120;
const XBUTTON1: u32 = 1;
const XBUTTON2: u32 = 2;

/// Evdev code -> (virtual key, extended key flag).
fn vk(code: u16) -> Option<(u16, bool)> {
    let letters = "QWERTYUIOPASDFGHJKLZXCVBNM";
    let evdev_letters: [u16; 26] = [
        16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 30, 31, 32, 33, 34, 35, 36, 37, 38, 44, 45, 46, 47,
        48, 49, 50,
    ];
    if let Some(i) = evdev_letters.iter().position(|&c| c == code) {
        return Some((letters.as_bytes()[i] as u16, false));
    }
    let v = match code {
        2..=10 => (0x31 + (code - 2), false),      // 1..9
        11 => (0x30, false),                       // 0
        1 => (0x1B, false),                        // Esc
        12 => (0xBD, false),                       // VK_OEM_MINUS
        13 => (0xBB, false),                       // VK_OEM_PLUS (the = key)
        14 => (0x08, false),                       // Backspace
        15 => (0x09, false),                       // Tab
        26 => (0xDB, false),                       // VK_OEM_4 [
        27 => (0xDD, false),                       // VK_OEM_6 ]
        28 => (0x0D, false),                       // Enter
        29 => (0xA2, false),                       // VK_LCONTROL
        39 => (0xBA, false),                       // VK_OEM_1 ;
        40 => (0xDE, false),                       // VK_OEM_7 '
        41 => (0xC0, false),                       // VK_OEM_3 `
        42 => (0xA0, false),                       // VK_LSHIFT
        43 => (0xDC, false),                       // VK_OEM_5 backslash
        51 => (0xBC, false),                       // VK_OEM_COMMA
        52 => (0xBE, false),                       // VK_OEM_PERIOD
        53 => (0xBF, false),                       // VK_OEM_2 /
        54 => (0xA1, false),                       // VK_RSHIFT
        56 => (0xA4, false),                       // VK_LMENU
        57 => (0x20, false),                       // Space
        58 => (0x14, false),                       // Caps Lock
        59..=68 => (0x70 + (code - 59), false),    // F1..F10
        87 => (0x7A, false),                       // F11
        88 => (0x7B, false),                       // F12
        97 => (0xA3, true),                        // VK_RCONTROL
        99 => (0x2C, true),                        // VK_SNAPSHOT
        100 => (0xA5, true),                       // VK_RMENU (AltGr)
        102 => (0x24, true),                       // Home
        103 => (0x26, true),                       // Up
        104 => (0x21, true),                       // Page Up
        105 => (0x25, true),                       // Left
        106 => (0x27, true),                       // Right
        107 => (0x23, true),                       // End
        108 => (0x28, true),                       // Down
        109 => (0x22, true),                       // Page Down
        110 => (0x2D, true),                       // Insert
        111 => (0x2E, true),                       // Delete
        113 => (0xAD, false),                      // VK_VOLUME_MUTE
        114 => (0xAE, false),                      // VK_VOLUME_DOWN
        115 => (0xAF, false),                      // VK_VOLUME_UP
        119 => (0x13, false),                      // Pause
        125 => (0x5B, true),                       // VK_LWIN
        126 => (0x5C, true),                       // VK_RWIN
        127 => (0x5D, true),                       // VK_APPS
        140 => (0xB7, false),                      // VK_LAUNCH_APP2 (calculator)
        158 => (0xA6, false),                      // VK_BROWSER_BACK
        159 => (0xA7, false),                      // VK_BROWSER_FORWARD
        163 => (0xB0, false),                      // VK_MEDIA_NEXT_TRACK
        164 => (0xB3, false),                      // VK_MEDIA_PLAY_PAUSE
        165 => (0xB1, false),                      // VK_MEDIA_PREV_TRACK
        166 => (0xB2, false),                      // VK_MEDIA_STOP
        172 => (0xAC, false),                      // VK_BROWSER_HOME
        183..=194 => (0x7C + (code - 183), false), // F13..F24
        217 => (0xAA, false),                      // VK_BROWSER_SEARCH
        _ => return None,
    };
    Some(v)
}

fn key_input(code: u16, down: bool) -> Option<INPUT> {
    let Some((vk, extended)) = vk(code) else {
        log::info!("virtual input: no Windows key for evdev code {code}");
        return None;
    };
    let mut flags = if down { 0 } else { KEYEVENTF_KEYUP };
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    Some(INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    })
}

fn mouse_input(flags: u32, data: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn button_input(code: u16, down: bool) -> Option<INPUT> {
    let (flags, data) = match (code, down) {
        (BTN_LEFT, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (BTN_LEFT, false) => (MOUSEEVENTF_LEFTUP, 0),
        (BTN_RIGHT, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (BTN_RIGHT, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (BTN_MIDDLE, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (BTN_MIDDLE, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (BTN_SIDE, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
        (BTN_SIDE, false) => (MOUSEEVENTF_XUP, XBUTTON1),
        (BTN_EXTRA, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
        (BTN_EXTRA, false) => (MOUSEEVENTF_XUP, XBUTTON2),
        _ => return None,
    };
    Some(mouse_input(flags, data))
}

fn send(inputs: &[INPUT]) -> io::Result<()> {
    if inputs.is_empty() {
        return Ok(());
    }
    // SAFETY: inputs is a valid slice of initialised INPUT structures.
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if sent as usize != inputs.len() {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub struct SendInputBackend;

impl SendInputBackend {
    fn keys(codes: &[u16], down: bool) -> Vec<INPUT> {
        codes
            .iter()
            .filter_map(|&c| button_input(c, down).or_else(|| key_input(c, down)))
            .collect()
    }
}

impl Backend for SendInputBackend {
    fn press(&mut self, codes: &[u16]) -> io::Result<()> {
        send(&Self::keys(codes, true))
    }

    fn release(&mut self, codes: &[u16]) -> io::Result<()> {
        send(&Self::keys(codes, false))
    }

    fn scroll(&mut self, vertical: i32, horizontal: i32) -> io::Result<()> {
        let mut inputs = Vec::new();
        if vertical != 0 {
            inputs.push(mouse_input(
                MOUSEEVENTF_WHEEL,
                (vertical * WHEEL_DELTA) as u32,
            ));
        }
        if horizontal != 0 {
            inputs.push(mouse_input(
                MOUSEEVENTF_HWHEEL,
                (horizontal * WHEEL_DELTA) as u32,
            ));
        }
        send(&inputs)
    }
}
