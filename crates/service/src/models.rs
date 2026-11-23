//! Which model a device is, and how to draw it: the snapshot's `family` (illustration
//! family) and `photo` (key into data/photos.json).
//!
//! `data/models.json` is our own table (see its `_comment`), matched first by the
//! product ids the device reports (0x0003) or connects with, then by its name. Without
//! a match, the family comes from heuristics on kind, name, features and controls.

use std::collections::HashSet;
use std::sync::OnceLock;

use glisa_hidpp::f;
use serde_json::{Map, Value};

/// Illustration families (docs/ARCHITECTURE.md, Device pictures).
pub const FAMILIES: &[&str] = &[
    "mouse-vertical",
    "mouse-ergo-large",
    "mouse-performance",
    "mouse-compact",
    "mouse-basic",
    "trackball",
    "keyboard-full",
    "keyboard-tkl",
    "keyboard-compact",
    "receiver",
    "generic",
];

const MODELS_JSON: &str = include_str!("../../../data/models.json");
const PHOTOS_JSON: &str = include_str!(concat!(env!("OUT_DIR"), "/photos.json"));

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub key: String,
    pub name: String,
    pub family: String,
    pub kind: String,
    pub pids: Vec<u16>,
    /// Lower-case words of the name and the other names it reports.
    words: Vec<Vec<String>>,
    pub extras: Map<String, Value>,
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn parse_models(text: &str) -> Vec<Model> {
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let Some(Value::Object(models)) = root.get("models") else {
        return Vec::new();
    };
    models
        .iter()
        .filter_map(|(key, m)| {
            let m = m.as_object()?;
            let s = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let mut pids = Vec::new();
            for field in ["bt", "wpid", "usb"] {
                for p in m.get(field).and_then(Value::as_array).into_iter().flatten() {
                    if let Some(v) = p.as_str().and_then(|p| u16::from_str_radix(p, 16).ok()) {
                        pids.push(v);
                    }
                }
            }
            let name = s("name");
            let mut all_words = vec![words(&name)];
            for n in m
                .get("names")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(n) = n.as_str() {
                    all_words.push(words(n));
                }
            }
            all_words.retain(|w| !w.is_empty());
            Some(Model {
                key: key.clone(),
                name,
                family: s("family"),
                kind: s("kind"),
                pids,
                words: all_words,
                extras: m
                    .get("extras")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default(),
            })
        })
        .collect()
}

pub fn models() -> &'static [Model] {
    static MODELS: OnceLock<Vec<Model>> = OnceLock::new();
    MODELS.get_or_init(|| parse_models(MODELS_JSON))
}

/// Models with a photo URL in data/photos.json (`{"<key>": …}` or `{"models": {…}}`).
fn parse_photos(text: &str) -> HashSet<String> {
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(text) else {
        return HashSet::new();
    };
    let table = match root.get("models") {
        Some(Value::Object(m)) => m.clone(),
        _ => root,
    };
    table
        .into_iter()
        .filter(|(k, v)| !k.starts_with('_') && !v.is_null())
        .map(|(k, _)| k)
        .collect()
}

fn photos() -> &'static HashSet<String> {
    static PHOTOS: OnceLock<HashSet<String>> = OnceLock::new();
    PHOTOS.get_or_init(|| parse_photos(PHOTOS_JSON))
}

fn contains_run(hay: &[String], needle: &[String]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// The model with one of `pids`, else the one whose name (as whole words, longest
/// first) appears in `name`.
pub fn lookup(pids: &[u16], name: &str) -> Option<&'static Model> {
    lookup_in(models(), pids, name)
}

fn lookup_in<'a>(models: &'a [Model], pids: &[u16], name: &str) -> Option<&'a Model> {
    if let Some(m) = models
        .iter()
        .find(|m| m.pids.iter().any(|p| pids.contains(p)))
    {
        return Some(m);
    }
    let hay = words(name);
    models
        .iter()
        .filter_map(|m| {
            m.words
                .iter()
                .filter(|w| contains_run(&hay, w))
                .map(|w| w.len())
                .max()
                .map(|len| (len, m))
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, m)| m)
}

/// The illustration family: the model's, else a guess.
pub fn family(
    model: Option<&Model>,
    kind: &str,
    name: &str,
    features: &[u16],
    cids: &[u16],
) -> &'static str {
    if let Some(m) = model {
        if let Some(f) = FAMILIES.iter().find(|f| **f == m.family) {
            return f;
        }
    }
    guess_family(kind, name, features, cids)
}

fn guess_family(kind: &str, name: &str, features: &[u16], cids: &[u16]) -> &'static str {
    let w = words(name);
    let has = |word: &str| w.iter().any(|x| x == word);
    match kind {
        "receiver" => "receiver",
        "trackball" => "trackball",
        "keyboard" | "numpad" => {
            if has("mini") || has("compact") || has("k380") || (has("pop") && has("keys")) {
                "keyboard-compact"
            } else if has("tkl") {
                "keyboard-tkl"
            } else {
                "keyboard-full"
            }
        }
        "mouse" => {
            if features.contains(&f::THUMB_WHEEL) {
                "mouse-performance"
            } else if has("vertical") || has("lift") {
                "mouse-vertical"
            } else if has("trackball") || has("ergo") {
                "trackball"
            } else if cids.contains(&0x00C3)
                || features.contains(&f::SMART_SHIFT)
                || features.contains(&f::SMART_SHIFT_ENHANCED)
            {
                "mouse-performance"
            } else if has("anywhere") || has("pebble") || has("pop") || has("mini") {
                "mouse-compact"
            } else if cids.contains(&0x0053) || cids.contains(&0x0056) {
                "mouse-basic"
            } else {
                "mouse-compact"
            }
        }
        _ => "generic",
    }
}

/// The key into data/photos.json when it has the model.
pub fn photo(model: Option<&Model>) -> Option<String> {
    model
        .filter(|m| photos().contains(&m.key))
        .map(|m| m.key.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYS: &[&str] = &[
        "lift",
        "mx-vertical",
        "mx-ergo",
        "mx-ergo-s",
        "mx-master-3s",
        "mx-master-3",
        "mx-master-2s",
        "mx-anywhere-3s",
        "mx-anywhere-3",
        "mx-anywhere-2s",
        "m720-triathlon",
        "signature-m650",
        "pop-mouse",
        "pebble-m350s",
        "mx-keys-s",
        "mx-keys",
        "mx-keys-mini",
        "mx-mechanical",
        "mx-mechanical-mini",
        "k380",
        "ergo-k860",
        "pop-keys",
        "k780",
        "wave-keys",
        "signature-k650",
        "bolt-receiver",
        "unifying-receiver",
    ];

    #[test]
    fn table_is_complete_and_valid() {
        let m = models();
        assert_eq!(m.len(), KEYS.len());
        for k in KEYS {
            let model = m
                .iter()
                .find(|x| x.key == *k)
                .unwrap_or_else(|| panic!("{k}"));
            assert!(FAMILIES.contains(&model.family.as_str()), "{k}");
            assert!(!model.name.is_empty());
        }
        let mut pids: Vec<u16> = m.iter().flat_map(|x| x.pids.clone()).collect();
        let n = pids.len();
        pids.sort_unstable();
        pids.dedup();
        assert_eq!(pids.len(), n, "a product id appears twice");
    }

    #[test]
    fn lookups() {
        assert_eq!(lookup(&[0xB031], "").unwrap().key, "lift");
        // The real LIFT's name, without ids.
        assert_eq!(
            lookup(&[], "LIFT VERTICAL ERGONOMIC MOUSE").unwrap().key,
            "lift"
        );
        assert_eq!(lookup(&[], "MX Master 3S").unwrap().key, "mx-master-3s");
        assert_eq!(
            lookup(&[], "Wireless Mouse MX Master 3").unwrap().key,
            "mx-master-3"
        );
        assert_eq!(lookup(&[], "MX Keys Mini").unwrap().key, "mx-keys-mini");
        assert_eq!(
            lookup(&[0xB35B], "Wireless Keyboard K-Sim").unwrap().key,
            "mx-keys"
        );
        assert!(lookup(&[0x1234], "Wireless Mouse MX-Sim").is_none());
        assert!(lookup(&[], "").is_none());
    }

    #[test]
    fn heuristics() {
        assert_eq!(
            guess_family(
                "mouse",
                "Wireless Mouse MX-Sim",
                &[f::SMART_SHIFT_ENHANCED],
                &[0xC3]
            ),
            "mouse-performance"
        );
        assert_eq!(
            guess_family("mouse", "M", &[f::THUMB_WHEEL], &[]),
            "mouse-performance"
        );
        assert_eq!(
            guess_family("mouse", "Some Vertical", &[], &[]),
            "mouse-vertical"
        );
        assert_eq!(guess_family("mouse", "M185", &[], &[0x53]), "mouse-basic");
        assert_eq!(
            guess_family("keyboard", "K400 Plus", &[], &[]),
            "keyboard-full"
        );
        assert_eq!(
            guess_family("keyboard", "Keys Mini", &[], &[]),
            "keyboard-compact"
        );
        assert_eq!(guess_family("trackball", "x", &[], &[]), "trackball");
        assert_eq!(guess_family("presenter", "x", &[], &[]), "generic");
        let lift = lookup(&[0xB031], "");
        assert_eq!(family(lift, "mouse", "", &[], &[]), "mouse-vertical");
        assert_eq!(
            lift.unwrap().extras["virtual_thumbwheel"],
            Value::Bool(true)
        );
    }

    #[test]
    fn photos_formats() {
        let flat = parse_photos(r#"{"_comment": "x", "lift": "https://…", "k380": null}"#);
        assert_eq!(flat, HashSet::from(["lift".to_string()]));
        let nested = parse_photos(r#"{"models": {"mx-keys": {"url": "https://…"}}}"#);
        assert!(nested.contains("mx-keys"));
        assert!(parse_photos("garbage").is_empty());
    }
}
