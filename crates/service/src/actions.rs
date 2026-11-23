//! The action model (what a button does) and the ready-made actions per OS.
//!
//! An action is a plain JSON object, stored as is in the settings file:
//!
//! ```text
//! {"type": "default"}                        hardware default
//! {"type": "disabled"}
//! {"type": "mouse", "button": "middle"}      left/right/middle/back/forward
//! {"type": "keys", "combo": "Ctrl+C"}
//! {"type": "command", "cmd": "konsole"}
//! {"type": "scroll", "dir": "left"}          left/right/up/down
//! {"type": "dpi_cycle"}
//! {"type": "gesture", "tap": {…}, "up": {…}, "down": {…}, "left": {…}, "right": {…}}
//! ```
//!
//! Presets are per platform (the Python GUI had KDE's hard-coded): Linux keeps the
//! KDE-friendly shortcuts, Windows uses Win+Tab, Win+D, Ctrl+Win+arrows…, macOS uses
//! Cmd and Mission Control's Ctrl+arrows. The `presets` API method returns them.

use glisa_common::Platform;
use glisa_i18n::{n_, tr, trc};
use glisa_input::keys::{BTN_EXTRA, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, BTN_SIDE};
use serde_json::{Value, json};

/// Raw sensor counts before a movement counts as a gesture.
pub const GESTURE_THRESHOLD: i32 = 60;

/// `(name, control id, evdev button code)`
pub const MOUSE_BUTTONS: &[(&str, u16, u16)] = &[
    ("left", 0x0050, BTN_LEFT),
    ("right", 0x0051, BTN_RIGHT),
    ("middle", 0x0052, BTN_MIDDLE),
    ("back", 0x0053, BTN_SIDE),
    ("forward", 0x0056, BTN_EXTRA),
];

pub fn mouse_button_cid(name: &str) -> Option<u16> {
    MOUSE_BUTTONS.iter().find(|b| b.0 == name).map(|b| b.1)
}

pub fn mouse_button_code(name: &str) -> Option<u16> {
    MOUSE_BUTTONS.iter().find(|b| b.0 == name).map(|b| b.2)
}

/// Gesture slots in the order a directional pad reads.
pub const GESTURE_SLOTS: &[&str] = &["tap", "up", "down", "left", "right"];

/// `action["type"]` (`None` when missing or not text).
pub fn action_type(action: &Value) -> Option<&str> {
    action.get("type").and_then(Value::as_str)
}

fn str_field<'a>(action: &'a Value, key: &str) -> &'a str {
    action.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Python truthiness of a JSON value.
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// True for "no customisation": nothing, `{}` or `{"type": "default"}`.
pub fn is_default(action: Option<&Value>) -> bool {
    !truthy(action)
        || action
            .and_then(|a| a.get("type"))
            .is_none_or(|t| t == "default")
}

/// Whether the control must be diverted to software to run `action`.
pub fn needs_divert(action: Option<&Value>, native_targets: &[u16]) -> bool {
    let Some(a) = action.filter(|a| truthy(Some(a))) else {
        return false;
    };
    match action_type(a) {
        Some("default") => false,
        Some("mouse") => {
            mouse_button_cid(str_field(a, "button")).is_none_or(|c| !native_targets.contains(&c))
        }
        _ => true,
    }
}

/// The control a mouse-button action can be natively remapped to (0: none).
pub fn native_remap(action: Option<&Value>, native_targets: &[u16]) -> u16 {
    match action {
        Some(a) if action_type(a) == Some("mouse") => mouse_button_cid(str_field(a, "button"))
            .filter(|c| native_targets.contains(c))
            .unwrap_or(0),
        _ => 0,
    }
}

/// Checks an action; the error is a message in this thread's language.
pub fn validate(action: &Value) -> Result<(), String> {
    if !action.is_object() {
        return Err(tr!("Invalid parameters"));
    }
    match action_type(action) {
        Some("keys") => glisa_input::parse_combo(str_field(action, "combo")).map(|_| ()),
        Some("mouse") => match mouse_button_code(str_field(action, "button")) {
            Some(_) => Ok(()),
            None => Err(tr!("Unknown mouse button")),
        },
        Some("command") => {
            if str_field(action, "cmd").trim().is_empty() {
                Err(tr!("Empty command"))
            } else {
                Ok(())
            }
        }
        Some("gesture") => {
            for slot in GESTURE_SLOTS {
                let sub = action.get(*slot);
                if truthy(sub) {
                    validate(sub.expect("truthy"))?;
                }
            }
            Ok(())
        }
        Some("default" | "disabled" | "scroll" | "dpi_cycle") => Ok(()),
        other => {
            let t = match action.get("type") {
                None | Some(Value::Null) => "None".to_string(),
                Some(Value::String(_)) => other.unwrap_or_default().to_string(),
                Some(v) => v.to_string(),
            };
            Err(tr!("Unknown action type: {type}", type = t))
        }
    }
}

/// A sensible gesture set for this platform.
pub fn default_gestures() -> Value {
    default_gestures_for(glisa_common::platform())
}

pub fn default_gestures_for(platform: Platform) -> Value {
    match platform {
        Platform::Windows => json!({
            "type": "gesture",
            "tap": {"type": "keys", "combo": "Meta+Tab"},
            "up": {"type": "keys", "combo": "Meta+Tab"},
            "down": {"type": "keys", "combo": "Meta+D"},
            "left": {"type": "keys", "combo": "Ctrl+Meta+Left"},
            "right": {"type": "keys", "combo": "Ctrl+Meta+Right"},
        }),
        Platform::Macos => json!({
            "type": "gesture",
            "tap": {"type": "keys", "combo": "Ctrl+Up"},
            "up": {"type": "keys", "combo": "Ctrl+Up"},
            "down": {"type": "keys", "combo": "Ctrl+Down"},
            "left": {"type": "keys", "combo": "Ctrl+Left"},
            "right": {"type": "keys", "combo": "Ctrl+Right"},
        }),
        _ => json!({
            "type": "gesture",
            "tap": {"type": "keys", "combo": "Meta+W"},
            "up": {"type": "keys", "combo": "Meta+W"},
            "down": {"type": "keys", "combo": "Meta+D"},
            "left": {"type": "keys", "combo": "Ctrl+Meta+Left"},
            "right": {"type": "keys", "combo": "Ctrl+Meta+Right"},
        }),
    }
}

type Items = Vec<(&'static str, Value)>;

fn keys(combo: &str) -> Value {
    json!({"type": "keys", "combo": combo})
}

fn mouse_items() -> Items {
    vec![
        (
            n_!("Left click"),
            json!({"type": "mouse", "button": "left"}),
        ),
        (
            n_!("Right click"),
            json!({"type": "mouse", "button": "right"}),
        ),
        (
            n_!("Middle click"),
            json!({"type": "mouse", "button": "middle"}),
        ),
        ("Back", json!({"type": "mouse", "button": "back"})),
        ("Forward", json!({"type": "mouse", "button": "forward"})),
        (n_!("Scroll left"), json!({"type": "scroll", "dir": "left"})),
        (
            n_!("Scroll right"),
            json!({"type": "scroll", "dir": "right"}),
        ),
        (n_!("Change DPI"), json!({"type": "dpi_cycle"})),
    ]
}

fn browser_items(cmd: &str, reload: &str) -> Items {
    vec![
        (n_!("New tab"), keys(&format!("{cmd}+T"))),
        (n_!("Close tab"), keys(&format!("{cmd}+W"))),
        (n_!("Reopen closed tab"), keys(&format!("{cmd}+Shift+T"))),
        (n_!("Next tab"), keys("Ctrl+Tab")),
        (n_!("Previous tab"), keys("Ctrl+Shift+Tab")),
        (n_!("Reload page"), keys(reload)),
    ]
}

/// The catalogue for `platform`: `[(category, [(label, action)])]`, English labels
/// (translate them with [`label`]).
pub fn presets_for(platform: Platform) -> Vec<(&'static str, Items)> {
    match platform {
        Platform::Macos => vec![
            (n_!("Mouse"), mouse_items()),
            (
                n_!("Editing"),
                vec![
                    (n_!("Copy"), keys("Cmd+C")),
                    (n_!("Paste"), keys("Cmd+V")),
                    (n_!("Cut"), keys("Cmd+X")),
                    (n_!("Undo"), keys("Cmd+Z")),
                    (n_!("Redo"), keys("Cmd+Shift+Z")),
                    (n_!("Select all"), keys("Cmd+A")),
                    (n_!("Find"), keys("Cmd+F")),
                    (n_!("Zoom in"), keys("Cmd+Plus")),
                    (n_!("Zoom out"), keys("Cmd+Minus")),
                ],
            ),
            (
                n_!("Windows and desktops"),
                vec![
                    (n_!("Switch apps"), keys("Cmd+Tab")),
                    (n_!("Overview"), keys("Ctrl+Up")),
                    (n_!("Show desktop"), keys("F11")),
                    (n_!("Next desktop"), keys("Ctrl+Right")),
                    (n_!("Previous desktop"), keys("Ctrl+Left")),
                    (n_!("Maximize window"), keys("Ctrl+Cmd+F")),
                    (n_!("Minimize window"), keys("Cmd+M")),
                    (n_!("Close window"), keys("Cmd+W")),
                ],
            ),
            (n_!("Browser"), browser_items("Cmd", "Cmd+R")),
            (
                n_!("System"),
                vec![
                    (n_!("Search and run"), keys("Cmd+Space")),
                    (n_!("Screenshot"), keys("Cmd+Shift+3")),
                    (n_!("Capture a region"), keys("Cmd+Shift+4")),
                    (n_!("Lock screen"), keys("Ctrl+Cmd+Q")),
                    (n_!("Emoji picker"), keys("Ctrl+Cmd+Space")),
                ],
            ),
        ],
        Platform::Windows => vec![
            (n_!("Mouse"), mouse_items()),
            (
                n_!("Editing"),
                vec![
                    (n_!("Copy"), keys("Ctrl+C")),
                    (n_!("Paste"), keys("Ctrl+V")),
                    (n_!("Cut"), keys("Ctrl+X")),
                    (n_!("Undo"), keys("Ctrl+Z")),
                    (n_!("Redo"), keys("Ctrl+Y")),
                    (n_!("Select all"), keys("Ctrl+A")),
                    (n_!("Find"), keys("Ctrl+F")),
                    (n_!("Zoom in"), keys("Ctrl+Plus")),
                    (n_!("Zoom out"), keys("Ctrl+Minus")),
                ],
            ),
            (
                n_!("Windows and desktops"),
                vec![
                    (n_!("Switch apps"), keys("Alt+Tab")),
                    (n_!("Overview"), keys("Meta+Tab")),
                    (n_!("Show desktop"), keys("Meta+D")),
                    (n_!("Next desktop"), keys("Ctrl+Meta+Right")),
                    (n_!("Previous desktop"), keys("Ctrl+Meta+Left")),
                    (n_!("Maximize window"), keys("Meta+Up")),
                    (n_!("Minimize window"), keys("Meta+Down")),
                    (n_!("Snap window left"), keys("Meta+Left")),
                    (n_!("Snap window right"), keys("Meta+Right")),
                    (n_!("Close window"), keys("Alt+F4")),
                ],
            ),
            (n_!("Browser"), browser_items("Ctrl", "F5")),
            (
                n_!("Media"),
                vec![
                    (n_!("Play / pause"), keys("PlayPause")),
                    (n_!("Next track"), keys("NextSong")),
                    (n_!("Previous track"), keys("PreviousSong")),
                    (n_!("Volume up"), keys("VolumeUp")),
                    (n_!("Volume down"), keys("VolumeDown")),
                    (n_!("Mute"), keys("Mute")),
                ],
            ),
            (
                n_!("System"),
                vec![
                    (n_!("App launcher"), keys("Meta")),
                    (n_!("Search and run"), keys("Meta+S")),
                    (n_!("Screenshot"), keys("Print")),
                    (n_!("Capture a region"), keys("Meta+Shift+S")),
                    (n_!("Lock screen"), keys("Meta+L")),
                    (n_!("Emoji picker"), keys("Meta+Dot")),
                    (n_!("Clipboard history"), keys("Meta+V")),
                    (n_!("File manager"), keys("Meta+E")),
                ],
            ),
        ],
        _ => vec![
            (n_!("Mouse"), mouse_items()),
            (
                n_!("Editing"),
                vec![
                    (n_!("Copy"), keys("Ctrl+C")),
                    (n_!("Paste"), keys("Ctrl+V")),
                    (n_!("Cut"), keys("Ctrl+X")),
                    (n_!("Undo"), keys("Ctrl+Z")),
                    (n_!("Redo"), keys("Ctrl+Shift+Z")),
                    (n_!("Select all"), keys("Ctrl+A")),
                    (n_!("Find"), keys("Ctrl+F")),
                    (n_!("Zoom in"), keys("Ctrl+Plus")),
                    (n_!("Zoom out"), keys("Ctrl+Minus")),
                ],
            ),
            (
                n_!("Windows and desktops"),
                vec![
                    (n_!("Switch apps"), keys("Alt+Tab")),
                    (n_!("Overview"), keys("Meta+W")),
                    (n_!("Show desktop"), keys("Meta+D")),
                    (n_!("Next desktop"), keys("Ctrl+Meta+Right")),
                    (n_!("Previous desktop"), keys("Ctrl+Meta+Left")),
                    (n_!("Maximize window"), keys("Meta+PageUp")),
                    (n_!("Minimize window"), keys("Meta+PageDown")),
                    (n_!("Snap window left"), keys("Meta+Left")),
                    (n_!("Snap window right"), keys("Meta+Right")),
                    (n_!("Close window"), keys("Alt+F4")),
                ],
            ),
            (n_!("Browser"), browser_items("Ctrl", "F5")),
            (
                n_!("Media"),
                vec![
                    (n_!("Play / pause"), keys("PlayPause")),
                    (n_!("Next track"), keys("NextSong")),
                    (n_!("Previous track"), keys("PreviousSong")),
                    (n_!("Volume up"), keys("VolumeUp")),
                    (n_!("Volume down"), keys("VolumeDown")),
                    (n_!("Mute"), keys("Mute")),
                    (n_!("Mute microphone"), keys("MicMute")),
                ],
            ),
            (
                n_!("System"),
                vec![
                    (n_!("App launcher"), keys("Meta")),
                    (n_!("Search and run"), keys("Alt+Space")),
                    (n_!("Screenshot"), keys("Print")),
                    (n_!("Capture a region"), keys("Meta+Shift+Print")),
                    (n_!("Lock screen"), keys("Meta+L")),
                    (n_!("Emoji picker"), keys("Meta+Dot")),
                    (n_!("Clipboard history"), keys("Meta+V")),
                    (n_!("Open terminal"), keys("Ctrl+Alt+T")),
                    (n_!("File manager"), keys("Meta+E")),
                    (n_!("Brightness up"), keys("BrightnessUp")),
                    (n_!("Brightness down"), keys("BrightnessDown")),
                ],
            ),
        ],
    }
}

/// Text to show for a catalogue label, category or gesture slot, in this thread's
/// language. Words that mean something else elsewhere ("Back" as navigation, "Left" as
/// a screen edge) are translated with a context.
pub fn label(text: &str) -> String {
    match text {
        "Back" => trc!("mouse button", "Back"),
        "Forward" => trc!("mouse button", "Forward"),
        "tap" | "Press" => trc!("gesture", "Press"),
        "up" | "Up" => trc!("gesture", "Up"),
        "down" | "Down" => trc!("gesture", "Down"),
        "left" | "Left" => trc!("gesture", "Left"),
        "right" | "Right" => trc!("gesture", "Right"),
        "" => String::new(),
        other => tr!(other),
    }
}

/// `(category, label)` of the catalogue entry equal to `action`.
pub fn find_preset(action: &Value) -> Option<(&'static str, &'static str)> {
    presets_for(glisa_common::platform())
        .into_iter()
        .find_map(|(cat, items)| {
            items
                .into_iter()
                .find(|(_, a)| a == action)
                .map(|(l, _)| (cat, l))
        })
}

/// What `action` does, in a few words, in this thread's language.
pub fn describe(action: Option<&Value>) -> String {
    if is_default(action) {
        return trc!("button action", "Default");
    }
    let a = action.expect("not default");
    if let Some((_, l)) = find_preset(a) {
        return label(l);
    }
    match action_type(a) {
        Some("disabled") => trc!("button action", "Disabled"),
        Some("keys") => tr!("Shortcut: {combo}", combo = str_field(a, "combo")),
        Some("command") => tr!("Run: {command}", command = str_field(a, "cmd")),
        Some("gesture") => tr!("Gesture button"),
        Some("mouse") => match str_field(a, "button") {
            "left" => label("Left click"),
            "right" => label("Right click"),
            "middle" => label("Middle click"),
            "back" => label("Back"),
            "forward" => label("Forward"),
            other => tr!("Button {name}", name = other),
        },
        Some("scroll") => match str_field(a, "dir") {
            "up" => tr!("Scroll up"),
            "down" => tr!("Scroll down"),
            "left" => tr!("Scroll left"),
            "right" => tr!("Scroll right"),
            _ => tr!("Scroll"),
        },
        Some(t) => t.to_string(),
        None => "?".to_string(),
    }
}

/// The `presets` API result for this platform, labels in this thread's language:
///
/// ```text
/// {"platform": "linux",
///  "categories": [{"id": "Mouse", "label": "Ratón",
///                  "items": [{"id": "Left click", "label": "Clic izquierdo", "action": {…}}]}],
///  "default_gestures": {"type": "gesture", …},
///  "gesture_slots": [{"id": "tap", "label": "Pulsar"}, …]}
/// ```
///
/// `id`s are the English labels (stable; the window may key icons on them).
pub fn presets_json() -> Value {
    let platform = glisa_common::platform();
    let categories: Vec<Value> = presets_for(platform)
        .into_iter()
        .map(|(cat, items)| {
            json!({
                "id": cat,
                "label": label(cat),
                "items": items.into_iter().map(|(l, a)| json!({
                    "id": l, "label": label(l), "action": a,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "platform": platform,
        "categories": categories,
        "default_gestures": default_gestures_for(platform),
        "gesture_slots": GESTURE_SLOTS.iter().map(|s| json!({"id": s, "label": label(s)}))
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_valid_on_every_platform() {
        for p in [Platform::Linux, Platform::Windows, Platform::Macos] {
            for (_, items) in presets_for(p) {
                for (label, a) in items {
                    assert_eq!(validate(&a), Ok(()), "{label} on {p:?}");
                }
            }
            assert_eq!(validate(&default_gestures_for(p)), Ok(()));
        }
    }

    #[test]
    fn native_remap_and_divert() {
        let targets = [0x50, 0x51, 0x52];
        let a = json!({"type": "mouse", "button": "middle"});
        assert!(!needs_divert(Some(&a), &targets));
        assert_eq!(native_remap(Some(&a), &targets), 0x52);
        let back = json!({"type": "mouse", "button": "back"});
        assert!(needs_divert(Some(&back), &targets));
        assert!(!needs_divert(None, &targets));
        assert!(!needs_divert(Some(&json!({})), &targets));
        assert!(!needs_divert(Some(&json!({"type": "default"})), &targets));
        assert!(needs_divert(
            Some(&json!({"type": "keys", "combo": "A"})),
            &targets
        ));
    }

    #[test]
    fn validation_messages() {
        glisa_i18n::with_language(Some("en"), || {
            assert_eq!(
                validate(&json!({"type": "mouse", "button": "x"})),
                Err("Unknown mouse button".into())
            );
            assert_eq!(
                validate(&json!({"type": "command", "cmd": "  "})),
                Err("Empty command".into())
            );
            assert_eq!(
                validate(&json!({"type": "nope"})),
                Err("Unknown action type: nope".into())
            );
            assert_eq!(
                validate(&json!({})),
                Err("Unknown action type: None".into())
            );
            assert_eq!(
                validate(&json!({"type": "gesture", "up": {"type": "keys", "combo": "Ctrl+Nope"}})),
                Err("Unknown key: Nope".into())
            );
            assert_eq!(validate(&json!("keys")), Err("Invalid parameters".into()));
        });
    }

    #[test]
    fn describe_and_labels() {
        glisa_i18n::with_language(Some("es"), || {
            assert_eq!(label("Back"), "Atrás");
            assert_eq!(describe(None), trc!("button action", "Default"));
            assert!(
                describe(Some(&json!({"type": "keys", "combo": "Ctrl+Alt+K"})))
                    .contains("Ctrl+Alt+K")
            );
            let p = presets_json();
            assert_eq!(p["categories"][0]["id"], json!("Mouse"));
            assert_ne!(p["categories"][0]["label"], json!("Mouse"));
            assert_eq!(p["gesture_slots"].as_array().unwrap().len(), 5);
        });
    }
}
