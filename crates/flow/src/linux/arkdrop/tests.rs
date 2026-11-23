use super::*;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::time::SystemTime;

/// A staging folder like the receiver's: `Proyecto/` (with a nested folder) and
/// `foto 1.jpg`, files 0600 and folders 0700; the top-level items.
fn staged(root: &Path) -> Vec<PathBuf> {
    let folder = root.join("1790000000-0a1b2c3d");
    fs::create_dir_all(folder.join("Proyecto/sub")).unwrap();
    fs::write(folder.join("Proyecto/notas.txt"), b"notas").unwrap();
    fs::write(folder.join("Proyecto/sub/hondo.txt"), b"hondo").unwrap();
    fs::write(folder.join("foto 1.jpg"), b"\xff\xd8foto").unwrap();
    for (p, m) in [
        ("", 0o700),
        ("Proyecto", 0o700),
        ("Proyecto/sub", 0o700),
        ("Proyecto/notas.txt", 0o600),
        ("Proyecto/sub/hondo.txt", 0o600),
        ("foto 1.jpg", 0o600),
    ] {
        fs::set_permissions(folder.join(p), fs::Permissions::from_mode(m)).unwrap();
    }
    vec![folder.join("Proyecto"), folder.join("foto 1.jpg")]
}

fn mode(p: &Path) -> u32 {
    fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn ready_drop(items: Vec<PathBuf>) -> Arc<ArkDrop> {
    let d = ArkDrop::new("/Flow/Drop");
    d.ready(items);
    d
}

fn root() -> bool {
    rustix::process::geteuid().is_root()
}

#[test]
fn the_object_path_has_a_random_token() {
    let (a, b) = (ArkDrop::new("/Flow/Drop"), ArkDrop::new("/Flow/Drop"));
    let token = a.path().strip_prefix("/Flow/Drop/").unwrap();
    assert_eq!(token.len(), 32);
    assert!(token.bytes().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(a.path(), b.path());
    assert!(zbus::zvariant::ObjectPath::try_from(a.path()).is_ok());
}

#[test]
fn copies_files_and_folders_without_overwriting() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let items = staged(src.path());
    // a file the sender had executable (the receiver never stages that, but still)
    fs::set_permissions(&items[1], fs::Permissions::from_mode(0o755)).unwrap();
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    fs::File::options()
        .write(true)
        .open(&items[1])
        .unwrap()
        .set_modified(when)
        .unwrap();
    // the destination has both names already
    fs::write(dst.path().join("foto 1.jpg"), b"mine").unwrap();
    fs::create_dir(dst.path().join("Proyecto")).unwrap();
    let d = ready_drop(items.clone());
    assert_eq!(d.extract(&dst.path().to_string_lossy(), FILES_WAIT), Ok(2));
    assert_eq!(
        names(dst.path()),
        ["Proyecto", "Proyecto (2)", "foto 1 (2).jpg", "foto 1.jpg"]
    );
    // nothing overwritten, nothing moved
    assert_eq!(fs::read(dst.path().join("foto 1.jpg")).unwrap(), b"mine");
    assert!(names(&dst.path().join("Proyecto")).is_empty());
    assert!(items.iter().all(|p| p.exists()));
    let copy = dst.path().join("Proyecto (2)");
    assert_eq!(fs::read(copy.join("notas.txt")).unwrap(), b"notas");
    assert_eq!(fs::read(copy.join("sub/hondo.txt")).unwrap(), b"hondo");
    let foto = dst.path().join("foto 1 (2).jpg");
    assert_eq!(fs::read(&foto).unwrap(), b"\xff\xd8foto");
    // modes as staged, never executable; times kept
    assert_eq!(mode(&foto), 0o644);
    assert_eq!(mode(&copy.join("notas.txt")), 0o600);
    assert_eq!((mode(&copy), mode(&copy.join("sub"))), (0o700, 0o700));
    assert_eq!(fs::metadata(&foto).unwrap().modified().unwrap(), when);
    // a second drop on the same folder: the next free names
    let d = ready_drop(items);
    let uri = format!("file://{}", dst.path().display());
    assert_eq!(d.extract(&uri, FILES_WAIT), Ok(2));
    assert!(dst.path().join("Proyecto (3)/sub/hondo.txt").exists());
    assert!(dst.path().join("foto 1 (3).jpg").exists());
}

#[test]
fn never_follows_a_link() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secreto"), b"secreto").unwrap();
    let mut items = staged(src.path());
    let folder = items[0].parent().unwrap().to_path_buf();
    // links in the staging folder (a received transfer never has any): skipped
    symlink(outside.path().join("secreto"), folder.join("enlace")).unwrap();
    symlink(outside.path(), folder.join("Proyecto/fuera")).unwrap();
    items.push(folder.join("enlace"));
    // a link in the destination with the name of a staged file: never written through
    symlink(outside.path().join("nuevo"), dst.path().join("foto 1.jpg")).unwrap();
    let d = ready_drop(items);
    assert_eq!(d.extract(&dst.path().to_string_lossy(), FILES_WAIT), Ok(2));
    assert_eq!(
        names(dst.path()),
        ["Proyecto", "foto 1 (2).jpg", "foto 1.jpg"]
    );
    assert_eq!(names(&dst.path().join("Proyecto")), ["notas.txt", "sub"]);
    assert_eq!(names(outside.path()), ["secreto"]);
    // only links: nothing to copy
    let d = ready_drop(vec![folder.join("enlace")]);
    assert!(
        d.extract(&dst.path().to_string_lossy(), FILES_WAIT)
            .is_err()
    );
    // items from different folders, or a staging folder that is a link: refused
    let d = ready_drop(vec![
        folder.join("foto 1.jpg"),
        outside.path().join("secreto"),
    ]);
    assert!(
        d.extract(&dst.path().to_string_lossy(), FILES_WAIT)
            .is_err()
    );
    let linked = src.path().join("enlazada");
    symlink(&folder, &linked).unwrap();
    let d = ready_drop(vec![linked.join("foto 1.jpg")]);
    assert!(
        d.extract(&dst.path().to_string_lossy(), FILES_WAIT)
            .is_err()
    );
    assert_eq!(names(outside.path()), ["secreto"]);
}

#[test]
fn the_destination_is_an_existing_writable_local_folder() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let items = staged(src.path());
    fs::write(dst.path().join("archivo"), b"x").unwrap();
    let ro = dst.path().join("solo lectura");
    fs::create_dir(&ro).unwrap();
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o500)).unwrap();
    let mut bad = vec![
        String::new(),
        "Escritorio".into(),
        "sftp://servidor/home/ana".into(),
        "desktop:/".into(),
        "file://otro/tmp".into(),
        "/no/existe/en/ningun/sitio".into(),
        dst.path().join("archivo").to_string_lossy().into_owned(),
        // inside the dragged files (copying a folder into itself)
        items[0].to_string_lossy().into_owned(),
        items[0].join("sub").to_string_lossy().into_owned(),
        items[0].parent().unwrap().to_string_lossy().into_owned(),
    ];
    if !root() {
        bad.push(ro.to_string_lossy().into_owned());
    }
    for dest in bad {
        let d = ready_drop(items.clone());
        let t0 = Instant::now();
        assert!(d.extract(&dest, FILES_WAIT).is_err(), "{dest:?}");
        assert!(t0.elapsed() < Duration::from_secs(1));
        assert!(matches!(d.wait_outcome(Duration::ZERO), Some(Err(_))));
    }
    assert!(names(&items[0]).contains(&"notas.txt".to_string()));
    assert_eq!(names(dst.path()), ["archivo", "solo lectura"]);
    // a link to a folder is that folder (canonicalized); a file: URI is decoded
    let real = dst.path().join("carpeta real");
    fs::create_dir(&real).unwrap();
    symlink(&real, dst.path().join("atajo")).unwrap();
    let d = ready_drop(items.clone());
    assert_eq!(
        d.extract(&dst.path().join("atajo").to_string_lossy(), FILES_WAIT),
        Ok(2)
    );
    assert_eq!(names(&real), ["Proyecto", "foto 1.jpg"]);
    let d = ready_drop(items);
    let uri = crate::transfer::uri_of(&real);
    assert!(uri.contains("%20"));
    assert_eq!(d.extract(&uri, FILES_WAIT), Ok(2));
    assert_eq!(names(&real).len(), 4);
}

#[test]
fn a_failed_copy_leaves_nothing_behind() {
    if root() {
        return; // root reads anything
    }
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let mut items = staged(src.path());
    items.reverse(); // the file first: copied, then removed again
    let hidden = items[1].join("sub/hondo.txt");
    fs::set_permissions(&hidden, fs::Permissions::from_mode(0o000)).unwrap();
    let d = ready_drop(items);
    let e = d
        .extract(&dst.path().to_string_lossy(), FILES_WAIT)
        .unwrap_err();
    assert!(e.starts_with("hondo.txt:"), "{e}");
    assert!(names(dst.path()).is_empty());
    fs::set_permissions(&hidden, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn one_call_per_drag() {
    let d = ArkDrop::new("/Flow/Drop");
    assert!(!d.claimed());
    assert!(d.claim());
    assert!(!d.claim());
    assert!(d.claimed());
}

#[test]
fn the_call_waits_for_the_files() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let items = staged(src.path());
    let dest = dst.path().to_string_lossy().into_owned();
    // they come a moment later: copied then
    let d = ArkDrop::new("/Flow/Drop");
    let (d2, dest2) = (d.clone(), dest.clone());
    let t = std::thread::spawn(move || d2.extract(&dest2, Duration::from_secs(5)));
    std::thread::sleep(Duration::from_millis(300));
    assert!(names(dst.path()).is_empty());
    assert_eq!(d.wait_outcome(Duration::ZERO), None);
    d.ready(items.clone());
    assert_eq!(t.join().unwrap(), Ok(2));
    assert_eq!(d.wait_outcome(Duration::ZERO), Some(Ok(2)));
    // they never come: bounded
    let d = ArkDrop::new("/Flow/Drop");
    let t0 = Instant::now();
    assert!(d.extract(&dest, Duration::from_millis(300)).is_err());
    assert!(t0.elapsed() >= Duration::from_millis(300));
    // the pickup ends while it waits: it stops at once
    let d = ArkDrop::new("/Flow/Drop");
    let (d2, dest2) = (d.clone(), dest.clone());
    let t = std::thread::spawn(move || d2.extract(&dest2, Duration::from_secs(30)));
    std::thread::sleep(Duration::from_millis(200));
    let t0 = Instant::now();
    d.retire();
    assert!(t.join().unwrap().is_err());
    assert!(t0.elapsed() < Duration::from_secs(2));
    assert_eq!(names(dst.path()), ["Proyecto", "foto 1.jpg"]);
}

#[test]
fn the_answer_comes_within_the_reply_wait() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let items = staged(src.path());
    let dest = dst.path().to_string_lossy().into_owned();
    let block_on = |a: Answer| {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(v) = lock(&a.0).0.take() {
                return v;
            }
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    // the files are here: the copy's result
    let d = ready_drop(items.clone());
    assert_eq!(block_on(d.start(dest.clone(), REPLY_WAIT)), Some(Ok(2)));
    // a bad destination: the error
    let d = ready_drop(items.clone());
    assert!(matches!(
        block_on(d.start("desktop:/".into(), REPLY_WAIT)),
        Some(Err(_))
    ));
    // the files still coming: no answer yet after the wait, the copy goes on
    let d = ArkDrop::new("/Flow/Drop");
    let t0 = Instant::now();
    assert_eq!(block_on(d.start(dest, Duration::from_millis(300))), None);
    assert!(t0.elapsed() < Duration::from_secs(2));
    d.ready(items);
    assert_eq!(d.wait_outcome(Duration::from_secs(5)), Some(Ok(2)));
    assert_eq!(names(dst.path()).len(), 4);
}

#[test]
fn the_drop_counts_as_taken_when_the_copy_succeeds() {
    let short = Duration::from_millis(200);
    // nobody calls: what the URI list says
    for taken in [true, false] {
        let d = ArkDrop::new("/Flow/Drop");
        d.sent(SERVICE_MIME);
        assert!(!d.offered());
        d.sent(PATH_MIME);
        assert!(d.offered());
        let t0 = Instant::now();
        assert_eq!(d.taken_after_drop(taken, short, short), taken);
        assert!(t0.elapsed() >= short);
    }
    // the call came (before or right after the drop): the copy decides
    for (outcome, want) in [(Ok(1), true), (Err("no".to_string()), false)] {
        let d = ArkDrop::new("/Flow/Drop");
        let d2 = d.clone();
        let o = outcome.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            d2.claim();
            std::thread::sleep(Duration::from_millis(100));
            lock(&d2.st).outcome = Some(o);
            d2.cv.notify_all();
        });
        assert_eq!(
            d.taken_after_drop(!want, Duration::from_secs(2), Duration::from_secs(2)),
            want
        );
        t.join().unwrap();
    }
    // a copy that takes too long: not taken (the files go on the clipboard too)
    let d = ArkDrop::new("/Flow/Drop");
    d.claim();
    assert!(!d.taken_after_drop(true, short, short));
}

// ================================================================ over D-Bus
/// A private bus of our own for the test (never the session's): killed when dropped.
struct Bus {
    child: std::process::Child,
    address: String,
    _dir: tempfile::TempDir,
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn private_bus() -> Option<Bus> {
    use std::io::BufRead;
    let dir = tempfile::tempdir().ok()?;
    let config = dir.path().join("bus.conf");
    fs::write(
        &config,
        format!(
            r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>"#,
            dir.path().join("bus").display()
        ),
    )
    .ok()?;
    let mut child = std::process::Command::new("dbus-daemon")
        .arg(format!("--config-file={}", config.display()))
        .args(["--nofork", "--print-address"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut line = String::new();
    let out = child.stdout.take()?;
    std::io::BufReader::new(out).read_line(&mut line).ok()?;
    let bus = Bus {
        child,
        address: line.trim().to_string(),
        _dir: dir,
    };
    (!bus.address.is_empty()).then_some(bus)
}

fn connect(bus: &Bus) -> zbus::blocking::Connection {
    zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .build()
        .unwrap()
}

#[test]
fn the_dbus_object_copies_once() {
    let Some(bus) = private_bus() else {
        eprintln!("no dbus-daemon here: skipped");
        return;
    };
    let server = connect(&bus);
    let client = connect(&bus);
    let service = server.unique_name().unwrap().to_string();
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let items = staged(src.path());
    let call = |path: &str, dest: &str| {
        client
            .call_method(
                Some(service.as_str()),
                path,
                Some(INTERFACE),
                "extractSelectedFilesTo",
                &(dest,),
            )
            .map(|_| ())
            .map_err(|e| match e {
                zbus::Error::MethodError(name, _, _) => name.to_string(),
                e => e.to_string(),
            })
    };
    // a drop with a bad destination: a D-Bus error, and the object is gone (one call)
    let a = ready_drop(items.clone());
    assert!(
        server
            .object_server()
            .at(a.path(), DndExtract(a.clone()))
            .unwrap()
    );
    assert_eq!(
        call(a.path(), "sftp://servidor/home"),
        Err("org.freedesktop.DBus.Error.Failed".into())
    );
    assert_eq!(
        call(a.path(), &dst.path().to_string_lossy()),
        Err("org.freedesktop.DBus.Error.UnknownObject".into())
    );
    assert!(names(dst.path()).is_empty());
    // no object at a path nobody was given
    assert_eq!(
        call("/Flow/Drop/0123456789abcdef0123456789abcdef", "/tmp"),
        Err("org.freedesktop.DBus.Error.UnknownObject".into())
    );
    // a good one: copied before the answer, then gone
    let b = ready_drop(items);
    server
        .object_server()
        .at(b.path(), DndExtract(b.clone()))
        .unwrap();
    assert_eq!(call(b.path(), &dst.path().to_string_lossy()), Ok(()));
    assert_eq!(names(dst.path()), ["Proyecto", "foto 1.jpg"]);
    assert!(b.claimed());
    assert_eq!(b.wait_outcome(Duration::ZERO), Some(Ok(2)));
    assert_eq!(
        call(b.path(), &dst.path().to_string_lossy()),
        Err("org.freedesktop.DBus.Error.UnknownObject".into())
    );
    assert_eq!(names(dst.path()), ["Proyecto", "foto 1.jpg"]);
}
