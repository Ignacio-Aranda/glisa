use super::*;
use std::os::unix::net::UnixStream;
use std::sync::Arc;

fn msg(extra: Value) -> Value {
    let mut m = json!({"v": 1, "type": "switch", "seq": 1, "side": "right", "t": 0.5,
                       "devices": [{"name": "LIFT", "kind": "mouse"}]});
    for (k, v) in extra.as_object().unwrap() {
        if v.is_null() {
            m.as_object_mut().unwrap().remove(k);
        } else {
            m[k] = v.clone();
        }
    }
    m
}

fn pair(
    recv_a: &'static [&'static str],
    send_a: &'static [&'static str],
    recv_b: &'static [&'static str],
    data_b: bool,
) -> (Conn, Conn, UnixStream) {
    let (a, b) = UnixStream::pair().unwrap();
    let raw = a.try_clone().unwrap();
    let ca = Conn::new(
        Box::new(a.try_clone().unwrap()),
        Arc::new(a),
        recv_a,
        send_a,
        false,
    );
    let cb = Conn::new(
        Box::new(b.try_clone().unwrap()),
        Arc::new(b),
        recv_b,
        &[],
        data_b,
    );
    (ca, cb, raw)
}

#[test]
fn valid_and_rejected() {
    validate(&msg(json!({})), ALL, 0).unwrap();
    let bad = [
        json!({"type": "nope"}),
        json!({"v": 2}),
        json!({"v": true}),
        json!({"seq": 0}),
        json!({"seq": "1"}),
        json!({"side": "up"}),
        json!({"t": 1.5}),
        json!({"t": true}),
        json!({"extra": 1}),
        json!({"devices": []}),
        json!({"devices": [{"name": "x", "kind": "mouse", "more": 1}]}),
        json!({"devices": [{"name": "x\u{0}", "kind": "mouse"}]}),
        json!({"t": null}),
    ];
    for b in bad {
        assert!(validate(&msg(b.clone()), ALL, 0).is_err(), "{b}");
    }
    assert!(validate(&msg(json!({})), &["ping"], 0).is_err()); // not allowed on this channel
}

#[test]
fn hello_port_and_caps_are_optional() {
    let hello =
        json!({"v": 1, "type": "hello", "seq": 1, "id": "0123456789abcdef", "name": "equipo-b"});
    validate(&hello, PEER_CLIENT, 0).unwrap(); // older peers
    let with = |k: &str, v: Value| {
        let mut h = hello.clone();
        h[k] = v;
        h
    };
    validate(&with("port", json!(27461)), PEER_CLIENT, 0).unwrap();
    for bad in [
        json!(0),
        json!(65535),
        json!("27461"),
        json!(true),
        json!(27461.0),
        Value::Null,
    ] {
        assert!(
            validate(&with("port", bad.clone()), PEER_CLIENT, 0).is_err(),
            "{bad}"
        );
    }
    let mut extra = with("port", json!(1));
    extra["extra"] = json!(1);
    assert!(validate(&extra, PEER_CLIENT, 0).is_err());
    assert!(
        validate(
            &json!({"v": 1, "type": "ping", "seq": 1, "port": 1}),
            PEER_CLIENT,
            0
        )
        .is_err()
    );
    validate(
        &with("caps", json!(["transfer", "future_thing"])),
        PEER_CLIENT,
        0,
    )
    .unwrap();
    for bad in [
        json!("transfer"),
        json!(["Transfer"]),
        json!([1]),
        json!(["x".repeat(30)]),
        json!(vec!["a"; 17]),
    ] {
        assert!(
            validate(&with("caps", bad.clone()), PEER_CLIENT, 0).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn json_strictness_and_sizes() {
    for body in [
        &b"{\"v\":1,\"type\":\"ping\",\"seq\":1,\"seq\":2}"[..],
        b"{\"v\":1,\"type\":\"switch\",\"seq\":1,\"side\":\"left\",\"t\":NaN,\"devices\":[{\"name\":\"a\",\"kind\":\"mouse\"}]}",
        b"\xff\xfe",
        b"[]",
        b"{\"v\":1,\"type\":\"clipboard\",\"seq\":1,\"text\":\"\\ud800\"}",
    ] {
        assert!(matches!(decode(body, ALL), Err(Error::Protocol(_))), "{body:?}");
    }
    let big =
        serde_json::to_vec(&json!({"v": 1, "type": "ping", "seq": 1, "pad": "x".repeat(9000)}))
            .unwrap();
    assert!(matches!(decode(&big, ALL), Err(Error::Protocol(e)) if e.contains("too large")));
    let text = "ñ".repeat(CLIPBOARD_MAX / 2);
    let ok = json!({"v": 1, "type": "clipboard", "seq": 1, "text": text});
    assert_eq!(
        decode(&encode(&ok)[4..], ALL).unwrap()["text"],
        text.as_str()
    );
    let mut over = ok.clone();
    over["text"] = json!(format!("{text}x"));
    assert!(validate(&over, ALL, 0).is_err());
    // nesting too deep to parse is a protocol error, not a crash
    let deep = [vec![b'['; 500_000], vec![b']'; 500_000]].concat();
    assert!(matches!(decode(&deep, ALL), Err(Error::Protocol(_))));
}

#[test]
fn sequence_numbers() {
    let (mut tx, mut rx, raw) = pair(&[], &["ping"], &["ping"], false);
    tx.send("ping", json!({})).unwrap();
    assert_eq!(rx.recv_msg(Some(Duration::from_secs(1))).unwrap()["seq"], 1);
    let mut w = raw.try_clone().unwrap();
    w.write_all(&encode(&json!({"v": 1, "type": "ping", "seq": 1})))
        .unwrap(); // replay
    assert!(
        matches!(rx.recv(Some(Duration::from_secs(1))), Err(Error::Protocol(e)) if e.contains("sequence"))
    );
    let (_tx, mut rx2, raw2) = pair(&[], &["ping"], &["ping"], false);
    let mut w2 = raw2.try_clone().unwrap();
    w2.write_all(&encode(&json!({"v": 1, "type": "ping", "seq": 2})))
        .unwrap(); // a gap
    assert!(
        matches!(rx2.recv(Some(Duration::from_secs(1))), Err(Error::Protocol(e)) if e.contains("sequence"))
    );
    let (_tx, mut rx3, raw3) = pair(&[], &["ping"], &["ping"], false);
    (&raw3)
        .write_all(&((FRAME_MAX + 1) as u32).to_be_bytes())
        .unwrap();
    assert!(matches!(
        rx3.recv(Some(Duration::from_secs(1))),
        Err(Error::Protocol(_))
    ));
    let (_tx, mut rx4, _raw4) = pair(&[], &["ping"], &["ping"], false);
    assert!(matches!(
        rx4.recv(Some(Duration::from_millis(50))),
        Err(Error::Idle)
    ));
    assert!(matches!(rx4.recv(Some(Duration::ZERO)), Err(Error::Idle)));
}

#[test]
fn length_checked_against_what_the_channel_accepts() {
    let max = |recv: &'static [&'static str], data: bool| pair(&[], &[], recv, data).1.max_frame();
    assert_eq!(max(INTRO_CLIENT, false), SMALL);
    assert_eq!(max(PAIR, false), SMALL);
    assert_eq!(max(PEER_SERVER, false), SMALL);
    assert_eq!(max(PEER_CLIENT, false), FRAME_MAX);
    assert_eq!(max(TRANSFER_SERVER, false), SMALL);
    assert_eq!(max(TRANSFER_CLIENT, true), DATA_MAX);
    assert_eq!(max(FIRST, false), MANIFEST_MAX);
    let (_tx, mut c, raw) = pair(&[], &[], INTRO_CLIENT, false);
    (&raw)
        .write_all(&((SMALL + 1) as u32).to_be_bytes())
        .unwrap(); // header only: the body never comes
    let t0 = Instant::now();
    assert!(
        matches!(c.recv(Some(Duration::from_secs(1))), Err(Error::Protocol(e)) if e.contains("size"))
    );
    assert!(t0.elapsed() < Duration::from_secs(1));
}

#[test]
fn whole_frame_deadline() {
    // One byte every 150 ms used to keep a connection (and its thread) forever.
    let (a, b) = UnixStream::pair().unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s2 = stop.clone();
    let drip = std::thread::spawn(move || {
        let mut a = a;
        let _ = a.write_all(&1000u32.to_be_bytes());
        while !s2.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(150));
            if a.write_all(b" ").is_err() {
                return;
            }
        }
    });
    set_frame_timeout_for_tests(Some(Duration::from_millis(600)));
    let mut c = Conn::new(
        Box::new(b.try_clone().unwrap()),
        Arc::new(b),
        &["ping"],
        &[],
        false,
    );
    let t0 = Instant::now();
    let r = c.recv(Some(Duration::from_secs(1)));
    set_frame_timeout_for_tests(None);
    assert!(matches!(r, Err(Error::Protocol(e)) if e.contains("incomplete")));
    assert!(t0.elapsed() < Duration::from_millis(1500));
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    c.close();
    let _ = drip.join();
}

#[test]
fn data_frames() {
    let (mut tx, mut rx, raw) = pair(&[], &["transfer_end"], TRANSFER_CLIENT, true);
    tx.send_data(3, 0, b"hola").unwrap();
    match rx.recv(Some(Duration::from_secs(1))).unwrap() {
        Frame::Data(d) => assert_eq!(
            (d.seq, d.index, d.offset, d.data.as_slice()),
            (1, 3, 0, &b"hola"[..])
        ),
        other => panic!("{other:?}"),
    }
    tx.send("transfer_end", json!({})).unwrap();
    assert_eq!(
        rx.recv(Some(Duration::from_secs(1))).unwrap().kind(),
        "transfer_end"
    );
    assert!(tx.send_data(0, 0, &vec![0u8; CHUNK_MAX + 1]).is_err());
    let frame = |seq: u64, len: usize| {
        let mut f = ((DATA_HEADER + len) as u32).to_be_bytes().to_vec();
        f.push(0);
        f.extend_from_slice(&seq.to_be_bytes());
        f.extend_from_slice(&0u32.to_be_bytes());
        f.extend_from_slice(&0u64.to_be_bytes());
        f.extend(std::iter::repeat_n(b'x', len));
        f
    };
    (&raw).write_all(&frame(2, 1)).unwrap(); // replayed seq
    assert!(
        matches!(rx.recv(Some(Duration::from_secs(1))), Err(Error::Protocol(e)) if e.contains("sequence"))
    );
    let (_tx, mut nodata, raw2) = pair(&[], &[], TRANSFER_CLIENT, false);
    (&raw2).write_all(&frame(1, 1)).unwrap(); // data where none is allowed
    assert!(matches!(
        nodata.recv(Some(Duration::from_secs(1))),
        Err(Error::Protocol(_))
    ));
    let (_tx, mut big, raw3) = pair(&[], &[], TRANSFER_CLIENT, true);
    (&raw3)
        .write_all(&((DATA_MAX + 1) as u32).to_be_bytes())
        .unwrap(); // bigger than a data frame
    assert!(matches!(
        big.recv(Some(Duration::from_secs(1))),
        Err(Error::Protocol(_))
    ));
}

#[test]
fn we_never_send_what_we_would_refuse() {
    let (mut tx, _rx, _raw) = pair(&[], &["ping", "hello"], &[], false);
    assert!(tx.send("pong", json!({"re": 1})).is_err()); // not ours to send here
    assert!(
        tx.send("hello", json!({"id": "nope", "name": "x"}))
            .is_err()
    );
}

#[test]
fn rate_limiter() {
    let now = Arc::new(Mutex::new(0.0f64));
    let n2 = now.clone();
    let rl = RateLimiter::new(1.0, 3, Box::new(move || *n2.lock().unwrap()));
    let got: Vec<bool> = (0..5).map(|_| rl.allow("ip")).collect();
    assert_eq!(got, vec![true, true, true, false, false]);
    assert!(rl.allow("other"));
    *now.lock().unwrap() = 2.0;
    assert!(rl.allow("ip"));
}

#[test]
fn manifest_validation() {
    let id = "0123456789abcdef";
    let m = |entries: Value, kind: &str, text: &str| json!({"v": 1, "type": "transfer", "seq": 1, "id": id, "kind": kind, "entries": entries, "text": text});
    let e = || {
        vec![
            json!({"type": "dir", "path": "Fotos", "mtime": 1}),
            json!({"type": "dir", "path": "Fotos/2026", "mtime": 2}),
            json!({"type": "file", "path": "Fotos/2026/año nuevo.jpg", "size": 3, "mtime": 3, "sha256": "a".repeat(64)}),
            json!({"type": "file", "path": "notas.txt", "size": 0, "mtime": 4, "sha256": "b".repeat(64)}),
        ]
    };
    validate(&m(json!(e()), "files", ""), &["transfer"], 0).unwrap();
    let image = json!({"type": "image", "mime": "image/png", "size": 10, "sha256": "c".repeat(64)});
    validate(
        &m(json!([image.clone()]), "image", "pie de foto"),
        &["transfer"],
        0,
    )
    .unwrap();
    let with_path = |p: &str| json!([{"type": "file", "path": p, "size": 1, "mtime": 1, "sha256": "a".repeat(64)}]);
    let long_comp = "x".repeat(256);
    let deep = vec!["d"; 33].join("/");
    let too_long = vec!["x".repeat(200); 21].join("/");
    for p in [
        "/etc/passwd",
        "../x",
        "a/../../x",
        "a/./b",
        ".",
        "..",
        "",
        "a//b",
        "a/",
        "a\0b",
        "a\nb",
        "esc\x1b[31m",
        "bidi\u{202e}txt.exe",
        &long_comp,
        &deep,
        &too_long,
    ] {
        assert!(
            validate(&m(with_path(p), "files", ""), &["transfer"], 0).is_err(),
            "{p:?}"
        );
    }
    let set = |v: &Value, k: &str, x: Value| {
        let mut v = v.clone();
        v[k] = x;
        v
    };
    let ee = e();
    let without_sha: Value = {
        let mut x = ee[3].clone();
        x.as_object_mut().unwrap().remove("sha256");
        x
    };
    let bad_lists = vec![
        json!([set(&ee[0], "type", json!("symlink"))]),
        json!([set(&ee[0], "type", json!("device"))]),
        json!([set(&ee[0], "type", json!("fifo"))]),
        json!([set(&ee[2], "path", json!("huérfano/a.txt"))]), // parent not listed
        json!([ee[0], ee[1], ee[2], ee[2]]),                   // duplicate
        json!([ee[2], ee[0], ee[1]]),                          // child before parent
        json!([
            set(&ee[3], "path", json!("f")),
            set(&ee[3], "path", json!("f/g"))
        ]), // a file as a parent
        json!([set(&ee[3], "size", json!(-1))]),
        json!([set(&ee[3], "size", json!(true))]),
        json!([set(&ee[3], "size", json!(1.5))]),
        json!([set(&ee[3], "size", json!(1u64 << 51))]),
        json!([set(&ee[3], "mtime", json!(-1))]),
        json!([set(&ee[3], "sha256", json!("z".repeat(64)))]),
        json!([set(&ee[3], "extra", json!(1))]),
        json!([without_sha]),
        json!([set(&ee[0], "size", json!(1))]),
        json!([]),
        json!([ee[3], ee[3]]),
        json!("nope"),
        json!([1]),
    ];
    for entries in bad_lists {
        assert!(
            validate(&m(entries.clone(), "files", ""), &["transfer"], 0).is_err(),
            "{entries}"
        );
    }
    let many: Vec<Value> = (0..=crate::MAX_FILES).map(|i| json!({"type": "file", "path": format!("f{i}"), "size": 0, "mtime": 1, "sha256": "a".repeat(64)})).collect();
    assert!(validate(&m(json!(many), "files", ""), &["transfer"], 0).is_err());
    for (entries, kind, text) in [
        (
            json!([set(&image, "mime", json!("image/svg+xml"))]),
            "image",
            "",
        ),
        (json!([set(&image, "size", json!(0))]), "image", ""),
        (json!([image, ee[3]]), "image", ""),
        (json!([image]), "files", ""),
        (json!(e()), "image", ""),
        (json!(e()), "files", "texto"),
    ] {
        assert!(
            validate(&m(entries.clone(), kind, text), &["transfer"], 0).is_err(),
            "{kind} {entries}"
        );
    }
    assert!(validate(&m(json!(e()), "files", ""), PEER_CLIENT, 0).is_err()); // never on the peer channel
}

#[test]
fn drag_notice() {
    let drag = |extra: Value| {
        let mut m = json!({"v": 1, "type": "drag", "seq": 1, "names": ["Proyecto", "foto 1.jpg"],
                           "count": 3, "bytes": 1234});
        for (k, v) in extra.as_object().unwrap() {
            if v.is_null() {
                m.as_object_mut().unwrap().remove(k);
            } else {
                m[k] = v.clone();
            }
        }
        m
    };
    validate(&drag(json!({})), PEER_CLIENT, 0).unwrap();
    validate(&drag(json!({"bytes": 0, "count": 2})), PEER_CLIENT, 0).unwrap();
    let names8: Vec<String> = (0..8).map(|i| format!("f{i}")).collect();
    validate(
        &drag(json!({"names": names8, "count": 10000})),
        PEER_CLIENT,
        0,
    )
    .unwrap();
    let names9: Vec<String> = (0..9).map(|i| format!("f{i}")).collect();
    let bad = [
        json!({"names": []}),
        json!({"names": names9}),
        json!({"names": ["a/b"]}),
        json!({"names": [".."]}),
        json!({"names": ["."]}),
        json!({"names": [""]}),
        json!({"names": ["x\u{202e}gpj.exe"]}),
        json!({"names": ["a\nb"]}),
        json!({"names": ["dup", "dup"]}),
        json!({"names": ["x".repeat(256)]}),
        json!({"names": [1]}),
        json!({"names": "Proyecto"}),
        json!({"count": 0}),
        json!({"count": 1}), // fewer than the names listed
        json!({"count": 10001}),
        json!({"count": 2.0}),
        json!({"bytes": -1}),
        json!({"bytes": (1u64 << 50) + 1}),
        json!({"bytes": null}),
        json!({"extra": true}),
    ];
    for b in bad {
        assert!(validate(&drag(b.clone()), PEER_CLIENT, 0).is_err(), "{b}");
    }
    // a peer-channel request only: not a first message, not on a transfer connection
    assert!(validate(&drag(json!({})), FIRST, 0).is_err());
    assert!(validate(&drag(json!({})), TRANSFER_CLIENT, 0).is_err());
    assert!(validate(&drag(json!({})), PEER_SERVER, 0).is_err());
}
