//! Translations for the Rust side of Glisa.
//!
//! English is the source language: the msgids in the code are the English text. Other
//! languages come from gettext `.po` files in `locale/<lang>/` (domains `core.po`,
//! `buttons.po`, `flow.po` and `app.po`; the window has its own `ui.po`), embedded at
//! build time and parsed on first use.
//!
//! ```ignore
//! use glisa_i18n::{tr, trn, trc};
//! tr!("Device not found");
//! tr!("Paired with {name}", name = peer);
//! trn!("{count} file", "{count} files", n, count = n);
//! trc!("mouse button", "Back");
//! ```
//!
//! The language is per thread: the process default ([`set_language`], initialised from
//! [`system_language`]) unless the thread runs inside [`with_language`], which is how a
//! request is answered in its client's language. Background messages that clients read
//! later are kept as [`Text`] (msgid + arguments) and rendered by the reader.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};

pub const SOURCE_LANGUAGE: &str = "en";
/// Languages with a catalogue, and their names in their own language.
pub const LANGUAGES: &[(&str, &str)] = &[("en", "English"), ("es", "Español")];

const CATALOGS: &[(&str, &str)] = &[
    ("es", include_str!("../../../locale/es/core.po")),
    ("es", include_str!("../../../locale/es/buttons.po")),
    ("es", include_str!("../../../locale/es/flow.po")),
    ("es", include_str!("../../../locale/es/app.po")),
];

type Key = (String, String); // (context, msgid); context "" = none

#[derive(Debug, Clone)]
enum Entry {
    Single(String),
    Plural(Vec<String>),
}

static DEFAULT: RwLock<Option<String>> = RwLock::new(None);
thread_local! {
    static OVERRIDE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// "es_ES.UTF-8" → "es"; unsupported → None.
pub fn normalize(lang: &str) -> Option<&'static str> {
    let code = lang
        .trim()
        .split(['_', '-', '.', '@'])
        .next()?
        .to_ascii_lowercase();
    LANGUAGES.iter().map(|(c, _)| *c).find(|c| *c == code)
}

/// `GLISA_LANG` if set, else the first supported entry of `LANGUAGE`, `LC_ALL`,
/// `LC_MESSAGES`, `LANG`; English otherwise. (Windows and macOS: the shell passes the
/// OS UI language through [`set_language`].)
pub fn system_language() -> &'static str {
    if let Some(l) = std::env::var("GLISA_LANG")
        .ok()
        .as_deref()
        .and_then(normalize)
    {
        return l;
    }
    for var in ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(var) {
            for part in value.split(':') {
                if part.is_empty() || part == "C" || part == "POSIX" {
                    continue;
                }
                if let Some(l) = normalize(part) {
                    return l;
                }
            }
        }
    }
    SOURCE_LANGUAGE
}

/// Sets the process default; `None` or unsupported → [`system_language`]. Returns it.
pub fn set_language(lang: Option<&str>) -> &'static str {
    let l = lang.and_then(normalize).unwrap_or_else(system_language);
    *DEFAULT.write().unwrap_or_else(|e| e.into_inner()) = Some(l.to_string());
    l
}

/// The language of this thread right now.
pub fn current_language() -> String {
    if let Some(l) = OVERRIDE.with(|o| o.borrow().clone()) {
        return l;
    }
    if let Some(l) = DEFAULT.read().unwrap_or_else(|e| e.into_inner()).clone() {
        return l;
    }
    system_language().to_string()
}

/// Runs `f` with this thread's language set to `lang` (unsupported or None: unchanged).
pub fn with_language<R>(lang: Option<&str>, f: impl FnOnce() -> R) -> R {
    let Some(l) = lang.and_then(normalize) else {
        return f();
    };
    let prev = OVERRIDE.with(|o| o.replace(Some(l.to_string())));
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let prev = self.0.take();
            OVERRIDE.with(|o| *o.borrow_mut() = prev);
        }
    }
    let _restore = Restore(prev);
    f()
}

fn catalogs() -> &'static HashMap<String, HashMap<Key, Entry>> {
    static ALL: OnceLock<HashMap<String, HashMap<Key, Entry>>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut all: HashMap<String, HashMap<Key, Entry>> = HashMap::new();
        for (lang, text) in CATALOGS {
            let cat = all.entry((*lang).to_string()).or_default();
            for (key, entry) in parse_po(text) {
                cat.insert(key, entry);
            }
        }
        all
    })
}

fn lookup(context: &str, msgid: &str) -> Option<&'static Entry> {
    let lang = current_language();
    if lang == SOURCE_LANGUAGE {
        return None;
    }
    catalogs()
        .get(&lang)?
        .get(&(context.to_string(), msgid.to_string()))
}

/// `msgid` in this thread's language.
/// A msgid that is also the singular of a plural entry gets that entry's first form.
pub fn gettext(msgid: &str) -> Cow<'_, str> {
    match lookup("", msgid) {
        Some(Entry::Single(s)) if !s.is_empty() => Cow::Borrowed(s.as_str()),
        Some(Entry::Plural(forms)) if forms.first().is_some_and(|s| !s.is_empty()) => {
            Cow::Borrowed(forms[0].as_str())
        }
        _ => Cow::Borrowed(msgid),
    }
}

/// `msgid` in `context` ("mouse button", "gesture"…), in this thread's language.
pub fn pgettext<'a>(context: &str, msgid: &'a str) -> Cow<'a, str> {
    match lookup(context, msgid) {
        Some(Entry::Single(s)) if !s.is_empty() => Cow::Owned(s.clone()),
        _ => Cow::Borrowed(msgid),
    }
}

/// Singular or plural form for `n` (English and Spanish: plural = n != 1).
pub fn ngettext<'a>(singular: &'a str, plural: &'a str, n: u64) -> Cow<'a, str> {
    let index = usize::from(n != 1);
    if let Some(Entry::Plural(forms)) = lookup("", singular) {
        if let Some(s) = forms.get(index).filter(|s| !s.is_empty()) {
            return Cow::Owned(s.clone());
        }
    }
    Cow::Borrowed(if index == 0 { singular } else { plural })
}

/// Replaces `{name}` placeholders. Unknown placeholders stay as they are; `{{`/`}}`
/// are literal braces (Python `str.format` compatible for the cases we use).
pub fn format(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push(tail.as_bytes()[0] as char);
            rest = &tail[2..];
            continue;
        }
        if tail.starts_with('{') {
            if let Some(end) = tail.find('}') {
                let name = &tail[1..end];
                if let Some((_, value)) = args.iter().find(|(k, _)| *k == name) {
                    out.push_str(&value.to_string());
                    rest = &tail[end + 1..];
                    continue;
                }
            }
        }
        out.push(tail.as_bytes()[0] as char);
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// Marks a string for translation without translating it (like Python's `N_`).
#[macro_export]
macro_rules! n_ {
    ($msgid:literal) => {
        $msgid
    };
}

/// Translates and formats: `tr!("Paired with {name}", name = x)`.
#[macro_export]
macro_rules! tr {
    ($msgid:expr) => {
        $crate::gettext($msgid).into_owned()
    };
    ($msgid:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::format(&$crate::gettext($msgid), &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+])
    };
}

/// Translates with a context: `trc!("mouse button", "Back")`.
#[macro_export]
macro_rules! trc {
    ($ctx:expr, $msgid:expr) => {
        $crate::pgettext($ctx, $msgid).into_owned()
    };
    ($ctx:expr, $msgid:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::format(&$crate::pgettext($ctx, $msgid), &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+])
    };
}

/// Plural: `trn!("{count} file", "{count} files", n, count = n)`.
#[macro_export]
macro_rules! trn {
    ($s:expr, $p:expr, $n:expr) => {
        $crate::ngettext($s, $p, ($n) as u64).into_owned()
    };
    ($s:expr, $p:expr, $n:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::format(&$crate::ngettext($s, $p, ($n) as u64), &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+])
    };
}

// ------------------------------------------------------------------ deferred text

/// An argument of a [`Text`]: plain value, or another text rendered in the same language.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TextArg {
    Int(i64),
    Float(f64),
    Str(String),
    Text(Box<Text>),
}

impl From<&str> for TextArg {
    fn from(s: &str) -> Self {
        TextArg::Str(s.to_string())
    }
}
impl From<String> for TextArg {
    fn from(s: String) -> Self {
        TextArg::Str(s)
    }
}
impl From<i64> for TextArg {
    fn from(n: i64) -> Self {
        TextArg::Int(n)
    }
}
impl From<u64> for TextArg {
    fn from(n: u64) -> Self {
        TextArg::Int(n as i64)
    }
}
impl From<Text> for TextArg {
    fn from(t: Text) -> Self {
        TextArg::Text(Box::new(t))
    }
}

/// User-facing text translated when it is read, not when it is made: Flow produces notices
/// in background threads that clients read later, each in its own language.
/// `Text::new(n_!("Paired with {name}")).arg("name", peer)`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Text {
    pub msgid: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<(String, TextArg)>,
}

impl Text {
    pub fn new(msgid: impl Into<String>) -> Self {
        Self {
            msgid: msgid.into(),
            args: Vec::new(),
        }
    }

    pub fn arg(mut self, name: &str, value: impl Into<TextArg>) -> Self {
        self.args.push((name.to_string(), value.into()));
        self
    }

    /// Rendered in this thread's language.
    pub fn render(&self) -> String {
        if self.msgid.is_empty() {
            return String::new();
        }
        let template = gettext(&self.msgid).into_owned();
        let values: Vec<(String, String)> = self
            .args
            .iter()
            .map(|(k, v)| {
                let s = match v {
                    TextArg::Int(n) => n.to_string(),
                    TextArg::Float(f) => f.to_string(),
                    TextArg::Str(s) => s.clone(),
                    TextArg::Text(t) => t.render(),
                };
                (k.clone(), s)
            })
            .collect();
        let refs: Vec<(&str, &dyn Display)> = values
            .iter()
            .map(|(k, v)| (k.as_str(), v as &dyn Display))
            .collect();
        format(&template, &refs)
    }

    /// Rendered in English (logs).
    pub fn english(&self) -> String {
        with_language(Some(SOURCE_LANGUAGE), || self.render())
    }
}

impl Display for Text {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render())
    }
}

// ------------------------------------------------------------------ .po parser

fn unquote(s: &str) -> String {
    let s = s.trim();
    let inner = s
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Parses a gettext catalogue: `msgctxt`, `msgid`, `msgid_plural`, `msgstr`,
/// `msgstr[n]`, continuation lines. Fuzzy and untranslated entries are skipped.
fn parse_po(text: &str) -> Vec<(Key, Entry)> {
    #[derive(Default)]
    struct Cur {
        ctx: String,
        id: Option<String>,
        plural: Option<String>,
        strs: Vec<(usize, String)>,
        fuzzy: bool,
    }
    fn flush(cur: &mut Cur, out: &mut Vec<(Key, Entry)>) {
        let c = std::mem::take(cur);
        let Some(id) = c.id else { return };
        if id.is_empty() || c.fuzzy {
            return;
        }
        let entry = if c.plural.is_some() {
            let n = c.strs.iter().map(|(i, _)| i + 1).max().unwrap_or(0).max(2);
            let mut forms = vec![String::new(); n];
            for (i, s) in c.strs {
                forms[i] = s;
            }
            if forms.iter().all(String::is_empty) {
                return;
            }
            Entry::Plural(forms)
        } else {
            let s = c
                .strs
                .into_iter()
                .next()
                .map(|(_, s)| s)
                .unwrap_or_default();
            if s.is_empty() {
                return;
            }
            Entry::Single(s)
        };
        out.push(((c.ctx, id), entry));
    }

    enum Field {
        None,
        Ctx,
        Id,
        Plural,
        Str(usize),
    }
    let mut out = Vec::new();
    let mut cur = Cur::default();
    let mut field = Field::None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(flags) = line.strip_prefix("#,") {
            if matches!(field, Field::Str(_)) {
                flush(&mut cur, &mut out);
                field = Field::None;
            }
            cur.fuzzy = flags.split(',').any(|f| f.trim() == "fuzzy");
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if line.starts_with('"') {
            let s = unquote(line);
            match field {
                Field::Ctx => cur.ctx.push_str(&s),
                Field::Id => cur.id.get_or_insert_with(String::new).push_str(&s),
                Field::Plural => cur.plural.get_or_insert_with(String::new).push_str(&s),
                Field::Str(i) => {
                    if let Some(last) = cur.strs.iter_mut().find(|(j, _)| *j == i) {
                        last.1.push_str(&s);
                    }
                }
                Field::None => {}
            }
            continue;
        }
        let (keyword, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        match keyword {
            "msgctxt" => {
                if matches!(field, Field::Str(_)) {
                    flush(&mut cur, &mut out);
                }
                cur.ctx = unquote(rest);
                field = Field::Ctx;
            }
            "msgid" => {
                if matches!(field, Field::Str(_)) {
                    flush(&mut cur, &mut out);
                }
                cur.id = Some(unquote(rest));
                field = Field::Id;
            }
            "msgid_plural" => {
                cur.plural = Some(unquote(rest));
                field = Field::Plural;
            }
            "msgstr" => {
                cur.strs.push((0, unquote(rest)));
                field = Field::Str(0);
            }
            k if k.starts_with("msgstr[") => {
                let i = k[7..].trim_end_matches(']').parse().unwrap_or(0);
                cur.strs.push((i, unquote(rest)));
                field = Field::Str(i);
            }
            _ => {}
        }
    }
    flush(&mut cur, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PO: &str = r#"
msgid ""
msgstr ""
"Language: es\n"

#: a.rs:1
msgid "Device not found"
msgstr "Dispositivo no encontrado"

msgctxt "mouse button"
msgid "Back"
msgstr "Atrás"

#, fuzzy
msgid "Old"
msgstr "Viejo"

msgid "{count} file"
msgid_plural "{count} files"
msgstr[0] "{count} archivo"
msgstr[1] "{count} archivos"

msgid ""
"Long "
"text"
msgstr "Texto "
"largo \"citado\""
"#;

    #[test]
    fn parses_po() {
        let cat: HashMap<Key, Entry> = parse_po(PO).into_iter().collect();
        let get = |c: &str, id: &str| cat.get(&(c.to_string(), id.to_string())).cloned();
        assert!(
            matches!(get("", "Device not found"), Some(Entry::Single(s)) if s == "Dispositivo no encontrado")
        );
        assert!(matches!(get("mouse button", "Back"), Some(Entry::Single(s)) if s == "Atrás"));
        assert!(get("", "Old").is_none());
        assert!(
            matches!(get("", "{count} file"), Some(Entry::Plural(f)) if f[1] == "{count} archivos")
        );
        assert!(
            matches!(get("", "Long text"), Some(Entry::Single(s)) if s == "Texto largo \"citado\"")
        );
        assert!(get("", "").is_none());
    }

    #[test]
    fn formats() {
        assert_eq!(
            format("Hi {name}, {n}", &[("name", &"Ana"), ("n", &3)]),
            "Hi Ana, 3"
        );
        assert_eq!(format("{{x}} {missing}", &[]), "{x} {missing}");
    }

    #[test]
    fn languages_per_thread() {
        assert_eq!(normalize("es_ES.UTF-8"), Some("es"));
        assert_eq!(normalize("fr_FR"), None);
        set_language(Some("en"));
        assert_eq!(tr!("Device not found"), "Device not found");
        with_language(Some("es"), || {
            assert_eq!(current_language(), "es");
            // Real catalogue entry from locale/es/core.po.
            assert_eq!(tr!("Device not found"), "Dispositivo no encontrado");
            let t = Text::new("Device not found");
            assert_eq!(t.render(), "Dispositivo no encontrado");
            assert_eq!(t.english(), "Device not found");
        });
        assert_eq!(current_language(), "en");
        let n = 2;
        assert_eq!(
            trn!("{count} file", "{count} files", n, count = n),
            "2 files"
        );
    }
}
