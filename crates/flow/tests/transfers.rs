//! Copied files and images between two Rust computers on loopback.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::*;
use glisa_flow::proto::{self, Conn, DATA_HEADER};
use glisa_flow::tls::{self, Identity};
use glisa_flow::transfer::uri_of;
use serde_json::{Value, json};

struct Setup {
    a: Computer,
    b: Computer,
    hub: Arc<SimHub>,
    src: PathBuf,
}

/// The mouse is on `c` and knows which channel `other` is on (else a crossing is ignored).
fn ready(c: &Computer, other: &Computer) -> bool {
    wait_until(5.0, || {
        c.flow
            .devices_status()
            .iter()
            .find(|d| d["online"] == true)
            .is_some_and(|d| !d["channels"][other.id()]["suggested"].is_null())
    })
}

fn setup() -> Setup {
    let (a, b) = paired_pair();
    let hub = shared_device(&a, &b);
    assert!(ready(&a, &b));
    let src = a.tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    Setup { a, b, hub, src }
}

impl Setup {
    fn cross_from(&self, c: &Computer) {
        c.backend.away(); // a new visit to the edge (one attempt per visit)
        c.backend.hit(
            if std::ptr::eq(c, &self.a) {
                "right"
            } else {
                "left"
            },
            0.5,
        );
    }

    fn cross(&self) {
        self.cross_from(&self.a);
    }

    /// b sends the mouse back to a (after the cooldown), ready to cross again.
    fn back_to_a(&self) {
        let n = self.hub.moves().len();
        assert!(ready(&self.b, &self.a));
        sleep(0.9);
        self.cross_from(&self.b);
        assert!(wait_until(5.0, || self.hub.moves().len() == n + 1));
        assert!(ready(&self.a, &self.b));
        sleep(0.9);
    }

    fn make_tree(&self) -> Vec<PathBuf> {
        let s = &self.src;
        fs::create_dir_all(s.join("Proyecto").join("docs").join("img")).unwrap();
        let big: Vec<u8> = (0..600_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let files: [(&str, Vec<u8>); 4] = [
            ("Proyecto/léeme.txt", b"hola\n".to_vec()),
            ("Proyecto/docs/img/x.bin", big),
            ("Proyecto/docs/vacío", Vec::new()),
            ("suelto.sh", b"#!/bin/sh\necho hi\n".to_vec()),
        ];
        for (rel, content) in files {
            fs::write(s.join(rel), content).unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(s.join("suelto.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        for (i, rel) in [
            "Proyecto/docs/img/x.bin",
            "Proyecto/docs",
            "Proyecto",
            "suelto.sh",
        ]
        .iter()
        .enumerate()
        {
            let t =
                std::time::UNIX_EPOCH + Duration::from_nanos(1_600_000_000_123_456_789 + i as u64);
            let f = fs::File::open(s.join(rel)).unwrap();
            f.set_modified(t).unwrap();
        }
        vec![s.join("Proyecto"), s.join("suelto.sh")]
    }

    /// A connection to b with a's credentials, as a misbehaving a would open it.
    fn raw(&self) -> Conn {
        let id = Identity::load(&self.a.flow.store.cert_path, &self.a.flow.store.key_path).unwrap();
        let cfg = tls::client_config(&id, &self.b.flow.store.pin()).unwrap();
        tls::connect(
            "127.0.0.1",
            self.b.port,
            cfg,
            Duration::from_secs(3),
            proto::TRANSFER_SERVER,
            &[],
        )
        .unwrap()
    }

    fn send_manifest(&self, c: &mut Conn, entries: Value, kind: &str, text: &str) {
        let body = serde_json::to_vec(
            &json!({"v": 1, "type": "transfer", "seq": 1, "id": self.a.id(),
                                              "kind": kind, "entries": entries, "text": text}),
        )
        .unwrap();
        c.send_raw(&[&(body.len() as u32).to_be_bytes()[..], &body].concat())
            .unwrap();
    }
}

fn frame(seq: u64, index: u32, offset: u64, payload: &[u8]) -> Vec<u8> {
    let mut f = ((DATA_HEADER + payload.len()) as u32)
        .to_be_bytes()
        .to_vec();
    f.push(0);
    f.extend_from_slice(&seq.to_be_bytes());
    f.extend_from_slice(&index.to_be_bytes());
    f.extend_from_slice(&offset.to_be_bytes());
    f.extend_from_slice(payload);
    f
}

fn json_frame(msg: Value) -> Vec<u8> {
    let mut m = msg;
    m["v"] = json!(1);
    let body = serde_json::to_vec(&m).unwrap();
    [&(body.len() as u32).to_be_bytes()[..], &body].concat()
}

/// {relative path: (kind, sha256, mode, mtime_ns)} of a folder.
fn tree_digest(root: &Path) -> BTreeMap<String, (String, String, u32, i128)> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, (String, String, u32, i128)>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let m = fs::symlink_metadata(&p).unwrap();
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            #[cfg(unix)]
            let (mode, mtime) = {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                (
                    m.permissions().mode() & 0o7777,
                    m.mtime() as i128 * 1_000_000_000 + m.mtime_nsec() as i128,
                )
            };
            #[cfg(not(unix))]
            let (mode, mtime) = (0u32, 0i128);
            if m.is_dir() {
                out.insert(rel, ("dir".into(), String::new(), mode, mtime));
                walk(root, &p, out);
            } else {
                out.insert(
                    rel,
                    ("file".into(), sha(&fs::read(&p).unwrap()), mode, mtime),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn png(w: u32, h: u32, tail: &[u8]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for b in data {
            crc ^= *b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    let chunk = |kind: &[u8], data: &[u8]| {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc32(&[kind, data].concat()).to_be_bytes());
        out
    };
    let mut ihdr = w.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend(chunk(b"IHDR", &ihdr));
    out.extend(chunk(
        b"IDAT",
        &[0x78, 0x01, 0x01, 0x00, 0x00, 0xff, 0xff, 0, 0, 0, 1],
    ));
    out.extend_from_slice(tail);
    out.extend(chunk(b"IEND", b""));
    out
}

fn jpeg(w: u16, h: u16) -> Vec<u8> {
    let mut out =
        b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00\xff\xc0\x00\x11\x08"
            .to_vec();
    out.extend_from_slice(&h.to_be_bytes());
    out.extend_from_slice(&w.to_be_bytes());
    out.extend_from_slice(b"\x03\x01\x22\x00\x02\x11\x01\x03\x11\x01\xff\xd9");
    out
}

fn set_rich(c: &Computer, items: &[(&str, Vec<u8>)]) {
    c.backend.rich_clip.as_ref().unwrap().set(
        items
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    );
}

// ---------------------------------------------------------------------- files
#[test]
fn files_and_folders_arrive_intact() {
    let s = setup();
    let (a, b) = (&s.a, &s.b);
    let paths = s.make_tree();
    copy_files(a, &paths, b"");
    s.cross();
    assert!(
        wait_until(10.0, || b.backend.rich_data().contains_key("text/uri-list")),
        "{:?}",
        a.events()
    );
    let rt = b.last_transfer();
    assert_eq!(
        (
            rt["state"].as_str(),
            rt["direction"].as_str(),
            rt["items"].as_u64(),
            rt["files"].as_u64(),
            rt["folders"].as_u64(),
            rt["placed"].as_str()
        ),
        (
            Some("done"),
            Some("receive"),
            Some(2),
            Some(4),
            Some(3),
            Some("clipboard")
        )
    );
    let folder = PathBuf::from(rt["folder"].as_str().unwrap());
    assert!(folder.starts_with(&b.flow.staging.root));
    let (want, got) = (tree_digest(&s.src), tree_digest(&folder));
    let strip =
        |m: &BTreeMap<String, (String, String, u32, i128)>| -> Vec<(String, String, String, i128)> {
            m.iter()
                .map(|(k, v)| (k.clone(), v.0.clone(), v.1.clone(), v.3))
                .collect()
        };
    assert_eq!(strip(&got), strip(&want)); // content and mtimes
    #[cfg(unix)]
    {
        assert!(got.values().filter(|v| v.0 == "file").all(|v| v.2 == 0o600)); // never executable
        assert!(got.values().filter(|v| v.0 == "dir").all(|v| v.2 == 0o700));
    }
    let clip = b.backend.rich_data();
    let uris: Vec<String> = ["Proyecto", "suelto.sh"]
        .iter()
        .map(|n| uri_of(&folder.join(n)))
        .collect();
    let list = String::from_utf8(clip["text/uri-list"].clone()).unwrap();
    assert_eq!(
        list.split("\r\n")
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>(),
        uris
    );
    let gnome = String::from_utf8(clip["x-special/gnome-copied-files"].clone()).unwrap();
    assert_eq!(
        gnome.lines().collect::<Vec<_>>(),
        [vec!["copy".to_string()], uris.clone()].concat()
    );
    let st = a.last_transfer();
    assert_eq!(
        (
            st["state"].as_str(),
            st["percent"].as_u64(),
            st["bytes"].clone()
        ),
        (Some("done"), Some(100), rt["bytes"].clone())
    );
    assert_eq!(b.backend.clip().as_deref(), Some("")); // the paths as text were not sent
    // crossing back: b does not send them back to a
    s.back_to_a();
    let dirs: Vec<Value> = a
        .flow
        .transfers_status()
        .iter()
        .map(|t| t["direction"].clone())
        .collect();
    assert_eq!(dirs, vec![json!("send")]);
    // and the same copy isn't sent to b again
    s.cross();
    assert!(wait_until(5.0, || s.hub.moves() == vec![1, 0, 1]));
    sleep(0.4);
    assert_eq!(b.flow.transfers_status().len(), 1);
}

#[test]
fn caps_with_a_clear_event() {
    let s = setup();
    let (a, b) = (&s.a, &s.b);
    let big = s.src.join("grande.bin");
    fs::write(&big, vec![0u8; 2 << 20]).unwrap();
    a.call("flow_set_options", json!({"max_transfer_mb": 1}))
        .unwrap(); // the sender's own limit
    copy_files(a, std::slice::from_ref(&big), b"");
    s.cross();
    assert!(
        wait_until(5.0, || a
            .events()
            .iter()
            .any(|t| t.contains("larger than 1 MB"))),
        "{:?}",
        a.events()
    );
    assert!(b.flow.transfers_status().is_empty());
    a.call("flow_set_options", json!({"max_transfer_mb": 2048}))
        .unwrap();
    b.call("flow_set_options", json!({"max_transfer_mb": 1}))
        .unwrap(); // the receiver's limit
    s.back_to_a();
    s.cross();
    assert!(
        wait_until(5.0, || a
            .events()
            .iter()
            .any(|t| t.contains("lower size limit"))),
        "{:?}",
        a.events()
    );
    assert!(
        b.events().iter().any(|t| t.contains("your limit is 1 MB")),
        "{:?}",
        b.events()
    );
    assert!(b.staged().is_empty());
    // too many files, too deep: the sender says so and sends nothing
    let many = s.src.join("muchos");
    fs::create_dir_all(&many).unwrap();
    for i in 0..6 {
        fs::write(many.join(format!("{i}.txt")), "x").unwrap();
    }
    copy_files(a, &[many], b"");
    a.flow.hooks().max_files = Some(5);
    assert!(a.flow.send_rich(&b.id(), &a.flow.store.options()));
    assert!(
        wait_until(5.0, || a
            .events()
            .iter()
            .any(|t| t.contains("More than 5 files"))),
        "{:?}",
        a.events()
    );
    a.flow.hooks().max_files = None;
    let mut deep = s.src.join("hondo");
    let top = deep.clone();
    for _ in 0..33 {
        deep.push("d");
    }
    fs::create_dir_all(&deep).unwrap();
    copy_files(a, &[top], b"");
    a.flow.send_rich(&b.id(), &a.flow.store.options());
    assert!(wait_until(5.0, || a
        .events()
        .iter()
        .any(|t| t.contains("more than 32 levels"))));
    assert!(b.staged().is_empty());
}

#[test]
fn malicious_manifests_are_rejected() {
    let s = setup();
    let b = &s.b;
    let good = b"0123456789";
    for entries in [
        json!([{"type": "file", "path": "/etc/passwd", "size": 1, "mtime": 1, "sha256": sha(b"x")}]),
        json!([{"type": "file", "path": "../../.bashrc", "size": 1, "mtime": 1, "sha256": sha(b"x")}]),
        json!([{"type": "symlink", "path": "l", "mtime": 1}]),
        json!([{"type": "device", "path": "sda", "mtime": 1}]),
        json!([{"type": "file", "path": "n".repeat(300), "size": 1, "mtime": 1, "sha256": sha(b"x")}]),
    ] {
        let mut c = s.raw();
        s.send_manifest(&mut c, entries, "files", "");
        let m = c.recv_msg(Some(Duration::from_secs(3))).unwrap();
        assert_eq!(
            (m["type"].as_str(), m["reason"].as_str()),
            (Some("error"), Some("protocol"))
        );
    }
    let one = json!([{"type": "file", "path": "a", "size": 10, "mtime": 1, "sha256": sha(good)}]);
    // sizes that lie: more data than declared
    let mut c = s.raw();
    s.send_manifest(&mut c, one.clone(), "files", "");
    assert_eq!(
        c.recv_msg(Some(Duration::from_secs(3))).unwrap()["type"],
        "transfer_ok"
    );
    c.send_raw(&frame(2, 0, 0, &[&good[..], b"extra"].concat()))
        .unwrap();
    assert_eq!(
        c.recv_msg(Some(Duration::from_secs(3))).unwrap()["reason"],
        "protocol"
    );
    // extra data after the last file
    let mut c = s.raw();
    s.send_manifest(&mut c, one.clone(), "files", "");
    assert_eq!(
        c.recv_msg(Some(Duration::from_secs(3))).unwrap()["type"],
        "transfer_ok"
    );
    c.send_raw(&[frame(2, 0, 0, good), frame(3, 1, 0, b"more")].concat())
        .unwrap();
    assert_eq!(
        c.recv_msg(Some(Duration::from_secs(3))).unwrap()["reason"],
        "protocol"
    );
    // fewer bytes than declared, then the end
    let mut c = s.raw();
    s.send_manifest(&mut c, one, "files", "");
    assert_eq!(
        c.recv_msg(Some(Duration::from_secs(3))).unwrap()["type"],
        "transfer_ok"
    );
    c.send_raw(
        &[
            frame(2, 0, 0, &good[..5]),
            json_frame(json!({"type": "transfer_end", "seq": 3})),
        ]
        .concat(),
    )
    .unwrap();
    assert_eq!(
        c.recv_msg(Some(Duration::from_secs(3))).unwrap()["reason"],
        "protocol"
    );
    assert!(
        wait_until(3.0, || b.staged().is_empty()),
        "{:?}",
        b.staged()
    );
    assert!(
        b.events().iter().any(|t| t.contains("wasn't valid")),
        "{:?}",
        b.events()
    );
    // content that doesn't match its checksum: that file is left out
    let mut c = s.raw();
    s.send_manifest(
        &mut c,
        json!([{"type": "file", "path": "a", "size": 10, "mtime": 1, "sha256": sha(b"xxxxxxxxxx")},
               {"type": "file", "path": "b", "size": 10, "mtime": 1, "sha256": sha(good)}]),
        "files",
        "",
    );
    assert_eq!(
        c.recv_msg(Some(Duration::from_secs(3))).unwrap()["type"],
        "transfer_ok"
    );
    c.send_raw(
        &[
            frame(2, 0, 0, good),
            frame(3, 1, 0, good),
            json_frame(json!({"type": "transfer_end", "seq": 4})),
        ]
        .concat(),
    )
    .unwrap();
    let m = c.recv_msg(Some(Duration::from_secs(3))).unwrap();
    assert_eq!(
        (m["type"].as_str(), m["failed"].as_u64()),
        (Some("transfer_done"), Some(1))
    );
    let folder = PathBuf::from(b.last_transfer()["folder"].as_str().unwrap().to_string());
    let names: Vec<String> = fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["b"]);
    assert!(wait_until(3.0, || b
        .backend
        .rich_data()
        .get("text/uri-list")
        .cloned()
        == Some(
            format!("{}\r\n", uri_of(&folder.join("b"))).into_bytes()
        )));
}

fn slow_poll(c: &Computer, ms: u64) {
    c.flow.hooks().send.poll_delay = Some(Duration::from_millis(ms)); // a slow network
}

#[test]
fn transfer_does_not_block_a_switch_and_can_be_cancelled() {
    let s = setup();
    let (a, b) = (&s.a, &s.b);
    let big = s.src.join("grande.bin");
    fs::write(&big, vec![7u8; 4 << 20]).unwrap();
    copy_files(a, &[big], b"");
    slow_poll(a, 300);
    s.cross();
    assert!(wait_until(5.0, || a.last_transfer()["state"] == "sending"
        && a.last_transfer()["done"].as_u64() > Some(0)));
    // the peer channel answers at once, and b can send the mouse back
    let t0 = std::time::Instant::now();
    assert_eq!(
        a.flow
            .link(&b.id())
            .unwrap()
            .request("ping", Duration::from_secs(1), json!({}))
            .unwrap()["type"],
        "pong"
    );
    assert!(t0.elapsed() < Duration::from_millis(500));
    sleep(0.9);
    s.cross_from(b);
    assert!(wait_until(3.0, || s.hub.moves() == vec![1, 0]));
    assert_eq!(a.last_transfer()["active"], true);
    // cancel on the sender
    let tid = a.last_transfer()["id"].as_str().unwrap().to_string();
    assert_eq!(
        a.call("flow_transfer_cancel", json!({"transfer": tid}))
            .unwrap(),
        true
    );
    assert!(wait_until(5.0, || a.last_transfer()["state"] == "cancelled"));
    assert!(wait_until(5.0, || b.last_transfer()["state"] == "cancelled"));
    assert!(wait_until(3.0, || b.staged().is_empty()));
    assert!(!b.backend.rich_data().contains_key("text/uri-list"));
    assert!(
        b.events().iter().any(|t| t.contains("equipo-a")),
        "{:?}",
        b.events()
    );
}

#[test]
fn receiver_cancels() {
    let s = setup();
    let (a, b) = (&s.a, &s.b);
    let big = s.src.join("grande.bin");
    fs::write(&big, vec![9u8; 3 << 20]).unwrap();
    copy_files(a, &[big], b"");
    slow_poll(a, 150);
    s.cross();
    assert!(wait_until(5.0, || b.last_transfer()["done"].as_u64() > Some(0)));
    let tid = b.last_transfer()["id"].as_str().unwrap().to_string();
    assert_eq!(
        b.call("flow_transfer_cancel", json!({"transfer": tid}))
            .unwrap(),
        true
    );
    assert!(wait_until(5.0, || b.last_transfer()["state"] == "cancelled"));
    assert!(wait_until(5.0, || a.last_transfer()["state"] == "cancelled"));
    assert!(wait_until(3.0, || a
        .events()
        .iter()
        .any(|t| t.contains("equipo-b"))));
    assert!(wait_until(3.0, || b.staged().is_empty()));
    glisa_i18n::with_language(Some("en"), || {
        assert_eq!(
            glisa_flow::text::transfer_text(&b.last_transfer()),
            "Cancelled: 1 file from equipo-a"
        );
    });
}

#[test]
fn a_file_deleted_while_sending() {
    let s = setup();
    let (a, b) = (&s.a, &s.b);
    let (first, second) = (s.src.join("1.bin"), s.src.join("2.bin"));
    for p in [&first, &second] {
        fs::write(p, vec![1u8; 300_000]).unwrap();
    }
    let gone = second.clone();
    a.flow.hooks().send.before_file = Some(Arc::new(move |p: &Path| {
        if p == gone {
            let _ = fs::remove_file(p); // gone after it was hashed
        }
    }));
    copy_files(a, &[first, second], b"");
    s.cross();
    assert!(wait_until(10.0, || b.last_transfer()["state"] == "done"));
    let folder = PathBuf::from(b.last_transfer()["folder"].as_str().unwrap().to_string());
    let names: Vec<String> = fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["1.bin"]);
    assert!(wait_until(3.0, || a.last_transfer()["state"] == "done"));
    assert_eq!(
        (
            b.last_transfer()["failed"].as_u64(),
            a.last_transfer()["failed"].as_u64()
        ),
        (Some(1), Some(1))
    );
    assert!(
        wait_until(3.0, || a
            .events()
            .iter()
            .any(|t| t.contains("1 ") && t.contains("equipo-b"))),
        "{:?}",
        a.events()
    );
}

#[test]
fn older_peer_gets_text_only() {
    let s = setup();
    let (a, b) = (&s.a, &s.b);
    b.flow.hooks().refuse_hello_fields = vec!["caps"]; // b reads hello as a Glisa from before `caps`
    a.flow.link(&b.id()).unwrap().close();
    assert!(wait_until(5.0, || a.flow.link(&b.id()).unwrap().online()));
    assert!(a.flow.link(&b.id()).unwrap().caps().is_empty());
    assert_eq!(
        a.call("flow_peers", json!({})).unwrap()["peers"][0]["files"],
        false
    );
    copy_files(a, &s.make_tree(), b"");
    a.backend.set_clip("file:///…/Proyecto");
    s.cross();
    assert!(wait_until(5.0, || b.backend.clip().as_deref()
        == Some("file:///…/Proyecto")));
    sleep(0.3);
    assert!(b.backend.rich_data().is_empty());
    assert!(b.flow.transfers_status().is_empty());
}

#[test]
fn option_off() {
    let s = setup();
    let (a, b) = (&s.a, &s.b);
    a.call("flow_set_options", json!({"files": false})).unwrap();
    copy_files(a, &s.make_tree(), b"");
    a.backend.set_clip("texto");
    s.cross();
    assert!(wait_until(5.0, || b.backend.clip().as_deref() == Some("texto"))); // text as before
    sleep(0.3);
    assert!(b.backend.rich_data().is_empty() && b.flow.transfers_status().is_empty());
    a.call("flow_set_options", json!({"files": true})).unwrap();
    b.call("flow_set_options", json!({"files": false})).unwrap(); // off on the receiver
    s.back_to_a();
    s.cross();
    assert!(
        wait_until(5.0, || a
            .events()
            .iter()
            .any(|t| t.contains("turned off there"))),
        "{:?}",
        a.events()
    );
    assert!(b.backend.rich_data().is_empty() && b.staged().is_empty());
}

#[test]
fn staging_cleanup() {
    let s = setup();
    let b = &s.b;
    copy_files(&s.a, &s.make_tree(), b"");
    s.cross();
    assert!(wait_until(10.0, || b.last_transfer()["state"] == "done"));
    let fresh = Path::new(b.last_transfer()["folder"].as_str().unwrap())
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let old = format!(
        "{:010}-deadbeef",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 25 * 3600
    );
    fs::create_dir(b.flow.staging.root.join(&old)).unwrap();
    b.flow.cleanup_staging_for_tests();
    assert_eq!(b.staged(), vec![fresh]);
    b.call("flow_enable", json!({"enabled": false})).unwrap(); // turning Flow off forgets them all
    assert!(b.staged().is_empty());
}

// ---------------------------------------------------------------------- images
#[test]
fn image_round_trip() {
    let s = setup();
    let (a, b) = (&s.a, &s.b);
    let image = png(479, 209, b"");
    set_rich(
        a,
        &[
            ("application/x-qt-image", vec![]),
            ("image/avif", vec![]),
            ("image/bmp", vec![]),
            ("image/png", image.clone()),
            ("image/jpeg", jpeg(479, 209)),
            ("text/plain;charset=utf-8", "captura".as_bytes().to_vec()),
        ],
    );
    s.cross();
    assert!(
        wait_until(10.0, || b.backend.rich_data().contains_key("image/png")),
        "{:?}",
        b.events()
    );
    let data = b.backend.rich_data();
    assert_eq!(data["image/png"], image); // bytes identical
    assert_eq!(data["text/plain;charset=utf-8"], b"captura");
    assert_eq!(
        data.keys().cloned().collect::<Vec<_>>(),
        vec!["image/png", "text/plain", "text/plain;charset=utf-8"]
    );
    let rt = b.last_transfer();
    assert_eq!(
        (
            rt["kind"].as_str(),
            rt["state"].as_str(),
            rt["placed"].as_str()
        ),
        (Some("image"), Some("done"), Some("clipboard"))
    );
    glisa_i18n::with_language(Some("en"), || {
        assert_eq!(
            glisa_flow::text::transfer_text(&rt),
            "Received an image from equipo-a — paste it with Ctrl+V"
        );
    });
    let staged: Vec<PathBuf> = fs::read_dir(rt["folder"].as_str().unwrap())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(staged.len(), 1);
    assert!(staged[0].extension().is_some_and(|e| e == "png"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&staged[0]).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(b.backend.clip().as_deref(), Some("")); // the text travelled with the image
    assert!(wait_until(3.0, || fs::read_dir(
        a.flow.staging.root.join(".spool")
    )
    .map(|r| r.count())
    .unwrap_or(0)
        == 0));
}

#[test]
fn image_checks() {
    let s = setup();
    let (a, b) = (&s.a, &s.b);
    // declared PNG, but the bytes are a JPEG
    let mut c = s.raw();
    let data = jpeg(10, 10);
    s.send_manifest(
        &mut c,
        json!([{"type": "image", "mime": "image/png", "size": data.len(), "sha256": sha(&data)}]),
        "image",
        "",
    );
    assert_eq!(
        c.recv_msg(Some(Duration::from_secs(3))).unwrap()["type"],
        "transfer_ok"
    );
    c.send_raw(&frame(2, 0, 0, &data)).unwrap();
    let m = c.recv_msg(Some(Duration::from_secs(3))).unwrap();
    assert_eq!(
        (m["type"].as_str(), m["reason"].as_str()),
        (Some("transfer_cancel"), Some("failed"))
    );
    // a header claiming 20000 × 20000 pixels
    let mut c = s.raw();
    let data = png(20000, 20000, b"");
    s.send_manifest(
        &mut c,
        json!([{"type": "image", "mime": "image/png", "size": data.len(), "sha256": sha(&data)}]),
        "image",
        "",
    );
    assert_eq!(
        c.recv_msg(Some(Duration::from_secs(3))).unwrap()["type"],
        "transfer_ok"
    );
    c.send_raw(
        &[
            frame(2, 0, 0, &data),
            json_frame(json!({"type": "transfer_end", "seq": 3})),
        ]
        .concat(),
    )
    .unwrap();
    let m = c.recv_msg(Some(Duration::from_secs(3))).unwrap();
    assert_eq!(
        (m["type"].as_str(), m["reason"].as_str()),
        (Some("transfer_cancel"), Some("too_large"))
    );
    assert!(wait_until(3.0, || b.staged().is_empty()));
    assert!(!b.backend.rich_data().contains_key("image/png"));
    assert!(
        b.events()
            .iter()
            .any(|t| t.contains("isn't the kind of image")),
        "{:?}",
        b.events()
    );
    assert!(
        b.events().iter().any(|t| t.contains("20000 × 20000")),
        "{:?}",
        b.events()
    );
    // bigger than the receiver accepts: refused before anything is sent
    b.call("flow_set_options", json!({"max_image_mb": 1}))
        .unwrap();
    let mut tail = b"\x00\x00\x00\x00tEXt".to_vec();
    tail.extend(vec![b'x'; 2 << 20]);
    set_rich(a, &[("image/png", png(100, 100, &tail))]);
    s.cross();
    assert!(wait_until(5.0, || a
        .flow
        .transfers_status()
        .iter()
        .any(|t| t["state"] == "refused")));
    assert!(!b.backend.rich_data().contains_key("image/png"));
    // over the sender's own limit, or too many pixels: not sent at all
    a.call("flow_set_options", json!({"max_image_mb": 1}))
        .unwrap();
    set_rich(a, &[("image/png", png(100, 100, &vec![0u8; 2 << 20]))]);
    assert!(a.flow.send_rich(&b.id(), &a.flow.store.options()));
    assert!(wait_until(3.0, || a
        .events()
        .iter()
        .any(|t| t.contains("larger than 1 MB"))));
    set_rich(
        a,
        &[
            ("image/png", png(20000, 20000, b"")),
            ("text/plain", b"t".to_vec()),
        ],
    );
    assert!(!a.flow.send_rich(&b.id(), &a.flow.store.options()));
}

#[test]
fn image_older_peer_and_option_off() {
    let s = setup();
    let (a, b) = (&s.a, &s.b);
    set_rich(a, &[("image/png", png(3, 2, b""))]);
    a.backend.set_clip("texto");
    a.call("flow_set_options", json!({"files": false})).unwrap();
    s.cross();
    assert!(wait_until(5.0, || b.backend.clip().as_deref() == Some("texto")));
    sleep(0.3);
    assert!(b.backend.rich_data().is_empty());
    a.call("flow_set_options", json!({"files": true})).unwrap();
    a.flow.link(&b.id()).unwrap().set_caps(Default::default()); // as with a Glisa from before images
    assert!(!a.flow.send_rich(&b.id(), &a.flow.store.options()));
}

#[test]
fn send_paths_directly() {
    // What drag and drop across the edge will use: no clipboard involved.
    let s = setup();
    let (a, b) = (&s.a, &s.b);
    let f = s.src.join("arrastrado.txt");
    fs::write(&f, "hola").unwrap();
    let tid = a
        .flow
        .send_paths(&b.id(), vec![f], glisa_flow::SendReason::Drag)
        .unwrap();
    assert!(wait_until(5.0, || b.last_transfer()["state"] == "done"));
    let t = a
        .flow
        .transfers_status()
        .into_iter()
        .find(|t| t["id"] == tid.as_str())
        .unwrap();
    assert_eq!(
        (t["state"].as_str(), t["reason"].as_str()),
        (Some("done"), Some("drag"))
    );
}
