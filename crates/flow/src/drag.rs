//! Files dragged across the edge (docs/FLOW-DRAG-DROP.md), the parts that don't depend on
//! the desktop:
//!
//! * [`parse_drag_uris`]: the `text/uri-list` a file manager offers while dragging, read
//!   as strictly as the copied-files clipboard (local `file:` URIs only) and stricter
//!   still: a path with a `.` or `..` component is dropped, not normalized;
//! * [`summarize`]: the short `drag` notice the receiver gets before the switch (the first
//!   names, how many items, about how big), computed with the same naming rules as the
//!   transfer that follows, within a small time budget.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::names::is_junk;
use crate::proto::{self, DRAG_NAMES_MAX};
use crate::transfer::{URI_LIST_MAX, parse_uri_list, unique_top, unquote};
use crate::{MAX_DEPTH, MAX_FILES};

/// How long the uri-list of a drag may take to arrive: longer is a plain drag.
pub const URI_LIST_TIMEOUT: Duration = Duration::from_millis(300);
/// How long the sender counts sizes for the notice before switching.
pub const SUMMARY_BUDGET: Duration = Duration::from_millis(250);

/// Does this URI's path (percent-decoded) have a `.` or `..` component?
fn dot_segments(uri: &str) -> bool {
    let rest = uri.split_once(':').map_or(uri, |(_, r)| r);
    let path = match rest.strip_prefix("//") {
        Some(after) => after.find('/').map_or("", |i| &after[i..]),
        None => rest,
    };
    unquote(path)
        .split(|b| *b == b'/')
        .any(|c| c == b"." || c == b"..")
}

/// Local paths from the `text/uri-list` of a drag: `file:` URIs with an empty or
/// `localhost` host, percent-decoded; anything else (other schemes, other hosts, relative
/// paths, a `.` or `..` component, query or fragment) is dropped. Order kept, duplicates
/// dropped, at most [`MAX_FILES`].
pub fn parse_drag_uris(data: &[u8]) -> Vec<PathBuf> {
    let data = &data[..data.len().min(URI_LIST_MAX)];
    let mut kept: Vec<u8> = Vec::new();
    for raw in data.split(|b| *b == b'\n') {
        let line = raw.trim_ascii();
        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        let Ok(text) = std::str::from_utf8(line) else {
            continue;
        };
        if dot_segments(text) {
            continue;
        }
        kept.extend_from_slice(line);
        kept.push(b'\n');
    }
    parse_uri_list(&kept, MAX_FILES)
}

/// What the `drag` notice says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// The first top-level names, as the transfer will call them.
    pub names: Vec<String>,
    /// Top-level items that will be sent.
    pub count: u64,
    /// Bytes of the files counted (all of them when `complete`).
    pub bytes: u64,
    /// Every folder was walked within the budget.
    pub complete: bool,
}

impl Summary {
    /// The fields of the `drag` message.
    pub fn fields(&self) -> Value {
        json!({"names": self.names, "count": self.count.min(MAX_FILES as u64),
               "bytes": self.bytes.min(1 << 50)})
    }
}

/// The notice for dragged `paths`, with the naming rules of the transfer (links, special
/// files, odd names and macOS bookkeeping left out; colliding names numbered). Sizes are
/// counted for at most `budget`: a huge folder gives a lower bound (the transfer's
/// manifest is exact). None when nothing would be sent.
pub fn summarize(paths: &[PathBuf], budget: Duration) -> Option<Summary> {
    let end = Instant::now() + budget;
    let mut used: Vec<String> = Vec::new();
    let mut s = Summary {
        names: Vec::new(),
        count: 0,
        bytes: 0,
        complete: true,
    };
    let mut seen = 0usize;
    for path in paths {
        let Ok(meta) = fs::symlink_metadata(path) else {
            continue;
        };
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if is_junk(name) {
            continue;
        }
        let ft = meta.file_type();
        if ft.is_symlink() || !(ft.is_dir() || ft.is_file()) || !proto::relpath_ok(name) {
            continue;
        }
        let Ok(name) = unique_top(name, &mut used, MAX_FILES) else {
            break;
        };
        s.count += 1;
        if s.names.len() < DRAG_NAMES_MAX {
            s.names.push(name);
        }
        seen += 1;
        if ft.is_file() {
            s.bytes = s.bytes.saturating_add(meta.len());
        } else if s.complete {
            s.complete = walk_size(path, &mut s.bytes, &mut seen, end, 1);
        }
    }
    (s.count > 0).then_some(s)
}

/// Adds the sizes of the files under `dir` (links never followed); false when it stopped
/// early (budget, entry count or depth).
fn walk_size(dir: &Path, bytes: &mut u64, seen: &mut usize, end: Instant, depth: usize) -> bool {
    if depth > MAX_DEPTH || Instant::now() > end {
        return false;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return true; // unreadable: the transfer skips it too
    };
    for de in rd.filter_map(Result::ok) {
        if *seen >= MAX_FILES || Instant::now() > end {
            return false;
        }
        let Ok(meta) = fs::symlink_metadata(de.path()) else {
            continue;
        };
        let ft = meta.file_type();
        if ft.is_symlink() || !(ft.is_dir() || ft.is_file()) {
            continue;
        }
        *seen += 1;
        if ft.is_file() {
            *bytes = bytes.saturating_add(meta.len());
        } else if !walk_size(&de.path(), bytes, seen, end, depth + 1) {
            return false;
        }
    }
    true
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn paths(data: &str) -> Vec<String> {
        parse_drag_uris(data.as_bytes())
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn uri_lists_of_a_drag() {
        assert_eq!(
            paths("file:///home/ana/Mis%20fotos/a%23b.jpg\r\nfile:///home/ana/x.txt\r\n"),
            ["/home/ana/Mis fotos/a#b.jpg", "/home/ana/x.txt"]
        );
        // localhost and an empty host are local; percent-encoded UTF-8 is decoded
        assert_eq!(
            paths("file://localhost/tmp/%C3%B1and%C3%BA\nfile:/tmp/y\n"),
            ["/tmp/ñandú", "/tmp/y"]
        );
        // other schemes, other hosts, comments, relative and query/fragment: dropped
        assert!(
            paths(
                "# comment\nhttps://example.com/a\nsmb://nas/share/x\nfile://otro/tmp/a\n\
                 file:relative\nfile:///tmp/a?x\nfile:///tmp/a#b\nfoo\n\n"
            )
            .is_empty()
        );
        // `.` and `..` components are dropped, not normalized (even percent-encoded)
        for bad in [
            "file:///home/ana/../../etc/shadow",
            "file:///home/ana/./x",
            "file:///home/ana/%2e%2e/x",
            "file:///tmp/a%2F..%2Fb",
            "file://localhost/tmp/..",
        ] {
            assert!(paths(bad).is_empty(), "{bad}");
        }
        assert_eq!(paths("file:///tmp/..x/a..b"), ["/tmp/..x/a..b"]);
        // duplicates once, order kept; NUL and the root are refused
        assert_eq!(
            paths("file:///b\nfile:///a\nfile:///b\nfile:///x%00y\nfile:///\n"),
            ["/b", "/a"]
        );
    }

    #[test]
    fn notice_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        fs::create_dir_all(d.join("Proyecto/sub")).unwrap();
        fs::write(d.join("Proyecto/a.txt"), b"12345").unwrap();
        fs::write(d.join("Proyecto/sub/b.bin"), vec![0u8; 1000]).unwrap();
        fs::write(d.join("suelto.txt"), b"hola").unwrap();
        fs::create_dir_all(d.join("otro")).unwrap();
        fs::write(d.join("otro/suelto.txt"), b"xy").unwrap();
        std::os::unix::fs::symlink(d.join("suelto.txt"), d.join("enlace")).unwrap();
        fs::write(d.join(".DS_Store"), b"junk").unwrap();
        let s = summarize(
            &[
                d.join("Proyecto"),
                d.join("suelto.txt"),
                d.join("otro/suelto.txt"),
                d.join("enlace"),
                d.join(".DS_Store"),
                d.join("no-existe"),
            ],
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(s.names, ["Proyecto", "suelto.txt", "suelto (2).txt"]);
        assert_eq!((s.count, s.bytes, s.complete), (3, 1011, true));
        let f = s.fields();
        assert_eq!(f["count"], 3);
        assert_eq!(f["bytes"], 1011);
        // valid as a message
        let mut m = f.as_object().unwrap().clone();
        m.insert("v".into(), json!(1));
        m.insert("type".into(), json!("drag"));
        m.insert("seq".into(), json!(1));
        proto::validate(&Value::Object(m), proto::PEER_CLIENT, 0).unwrap();
        // nothing sendable: no notice
        assert!(summarize(&[d.join("enlace")], Duration::from_secs(1)).is_none());
        // many items: the first names only, every item counted
        let many: Vec<PathBuf> = (0..12)
            .map(|i| {
                let p = d.join(format!("f{i:02}"));
                fs::write(&p, b"x").unwrap();
                p
            })
            .collect();
        let s = summarize(&many, Duration::from_secs(5)).unwrap();
        assert_eq!((s.names.len(), s.count, s.bytes), (DRAG_NAMES_MAX, 12, 12));
        // no time left: a lower bound
        let s = summarize(&[d.join("Proyecto")], Duration::ZERO).unwrap();
        assert!(!s.complete && s.bytes <= 1005);
    }
}
