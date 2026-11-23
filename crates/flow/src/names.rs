//! Received file names made valid and unique on this computer.
//!
//! Peers may be Linux, Windows or macOS. Every manifest path has already passed
//! `proto::relpath_ok` (no `..`, no control or format characters, …); this module only
//! turns each of its components into a name this file system accepts, without ever
//! colliding with another entry of the same transfer:
//!
//! * names are normalized to NFC (macOS sends NFD; two spellings of "é" are one name);
//! * on Windows, `< > : " / \ | ? *` and control characters become `_`, trailing dots and
//!   spaces are removed, and the reserved device names (CON, PRN, AUX, NUL, COM1–9,
//!   LPT1–9, with or without an extension) get a leading `_`;
//! * names that differ only in case are the same name on Windows and macOS (their default
//!   file systems are case-insensitive): the later one becomes `name (2).ext`;
//! * a component longer than 255 bytes after this is shortened (keeping its extension).
//!
//! Nothing is ever overwritten: a collision gets the next free ` (n)` suffix, and files are
//! created with O_EXCL anyway.

use std::collections::{HashMap, HashSet};

use unicode_normalization::UnicodeNormalization;

use crate::MAX_NAME;

const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// How this computer's file system treats names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameRules {
    /// Apply the Windows character and reserved-name rules.
    pub windows: bool,
    /// Names that differ only in case are the same file.
    pub case_insensitive: bool,
}

impl NameRules {
    pub fn this_os() -> Self {
        Self {
            windows: cfg!(windows),
            case_insensitive: cfg!(any(windows, target_os = "macos")),
        }
    }
}

/// One component made valid for the local file system (not yet unique).
pub fn sanitize_component(name: &str, rules: NameRules) -> String {
    let mut out: String = name.nfc().collect();
    if rules.windows {
        out = out
            .chars()
            .map(|c| {
                if "<>:\"/\\|?*".contains(c) || c.is_control() {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        while out.ends_with('.') || out.ends_with(' ') {
            out.pop();
        }
        if out.is_empty() {
            out.push('_');
        }
        let base = out
            .split('.')
            .next()
            .unwrap_or("")
            .trim_end()
            .to_ascii_uppercase();
        if WINDOWS_RESERVED.contains(&base.as_str()) {
            out.insert(0, '_');
        }
    }
    if out == "." || out == ".." || out.is_empty() {
        out = format!("_{out}");
    }
    shorten(&out, MAX_NAME)
}

/// (stem, ".ext") the way the sender splits names (Python `rpartition(".")`).
pub fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && !name[..i].starts_with('.') => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// `name` cut to at most `max` bytes of UTF-8, keeping its extension when it can.
fn shorten(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_string();
    }
    let (stem, ext) = split_ext(name);
    let ext = if ext.len() < max / 2 { ext } else { "" };
    let mut cut = String::new();
    for c in stem.chars() {
        if cut.len() + c.len_utf8() + ext.len() > max {
            break;
        }
        cut.push(c);
    }
    cut + ext
}

/// "name (n).ext"
pub fn numbered(name: &str, n: usize) -> String {
    let (stem, ext) = split_ext(name);
    let suffix = format!(" ({n})");
    let stem = shorten(
        stem,
        MAX_NAME.saturating_sub(suffix.len() + ext.len()).max(1),
    );
    format!("{stem}{suffix}{ext}")
}

/// Maps the manifest paths of one transfer to unique local relative paths (`/`-separated).
pub struct LocalNames {
    rules: NameRules,
    used: HashMap<String, HashSet<String>>,
    map: HashMap<String, String>,
}

impl LocalNames {
    pub fn new(rules: NameRules) -> Self {
        Self {
            rules,
            used: HashMap::new(),
            map: HashMap::new(),
        }
    }

    fn key(&self, name: &str) -> String {
        if self.rules.case_insensitive {
            name.to_lowercase()
        } else {
            name.to_string()
        }
    }

    /// The local path of `manifest_path` (whose parent, if any, was mapped before).
    pub fn local(&mut self, manifest_path: &str) -> String {
        if let Some(done) = self.map.get(manifest_path) {
            return done.clone();
        }
        let (parent, name) = match manifest_path.rsplit_once('/') {
            Some((p, n)) => (p, n),
            None => ("", manifest_path),
        };
        let local_parent = if parent.is_empty() {
            String::new()
        } else {
            self.local(parent)
        };
        let base = sanitize_component(name, self.rules);
        let mut candidate = base.clone();
        let mut n = 2;
        loop {
            let key = self.key(&candidate);
            let used = self.used.entry(local_parent.clone()).or_default();
            if used.insert(key) {
                break;
            }
            candidate = numbered(&base, n);
            n += 1;
        }
        let local = if local_parent.is_empty() {
            candidate
        } else {
            format!("{local_parent}/{candidate}")
        };
        self.map.insert(manifest_path.to_string(), local.clone());
        local
    }

    /// The local name chosen for a single file outside any manifest path (an image).
    pub fn reserve(&mut self, name: &str) -> String {
        self.local(name)
    }
}

/// Files the sender leaves out: macOS bookkeeping that means nothing on another computer.
pub fn is_junk(name: &str) -> bool {
    matches!(
        name,
        ".DS_Store" | ".Spotlight-V100" | ".Trashes" | ".fseventsd"
    ) || name.starts_with("._")
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN: NameRules = NameRules {
        windows: true,
        case_insensitive: true,
    };
    const MAC: NameRules = NameRules {
        windows: false,
        case_insensitive: true,
    };
    const LINUX: NameRules = NameRules {
        windows: false,
        case_insensitive: false,
    };

    #[test]
    fn components() {
        assert_eq!(
            sanitize_component("e\u{301}t\u{e9}", LINUX),
            "\u{e9}t\u{e9}"
        ); // NFC
        assert_eq!(
            sanitize_component("a<b>c:d\"e|f?g*h\\i", WIN),
            "a_b_c_d_e_f_g_h_i"
        );
        assert_eq!(sanitize_component("notes. . ", WIN), "notes");
        assert_eq!(sanitize_component("...", WIN), "_");
        for name in ["CON", "con.txt", "Lpt9.tar.gz", "nul ", "COM1"] {
            assert!(sanitize_component(name, WIN).starts_with('_'), "{name}");
        }
        assert_eq!(sanitize_component("console.txt", WIN), "console.txt");
        assert_eq!(sanitize_component("a:b", LINUX), "a:b");
        let long = format!("{}.jpg", "ñ".repeat(200));
        let short = sanitize_component(&long, LINUX);
        assert!(short.len() <= 255 && short.ends_with(".jpg"));
    }

    #[test]
    fn collisions() {
        let mut n = LocalNames::new(MAC);
        assert_eq!(n.local("Fotos"), "Fotos");
        assert_eq!(n.local("Fotos/Foto.jpg"), "Fotos/Foto.jpg");
        assert_eq!(n.local("Fotos/foto.jpg"), "Fotos/foto (2).jpg");
        assert_eq!(n.local("fotos"), "fotos (2)");
        assert_eq!(n.local("fotos/foto.jpg"), "fotos (2)/foto.jpg");
        assert_eq!(n.local("e\u{301}"), "\u{e9}");
        assert_eq!(n.local("\u{e9}"), "\u{e9} (2)");
        let mut l = LocalNames::new(LINUX);
        assert_eq!(l.local("Foto.jpg"), "Foto.jpg");
        assert_eq!(l.local("foto.jpg"), "foto.jpg");
        assert_eq!(l.local(".bashrc"), ".bashrc");
        let mut w = LocalNames::new(WIN);
        assert_eq!(w.local("a?"), "a_");
        assert_eq!(w.local("a*"), "a_ (2)");
        assert_eq!(numbered(".bashrc", 2), ".bashrc (2)");
        assert_eq!(numbered("archive.tar.gz", 3), "archive.tar (3).gz");
        assert!(is_junk(".DS_Store") && is_junk("._foto.jpg") && !is_junk(".bashrc"));
    }
}
