//! Flow wire protocol v1 (see docs/FLOW.md §4).
//!
//! Every message travels inside TLS 1.3 as one frame:
//!
//! ```text
//!     uint32 big-endian length ‖ UTF-8 JSON object
//! ```
//!
//! The object always has exactly the fields `v` (protocol version), `type`, `seq` and the
//! fields its type defines — no more, no fewer (only `hello.port` and `hello.caps` are
//! optional). `seq` starts at 1 on each connection and direction and must grow by exactly
//! one, so a replayed, dropped or reordered frame ends the connection. Each type has its own
//! size limit, checked against the length prefix before the body is read; anything unknown,
//! malformed (duplicate keys, NaN, lone surrogates, too deeply nested…) or oversized is a
//! protocol error and the connection is closed.
//!
//! Transfer connections (copied files and images) use the same framing plus binary data
//! frames: the body starts with a 0x00 byte (a JSON body always starts with `{`), then seq
//! (uint64), entry index (uint32) and offset (uint64), all big-endian, then 1 … CHUNK_MAX
//! bytes of the entry's content. They share the sequence numbers of the JSON frames.

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value, json};

use crate::util::{forbidden_in_path, is_hex_lower, printable};
use crate::{
    CLIPBOARD_MAX, IMAGE_TYPES, MAX_DEPTH, MAX_FILES, MAX_NAME, MAX_PATH, PROTOCOL_VERSION, SIDES,
};

pub const SMALL: usize = 8 * 1024;
/// Clipboard text: 1 MiB of UTF-8, worst-case JSON escaping.
pub const FRAME_MAX: usize = 7 * 1024 * 1024;
/// The list of files of a transfer (or an image + its text).
pub const MANIFEST_MAX: usize = 8 * 1024 * 1024;
/// Content bytes in one data frame.
pub const CHUNK_MAX: usize = 256 * 1024;
pub const DATA_TAG: u8 = 0;
/// tag (u8), seq (u64), entry index (u32), offset (u64).
pub const DATA_HEADER: usize = 1 + 8 + 4 + 8;
pub const DATA_MAX: usize = DATA_HEADER + CHUNK_MAX;
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// Once a frame has started, the rest must follow this fast (a deadline for the frame).
pub const FRAME_TIMEOUT: Duration = Duration::from_secs(10);

pub type Msg = Map<String, Value>;

/// Why a connection stops: nothing arrived in time (not an error), the peer broke the
/// protocol (the connection must be closed), or the transport failed.
#[derive(Debug)]
pub enum Error {
    Idle,
    Protocol(String),
    Io(io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Idle => f.write_str("idle"),
            Error::Protocol(e) => write!(f, "protocol error: {e}"),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

fn perr(text: impl Into<String>) -> Error {
    Error::Protocol(text.into())
}

// ================================================================== field checks
type Check = fn(&Value) -> bool;

fn int_in(v: &Value, lo: i128, hi: i128) -> bool {
    let Value::Number(n) = v else { return false };
    let x: i128 = if let Some(u) = n.as_u64() {
        u as i128
    } else if let Some(i) = n.as_i64() {
        i as i128
    } else {
        return false; // a float, even 1.0, is not an integer
    };
    lo <= x && x <= hi
}

fn str_check(v: &Value, max_len: usize, printable_only: bool) -> Option<&str> {
    let s = v.as_str()?;
    let n = s.chars().count();
    if n == 0 || n > max_len || (printable_only && !printable(s)) {
        return None;
    }
    Some(s)
}

pub fn is_id(v: &Value) -> bool {
    v.as_str().is_some_and(|s| is_hex_lower(s, 16))
}

pub fn is_name(v: &Value) -> bool {
    str_check(v, 64, true).is_some()
}

pub fn is_seq(v: &Value) -> bool {
    int_in(v, 1, 1 << 53)
}

fn is_hex64(v: &Value) -> bool {
    v.as_str().is_some_and(|s| is_hex_lower(s, 64))
}

fn is_unit(v: &Value) -> bool {
    match v {
        Value::Number(n) => n
            .as_f64()
            .is_some_and(|f| f.is_finite() && (0.0..=1.0).contains(&f)),
        _ => false,
    }
}

fn is_bool(v: &Value) -> bool {
    v.is_boolean()
}

fn one_of(v: &Value, allowed: &[&str]) -> bool {
    v.as_str().is_some_and(|s| allowed.contains(&s))
}

pub const KINDS: [&str; 5] = ["mouse", "trackball", "trackpad", "keyboard", "unknown"];
pub const REASONS: [&str; 13] = [
    "disabled",
    "busy",
    "not_pairing",
    "rate_limited",
    "version",
    "unknown_peer",
    "no_layout",
    "protocol",
    "bye",
    "rejected",
    "expired",
    "mismatch",
    "error",
];
/// Why a transfer was refused or cancelled (transfer connections only).
pub const TRANSFER_REASONS: [&str; 10] = [
    "disabled",
    "busy",
    "too_large",
    "too_many",
    "no_space",
    "cancelled",
    "failed",
    "protocol",
    "unknown_peer",
    "error",
];

fn is_kind(v: &Value) -> bool {
    one_of(v, &KINDS)
}
fn is_reason(v: &Value) -> bool {
    one_of(v, &REASONS)
}
fn is_transfer_reason(v: &Value) -> bool {
    one_of(v, &TRANSFER_REASONS)
}
fn is_side(v: &Value) -> bool {
    one_of(v, &SIDES)
}
fn is_transfer_kind(v: &Value) -> bool {
    one_of(v, &["files", "image"])
}

fn utf8_len_ok(s: &str) -> bool {
    s.len() <= CLIPBOARD_MAX
}

fn is_clip_text(v: &Value) -> bool {
    v.as_str().is_some_and(|s| !s.is_empty() && utf8_len_ok(s))
}

/// Clipboard text that may be empty ("" = none).
fn is_opt_text(v: &Value) -> bool {
    v.as_str() == Some("") || is_clip_text(v)
}

fn cap_name_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 24
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

/// Capability names (unknown ones are ignored, so a newer peer can add some).
fn is_caps(v: &Value) -> bool {
    v.as_array()
        .is_some_and(|a| a.len() <= 16 && a.iter().all(|c| c.as_str().is_some_and(cap_name_ok)))
}

fn is_port(v: &Value) -> bool {
    int_in(v, 1, 65534)
}

fn is_pem(v: &Value) -> bool {
    v.as_str().is_some_and(|s| {
        s.chars().count() <= 4096
            && s.starts_with("-----BEGIN CERTIFICATE-----")
            && s.trim_end().ends_with("-----END CERTIFICATE-----")
    })
}

fn is_devices(v: &Value) -> bool {
    let Some(list) = v.as_array() else {
        return false;
    };
    if list.is_empty() || list.len() > 8 {
        return false;
    }
    list.iter().all(|d| {
        d.as_object().is_some_and(|d| {
            d.len() == 2 && d.get("name").is_some_and(is_name) && d.get("kind").is_some_and(is_kind)
        })
    })
}

/// A relative path inside a transfer: `a/b/c`, components separated by `/`, none empty,
/// `.` or `..`, no NUL, control, format or line-separator characters, each ≤ MAX_NAME and
/// the whole ≤ MAX_PATH bytes of UTF-8, at most MAX_DEPTH components. Nothing here can name
/// a place outside the folder it is created in.
pub fn relpath_ok(path: &str) -> bool {
    if path.is_empty() || path.len() > MAX_PATH {
        return false;
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() > MAX_DEPTH {
        return false;
    }
    parts.iter().all(|part| {
        !matches!(*part, "" | "." | "..")
            && part.len() <= MAX_NAME
            && !part.chars().any(forbidden_in_path)
    })
}

fn is_size(v: &Value) -> bool {
    int_in(v, 0, 1 << 50)
}

/// Nanoseconds since the epoch.
fn is_mtime(v: &Value) -> bool {
    int_in(v, 0, (1i128 << 63) - 1)
}

fn keys_are(o: &Map<String, Value>, keys: &[&str]) -> bool {
    o.len() == keys.len() && keys.iter().all(|k| o.contains_key(*k))
}

/// The entries of a transfer: folders and files (kind files) or one image (kind image).
/// Paths are unique and each one's parent folder is listed before it, so the tree is well
/// formed before anything is created.
pub fn is_entries(v: &Value) -> bool {
    let Some(list) = v.as_array() else {
        return false;
    };
    if list.is_empty() || list.len() > MAX_FILES {
        return false;
    }
    if let Some(first) = list[0].as_object() {
        if first.get("type").and_then(Value::as_str) == Some("image") {
            return list.len() == 1
                && keys_are(first, &["type", "mime", "size", "sha256"])
                && one_of(&first["mime"], &IMAGE_TYPES)
                && is_size(&first["size"])
                && first["size"].as_u64().unwrap_or(0) > 0
                && is_hex64(&first["sha256"]);
        }
    }
    let mut dirs: HashSet<&str> = HashSet::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for e in list {
        let Some(e) = e.as_object() else { return false };
        let t = e.get("type").and_then(Value::as_str);
        match t {
            Some("dir") => {
                if !keys_are(e, &["type", "path", "mtime"]) {
                    return false;
                }
            }
            Some("file") => {
                if !keys_are(e, &["type", "path", "size", "mtime", "sha256"])
                    || !is_size(&e["size"])
                    || !is_hex64(&e["sha256"])
                {
                    return false;
                }
            }
            _ => return false,
        }
        let Some(path) = e["path"].as_str() else {
            return false;
        };
        if !relpath_ok(path) || !is_mtime(&e["mtime"]) || seen.contains(path) {
            return false;
        }
        let parent = path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        if !parent.is_empty() && !dirs.contains(parent) {
            return false;
        }
        seen.insert(path);
        if t == Some("dir") {
            dirs.insert(path);
        }
    }
    true
}

/// Names in a `drag` notice (the first top-level items of what is dragged).
pub const DRAG_NAMES_MAX: usize = 8;

/// The names in a `drag` notice: 1 … DRAG_NAMES_MAX single path components (what a
/// transfer's top-level entry may be called), unique.
fn is_drag_names(v: &Value) -> bool {
    let Some(list) = v.as_array() else {
        return false;
    };
    if list.is_empty() || list.len() > DRAG_NAMES_MAX {
        return false;
    }
    let mut seen: HashSet<&str> = HashSet::new();
    list.iter().all(|n| {
        n.as_str()
            .is_some_and(|s| !s.contains('/') && relpath_ok(s) && seen.insert(s))
    })
}

fn is_drag_count(v: &Value) -> bool {
    int_in(v, 1, MAX_FILES as i128)
}

/// A `drag` notice counts at least the names it lists.
fn drag_consistent(m: &Msg) -> bool {
    let names = m["names"].as_array().map_or(0, Vec::len) as u64;
    m["count"].as_u64().is_some_and(|c| c >= names)
}

/// `kind` matches the entries; only an image may bring text along.
fn transfer_consistent(m: &Msg) -> bool {
    let image = m["entries"][0]["type"] == "image";
    image == (m["kind"] == "image") && (image || m["text"] == "")
}

/// (fields, max frame bytes) of each message type.
fn schema(t: &str) -> Option<(&'static [(&'static str, Check)], usize)> {
    const HELLO: &[(&str, Check)] = &[("id", is_id), ("name", is_name)];
    const NONE: &[(&str, Check)] = &[];
    const RE: &[(&str, Check)] = &[("re", is_seq)];
    const SWITCH: &[(&str, Check)] = &[("side", is_side), ("t", is_unit), ("devices", is_devices)];
    const SWITCH_REFUSED: &[(&str, Check)] = &[("re", is_seq), ("reason", is_reason)];
    const CLIPBOARD: &[(&str, Check)] = &[("text", is_clip_text)];
    const REASON: &[(&str, Check)] = &[("reason", is_reason)];
    const INTRO: &[(&str, Check)] = &[
        ("id", is_id),
        ("name", is_name),
        ("cert", is_pem),
        ("port", is_port),
    ];
    const INTRO_OK: &[(&str, Check)] = &[("id", is_id), ("name", is_name), ("cert", is_pem)];
    const COMMIT: &[(&str, Check)] = &[("commit", is_hex64)];
    const REVEAL: &[(&str, Check)] = &[("nonce", is_hex64)];
    const CONFIRM: &[(&str, Check)] = &[("accept", is_bool)];
    const TRANSFER: &[(&str, Check)] = &[
        ("id", is_id),
        ("kind", is_transfer_kind),
        ("entries", is_entries),
        ("text", is_opt_text),
    ];
    const TRANSFER_REFUSED: &[(&str, Check)] = &[("re", is_seq), ("reason", is_transfer_reason)];
    const SKIP: &[(&str, Check)] = &[("index", is_skip_index)];
    const DONE: &[(&str, Check)] = &[("re", is_seq), ("failed", is_failed_count)];
    const CANCEL: &[(&str, Check)] = &[("reason", is_transfer_reason)];
    const DRAG: &[(&str, Check)] = &[
        ("names", is_drag_names),
        ("count", is_drag_count),
        ("bytes", is_size),
    ];
    Some(match t {
        // authenticated peer channel (client -> server requests, server -> client answers)
        "hello" => (HELLO, SMALL),
        "ping" => (NONE, SMALL),
        "pong" => (RE, SMALL),
        "switch" => (SWITCH, SMALL),
        "switch_ok" => (RE, SMALL),
        "switch_refused" => (SWITCH_REFUSED, SMALL),
        "clipboard" => (CLIPBOARD, FRAME_MAX),
        // files dragged across the edge are coming (only to a peer that announced `drag`)
        "drag" => (DRAG, SMALL),
        "ack" => (RE, SMALL),
        "unpair" => (NONE, SMALL),
        "error" => (REASON, SMALL),
        // pairing introduction (separate listener, open only while pairing)
        "pair_intro" => (INTRO, SMALL),
        "pair_intro_ok" => (INTRO_OK, SMALL),
        "pair_refused" => (REASON, SMALL),
        // pairing session (mutually authenticated TLS with the introduced certificates)
        "pair_hello" => (HELLO, SMALL),
        "pair_commit" => (COMMIT, SMALL),
        "pair_reveal" => (REVEAL, SMALL),
        "pair_confirm" => (CONFIRM, SMALL),
        // transfer connection (copied files and images), client = the sender
        "transfer" => (TRANSFER, MANIFEST_MAX),
        "transfer_ok" => (RE, SMALL),
        "transfer_refused" => (TRANSFER_REFUSED, SMALL),
        "transfer_skip" => (SKIP, SMALL),
        "transfer_end" => (NONE, SMALL),
        "transfer_done" => (DONE, SMALL),
        "transfer_cancel" => (CANCEL, SMALL),
        _ => return None,
    })
}

fn is_skip_index(v: &Value) -> bool {
    int_in(v, 0, MAX_FILES as i128 - 1)
}

fn is_failed_count(v: &Value) -> bool {
    int_in(v, 0, MAX_FILES as i128)
}

/// Fields a message may carry or leave out (added after v1 shipped: older peers send
/// messages without them, and refuse messages that have them).
fn optional(t: &str) -> &'static [(&'static str, Check)] {
    const HELLO: &[(&str, Check)] = &[("port", is_port), ("caps", is_caps)];
    if t == "hello" { HELLO } else { &[] }
}

/// Every message type.
pub const ALL: &[&str] = &[
    "hello",
    "ping",
    "pong",
    "switch",
    "switch_ok",
    "switch_refused",
    "clipboard",
    "drag",
    "ack",
    "unpair",
    "error",
    "pair_intro",
    "pair_intro_ok",
    "pair_refused",
    "pair_hello",
    "pair_commit",
    "pair_reveal",
    "pair_confirm",
    "transfer",
    "transfer_ok",
    "transfer_refused",
    "transfer_skip",
    "transfer_end",
    "transfer_done",
    "transfer_cancel",
];
/// What a peer-channel server accepts.
pub const PEER_CLIENT: &[&str] = &["hello", "ping", "switch", "clipboard", "drag", "unpair"];
pub const PEER_SERVER: &[&str] = &[
    "hello",
    "pong",
    "switch_ok",
    "switch_refused",
    "ack",
    "error",
];
pub const INTRO_CLIENT: &[&str] = &["pair_intro"];
pub const INTRO_SERVER: &[&str] = &["pair_intro_ok", "pair_refused"];
pub const PAIR: &[&str] = &[
    "pair_hello",
    "pair_commit",
    "pair_reveal",
    "pair_confirm",
    "pair_refused",
    "error",
];
/// The first message on the authenticated port says what the connection is for.
pub const FIRST: &[&str] = &["hello", "transfer"];
/// What the sender of a transfer says after the manifest (plus data frames).
pub const TRANSFER_CLIENT: &[&str] = &["transfer_skip", "transfer_end", "transfer_cancel"];
pub const TRANSFER_SENDER: &[&str] = &[
    "transfer",
    "transfer_skip",
    "transfer_end",
    "transfer_cancel",
];
pub const TRANSFER_SERVER: &[&str] = &[
    "transfer_ok",
    "transfer_refused",
    "transfer_done",
    "transfer_cancel",
    "error",
];
/// What the server of the authenticated port may answer before it knows the connection's role.
pub const FIRST_ANSWERS: &[&str] = &[
    "hello",
    "pong",
    "switch_ok",
    "switch_refused",
    "ack",
    "error",
    "transfer_ok",
    "transfer_refused",
    "transfer_done",
    "transfer_cancel",
];

/// Raises a protocol error unless `msg` is a well-formed message of an allowed type.
/// `size` is the length of its frame body (0: unknown).
pub fn validate(msg: &Value, allowed: &[&str], size: usize) -> Result<(), Error> {
    let Some(m) = msg.as_object() else {
        return Err(perr("the message is not an object"));
    };
    let t = m.get("type").and_then(Value::as_str);
    let Some((t, (fields, limit))) = t.and_then(|t| Some((t, schema(t)?))) else {
        let shown: String = m
            .get("type")
            .map(|v| v.to_string())
            .unwrap_or_default()
            .chars()
            .take(32)
            .collect();
        return Err(perr(format!("message type not allowed: {shown}")));
    };
    if !allowed.contains(&t) {
        return Err(perr(format!("message type not allowed: {t}")));
    }
    if size > limit {
        return Err(perr(format!("{t} message too large")));
    }
    if !m
        .get("v")
        .is_some_and(|v| int_in(v, PROTOCOL_VERSION as i128, PROTOCOL_VERSION as i128))
    {
        return Err(perr("incompatible protocol version"));
    }
    let opt = optional(t);
    let required = fields.len() + 3;
    let present_opt = opt.iter().filter(|(k, _)| m.contains_key(*k)).count();
    let known = |k: &str| {
        matches!(k, "v" | "type" | "seq")
            || fields.iter().any(|(f, _)| *f == k)
            || opt.iter().any(|(f, _)| *f == k)
    };
    if m.len() != required + present_opt
        || !m.keys().all(|k| known(k))
        || !fields.iter().all(|(k, _)| m.contains_key(*k))
        || !m.contains_key("seq")
    {
        return Err(perr(format!("invalid fields in {t}")));
    }
    if !is_seq(&m["seq"]) {
        return Err(perr("invalid sequence number"));
    }
    for (name, check) in fields.iter().chain(opt.iter()) {
        if let Some(v) = m.get(*name) {
            if !check(v) {
                return Err(perr(format!("invalid field {name} in {t}")));
            }
        }
    }
    if t == "transfer" && !transfer_consistent(m) {
        return Err(perr(format!("inconsistent fields in {t}")));
    }
    if t == "drag" && !drag_consistent(m) {
        return Err(perr(format!("inconsistent fields in {t}")));
    }
    Ok(())
}

// ================================================================== strict JSON
/// A JSON value parsed strictly: duplicate keys are an error (serde_json already refuses
/// NaN/Infinity, lone surrogates and invalid UTF-8, and nesting deeper than 128 levels).
struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(StrictVisitor)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Strict;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_bool<E>(self, v: bool) -> Result<Strict, E> {
        Ok(Strict(Value::Bool(v)))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Strict, E> {
        Ok(Strict(Value::Number(v.into())))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Strict, E> {
        Ok(Strict(Value::Number(v.into())))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Strict, E> {
        Number::from_f64(v)
            .map(|n| Strict(Value::Number(n)))
            .ok_or_else(|| E::custom("constant not allowed"))
    }
    fn visit_str<E>(self, v: &str) -> Result<Strict, E> {
        Ok(Strict(Value::String(v.to_string())))
    }
    fn visit_string<E>(self, v: String) -> Result<Strict, E> {
        Ok(Strict(Value::String(v)))
    }
    fn visit_unit<E>(self) -> Result<Strict, E> {
        Ok(Strict(Value::Null))
    }
    fn visit_none<E>(self) -> Result<Strict, E> {
        Ok(Strict(Value::Null))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Strict, A::Error> {
        let mut out = Vec::new();
        while let Some(Strict(v)) = seq.next_element()? {
            out.push(v);
        }
        Ok(Strict(Value::Array(out)))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Strict, A::Error> {
        let mut out = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if out.contains_key(&key) {
                return Err(de::Error::custom(format!("repeated field: {key}")));
            }
            let Strict(v) = map.next_value()?;
            out.insert(key, v);
        }
        Ok(Strict(Value::Object(out)))
    }
}

/// Parses JSON strictly (see [`Strict`]); errors are protocol errors.
pub fn parse_strict(body: &[u8]) -> Result<Value, Error> {
    let Strict(v) = serde_json::from_slice(body).map_err(|e| perr(format!("invalid JSON: {e}")))?;
    Ok(v)
}

pub fn decode(body: &[u8], allowed: &[&str]) -> Result<Msg, Error> {
    let v = parse_strict(body)?;
    validate(&v, allowed, body.len())?;
    match v {
        Value::Object(m) => Ok(m),
        _ => Err(perr("the message is not an object")),
    }
}

pub fn encode(msg: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(msg).expect("JSON values always serialize");
    let mut out = (body.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(&body);
    out
}

// ================================================================== the connection
/// The socket under a connection: what timeouts and shutdowns act on.
pub trait RawSocket: Send + Sync {
    fn set_read_timeout(&self, t: Option<Duration>) -> io::Result<()>;
    fn set_write_timeout(&self, t: Option<Duration>) -> io::Result<()>;
    fn set_nonblocking(&self, on: bool) -> io::Result<()>;
    /// Ends the connection both ways, waking a thread blocked on it.
    fn shutdown(&self);
}

impl RawSocket for std::net::TcpStream {
    fn set_read_timeout(&self, t: Option<Duration>) -> io::Result<()> {
        std::net::TcpStream::set_read_timeout(self, t)
    }
    fn set_write_timeout(&self, t: Option<Duration>) -> io::Result<()> {
        std::net::TcpStream::set_write_timeout(self, t)
    }
    fn set_nonblocking(&self, on: bool) -> io::Result<()> {
        std::net::TcpStream::set_nonblocking(self, on)
    }
    fn shutdown(&self) {
        let _ = std::net::TcpStream::shutdown(self, std::net::Shutdown::Both);
    }
}

#[cfg(unix)]
impl RawSocket for std::os::unix::net::UnixStream {
    fn set_read_timeout(&self, t: Option<Duration>) -> io::Result<()> {
        std::os::unix::net::UnixStream::set_read_timeout(self, t)
    }
    fn set_write_timeout(&self, t: Option<Duration>) -> io::Result<()> {
        std::os::unix::net::UnixStream::set_write_timeout(self, t)
    }
    fn set_nonblocking(&self, on: bool) -> io::Result<()> {
        std::os::unix::net::UnixStream::set_nonblocking(self, on)
    }
    fn shutdown(&self) {
        let _ = std::os::unix::net::UnixStream::shutdown(self, std::net::Shutdown::Both);
    }
}

pub trait Stream: Read + Write + Send {}
impl<T: Read + Write + Send> Stream for T {}

/// A binary data frame of a transfer connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataFrame {
    pub seq: u64,
    pub index: u32,
    pub offset: u64,
    pub data: Vec<u8>,
}

/// What [`Conn::recv`] returns.
#[derive(Debug)]
pub enum Frame {
    Msg(Msg),
    Data(DataFrame),
}

impl Frame {
    /// The message's type ("data" for a data frame).
    pub fn kind(&self) -> &str {
        match self {
            Frame::Msg(m) => m.get("type").and_then(Value::as_str).unwrap_or(""),
            Frame::Data(_) => "data",
        }
    }
}

/// One framed, sequenced connection. Exactly one thread drives it at a time; another
/// thread may only shut it down (through [`Conn::shutdown_handle`]).
pub struct Conn {
    io: Box<dyn Stream>,
    sock: std::sync::Arc<dyn RawSocket>,
    pub tx: u64,
    pub rx: u64,
    recv_types: &'static [&'static str],
    send_types: &'static [&'static str],
    data: bool,
    max_frame: usize,
    /// The peer's certificate (DER) on a TLS connection.
    pub peer_cert: Option<Vec<u8>>,
    closed: bool,
}

impl Conn {
    pub fn new(
        io: Box<dyn Stream>,
        sock: std::sync::Arc<dyn RawSocket>,
        recv_types: &'static [&'static str],
        send_types: &'static [&'static str],
        data: bool,
    ) -> Self {
        let mut c = Self {
            io,
            sock,
            tx: 0,
            rx: 0,
            recv_types,
            send_types,
            data,
            max_frame: SMALL,
            peer_cert: None,
            closed: false,
        };
        c.set_types(recv_types, send_types, data);
        c
    }

    /// What this connection accepts from now on (`data`: binary data frames too).
    pub fn set_types(
        &mut self,
        recv_types: &'static [&'static str],
        send_types: &'static [&'static str],
        data: bool,
    ) {
        self.recv_types = recv_types;
        self.send_types = send_types;
        self.data = data;
        // Biggest frame any type accepted here may need: checked against the length
        // prefix, before reading the body (only the authenticated peer channel accepts
        // `clipboard`, only its first message may be a `transfer`, only a transfer
        // connection carries data frames; everything else stays at SMALL).
        self.max_frame = recv_types
            .iter()
            .filter_map(|t| schema(t).map(|s| s.1))
            .chain(data.then_some(DATA_MAX))
            .max()
            .unwrap_or(SMALL)
            .max(SMALL);
    }

    pub fn max_frame(&self) -> usize {
        self.max_frame
    }

    pub fn shutdown_handle(&self) -> std::sync::Arc<dyn RawSocket> {
        self.sock.clone()
    }

    /// Sends one message; returns its `seq`. What we send is validated with the same rules
    /// as what we receive.
    pub fn send(&mut self, type_: &str, fields: Value) -> Result<u64, Error> {
        self.tx += 1;
        let mut msg = Map::new();
        msg.insert("v".into(), json!(PROTOCOL_VERSION));
        msg.insert("type".into(), json!(type_));
        msg.insert("seq".into(), json!(self.tx));
        if let Value::Object(extra) = fields {
            msg.extend(extra);
        }
        let msg = Value::Object(msg);
        let data = encode(&msg);
        validate(&msg, self.send_types, data.len() - 4)?;
        self.write_all(&data)?;
        Ok(self.tx)
    }

    /// One binary data frame: `payload` (1 … CHUNK_MAX bytes) of entry `index` at `offset`.
    pub fn send_data(&mut self, index: u32, offset: u64, payload: &[u8]) -> Result<u64, Error> {
        if payload.is_empty() || payload.len() > CHUNK_MAX || index as usize >= MAX_FILES {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid data frame",
            )));
        }
        self.tx += 1;
        let mut frame = Vec::with_capacity(4 + DATA_HEADER + payload.len());
        frame.extend_from_slice(&((DATA_HEADER + payload.len()) as u32).to_be_bytes());
        frame.push(DATA_TAG);
        frame.extend_from_slice(&self.tx.to_be_bytes());
        frame.extend_from_slice(&index.to_be_bytes());
        frame.extend_from_slice(&offset.to_be_bytes());
        frame.extend_from_slice(payload);
        self.write_all(&frame)?;
        Ok(self.tx)
    }

    /// Raw bytes (tests of malformed traffic).
    pub fn send_raw(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.write_all(bytes)
    }

    fn write_all(&mut self, data: &[u8]) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::NotConnected,
                "closed",
            )));
        }
        self.sock.set_write_timeout(Some(FRAME_TIMEOUT))?;
        self.io.write_all(data)?;
        self.io.flush()?;
        Ok(())
    }

    fn read_some(&mut self, buf: &mut [u8], deadline: Instant) -> Result<usize, Error> {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(perr("incomplete message"));
            }
            self.sock
                .set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
            match self.io.read(buf) {
                Ok(0) => {
                    return Err(Error::Io(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "connection closed",
                    )));
                }
                Ok(n) => return Ok(n),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(Error::Io(e)),
            }
        }
    }

    fn exact(&mut self, n: usize, deadline: Instant) -> Result<Vec<u8>, Error> {
        let mut buf = vec![0u8; n];
        let mut got = 0;
        while got < n {
            got += self.read_some(&mut buf[got..], deadline)?;
        }
        Ok(buf)
    }

    /// The first byte of the next frame, or Idle if nothing started within `timeout`
    /// (zero: don't wait; None: wait forever).
    fn first_byte(&mut self, timeout: Option<Duration>) -> Result<u8, Error> {
        let mut b = [0u8; 1];
        let result = match timeout {
            Some(t) if t.is_zero() => {
                self.sock.set_nonblocking(true)?;
                let r = self.io.read(&mut b);
                self.sock.set_nonblocking(false)?;
                r
            }
            other => {
                self.sock.set_read_timeout(other)?;
                self.io.read(&mut b)
            }
        };
        match result {
            Ok(0) => Err(Error::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed",
            ))),
            Ok(_) => Ok(b[0]),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Err(Error::Idle)
            }
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// Next frame. Idle if nothing started within `timeout`.
    pub fn recv(&mut self, timeout: Option<Duration>) -> Result<Frame, Error> {
        if self.closed {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::NotConnected,
                "closed",
            )));
        }
        let first = self.first_byte(timeout)?;
        let deadline = Instant::now() + frame_timeout();
        let rest = self.exact(3, deadline)?;
        let n = u32::from_be_bytes([first, rest[0], rest[1], rest[2]]) as usize;
        if n == 0 || n > self.max_frame {
            return Err(perr("invalid message size"));
        }
        let body = self.exact(n, deadline)?;
        if body[0] == DATA_TAG {
            if !self.data || body.len() <= DATA_HEADER {
                return Err(perr("data frame not allowed here"));
            }
            let seq = u64::from_be_bytes(body[1..9].try_into().expect("8 bytes"));
            let index = u32::from_be_bytes(body[9..13].try_into().expect("4 bytes"));
            let offset = u64::from_be_bytes(body[13..21].try_into().expect("8 bytes"));
            if seq != self.rx + 1 {
                return Err(perr(
                    "unexpected sequence number (replayed or out of order)",
                ));
            }
            self.rx = seq;
            return Ok(Frame::Data(DataFrame {
                seq,
                index,
                offset,
                data: body[DATA_HEADER..].to_vec(),
            }));
        }
        let msg = decode(&body, self.recv_types)?;
        if msg["seq"].as_u64() != Some(self.rx + 1) {
            return Err(perr(
                "unexpected sequence number (replayed or out of order)",
            ));
        }
        self.rx += 1;
        Ok(Frame::Msg(msg))
    }

    /// Next frame, which must be a JSON message.
    pub fn recv_msg(&mut self, timeout: Option<Duration>) -> Result<Msg, Error> {
        match self.recv(timeout)? {
            Frame::Msg(m) => Ok(m),
            Frame::Data(_) => Err(perr("data frame not allowed here")),
        }
    }

    pub fn close(&mut self) {
        if !self.closed {
            self.closed = true;
            self.sock.shutdown();
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

impl std::fmt::Debug for Conn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Conn")
            .field("tx", &self.tx)
            .field("rx", &self.rx)
            .field("closed", &self.closed)
            .finish()
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.close();
    }
}

static FRAME_TIMEOUT_OVERRIDE: Mutex<Option<Duration>> = Mutex::new(None);

fn frame_timeout() -> Duration {
    crate::util::lock(&FRAME_TIMEOUT_OVERRIDE).unwrap_or(FRAME_TIMEOUT)
}

/// Tests only: a shorter frame deadline.
#[doc(hidden)]
pub fn set_frame_timeout_for_tests(t: Option<Duration>) {
    *crate::util::lock(&FRAME_TIMEOUT_OVERRIDE) = t;
}

/// Token bucket per key (e.g. source address).
pub struct RateLimiter {
    rate: f64,
    burst: f64,
    clock: Box<dyn Fn() -> f64 + Send + Sync>,
    buckets: Mutex<HashMap<String, (f64, f64)>>,
}

impl RateLimiter {
    pub fn new(rate: f64, burst: u32, clock: Box<dyn Fn() -> f64 + Send + Sync>) -> Self {
        Self {
            rate,
            burst: burst as f64,
            clock,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    pub fn allow(&self, key: &str) -> bool {
        let now = (self.clock)();
        let mut b = crate::util::lock(&self.buckets);
        let (tokens, last) = b.get(key).copied().unwrap_or((self.burst, now));
        let tokens = self.burst.min(tokens + (now - last) * self.rate);
        if b.len() > 512 {
            b.clear();
        }
        if tokens < 1.0 {
            b.insert(key.to_string(), (tokens, now));
            return false;
        }
        b.insert(key.to_string(), (tokens - 1.0, now));
        true
    }
}

#[cfg(all(test, unix))]
mod tests;
