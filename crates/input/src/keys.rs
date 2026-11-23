//! Platform-neutral key names and combos ("Ctrl+Shift+T", "Meta+W", "Cmd+C").
//!
//! Keys are identified by their Linux evdev code everywhere (the Python implementation's
//! table, kept as is); each backend maps the code to its own (Windows virtual key, macOS
//! key code).

use glisa_i18n::tr;

pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;
pub const BTN_SIDE: u16 = 0x113;
pub const BTN_EXTRA: u16 = 0x114;

/// Key names (upper case) and their evdev codes.
#[rustfmt::skip]
pub const KEYS: &[(&str, u16)] = &[
    ("ESC", 1), ("1", 2), ("2", 3), ("3", 4), ("4", 5), ("5", 6), ("6", 7), ("7", 8),
    ("8", 9), ("9", 10), ("0", 11), ("MINUS", 12), ("EQUAL", 13), ("BACKSPACE", 14),
    ("TAB", 15), ("Q", 16), ("W", 17), ("E", 18), ("R", 19), ("T", 20), ("Y", 21),
    ("U", 22), ("I", 23), ("O", 24), ("P", 25), ("LEFTBRACE", 26), ("RIGHTBRACE", 27),
    ("ENTER", 28), ("CTRL", 29), ("A", 30), ("S", 31), ("D", 32), ("F", 33), ("G", 34),
    ("H", 35), ("J", 36), ("K", 37), ("L", 38), ("SEMICOLON", 39), ("APOSTROPHE", 40),
    ("GRAVE", 41), ("SHIFT", 42), ("BACKSLASH", 43), ("Z", 44), ("X", 45), ("C", 46),
    ("V", 47), ("B", 48), ("N", 49), ("M", 50), ("COMMA", 51), ("DOT", 52), ("SLASH", 53),
    ("RIGHTSHIFT", 54), ("ALT", 56), ("SPACE", 57), ("CAPSLOCK", 58), ("F1", 59),
    ("F2", 60), ("F3", 61), ("F4", 62), ("F5", 63), ("F6", 64), ("F7", 65), ("F8", 66),
    ("F9", 67), ("F10", 68), ("F11", 87), ("F12", 88), ("RIGHTCTRL", 97), ("PRINT", 99),
    ("ALTGR", 100), ("HOME", 102), ("UP", 103), ("PAGEUP", 104), ("LEFT", 105),
    ("RIGHT", 106), ("END", 107), ("DOWN", 108), ("PAGEDOWN", 109), ("INSERT", 110),
    ("DELETE", 111), ("MUTE", 113), ("VOLUMEDOWN", 114), ("VOLUMEUP", 115), ("PAUSE", 119),
    ("META", 125), ("RIGHTMETA", 126), ("MENU", 127), ("CALC", 140), ("BACK", 158),
    ("FORWARD", 159), ("NEXTSONG", 163), ("PLAYPAUSE", 164), ("PREVIOUSSONG", 165),
    ("STOPCD", 166), ("HOMEPAGE", 172), ("F13", 183), ("F14", 184), ("F15", 185),
    ("F16", 186), ("F17", 187), ("F18", 188), ("F19", 189), ("F20", 190), ("F21", 191),
    ("F22", 192), ("F23", 193), ("F24", 194), ("SEARCH", 217), ("BRIGHTNESSDOWN", 224),
    ("BRIGHTNESSUP", 225), ("MICMUTE", 248), ("ZOOMIN", 418), ("ZOOMOUT", 419),
    ("BTN_LEFT", BTN_LEFT), ("BTN_RIGHT", BTN_RIGHT), ("BTN_MIDDLE", BTN_MIDDLE),
    ("BTN_SIDE", BTN_SIDE), ("BTN_EXTRA", BTN_EXTRA),
];

/// Other names people type. "Cmd"/"Command" (macOS) and "Win"/"Super" are Meta;
/// "Option" is Alt.
#[rustfmt::skip]
pub const ALIASES: &[(&str, &str)] = &[
    ("CONTROL", "CTRL"), ("LEFTCTRL", "CTRL"), ("SUPER", "META"), ("WIN", "META"),
    ("LEFTMETA", "META"), ("LEFTALT", "ALT"), ("LEFTSHIFT", "SHIFT"), ("RETURN", "ENTER"),
    ("DEL", "DELETE"), ("PGUP", "PAGEUP"), ("PGDOWN", "PAGEDOWN"), ("ESCAPE", "ESC"),
    ("PLUS", "EQUAL"), ("PERIOD", "DOT"), ("CMD", "META"), ("COMMAND", "META"),
    ("OPTION", "ALT"), ("OPT", "ALT"),
];

pub const MODIFIERS: &[&str] = &[
    "CTRL",
    "SHIFT",
    "ALT",
    "META",
    "ALTGR",
    "RIGHTCTRL",
    "RIGHTSHIFT",
    "RIGHTMETA",
];

pub const KEY_CTRL: u16 = 29;
pub const KEY_SHIFT: u16 = 42;
pub const KEY_ALT: u16 = 56;
pub const KEY_META: u16 = 125;
pub const KEY_RIGHTCTRL: u16 = 97;
pub const KEY_RIGHTSHIFT: u16 = 54;
pub const KEY_ALTGR: u16 = 100;
pub const KEY_RIGHTMETA: u16 = 126;

/// Evdev code of a key name (any case, aliases accepted).
pub fn code_of(name: &str) -> Option<u16> {
    let upper = name.to_uppercase();
    let canonical = ALIASES
        .iter()
        .find(|(a, _)| *a == upper)
        .map_or(upper.as_str(), |(_, k)| *k);
    KEYS.iter().find(|(k, _)| *k == canonical).map(|(_, c)| *c)
}

/// Name of an evdev code ("CTRL").
pub fn name_of(code: u16) -> Option<&'static str> {
    KEYS.iter().find(|(_, c)| *c == code).map(|(k, _)| *k)
}

pub fn is_modifier(code: u16) -> bool {
    matches!(
        code,
        KEY_CTRL
            | KEY_SHIFT
            | KEY_ALT
            | KEY_META
            | KEY_RIGHTCTRL
            | KEY_RIGHTSHIFT
            | KEY_ALTGR
            | KEY_RIGHTMETA
    )
}

pub fn is_mouse_button(code: u16) -> bool {
    (BTN_LEFT..=BTN_EXTRA).contains(&code)
}

/// `"Ctrl+Shift+T"` -> `[29, 42, 20]`. The error is a message in this thread's
/// language.
pub fn parse_combo(combo: &str) -> Result<Vec<u16>, String> {
    let mut codes = Vec::new();
    let compact: String = combo.chars().filter(|c| *c != ' ').collect();
    for part in compact.split('+') {
        if part.is_empty() {
            continue;
        }
        match code_of(part) {
            Some(c) => codes.push(c),
            None => return Err(tr!("Unknown key: {key}", key = part)),
        }
    }
    if codes.is_empty() {
        return Err(tr!("Empty shortcut"));
    }
    Ok(codes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos() {
        assert_eq!(parse_combo("Ctrl+Shift+T"), Ok(vec![29, 42, 20]));
        assert_eq!(parse_combo("super + d"), Ok(vec![125, 32]));
        assert_eq!(parse_combo("Cmd+C"), Ok(vec![125, 46]));
        assert_eq!(parse_combo("Ctrl+Plus"), Ok(vec![29, 13]));
        assert_eq!(parse_combo("Meta"), Ok(vec![125]));
        assert!(parse_combo("Ctrl+Nope").is_err());
        assert!(parse_combo(" + ").is_err());
        glisa_i18n::with_language(Some("en"), || {
            assert_eq!(
                parse_combo("Ctrl+Nope"),
                Err("Unknown key: Nope".to_string())
            );
            assert_eq!(parse_combo(""), Err("Empty shortcut".to_string()));
        });
    }

    #[test]
    fn names() {
        assert_eq!(name_of(29), Some("CTRL"));
        assert!(is_modifier(KEY_META) && !is_modifier(20));
        assert!(is_mouse_button(BTN_SIDE) && !is_mouse_button(BTN_EXTRA + 1));
    }
}
