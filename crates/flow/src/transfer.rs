//! Copied files, folders and images between two Flow computers.
//!
//! They travel over a *transfer connection*: a separate TCP connection to the peer's
//! authenticated port, with the same mutual TLS 1.3 and SPKI pin as the peer channel, so a
//! big transfer never delays `switch` or `ping` (docs/FLOW.md §6). This module holds the
//! mechanics, independent of sockets:
//!
//! * sender — [`scan_files`] turns what the user copied (absolute paths) into a manifest:
//!   relative paths, kinds, sizes, mtimes and a SHA-256 per file, folders recursive, links,
//!   special files and macOS bookkeeping files skipped, caps checked before anything is
//!   sent; [`send_entries`] streams the content in data frames, reading each file in chunks
//!   (never whole), and skips a file that changed or disappeared since it was hashed;
//! * receiver — [`Staging`] owns the private folder `<cache>/flow-received/` (0700) and its
//!   clean-up; [`Incoming`] creates one transfer's tree inside it (folders 0700, files 0600,
//!   never an executable bit, never a link, device or fifo, nothing outside the folder:
//!   every path was checked by `proto::relpath_ok`, is made valid and unique for this file
//!   system by [`crate::names`], and is opened step by step without following links),
//!   checks each file's size and SHA-256 as it arrives, restores mtimes and marks the files
//!   as downloaded from another computer (macOS quarantine, Windows Mark of the Web).
//!
//! Nothing received is ever executed or opened: it stays in the staging folder until the
//! user pastes it.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use glisa_i18n::n_;
use serde_json::{Value, json};

use crate::names::{LocalNames, NameRules, is_junk, numbered};
use crate::proto::{self, CHUNK_MAX, Conn};
use crate::text::{Text, human_size};
use crate::util::{Sha256, random_hex, sha256_hex};
use crate::{IMAGE_TYPES, MAX_DEPTH, imageinfo};

/// Bytes of text/uri-list read from the clipboard.
pub const URI_LIST_MAX: usize = 4 << 20;
pub const HASH_CHUNK: usize = 1 << 20;
/// A transfer connection without a frame this long is dropped.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// The receiver answers transfer_end within this.
pub const DONE_TIMEOUT: Duration = Duration::from_secs(60);
/// Received transfers are deleted after a day.
pub const STAGING_TTL: u64 = 24 * 3600;
/// Free space left on the disk after a transfer.
pub const FREE_MARGIN: u64 = 64 << 20;
pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
pub const TEXT_TYPES: [&str; 3] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];

pub fn extension(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "bmp",
    }
}

/// Why a transfer stopped.
#[derive(Debug)]
pub enum XferError {
    /// Cancelled here (`by_peer` false) or by the other computer.
    Cancelled {
        reason: String,
        by_peer: bool,
    },
    /// It can't start or go on: `reason` is a protocol reason (proto::TRANSFER_REASONS),
    /// `message` what to tell the user.
    Refused {
        reason: &'static str,
        message: Option<Text>,
    },
    Protocol(String),
    Io(io::Error),
}

impl XferError {
    pub fn refused(reason: &str, message: Option<Text>) -> Self {
        let reason = proto::TRANSFER_REASONS
            .iter()
            .copied()
            .find(|r| *r == reason)
            .unwrap_or("error");
        XferError::Refused { reason, message }
    }

    pub fn cancelled() -> Self {
        XferError::Cancelled {
            reason: "cancelled".into(),
            by_peer: false,
        }
    }
}

impl std::fmt::Display for XferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            XferError::Cancelled { reason, by_peer } => {
                write!(f, "cancelled ({reason}, by peer: {by_peer})")
            }
            XferError::Refused { reason, message } => {
                write!(f, "{reason}")?;
                if let Some(m) = message {
                    write!(f, ": {}", m.english())?;
                }
                Ok(())
            }
            XferError::Protocol(e) => write!(f, "protocol error: {e}"),
            XferError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for XferError {
    fn from(e: io::Error) -> Self {
        XferError::Io(e)
    }
}

impl From<proto::Error> for XferError {
    fn from(e: proto::Error) -> Self {
        match e {
            proto::Error::Idle => {
                XferError::Io(io::Error::new(io::ErrorKind::TimedOut, "no answer"))
            }
            proto::Error::Protocol(p) => XferError::Protocol(p),
            proto::Error::Io(e) => XferError::Io(e),
        }
    }
}

// ====================================================================== URIs
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn unquote(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex_val(b[i + 1]), hex_val(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Lexical normalization of an absolute `/`-path (Python `os.path.normpath`).
fn normpath(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("/{}", parts.join("/"))
}

#[cfg(unix)]
fn path_from_bytes(bytes: Vec<u8>) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: Vec<u8>) -> Option<PathBuf> {
    String::from_utf8(bytes).ok().map(PathBuf::from)
}

/// Local paths from a text/uri-list (RFC 2483): `file:` URIs with an empty or `localhost`
/// host only; anything else (http:, remote hosts, relative paths) is ignored. Duplicates
/// are dropped, order kept.
pub fn parse_uri_list(data: &[u8], limit: usize) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for raw in data.split(|b| *b == b'\n') {
        let line = trim_bytes(raw);
        if line.is_empty() || line[0] == b'#' || !line.is_ascii() {
            continue;
        }
        let line = std::str::from_utf8(line).unwrap_or("");
        let Some((scheme, rest)) = line.split_once(':') else {
            continue;
        };
        if !scheme.eq_ignore_ascii_case("file") || rest.contains('?') || rest.contains('#') {
            continue;
        }
        let path = if let Some(after) = rest.strip_prefix("//") {
            let (host, path) = match after.find('/') {
                Some(i) => (&after[..i], &after[i..]),
                None => (after, ""),
            };
            if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
                continue;
            }
            path
        } else {
            rest
        };
        let bytes = unquote(path);
        if bytes.first() != Some(&b'/') || bytes.contains(&0) {
            continue;
        }
        let normalized = match String::from_utf8(bytes.clone()) {
            Ok(s) => normpath(&s).into_bytes(),
            Err(_) => bytes,
        };
        if normalized == b"/" {
            continue;
        }
        let normalized = windows_drive(normalized);
        let Some(p) = path_from_bytes(normalized) else {
            continue;
        };
        if !out.contains(&p) {
            out.push(p);
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

/// "/C:/Users/x" → "C:\Users\x" on Windows; unchanged elsewhere.
fn windows_drive(path: Vec<u8>) -> Vec<u8> {
    if cfg!(windows)
        && path.len() >= 3
        && path[0] == b'/'
        && path[1].is_ascii_alphabetic()
        && path[2] == b':'
    {
        return path[1..]
            .iter()
            .map(|b| if *b == b'/' { b'\\' } else { *b })
            .collect();
    }
    path
}

fn trim_bytes(b: &[u8]) -> &[u8] {
    let is_ws = |c: &u8| matches!(c, b'\r' | b'\t' | b' ');
    let start = b.iter().position(|c| !is_ws(c)).unwrap_or(b.len());
    let end = b
        .iter()
        .rposition(|c| !is_ws(c))
        .map(|i| i + 1)
        .unwrap_or(start);
    &b[start..end.max(start)]
}

fn path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        let s = path.to_string_lossy().replace('\\', "/");
        if s.len() >= 2 && s.as_bytes()[1] == b':' {
            format!("/{s}").into_bytes()
        } else {
            s.into_bytes()
        }
    }
}

/// file:// URI of an absolute local path (percent-encoded bytes).
pub fn uri_of(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path_bytes(path) {
        if b.is_ascii_alphanumeric() || b"/_.-~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// What was copied, cheaply (no hashing): paths + type/size/mtime of each. Used so the same
/// copied files aren't sent again to the same computer, nor sent back to the computer they
/// came from.
pub fn quick_key(paths: &[PathBuf]) -> String {
    let mut h = Sha256::new();
    for p in paths {
        let sig = match fs::symlink_metadata(p) {
            Ok(m) => format!("{}:{}:{}", mode_of(&m), m.len(), mtime_ns(&m)),
            Err(_) => "-".to_string(),
        };
        h.update(&path_bytes(p));
        h.update(b"\0");
        h.update(sig.as_bytes());
        h.update(b"\n");
    }
    h.hex()
}

fn mode_of(m: &fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!("{:o}", m.mode())
    }
    #[cfg(not(unix))]
    {
        format!("{}", u8::from(m.is_dir()))
    }
}

/// Nanoseconds since the epoch (0 for anything earlier).
pub fn mtime_ns(m: &fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let ns = m.mtime() as i128 * 1_000_000_000 + m.mtime_nsec() as i128;
        ns.clamp(0, i64::MAX as i128) as u64
    }
    #[cfg(not(unix))]
    {
        m.modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos().min(i64::MAX as u128) as u64)
            .unwrap_or(0)
    }
}

/// What identifies a file's content since it was hashed: device, inode, size, mtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileIdent {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: u64,
}

fn ident_of(m: &fs::Metadata) -> FileIdent {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        FileIdent {
            dev: m.dev(),
            ino: m.ino(),
            size: m.len(),
            mtime: mtime_ns(m),
        }
    }
    #[cfg(not(unix))]
    {
        FileIdent {
            dev: 0,
            ino: 0,
            size: m.len(),
            mtime: mtime_ns(m),
        }
    }
}

// ===================================================================== sender
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Dir,
    File,
    Image,
}

#[derive(Debug, Clone)]
pub struct Source {
    pub kind: SourceKind,
    /// Path in the manifest ("" for an image).
    pub rel: String,
    /// Where it is on this computer.
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ns: u64,
    pub sha256: String,
    pub ident: FileIdent,
}

#[derive(Debug, Clone, Default)]
pub struct Outgoing {
    /// "files" | "image"
    pub kind: String,
    pub sources: Vec<Source>,
    pub text: String,
    /// Top-level things the user copied.
    pub items: u64,
    pub files: u64,
    pub folders: u64,
    /// Links, special files, unreadable or odd names.
    pub skipped: u64,
    /// Name of the first top-level item.
    pub first: String,
    pub mime: String,
}

impl Outgoing {
    pub fn total(&self) -> u64 {
        self.sources
            .iter()
            .filter(|s| s.kind != SourceKind::Dir)
            .map(|s| s.size)
            .sum()
    }

    pub fn manifest(&self) -> Value {
        Value::Array(
            self.sources
                .iter()
                .map(|s| match s.kind {
                    SourceKind::Dir => json!({"type": "dir", "path": s.rel, "mtime": s.mtime_ns}),
                    SourceKind::File => json!({"type": "file", "path": s.rel, "size": s.size,
                                               "mtime": s.mtime_ns, "sha256": s.sha256}),
                    SourceKind::Image => json!({"type": "image", "mime": self.mime, "size": s.size,
                                                "sha256": s.sha256}),
                })
                .collect(),
        )
    }
}

fn limit_text(limit: u64) -> Text {
    Text::new(n_!(
        "The copied files are larger than {limit}, the limit for sending them (Flow → Options)"
    ))
    .arg("limit", human_size(limit))
}

fn too_many(max: usize) -> Text {
    Text::new(n_!(
        "More than {count} files and folders were copied: that's the most Flow sends at once"
    ))
    .arg("count", max as u64)
}

fn too_deep() -> Text {
    Text::new(n_!(
        "The copied folders are nested more than {depth} levels deep: Flow doesn't send them"
    ))
    .arg("depth", MAX_DEPTH as u64)
}

/// Limits of a scan (the file count is a parameter so tests can lower it).
#[derive(Debug, Clone, Copy)]
pub struct ScanLimits {
    pub max_bytes: u64,
    pub max_files: usize,
}

/// The manifest for what the user copied (stat only; hash with [`hash_sources`]). Fails
/// when a cap is exceeded or there is nothing to send. Symlinks are never followed; only
/// the copied paths and what is inside copied folders is ever included.
pub fn scan_files(
    paths: &[PathBuf],
    limits: ScanLimits,
    cancel: &AtomicBool,
) -> Result<Outgoing, XferError> {
    let mut out = Outgoing {
        kind: "files".into(),
        ..Default::default()
    };
    let mut used: Vec<String> = Vec::new();
    let mut total = 0u64;
    for path in paths {
        let Ok(meta) = fs::symlink_metadata(path) else {
            out.skipped += 1;
            continue;
        };
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if is_junk(name) {
            continue;
        }
        let ft = meta.file_type();
        if !(ft.is_dir() || ft.is_file()) || ft.is_symlink() || !proto::relpath_ok(name) {
            out.skipped += 1;
            continue;
        }
        let name = unique_top(name, &mut used, limits.max_files)?;
        out.items += 1;
        if out.first.is_empty() {
            out.first = name.clone();
        }
        if ft.is_dir() {
            add(
                &mut out,
                &mut total,
                limits,
                Source::dir(&name, path, &meta),
            )?;
            walk(&mut out, &mut total, limits, path, &name, 1, cancel)?;
        } else {
            add(
                &mut out,
                &mut total,
                limits,
                Source::file(&name, path, &meta),
            )?;
        }
    }
    if out.sources.is_empty() {
        return Err(XferError::refused(
            "error",
            Some(Text::new(n_!(
                "Nothing that was copied can be sent: links and special files aren't sent"
            ))),
        ));
    }
    Ok(out)
}

pub(crate) fn unique_top(
    name: &str,
    used: &mut Vec<String>,
    max: usize,
) -> Result<String, XferError> {
    if !used.iter().any(|u| u == name) {
        used.push(name.to_string());
        return Ok(name.to_string());
    }
    for n in 2..10_000 {
        let cand = numbered(name, n);
        if !used.contains(&cand) {
            used.push(cand.clone());
            return Ok(cand);
        }
    }
    Err(XferError::refused("too_many", Some(too_many(max))))
}

impl Source {
    fn dir(rel: &str, path: &Path, m: &fs::Metadata) -> Self {
        Source {
            kind: SourceKind::Dir,
            rel: rel.to_string(),
            path: path.to_path_buf(),
            size: 0,
            mtime_ns: mtime_ns(m),
            sha256: String::new(),
            ident: FileIdent::default(),
        }
    }

    fn file(rel: &str, path: &Path, m: &fs::Metadata) -> Self {
        Source {
            kind: SourceKind::File,
            rel: rel.to_string(),
            path: path.to_path_buf(),
            size: m.len(),
            mtime_ns: mtime_ns(m),
            sha256: String::new(),
            ident: ident_of(m),
        }
    }
}

fn add(
    out: &mut Outgoing,
    total: &mut u64,
    limits: ScanLimits,
    src: Source,
) -> Result<(), XferError> {
    if out.sources.len() >= limits.max_files {
        return Err(XferError::refused(
            "too_many",
            Some(too_many(limits.max_files)),
        ));
    }
    if src.kind == SourceKind::File {
        out.files += 1;
        *total += src.size;
        if *total > limits.max_bytes {
            return Err(XferError::refused(
                "too_large",
                Some(limit_text(limits.max_bytes)),
            ));
        }
    } else {
        out.folders += 1;
    }
    out.sources.push(src);
    Ok(())
}

fn walk(
    out: &mut Outgoing,
    total: &mut u64,
    limits: ScanLimits,
    dir: &Path,
    rel: &str,
    depth: usize,
    cancel: &AtomicBool,
) -> Result<(), XferError> {
    if cancel.load(Ordering::SeqCst) {
        return Err(XferError::cancelled());
    }
    let Ok(rd) = fs::read_dir(dir) else {
        out.skipped += 1;
        return Ok(());
    };
    let mut children: Vec<fs::DirEntry> = rd.filter_map(Result::ok).collect();
    children.sort_by_key(|d| d.file_name());
    for de in children {
        let fname = de.file_name();
        let Some(name) = fname.to_str() else {
            out.skipped += 1; // a name the other side would refuse (not UTF-8)
            continue;
        };
        if is_junk(name) {
            continue;
        }
        let child = format!("{rel}/{name}");
        let Ok(meta) = fs::symlink_metadata(de.path()) else {
            out.skipped += 1;
            continue;
        };
        let ft = meta.file_type();
        if ft.is_symlink() || !(ft.is_dir() || ft.is_file()) {
            out.skipped += 1; // links, devices, fifos, sockets
            continue;
        }
        if depth + 1 > MAX_DEPTH {
            return Err(XferError::refused("too_many", Some(too_deep())));
        }
        if !proto::relpath_ok(&child) {
            out.skipped += 1;
            continue;
        }
        if ft.is_dir() {
            add(out, total, limits, Source::dir(&child, &de.path(), &meta))?;
            walk(out, total, limits, &de.path(), &child, depth + 1, cancel)?;
        } else {
            add(out, total, limits, Source::file(&child, &de.path(), &meta))?;
        }
    }
    Ok(())
}

/// Opens a regular file for reading without following a link.
fn open_nofollow(path: &Path) -> io::Result<File> {
    let mut opts = fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let f = opts.open(path)?;
    if !f.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(f)
}

/// SHA-256 of every file, read in chunks. A file that can't be read is left out (counted
/// in `skipped`); its identity (device, inode, size, mtime) is remembered so a file changed
/// afterwards is detected.
pub fn hash_sources(
    out: &mut Outgoing,
    cancel: &AtomicBool,
    progress: &dyn Fn(u64),
) -> Result<(), XferError> {
    let mut keep = Vec::with_capacity(out.sources.len());
    let sources = std::mem::take(&mut out.sources);
    let mut buf = vec![0u8; HASH_CHUNK];
    for mut src in sources {
        if src.kind != SourceKind::File {
            keep.push(src);
            continue;
        }
        let ok = (|| -> Result<bool, XferError> {
            let Ok(mut f) = open_nofollow(&src.path) else {
                return Ok(false);
            };
            let meta = f.metadata()?;
            let mut h = Sha256::new();
            let mut left = meta.len();
            while left > 0 {
                if cancel.load(Ordering::SeqCst) {
                    return Err(XferError::cancelled());
                }
                let want = (left as usize).min(HASH_CHUNK);
                let n = f.read(&mut buf[..want])?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
                left -= n as u64;
                progress(n as u64);
            }
            if left > 0 {
                return Ok(false); // shrank while reading
            }
            src.size = meta.len();
            src.mtime_ns = mtime_ns(&meta);
            src.ident = ident_of(&meta);
            src.sha256 = h.hex();
            Ok(true)
        })();
        match ok {
            Ok(true) => keep.push(src),
            Ok(false) | Err(XferError::Io(_)) => {
                out.skipped += 1;
                out.files = out.files.saturating_sub(1);
            }
            Err(e) => return Err(e),
        }
    }
    out.sources = keep;
    if out.sources.is_empty() {
        return Err(XferError::refused(
            "error",
            Some(Text::new(n_!("The copied files can't be read"))),
        ));
    }
    Ok(())
}

/// One image (already written to a private file) plus, optionally, text.
pub fn image_outgoing(path: &Path, mime: &str, text: &str) -> io::Result<Outgoing> {
    let mut f = File::open(path)?;
    let meta = f.metadata()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; HASH_CHUNK];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(Outgoing {
        kind: "image".into(),
        text: text.to_string(),
        items: 1,
        mime: mime.to_string(),
        sources: vec![Source {
            kind: SourceKind::Image,
            rel: String::new(),
            path: path.to_path_buf(),
            size: meta.len(),
            mtime_ns: mtime_ns(&meta),
            sha256: h.hex(),
            ident: ident_of(&meta),
        }],
        ..Default::default()
    })
}

/// Called before each file is sent (tests).
pub type FileHook = Arc<dyn Fn(&Path) + Send + Sync>;

/// Test hooks of the sender (a slow network, a file deleted while sending).
#[derive(Clone, Default)]
pub struct SendHooks {
    pub poll_delay: Option<Duration>,
    pub before_file: Option<FileHook>,
}

/// Streams every file's content (chunks ≤ CHUNK_MAX) after transfer_ok. Returns how many
/// files couldn't be sent (changed or gone: the receiver is told with transfer_skip and
/// drops them).
pub fn send_entries(
    conn: &mut Conn,
    out: &Outgoing,
    cancel: &AtomicBool,
    progress: &dyn Fn(u64),
    hooks: &SendHooks,
) -> Result<usize, XferError> {
    let mut failed = 0;
    for (index, src) in out.sources.iter().enumerate() {
        if src.kind == SourceKind::Dir || src.size == 0 {
            continue;
        }
        if let Some(f) = &hooks.before_file {
            f(&src.path);
        }
        if !send_one(conn, index as u32, src, cancel, progress, hooks)? {
            failed += 1;
        }
    }
    Ok(failed)
}

/// Anything from the receiver while we send: a cancellation or an error ends the transfer.
fn poll(conn: &mut Conn, cancel: &AtomicBool, hooks: &SendHooks) -> Result<(), XferError> {
    if let Some(d) = hooks.poll_delay {
        std::thread::sleep(d);
    }
    if cancel.load(Ordering::SeqCst) {
        return Err(XferError::cancelled());
    }
    let m = match conn.recv(Some(Duration::ZERO)) {
        Err(proto::Error::Idle) => return Ok(()),
        Err(e) => return Err(e.into()),
        Ok(proto::Frame::Msg(m)) => m,
        Ok(proto::Frame::Data(_)) => return Err(XferError::Protocol("data out of place".into())),
    };
    let reason = m
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match m.get("type").and_then(Value::as_str) {
        Some("transfer_cancel") => Err(XferError::Cancelled {
            reason,
            by_peer: true,
        }),
        Some("error") => Err(XferError::refused(
            if reason == "protocol" {
                "protocol"
            } else {
                "error"
            },
            None,
        )),
        other => Err(XferError::Protocol(format!(
            "{} out of place",
            other.unwrap_or("?")
        ))),
    }
}

fn send_one(
    conn: &mut Conn,
    index: u32,
    src: &Source,
    cancel: &AtomicBool,
    progress: &dyn Fn(u64),
    hooks: &SendHooks,
) -> Result<bool, XferError> {
    let mut sent = 0u64;
    let mut ok = false;
    if let Ok(mut f) = open_nofollow(&src.path) {
        ok = f
            .metadata()
            .map(|m| ident_of(&m) == src.ident)
            .unwrap_or(false);
        if ok {
            let mut h = Sha256::new();
            let mut buf = vec![0u8; CHUNK_MAX];
            while sent < src.size {
                poll(conn, cancel, hooks)?;
                let want = ((src.size - sent) as usize).min(CHUNK_MAX);
                let n = f.read(&mut buf[..want])?;
                if n == 0 {
                    ok = false; // shrank while sending
                    break;
                }
                h.update(&buf[..n]);
                conn.send_data(index, sent, &buf[..n])?;
                sent += n as u64;
                progress(n as u64);
            }
            if ok && h.hex() != src.sha256 {
                ok = false; // changed while sending: the receiver sees it too
            }
        }
    }
    if !ok {
        log::info!("Flow: a copied file changed or disappeared while sending it; skipped");
        if sent < src.size {
            conn.send("transfer_skip", json!({"index": index}))?;
            progress(src.size - sent);
        }
    }
    Ok(ok)
}

// =================================================================== receiver
/// `<cache>/flow-received`.
pub fn default_staging_root() -> PathBuf {
    glisa_common::paths::cache_dir().join(crate::local::STAGING_DIR)
}

fn is_transfer_folder(name: &str) -> Option<u64> {
    let (secs, rand) = name.split_once('-')?;
    if secs.len() == 10
        && secs.bytes().all(|b| b.is_ascii_digit())
        && crate::util::is_hex_lower(rand, 8)
    {
        secs.parse().ok()
    } else {
        None
    }
}

/// `root` holds one folder per received transfer, named `<unix time>-<8 hex>`: private
/// (0700) and cleaned up after STAGING_TTL.
#[derive(Debug, Clone)]
pub struct Staging {
    pub root: PathBuf,
}

impl Staging {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Creates/checks the root (a real directory of ours, 0700).
    pub fn prepare(&self) -> io::Result<fsops::Dir> {
        if let Some(parent) = self.root.parent() {
            fs::create_dir_all(parent)?;
        }
        match fsops::mkdir_path(&self.root) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        fsops::open_root(&self.root)
    }

    pub fn free_bytes(&self) -> u64 {
        fsops::free_bytes(&self.root).unwrap_or(0)
    }

    /// (transfer id, path) of a new empty private folder.
    pub fn new_folder(&self) -> io::Result<(String, PathBuf)> {
        let root = self.prepare()?;
        for _ in 0..8 {
            let tid = format!("{:010}-{}", crate::util::unix_time() as u64, random_hex(4));
            match fsops::mkdir(&root, &tid) {
                Ok(()) => return Ok((tid.clone(), self.root.join(&tid))),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("can't create a staging folder"))
    }

    /// A new empty private file (0600) under `.spool/` (an image read from the clipboard).
    pub fn spool_file(&self) -> io::Result<PathBuf> {
        let root = self.prepare()?;
        match fsops::mkdir(&root, ".spool") {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let spool = fsops::open_dir(&self.root, ".spool")?;
        let name = format!("image-{}", random_hex(8));
        drop(fsops::create_file(&spool, &name)?);
        Ok(self.root.join(".spool").join(name))
    }

    pub fn remove(&self, tid: &str) {
        if is_transfer_folder(tid).is_some() {
            let _ = fs::remove_dir_all(self.root.join(tid));
        }
    }

    /// Delete transfers older than `max_age` seconds (0: all), except `keep`.
    pub fn cleanup(&self, max_age: u64, keep: &[String]) -> usize {
        let Ok(rd) = fs::read_dir(&self.root) else {
            return 0;
        };
        let now = crate::util::unix_time() as u64;
        let mut removed = 0;
        for de in rd.filter_map(Result::ok) {
            let name = de.file_name().to_string_lossy().into_owned();
            if keep.contains(&name) {
                continue;
            }
            let path = de.path();
            let born = match is_transfer_folder(&name) {
                Some(t) => t,
                None => match fs::symlink_metadata(&path) {
                    Ok(m) => mtime_ns(&m) / 1_000_000_000,
                    Err(_) => continue,
                },
            };
            if max_age > 0 && now.saturating_sub(born) < max_age {
                continue;
            }
            let result = match fs::symlink_metadata(&path) {
                Ok(m) if m.is_dir() => fs::remove_dir_all(&path),
                Ok(_) => fs::remove_file(&path),
                Err(e) => Err(e),
            };
            match result {
                Ok(()) => removed += 1,
                Err(e) => log::info!("Flow: couldn't remove a received transfer: {e}"),
            }
        }
        removed
    }
}

/// One accepted transfer being written into `folder` (already created, empty). Feed it the
/// frames in order; any inconsistency is a protocol error (the caller drops the transfer).
pub struct Incoming {
    pub folder: PathBuf,
    pub kind: String,
    pub entries: Vec<Value>,
    pub text: String,
    pub total: u64,
    pub received: u64,
    pub failed: Vec<usize>,
    pub done: Vec<usize>,
    data_entries: Vec<usize>,
    pos: usize,
    file: Option<File>,
    parent: Option<fsops::Dir>,
    written: u64,
    hash: Option<Sha256>,
    names: LocalNames,
    locals: std::collections::HashMap<usize, String>,
    pub image_path: Option<PathBuf>,
    pub image_size: (u64, u64),
}

fn entry_size(e: &Value) -> u64 {
    e.get("size").and_then(Value::as_u64).unwrap_or(0)
}

fn entry_str<'a>(e: &'a Value, key: &str) -> &'a str {
    e.get(key).and_then(Value::as_str).unwrap_or("")
}

impl Incoming {
    pub fn new(folder: PathBuf, kind: &str, entries: Vec<Value>, text: &str) -> Self {
        Self::with_rules(folder, kind, entries, text, NameRules::this_os())
    }

    pub fn with_rules(
        folder: PathBuf,
        kind: &str,
        entries: Vec<Value>,
        text: &str,
        rules: NameRules,
    ) -> Self {
        let data_entries = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| entry_str(e, "type") != "dir" && entry_size(e) > 0)
            .map(|(i, _)| i)
            .collect();
        Self {
            total: entries.iter().map(entry_size).sum(),
            folder,
            kind: kind.to_string(),
            entries,
            text: text.to_string(),
            received: 0,
            failed: Vec::new(),
            done: Vec::new(),
            data_entries,
            pos: 0,
            file: None,
            parent: None,
            written: 0,
            hash: None,
            names: LocalNames::new(rules),
            locals: std::collections::HashMap::new(),
            image_path: None,
            image_size: (0, 0),
        }
    }

    /// The local relative path of entry `i` (valid and unique on this file system).
    fn local(&mut self, i: usize) -> String {
        if let Some(done) = self.locals.get(&i) {
            return done.clone();
        }
        let local = self.choose_local(i);
        self.locals.insert(i, local.clone());
        local
    }

    fn choose_local(&mut self, i: usize) -> String {
        let e = &self.entries[i];
        if entry_str(e, "type") == "image" {
            let now = time::OffsetDateTime::now_utc();
            let name = format!(
                "image-{:04}{:02}{:02}-{:02}{:02}{:02}.{}",
                now.year(),
                now.month() as u8,
                now.day(),
                now.hour(),
                now.minute(),
                now.second(),
                extension(entry_str(e, "mime"))
            );
            return self.names.reserve(&name);
        }
        let path = entry_str(e, "path").to_string();
        self.names.local(&path)
    }

    /// Folders (0700) and empty files now; the rest as data arrives.
    pub fn create(&mut self) -> Result<(), XferError> {
        for i in 0..self.entries.len() {
            let t = entry_str(&self.entries[i], "type").to_string();
            if t == "dir" {
                let local = self.local(i);
                let (parent, name) = split_parent(&local);
                let pfd = fsops::open_dir(&self.folder, parent)?;
                fsops::mkdir(&pfd, name)?;
            } else if entry_size(&self.entries[i]) == 0 {
                if entry_str(&self.entries[i], "sha256") != EMPTY_SHA256 {
                    self.failed.push(i); // an empty file can't have that hash
                    continue;
                }
                self.open(i)?;
                self.finish(i)?;
            }
        }
        Ok(())
    }

    fn open(&mut self, i: usize) -> Result<(), XferError> {
        let local = self.local(i);
        let (parent, name) = split_parent(&local);
        let pfd = fsops::open_dir(&self.folder, parent)?;
        let f = fsops::create_file(&pfd, name)?;
        self.file = Some(f);
        self.parent = Some(pfd);
        self.written = 0;
        self.hash = Some(Sha256::new());
        if entry_str(&self.entries[i], "type") == "image" {
            self.image_path = Some(self.folder.join(&local));
        }
        Ok(())
    }

    fn close(&mut self) {
        self.file = None;
        self.parent = None;
        self.written = 0;
    }

    fn finish(&mut self, i: usize) -> Result<(), XferError> {
        let local = self.local(i);
        let name = split_parent(&local).1.to_string();
        let e = &self.entries[i];
        let digest = self.hash.take().map(Sha256::hex).unwrap_or_default();
        let ok = digest == entry_str(e, "sha256") && self.written == entry_size(e);
        let file = self.file.take();
        let parent = self.parent.take();
        self.written = 0;
        if ok {
            if let Some(f) = &file {
                if entry_str(e, "type") == "file" {
                    fsops::set_file_mtime(f, e.get("mtime").and_then(Value::as_u64).unwrap_or(0))?;
                }
                fsops::mark_downloaded(f, &self.folder.join(&local));
            }
            self.done.push(i);
        } else {
            drop(file);
            if let Some(p) = &parent {
                fsops::unlink(p, &name)?;
            }
            self.failed.push(i);
            log::info!("Flow: a received file didn't match its checksum; deleted");
        }
        Ok(())
    }

    /// The entry the next data frame must belong to.
    pub fn current(&self) -> Option<usize> {
        self.data_entries.get(self.pos).copied()
    }

    pub fn data(&mut self, index: u32, offset: u64, payload: &[u8]) -> Result<(), XferError> {
        let cur = self.current();
        if cur != Some(index as usize) || offset != self.written {
            return Err(XferError::Protocol("data out of order".into()));
        }
        let cur = index as usize;
        let size = entry_size(&self.entries[cur]);
        if self.written + payload.len() as u64 > size {
            return Err(XferError::Protocol("more data than declared".into()));
        }
        if self.file.is_none() {
            let e = &self.entries[cur];
            if entry_str(e, "type") == "image"
                && imageinfo::sniff(&payload[..payload.len().min(16)]) != Some(entry_str(e, "mime"))
            {
                return Err(XferError::refused(
                    "failed",
                    Some(Text::new(n_!("it isn't the kind of image it said it was"))),
                ));
            }
            self.open(cur)?;
        }
        self.file.as_mut().expect("open file").write_all(payload)?;
        self.hash.as_mut().expect("hash").update(payload);
        self.written += payload.len() as u64;
        self.received += payload.len() as u64;
        if self.written == size {
            self.finish(cur)?;
            self.pos += 1;
        }
        Ok(())
    }

    pub fn skip(&mut self, index: u32) -> Result<(), XferError> {
        let Some(cur) = self.current().filter(|c| *c == index as usize) else {
            return Err(XferError::Protocol("skip out of order".into()));
        };
        self.received += entry_size(&self.entries[cur]) - self.written;
        if self.file.is_some() {
            let local = self.local(cur);
            let name = split_parent(&local).1.to_string();
            let parent = self.parent.take();
            self.close();
            if let Some(p) = &parent {
                fsops::unlink(p, &name)?;
            }
        }
        self.hash = None;
        self.failed.push(cur);
        self.pos += 1;
        Ok(())
    }

    /// transfer_end: everything must have arrived (or been skipped).
    pub fn end(&mut self) -> Result<(), XferError> {
        if self.current().is_some() {
            return Err(XferError::Protocol("transfer_end before all data".into()));
        }
        let mut dirs: Vec<usize> = (0..self.entries.len())
            .filter(|i| entry_str(&self.entries[*i], "type") == "dir")
            .collect();
        dirs.sort_by_key(|i| {
            std::cmp::Reverse(entry_str(&self.entries[*i], "path").matches('/').count())
        });
        for i in dirs {
            let local = self.local(i);
            let (parent, name) = split_parent(&local);
            let pfd = fsops::open_dir(&self.folder, parent)?;
            fsops::set_dir_mtime(
                &pfd,
                name,
                self.entries[i]
                    .get("mtime")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            )?;
        }
        Ok(())
    }

    /// The received image: its signature matches the declared type and its dimensions are
    /// sensible (from the header; nothing is decoded).
    pub fn check_image(&mut self, max_bytes: u64) -> Result<(u64, u64), XferError> {
        let e = &self.entries[0];
        let Some(path) = self
            .image_path
            .clone()
            .filter(|_| !self.failed.contains(&0))
        else {
            return Err(XferError::refused(
                "failed",
                Some(Text::new(n_!("it didn't arrive intact"))),
            ));
        };
        if entry_size(e) > max_bytes {
            return Err(XferError::refused(
                "too_large",
                Some(
                    Text::new(n_!("it's larger than {limit}")).arg("limit", human_size(max_bytes)),
                ),
            ));
        }
        let Ok((mime, w, h)) = imageinfo::inspect(&path) else {
            return Err(XferError::refused(
                "failed",
                Some(Text::new(n_!("it isn't a valid image"))),
            ));
        };
        if mime != entry_str(e, "mime") {
            return Err(XferError::refused(
                "failed",
                Some(Text::new(n_!("it isn't the kind of image it said it was"))),
            ));
        }
        if !imageinfo::acceptable(w, h) {
            return Err(XferError::refused(
                "too_large",
                Some(
                    Text::new(n_!("it's too big ({width} × {height} pixels)"))
                        .arg("width", w)
                        .arg("height", h),
                ),
            ));
        }
        self.image_size = (w, h);
        Ok((w, h))
    }

    pub fn abort(&mut self) {
        self.close();
        let _ = fs::remove_dir_all(&self.folder);
    }

    /// Absolute paths of the top-level items that arrived (for the clipboard).
    pub fn top_level(&mut self) -> Vec<PathBuf> {
        if self.kind == "image" {
            return match &self.image_path {
                Some(p) if !self.failed.contains(&0) => vec![p.clone()],
                _ => Vec::new(),
            };
        }
        let failed: Vec<String> = self
            .failed
            .iter()
            .map(|i| entry_str(&self.entries[*i], "path").to_string())
            .collect();
        let mut out = Vec::new();
        for i in 0..self.entries.len() {
            let path = entry_str(&self.entries[i], "path").to_string();
            if !path.contains('/') && !failed.contains(&path) {
                let local = self.local(i);
                out.push(self.folder.join(local));
            }
        }
        out
    }
}

fn split_parent(local: &str) -> (&str, &str) {
    match local.rsplit_once('/') {
        Some((p, n)) => (p, n),
        None => ("", local),
    }
}

/// Receiver-side caps on a valid `transfer` message (the structure, count and depth were
/// checked by proto).
pub fn check_manifest(
    kind: &str,
    entries: &[Value],
    max_bytes: u64,
    max_image: u64,
    free: u64,
) -> Option<XferError> {
    let total: u64 = entries.iter().map(entry_size).sum();
    if kind == "image" {
        if total > max_image {
            return Some(XferError::refused("too_large", None));
        }
    } else if total > max_bytes {
        return Some(XferError::refused("too_large", None));
    }
    if total.saturating_add(FREE_MARGIN) > free {
        return Some(XferError::refused("no_space", None));
    }
    None
}

/// Data offered on the clipboard: bytes, or a file streamed when someone pastes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipData {
    Bytes(Vec<u8>),
    File(PathBuf),
}

impl ClipData {
    pub fn bytes(&self) -> io::Result<Vec<u8>> {
        match self {
            ClipData::Bytes(b) => Ok(b.clone()),
            ClipData::File(p) => fs::read(p),
        }
    }
}

pub type Offers = BTreeMap<String, ClipData>;

/// What a file manager understands as copied files (never as a cut).
pub fn clipboard_offers(paths: &[PathBuf]) -> Offers {
    let uris: Vec<String> = paths.iter().map(|p| uri_of(p)).collect();
    let text: Vec<u8> = paths
        .iter()
        .map(|p| path_bytes(p))
        .collect::<Vec<_>>()
        .join(&b'\n');
    let mut o = Offers::new();
    o.insert(
        "text/uri-list".into(),
        ClipData::Bytes(format!("{}\r\n", uris.join("\r\n")).into_bytes()),
    );
    o.insert(
        "x-special/gnome-copied-files".into(),
        ClipData::Bytes(format!("copy\n{}", uris.join("\n")).into_bytes()),
    );
    o.insert(
        "text/plain;charset=utf-8".into(),
        ClipData::Bytes(text.clone()),
    );
    o.insert("text/plain".into(), ClipData::Bytes(text));
    o
}

pub fn image_offers(path: &Path, mime: &str, text: &str) -> Offers {
    let mut o = Offers::new();
    o.insert(mime.to_string(), ClipData::File(path.to_path_buf()));
    if !text.is_empty() {
        o.insert(
            "text/plain;charset=utf-8".into(),
            ClipData::Bytes(text.as_bytes().to_vec()),
        );
        o.insert(
            "text/plain".into(),
            ClipData::Bytes(text.as_bytes().to_vec()),
        );
    }
    o
}

pub fn pick_image_type(formats: &[String]) -> Option<&'static str> {
    IMAGE_TYPES
        .iter()
        .copied()
        .find(|m| formats.iter().any(|f| f == m))
}

pub fn pick_text_type(formats: &[String]) -> Option<&'static str> {
    TEXT_TYPES
        .iter()
        .copied()
        .find(|m| formats.iter().any(|f| f == m))
}

/// The SHA-256 of some text, as the "already exchanged" key of the clipboard.
pub fn text_key(text: &str) -> String {
    sha256_hex(text.as_bytes())
}

// ================================================================ file system
/// The few file operations the receiver needs, done so that nothing can lead outside the
/// staging folder: on Unix every component is opened relative to its parent with
/// O_DIRECTORY | O_NOFOLLOW and files are created with O_CREAT | O_EXCL | O_NOFOLLOW; on
/// Windows every existing component is checked not to be a link or junction and files are
/// created new (long paths through `\\?\`).
pub mod fsops {
    use super::*;

    #[cfg(not(unix))]
    pub use portable::*;
    #[cfg(unix)]
    pub use unix::*;

    #[cfg(unix)]
    mod unix {
        use super::*;
        use rustix::fs::{AtFlags, Mode, OFlags, Timespec, Timestamps};
        use std::os::fd::{AsFd, OwnedFd};

        pub struct Dir(OwnedFd);

        fn dir_flags() -> OFlags {
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
        }

        pub fn mkdir_path(path: &Path) -> io::Result<()> {
            rustix::fs::mkdir(path, Mode::from_raw_mode(0o700)).map_err(Into::into)
        }

        /// The staging root: a real directory of this user, made 0700.
        pub fn open_root(path: &Path) -> io::Result<Dir> {
            let fd = rustix::fs::open(path, dir_flags(), Mode::empty())?;
            let st = rustix::fs::fstat(&fd)?;
            if st.st_uid != rustix::process::getuid().as_raw() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("{} doesn't belong to this user", path.display()),
                ));
            }
            if st.st_mode & 0o7777 != 0o700 {
                rustix::fs::fchmod(&fd, Mode::from_raw_mode(0o700))?;
            }
            Ok(Dir(fd))
        }

        /// `rel` inside `root`, opened one component at a time without following links.
        pub fn open_dir(root: &Path, rel: &str) -> io::Result<Dir> {
            let mut fd = rustix::fs::open(root, dir_flags(), Mode::empty())?;
            for part in rel.split('/').filter(|p| !p.is_empty()) {
                fd = rustix::fs::openat(&fd, part, dir_flags(), Mode::empty())?;
            }
            Ok(Dir(fd))
        }

        pub fn mkdir(dir: &Dir, name: &str) -> io::Result<()> {
            rustix::fs::mkdirat(&dir.0, name, Mode::from_raw_mode(0o700))?;
            let fd = rustix::fs::openat(&dir.0, name, dir_flags(), Mode::empty())?;
            rustix::fs::fchmod(&fd, Mode::from_raw_mode(0o700))?;
            Ok(())
        }

        pub fn create_file(dir: &Dir, name: &str) -> io::Result<File> {
            let fd = rustix::fs::openat(
                &dir.0,
                name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            )?;
            rustix::fs::fchmod(&fd, Mode::from_raw_mode(0o600))?; // whatever the umask, never executable
            Ok(File::from(fd))
        }

        pub fn unlink(dir: &Dir, name: &str) -> io::Result<()> {
            rustix::fs::unlinkat(&dir.0, name, AtFlags::empty()).map_err(Into::into)
        }

        fn ts(ns: u64) -> Timestamps {
            let t = Timespec {
                tv_sec: (ns / 1_000_000_000) as _,
                tv_nsec: (ns % 1_000_000_000) as _,
            };
            Timestamps {
                last_access: t,
                last_modification: t,
            }
        }

        pub fn set_file_mtime(f: &File, ns: u64) -> io::Result<()> {
            rustix::fs::futimens(f.as_fd(), &ts(ns)).map_err(Into::into)
        }

        pub fn set_dir_mtime(dir: &Dir, name: &str, ns: u64) -> io::Result<()> {
            rustix::fs::utimensat(&dir.0, name, &ts(ns), AtFlags::SYMLINK_NOFOLLOW)
                .map_err(Into::into)
        }

        pub fn free_bytes(path: &Path) -> io::Result<u64> {
            let v = rustix::fs::statvfs(path)?;
            Ok(v.f_bavail.saturating_mul(v.f_frsize))
        }

        /// macOS: the quarantine attribute AirDrop and browsers set, so Gatekeeper asks
        /// before anything received is opened. Linux has no such convention.
        pub fn mark_downloaded(f: &File, _path: &Path) {
            #[cfg(target_os = "macos")]
            crate::macos::quarantine(f);
            #[cfg(not(target_os = "macos"))]
            let _ = f;
        }
    }

    #[cfg(not(unix))]
    mod portable {
        use super::*;

        pub struct Dir(PathBuf);

        /// `\\?\` + the absolute path (no MAX_PATH limit on Windows).
        pub fn long(p: &Path) -> PathBuf {
            #[cfg(windows)]
            {
                let s = p.as_os_str().to_string_lossy();
                if s.starts_with(r"\\?\") {
                    return p.to_path_buf();
                }
                if let Some(unc) = s.strip_prefix(r"\\") {
                    return PathBuf::from(format!(r"\\?\UNC\{unc}"));
                }
                if p.is_absolute() {
                    return PathBuf::from(format!(r"\\?\{s}"));
                }
            }
            p.to_path_buf()
        }

        fn not_link(p: &Path) -> io::Result<()> {
            let m = fs::symlink_metadata(p)?;
            if m.file_type().is_symlink() || !m.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "not a plain folder",
                ));
            }
            Ok(())
        }

        pub fn mkdir_path(path: &Path) -> io::Result<()> {
            fs::create_dir(long(path))
        }

        pub fn open_root(path: &Path) -> io::Result<Dir> {
            not_link(&long(path))?;
            Ok(Dir(path.to_path_buf()))
        }

        pub fn open_dir(root: &Path, rel: &str) -> io::Result<Dir> {
            let mut p = root.to_path_buf();
            not_link(&long(&p))?;
            for part in rel.split('/').filter(|p| !p.is_empty()) {
                p.push(part);
                not_link(&long(&p))?;
            }
            Ok(Dir(p))
        }

        pub fn mkdir(dir: &Dir, name: &str) -> io::Result<()> {
            fs::create_dir(long(&dir.0.join(name)))
        }

        pub fn create_file(dir: &Dir, name: &str) -> io::Result<File> {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(long(&dir.0.join(name)))
        }

        pub fn unlink(dir: &Dir, name: &str) -> io::Result<()> {
            fs::remove_file(long(&dir.0.join(name)))
        }

        fn time_of(ns: u64) -> std::time::SystemTime {
            std::time::UNIX_EPOCH + Duration::from_nanos(ns)
        }

        pub fn set_file_mtime(f: &File, ns: u64) -> io::Result<()> {
            f.set_modified(time_of(ns))
        }

        pub fn set_dir_mtime(dir: &Dir, name: &str, ns: u64) -> io::Result<()> {
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
                const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
                const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
                let f = fs::OpenOptions::new()
                    .access_mode(FILE_WRITE_ATTRIBUTES)
                    .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                    .open(long(&dir.0.join(name)))?;
                f.set_modified(time_of(ns))
            }
            #[cfg(not(windows))]
            {
                let _ = (dir, name, ns);
                Ok(())
            }
        }

        pub fn free_bytes(path: &Path) -> io::Result<u64> {
            #[cfg(windows)]
            {
                crate::windows::free_bytes(path)
            }
            #[cfg(not(windows))]
            {
                let _ = path;
                Ok(u64::MAX)
            }
        }

        /// Windows: the Zone.Identifier stream browsers write (Mark of the Web, zone 3 =
        /// Internet), so SmartScreen and Office treat what was received with care.
        pub fn mark_downloaded(_f: &File, path: &Path) {
            #[cfg(windows)]
            {
                let mut ads = long(path).into_os_string();
                ads.push(":Zone.Identifier");
                if let Err(e) = fs::write(&ads, b"[ZoneTransfer]\r\nZoneId=3\r\n") {
                    log::debug!("Flow: couldn't mark a received file as downloaded: {e}");
                }
            }
            #[cfg(not(windows))]
            let _ = path;
        }
    }
}

#[cfg(test)]
mod tests;
