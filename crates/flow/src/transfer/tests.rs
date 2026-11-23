use super::*;
use crate::util::sha256_hex as sha;

fn staging(tmp: &Path) -> Staging {
    Staging::new(tmp.join("cache").join("glisa").join("flow-received"))
}

#[cfg(unix)]
fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777
}

#[test]
fn uri_list() {
    let data = b"# comment\r\nfile:///home/ana/Mis%20fotos\r\nfile://localhost/tmp/a.txt\r\n\
file://otro-equipo/tmp/b.txt\r\nhttps://example.com/x\r\nfile:relativo\r\nfile:///tmp/%00nul\r\n\
file:///home/ana/Mis%20fotos\r\nfile:///tmp/e%CC%81\r\nfile:///tmp/x/../y\r\n";
    let got: Vec<String> = parse_uri_list(data, 100)
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    if cfg!(unix) {
        assert_eq!(
            got,
            vec![
                "/home/ana/Mis fotos",
                "/tmp/a.txt",
                "/tmp/e\u{301}",
                "/tmp/y"
            ]
        );
        let p = Path::new("/tmp/Mis fotos/ñ.txt");
        assert_eq!(uri_of(p), "file:///tmp/Mis%20fotos/%C3%B1.txt");
        assert_eq!(
            parse_uri_list(uri_of(p).as_bytes(), 10),
            vec![p.to_path_buf()]
        );
    }
}

#[cfg(unix)]
#[test]
fn staging_is_private_and_cleaned() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let s = staging(tmp.path());
    let (_tid, folder) = s.new_folder().unwrap();
    assert_eq!(mode(&s.root), 0o700);
    assert_eq!(mode(&folder), 0o700);
    let old = s.root.join(format!(
        "{:010}-0123abcd",
        crate::util::unix_time() as u64 - 25 * 3600
    ));
    fs::create_dir(&old).unwrap();
    assert_eq!(s.cleanup(STAGING_TTL, &[]), 1);
    assert!(!old.exists());
    assert!(folder.is_dir());
    fs::set_permissions(&s.root, fs::Permissions::from_mode(0o755)).unwrap();
    drop(s.prepare().unwrap());
    assert_eq!(mode(&s.root), 0o700);
    assert_eq!(s.cleanup(0, &[]), 1);
    assert_eq!(fs::read_dir(&s.root).unwrap().count(), 0);
}

#[cfg(unix)]
fn incoming(s: &Staging, entries: Vec<Value>) -> (Incoming, PathBuf) {
    let (_tid, folder) = s.new_folder().unwrap();
    let mut inc = Incoming::with_rules(
        folder.clone(),
        "files",
        entries,
        "",
        NameRules {
            windows: false,
            case_insensitive: false,
        },
    );
    inc.create().unwrap();
    (inc, folder)
}

#[cfg(unix)]
#[test]
fn sizes_order_and_hashes_are_enforced() {
    use std::os::unix::fs::MetadataExt;
    let tmp = tempfile::tempdir().unwrap();
    let s = staging(tmp.path());
    let (good, bad) = (b"contenido".to_vec(), b"otra cosa".to_vec());
    let entries = vec![
        json!({"type": "dir", "path": "d", "mtime": 1_000_000_000_000_000_000u64}),
        json!({"type": "file", "path": "d/a", "size": good.len(), "mtime": 1, "sha256": sha(&good)}),
        json!({"type": "file", "path": "b", "size": bad.len(), "mtime": 2, "sha256": sha(&good)}),
        json!({"type": "file", "path": "vacío", "size": 0, "mtime": 3, "sha256": "f".repeat(64)}),
    ];
    let (mut inc, folder) = incoming(&s, entries);
    assert!(matches!(inc.data(2, 0, &good), Err(XferError::Protocol(_)))); // not the current entry
    assert!(matches!(inc.data(1, 1, &good), Err(XferError::Protocol(_)))); // a gap
    assert!(matches!(
        inc.data(1, 0, &[good.clone(), b"!".to_vec()].concat()),
        Err(XferError::Protocol(_))
    )); // too much
    inc.data(1, 0, &good[..4]).unwrap();
    assert!(matches!(inc.end(), Err(XferError::Protocol(_)))); // ends before all data
    inc.data(1, 4, &good[4..]).unwrap();
    inc.data(2, 0, &bad).unwrap(); // wrong hash: deleted
    inc.end().unwrap();
    let mut failed = inc.failed.clone();
    failed.sort();
    assert_eq!(failed, vec![2, 3]); // and the empty file whose hash lies
    let names: Vec<String> = fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["d"]);
    let m = fs::metadata(folder.join("d").join("a")).unwrap();
    assert_eq!(
        (mode(&folder.join("d").join("a")), m.mtime(), m.mtime_nsec()),
        (0o600, 0, 1)
    );
    let dm = fs::metadata(folder.join("d")).unwrap();
    assert_eq!(
        dm.mtime() as i128 * 1_000_000_000 + dm.mtime_nsec() as i128,
        1_000_000_000_000_000_000
    );
    assert_eq!(inc.top_level(), vec![folder.join("d")]);
}

#[cfg(unix)]
#[test]
fn skip_drops_the_partial_file() {
    let tmp = tempfile::tempdir().unwrap();
    let s = staging(tmp.path());
    let data = vec![b'x'; 10];
    let (mut inc, folder) = incoming(
        &s,
        vec![json!({"type": "file", "path": "a", "size": 10, "mtime": 1, "sha256": sha(&data)})],
    );
    inc.data(0, 0, &data[..4]).unwrap();
    assert!(inc.skip(1).is_err());
    inc.skip(0).unwrap();
    inc.end().unwrap();
    assert_eq!(fs::read_dir(&folder).unwrap().count(), 0);
    assert_eq!((inc.failed.clone(), inc.received), (vec![0], 10));
}

#[cfg(unix)]
#[test]
fn nothing_escapes_through_a_planted_link() {
    let tmp = tempfile::tempdir().unwrap();
    let s = staging(tmp.path());
    let outside = tmp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let (mut inc, folder) = incoming(
        &s,
        vec![
            json!({"type": "dir", "path": "d", "mtime": 1}),
            json!({"type": "file", "path": "d/x", "size": 1, "mtime": 1, "sha256": sha(b"1")}),
        ],
    );
    fs::remove_dir(folder.join("d")).unwrap();
    std::os::unix::fs::symlink(&outside, folder.join("d")).unwrap(); // the folder swapped for a link
    assert!(matches!(inc.data(1, 0, b"1"), Err(XferError::Io(_))));
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
}

#[test]
fn cross_os_names_never_collide_or_overwrite() {
    let tmp = tempfile::tempdir().unwrap();
    let s = staging(tmp.path());
    let (_tid, folder) = s.new_folder().unwrap();
    let a = b"one".to_vec();
    let b = b"two".to_vec();
    let entries = vec![
        json!({"type": "file", "path": "Foto.jpg", "size": 3, "mtime": 1, "sha256": sha(&a)}),
        json!({"type": "file", "path": "foto.jpg", "size": 3, "mtime": 1, "sha256": sha(&b)}),
        json!({"type": "file", "path": "e\u{301}", "size": 0, "mtime": 1, "sha256": EMPTY_SHA256}),
    ];
    let rules = NameRules {
        windows: cfg!(windows),
        case_insensitive: true,
    };
    let mut inc = Incoming::with_rules(folder.clone(), "files", entries, "", rules);
    inc.create().unwrap();
    inc.data(0, 0, &a).unwrap();
    inc.data(1, 0, &b).unwrap();
    inc.end().unwrap();
    let mut names: Vec<String> = fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["Foto.jpg", "foto (2).jpg", "\u{e9}"]);
    assert_eq!(fs::read(folder.join("foto (2).jpg")).unwrap(), b);
    assert_eq!(inc.top_level().len(), 3);
}

#[cfg(unix)]
#[test]
fn links_special_files_and_macos_junk_are_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let p = |parts: &[&str]| {
        parts
            .iter()
            .fold(tmp.path().to_path_buf(), |acc, x| acc.join(x))
    };
    fs::create_dir_all(p(&["src", "sub"])).unwrap();
    fs::write(p(&["src", "sub", "a.txt"]), "a").unwrap();
    fs::write(p(&["src", ".DS_Store"]), "junk").unwrap();
    fs::write(p(&["src", "._a.txt"]), "junk").unwrap();
    fs::write(p(&["secret"]), "never").unwrap();
    std::os::unix::fs::symlink(p(&["secret"]), p(&["src", "link"])).unwrap();
    std::os::unix::fs::symlink(p(&["secret"]), p(&["toplink"])).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        p(&["src", "fifo"]),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    let cancel = AtomicBool::new(false);
    let mut out = scan_files(
        &[p(&["src"]), p(&["toplink"])],
        ScanLimits {
            max_bytes: 1 << 30,
            max_files: 10_000,
        },
        &cancel,
    )
    .unwrap();
    hash_sources(&mut out, &cancel, &|_| {}).unwrap();
    let rels: Vec<&str> = out.sources.iter().map(|s| s.rel.as_str()).collect();
    assert_eq!(rels, vec!["src", "src/sub", "src/sub/a.txt"]);
    assert_eq!(
        (out.items, out.files, out.folders, out.skipped),
        (1, 1, 2, 3)
    );
    assert_eq!(out.manifest()[2]["sha256"], sha(b"a"));
}

#[test]
fn caps() {
    let tmp = tempfile::tempdir().unwrap();
    let p = |parts: &[&str]| {
        parts
            .iter()
            .fold(tmp.path().to_path_buf(), |acc, x| acc.join(x))
    };
    let cancel = AtomicBool::new(false);
    fs::create_dir_all(p(&["big"])).unwrap();
    fs::write(p(&["big", "f"]), vec![0u8; 2048]).unwrap();
    let lim = |max_bytes, max_files| ScanLimits {
        max_bytes,
        max_files,
    };
    assert!(matches!(
        scan_files(&[p(&["big"])], lim(1024, 10_000), &cancel),
        Err(XferError::Refused {
            reason: "too_large",
            ..
        })
    ));
    let mut deep = p(&["deep"]);
    for _ in 0..33 {
        deep.push("d");
    }
    fs::create_dir_all(&deep).unwrap();
    match scan_files(&[p(&["deep"])], lim(1 << 30, 10_000), &cancel) {
        Err(XferError::Refused {
            message: Some(m), ..
        }) => assert!(m.english().contains("32")),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        scan_files(&[p(&["deep"])], lim(1 << 30, 3), &cancel),
        Err(XferError::Refused {
            reason: "too_many",
            ..
        })
    ));
    fs::write(p(&["same"]), "1").unwrap();
    fs::create_dir_all(p(&["other"])).unwrap();
    fs::write(p(&["other", "same"]), "2").unwrap();
    let out = scan_files(
        &[p(&["same"]), p(&["other", "same"])],
        lim(1 << 30, 10_000),
        &cancel,
    )
    .unwrap();
    let rels: Vec<&str> = out.sources.iter().map(|s| s.rel.as_str()).collect();
    assert_eq!(rels, vec!["same", "same (2)"]);
}
