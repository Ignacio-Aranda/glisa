//! The real desktop code paths in an ISOLATED nested session: a `kwin_wayland --virtual
//! --xwayland` with its own D-Bus session (nothing reaches the user's desktop, clipboard or
//! input devices; no uinput device is created, no evdev node is read: the drag watch hears
//! the test instead of the mouse). Run with `tests/live_desktop.sh`, which starts the nested
//! KWin and runs these tests inside it.

#![cfg(target_os = "linux")]

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use glisa_flow::backends::{EdgeBackend, PickupNews, PickupStep, Sink};
use glisa_flow::clipboard::RichClipboard;
use glisa_flow::geometry::Rect;
use glisa_flow::linux::datacontrol::DataControl;
use glisa_flow::linux::input::{PointerEvent, PointerSource};
use glisa_flow::linux::kwin::{KWinBackend, KWinHandle};
use glisa_flow::linux::x11::X11Clipboard;
use glisa_flow::text::Text;
use glisa_flow::transfer::{ClipData, Offers};

fn nested() -> bool {
    std::env::var("LO_FLOW_NESTED").is_ok_and(|v| v == "1")
}

fn wait(timeout: f64, mut f: impl FnMut() -> bool) -> bool {
    let end = std::time::Instant::now() + Duration::from_secs_f64(timeout);
    while std::time::Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    f()
}

#[derive(Default)]
struct Rec(Mutex<Vec<String>>);

impl Sink for Rec {
    fn edge(&self, side: &'static str, x: f64, y: f64, _r: Rect) {
        self.0.lock().unwrap().push(format!("edge {side} {x} {y}"));
    }
    fn leave(&self, side: &'static str) {
        self.0.lock().unwrap().push(format!("leave {side}"));
    }
    fn ctrl(&self, held: bool) {
        self.0.lock().unwrap().push(format!("ctrl {held}"));
    }
    fn screens(&self, r: Vec<Rect>) {
        self.0.lock().unwrap().push(format!("screens {r:?}"));
    }
    fn state(&self, state: &str, m: Option<Text>) {
        self.0.lock().unwrap().push(format!(
            "state {state} {}",
            m.map(|t| t.english()).unwrap_or_default()
        ));
    }
}

#[test]
#[ignore = "needs the nested session of tests/live_desktop.sh"]
fn kwin_backend_in_a_nested_session() {
    assert!(nested(), "run through tests/live_desktop.sh");
    let inner = Arc::new(KWinBackend::new());
    inner.create_pointer.store(false, Ordering::SeqCst);
    let b = KWinHandle(inner);
    let sink = Arc::new(Rec::default());
    b.start(sink.clone());
    // the script was loaded through org.kde.KWin /Scripting, ran, and said hello on our object
    assert!(
        wait(10.0, || b.state().0 == "ok"),
        "{:?} {:?}",
        b.state(),
        sink.0.lock().unwrap()
    );
    assert!(!b.screens().is_empty(), "{:?}", sink.0.lock().unwrap());
    eprintln!(
        "live: KWin script loaded; hello received; screens {:?}",
        b.screens()
    );
    eprintln!("live: Ctrl available: {} (keystate)", b.ctrl_available());
    // data-control: our selection is what another client sees
    assert!(wait(10.0, || b.rich().is_some_and(|r| r.available())));
    assert_eq!(b.clipboard_kind(), "data-control"); // no klipper in the nested session
    let mut o = Offers::new();
    o.insert(
        "text/uri-list".into(),
        ClipData::Bytes(b"file:///tmp/x\r\n".to_vec()),
    );
    o.insert(
        "text/plain;charset=utf-8".into(),
        ClipData::Bytes(b"/tmp/x".to_vec()),
    );
    assert!(b.rich().unwrap().put(&o));
    let other = DataControl::new();
    assert!(other.start(None), "{}", other.error.lock().unwrap());
    let formats = other.formats(Duration::from_secs(2)).unwrap();
    assert!(
        formats.contains(&"text/uri-list".to_string()),
        "{formats:?}"
    );
    assert_eq!(
        other
            .read("text/uri-list", 1024, Duration::from_secs(2))
            .unwrap()
            .unwrap(),
        b"file:///tmp/x\r\n"
    );
    assert!(b.clipboard_set("hola ñandú"));
    assert_eq!(
        b.clipboard_get(Duration::from_secs(2)).as_deref(),
        Some("hola ñandú")
    );
    assert_eq!(
        other.text(Duration::from_secs(2)).as_deref(),
        Some("hola ñandú")
    );
    other.close();
    eprintln!("live: data-control put/read through the compositor works");
    b.stop();
    assert_eq!(b.state().0, "off");
}

#[test]
#[ignore = "needs the nested session of tests/live_desktop.sh"]
fn x11_selections_in_a_nested_session() {
    assert!(nested(), "run through tests/live_desktop.sh");
    let owner = X11Clipboard::start().expect("X11");
    let reader = X11Clipboard::start().expect("X11");
    let big: Vec<u8> = (0..1_500_000u32).map(|i| (i % 251) as u8).collect(); // needs INCR
    let mut o = Offers::new();
    o.insert("image/png".into(), ClipData::Bytes(big.clone()));
    o.insert(
        "UTF8_STRING".into(),
        ClipData::Bytes("texto ñ".as_bytes().to_vec()),
    );
    assert!(owner.put(&o));
    let formats = reader.formats(Duration::from_secs(2)).unwrap();
    assert!(
        formats.contains(&"image/png".to_string()) && formats.contains(&"TARGETS".to_string()),
        "{formats:?}"
    );
    assert_eq!(
        reader
            .read("UTF8_STRING", 1024, Duration::from_secs(2))
            .unwrap()
            .unwrap(),
        "texto ñ".as_bytes()
    );
    let got = reader
        .read("image/png", 4 << 20, Duration::from_secs(10))
        .unwrap()
        .unwrap();
    assert_eq!(got.len(), big.len());
    assert!(got == big, "INCR data differs");
    assert!(
        reader
            .read("image/png", 1000, Duration::from_secs(10))
            .is_err()
    ); // over the limit
    owner.close();
    reader.close();
    eprintln!("live: X11 selections (TARGETS, text, 1.5 MB INCR) work");
}

// ============================================================ dragging files to the edge
mod dnd {
    //! Two tiny wire-level clients for the drag test: a drag source (a layer surface that
    //! starts a drag from a button press on it) and KWin's fake input (pointer motion and
    //! buttons, allowed in the nested session by KWIN_WAYLAND_NO_PERMISSION_CHECKS=1).

    use std::collections::HashMap;
    use std::io::Write;
    use std::os::fd::AsFd;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use glisa_flow::linux::wayland::{self, Wire, read_string, read_u32, string, u32s};

    // Object ids: a client must create them in order, without gaps (registry 2, sync 3).
    const REG: u32 = 2;

    fn globals(wire: &mut Wire) -> HashMap<String, (u32, u32)> {
        wayland::globals(wire, REG, 3, Duration::from_secs(2)).expect("globals")
    }

    pub struct FakeInput {
        wire: Wire,
        id: u32,
    }

    impl FakeInput {
        pub fn connect() -> Option<Self> {
            let mut wire = Wire::new(wayland::connect(Duration::from_secs(2)).ok()?);
            let g = globals(&mut wire);
            let (name, version) = *g.get("org_kde_kwin_fake_input")?;
            if version < 3 {
                return None;
            }
            wayland::bind(&wire, REG, name, "org_kde_kwin_fake_input", 3, 4).ok()?;
            let mut p = string("glisa-test");
            p.extend(string("drag test"));
            wire.send(4, 0, &p).ok()?; // authenticate
            Some(Self { wire, id: 4 })
        }

        fn fixed(v: f64) -> [u8; 4] {
            ((v * 256.0) as i32).to_ne_bytes()
        }

        pub fn move_to(&mut self, x: f64, y: f64) {
            let p = [Self::fixed(x), Self::fixed(y)].concat();
            self.wire.send(self.id, 9, &p).unwrap(); // pointer_motion_absolute
            let _ = self.wire.read(Duration::from_millis(30));
        }

        pub fn nudge(&mut self, dx: f64, dy: f64) {
            let p = [Self::fixed(dx), Self::fixed(dy)].concat();
            self.wire.send(self.id, 1, &p).unwrap(); // pointer_motion
            let _ = self.wire.read(Duration::from_millis(30));
        }

        pub fn button(&mut self, pressed: bool) {
            self.wire
                .send(self.id, 2, &u32s(&[0x110, pressed as u32]))
                .unwrap(); // BTN_LEFT
            let _ = self.wire.read(Duration::from_millis(30));
        }
    }

    #[derive(Debug, Default)]
    pub struct SourceState {
        pub configured: bool,
        pub entered: bool,
        pub press_serial: Option<u32>,
        pub cancelled: bool,
        pub drop_performed: bool,
        pub finished: bool,
        pub action: u32,
        pub sent: Vec<String>,
        /// Pointer motion events on the source surface, and whether its button came up.
        pub motions: u32,
        pub released: bool,
        /// Every `target` event of the drag (what the surface under the pointer accepts).
        pub targets: Vec<String>,
    }

    /// A 300×200 layer surface in the middle of the screen; a button press on it starts a
    /// drag offering `mimes` (copy or move), serving `data` for each.
    pub struct DragSource {
        pub st: Arc<Mutex<SourceState>>,
        sender: Arc<wayland::Sender>,
        ids: (u32, u32, u32), // surface, device, data manager
        next: Arc<Mutex<u32>>,
        offers: Vec<(&'static str, Vec<u8>)>,
        source: Arc<Mutex<u32>>,
    }

    impl DragSource {
        pub fn start(offers: Vec<(&'static str, Vec<u8>)>) -> Self {
            let mut wire = Wire::new(wayland::connect(Duration::from_secs(2)).unwrap());
            let g = globals(&mut wire);
            let b = |w: &Wire, iface: &str, v: u32, id: u32| {
                let (name, ver) = g[iface];
                wayland::bind(w, REG, name, iface, v.min(ver), id).unwrap();
            };
            let (comp, shm, layer_shell, seat, ddm) = (4, 5, 6, 7, 8);
            b(&wire, "wl_compositor", 4, comp);
            b(&wire, "wl_shm", 1, shm);
            b(&wire, "zwlr_layer_shell_v1", 3, layer_shell);
            b(&wire, "wl_seat", 5, seat);
            b(&wire, "wl_data_device_manager", 3, ddm);
            let (pointer, device, surface, layer) = (9, 10, 11, 12);
            wire.send(seat, 0, &u32s(&[pointer])).unwrap(); // get_pointer
            wire.send(ddm, 1, &u32s(&[device, seat])).unwrap(); // get_data_device
            wire.send(comp, 0, &u32s(&[surface])).unwrap();
            let mut p = u32s(&[layer, surface, 0, 2]); // top layer, any output
            p.extend(string("glisa-test-source"));
            wire.send(layer_shell, 0, &p).unwrap();
            wire.send(layer, 0, &u32s(&[300, 200])).unwrap(); // size, centred
            wire.send(layer, 4, &u32s(&[0])).unwrap();
            wire.send(surface, 6, &[]).unwrap(); // commit
            let st = Arc::new(Mutex::new(SourceState::default()));
            let sender = wire.sender().unwrap();
            let next = Arc::new(Mutex::new(13u32));
            let source = Arc::new(Mutex::new(0u32));
            let (st2, sender2, next2, source2) =
                (st.clone(), sender.clone(), next.clone(), source.clone());
            let offers2 = offers.clone();
            std::thread::spawn(move || {
                loop {
                    let Ok(msgs) = wire.read(Duration::from_millis(20)) else {
                        return;
                    };
                    for m in msgs {
                        let r = |o| read_u32(&m.body, o).unwrap();
                        let mut s = st2.lock().unwrap();
                        if m.object == layer && m.opcode == 0 {
                            sender2.send(layer, 6, &u32s(&[r(0)])).unwrap(); // ack
                            let (w, h) = (r(4).max(1), r(8).max(1));
                            let size = (w * h * 4) as usize;
                            let fd =
                                rustix::fs::memfd_create("src", rustix::fs::MemfdFlags::CLOEXEC)
                                    .unwrap();
                            let mut f = std::fs::File::from(fd);
                            f.write_all(&vec![0xFF; size]).unwrap();
                            let mut n = next2.lock().unwrap();
                            let (pool, buf) = (*n, *n + 1);
                            *n += 2;
                            let mut p = u32s(&[pool]);
                            p.extend((size as i32).to_ne_bytes());
                            sender2.send_fd(shm, 0, &p, f.as_fd()).unwrap();
                            let mut q = u32s(&[buf]);
                            for v in [0i32, w as i32, h as i32, (w * 4) as i32] {
                                q.extend(v.to_ne_bytes());
                            }
                            q.extend(u32s(&[0]));
                            sender2.send(pool, 0, &q).unwrap();
                            let mut a = u32s(&[buf]);
                            a.extend([0u8; 8]);
                            sender2.send(surface, 1, &a).unwrap();
                            let mut d = Vec::new();
                            for v in [0i32, 0, w as i32, h as i32] {
                                d.extend(v.to_ne_bytes());
                            }
                            sender2.send(surface, 9, &d).unwrap(); // damage_buffer
                            sender2.send(surface, 6, &[]).unwrap();
                            s.configured = true;
                        } else if m.object == pointer && m.opcode == 0 {
                            s.entered = r(4) == surface;
                        } else if m.object == pointer && m.opcode == 2 {
                            s.motions += 1;
                        } else if m.object == pointer && m.opcode == 3 && r(12) == 1 {
                            s.press_serial = Some(r(0));
                        } else if m.object == pointer && m.opcode == 3 && r(12) == 0 {
                            s.released = true;
                        } else if m.object == *source2.lock().unwrap() && m.object != 0 {
                            match m.opcode {
                                0 => {
                                    let t = if r(0) == 0 {
                                        "null".to_string()
                                    } else {
                                        read_string(&m.body, 0).unwrap().0
                                    };
                                    s.targets.push(t);
                                }
                                1 => {
                                    let (mime, _) = read_string(&m.body, 0).unwrap();
                                    let fd = wire.fds.pop_front().unwrap();
                                    let data = offers2
                                        .iter()
                                        .find(|(t, _)| *t == mime)
                                        .map(|(_, d)| d.clone())
                                        .unwrap_or_default();
                                    s.sent.push(mime);
                                    std::thread::spawn(move || {
                                        let _ = std::fs::File::from(fd).write_all(&data);
                                    });
                                }
                                2 => s.cancelled = true,
                                3 => s.drop_performed = true,
                                4 => s.finished = true,
                                5 => s.action = r(0),
                                _ => {}
                            }
                        } else if m.object == device && m.opcode == 0 {
                            // a data offer for our own drag or the selection: not needed
                            let _ = sender2.send(r(0), 2, &[]);
                        }
                    }
                }
            });
            Self {
                st,
                sender,
                ids: (surface, device, ddm),
                next,
                offers,
                source,
            }
        }

        /// Starts the drag (the button is held on the surface).
        pub fn start_drag(&self, serial: u32) {
            let (surface, device, ddm) = self.ids;
            let mut n = self.next.lock().unwrap();
            let source = *n;
            *n += 1;
            *self.source.lock().unwrap() = source;
            self.sender.send(ddm, 0, &u32s(&[source])).unwrap(); // create_data_source
            for (mime, _) in &self.offers {
                self.sender.send(source, 0, &string(mime)).unwrap();
            }
            self.sender.send(source, 2, &u32s(&[1 | 2])).unwrap(); // copy | move
            self.sender
                .send(device, 0, &u32s(&[source, surface, 0, serial]))
                .unwrap(); // start_drag
        }

        pub fn reset(&self) {
            let mut s = self.st.lock().unwrap();
            let configured = s.configured;
            *s = SourceState {
                configured,
                ..Default::default()
            };
        }

        pub fn wait(&self, secs: f64, f: impl Fn(&SourceState) -> bool) -> bool {
            let end = Instant::now() + Duration::from_secs_f64(secs);
            while Instant::now() < end {
                if f(&self.st.lock().unwrap()) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            f(&self.st.lock().unwrap())
        }
    }

    #[derive(Debug, Default)]
    pub struct TargetState {
        pub configured: bool,
        /// Drags that entered it, and the MIME types of the last one.
        pub entered: u32,
        pub offered: Vec<String>,
        pub dropped: u32,
        /// What it read from the last drop (`text/uri-list`).
        pub data: Option<Vec<u8>>,
        /// Ordinary clicks that reached it.
        pub presses: u32,
        /// Behave like KDE (Dolphin): when a drop offers Ark's two types, read them and,
        /// before finishing, ask that object to copy the files into this folder.
        pub kde_dest: Option<std::path::PathBuf>,
        /// What it read for them (service, object path), and how the call went (Err: the
        /// D-Bus error's name).
        pub ark: Option<(String, String)>,
        pub ark_call: Option<Result<(), String>>,
    }

    pub const ARK_SERVICE: &str = "application/x-kde-ark-dndextract-service";
    pub const ARK_PATH: &str = "application/x-kde-ark-dndextract-path";

    /// `org.kde.ark.DndExtract.extractSelectedFilesTo(dest)` on the nested session's bus
    /// (Err: the D-Bus error's name).
    pub fn extract(service: &str, path: &str, dest: &str) -> Result<(), String> {
        let conn = zbus::blocking::Connection::session().map_err(|e| e.to_string())?;
        conn.call_method(
            Some(service),
            path,
            Some("org.kde.ark.DndExtract"),
            "extractSelectedFilesTo",
            &(dest,),
        )
        .map(|_| ())
        .map_err(|e| match e {
            zbus::Error::MethodError(name, _, _) => name.to_string(),
            e => e.to_string(),
        })
    }

    /// A file manager's folder: a `w`×`h` layer surface (top layer) anchored `anchor`, that
    /// takes `text/uri-list` drops as a copy, reads them and finishes; and counts clicks.
    pub struct DropTarget {
        pub st: Arc<Mutex<TargetState>>,
    }

    impl DropTarget {
        pub fn start(anchor: u32, size: (u32, u32)) -> Self {
            let mut wire = Wire::new(wayland::connect(Duration::from_secs(2)).unwrap());
            let g = globals(&mut wire);
            let b = |w: &Wire, iface: &str, v: u32, id: u32| {
                let (name, ver) = g[iface];
                wayland::bind(w, REG, name, iface, v.min(ver), id).unwrap();
            };
            let (comp, shm, layer_shell, seat, ddm) = (4, 5, 6, 7, 8);
            b(&wire, "wl_compositor", 4, comp);
            b(&wire, "wl_shm", 1, shm);
            b(&wire, "zwlr_layer_shell_v1", 3, layer_shell);
            b(&wire, "wl_seat", 5, seat);
            b(&wire, "wl_data_device_manager", 3, ddm);
            let (pointer, device, surface, layer) = (9, 10, 11, 12);
            wire.send(seat, 0, &u32s(&[pointer])).unwrap();
            wire.send(ddm, 1, &u32s(&[device, seat])).unwrap();
            wire.send(comp, 0, &u32s(&[surface])).unwrap();
            let mut p = u32s(&[layer, surface, 0, 2]); // top layer
            p.extend(string("glisa-test-target"));
            wire.send(layer_shell, 0, &p).unwrap();
            wire.send(layer, 0, &u32s(&[size.0, size.1])).unwrap();
            wire.send(layer, 1, &u32s(&[anchor])).unwrap();
            wire.send(layer, 4, &u32s(&[0])).unwrap();
            wire.send(surface, 6, &[]).unwrap();
            let st = Arc::new(Mutex::new(TargetState::default()));
            let sender = wire.sender().unwrap();
            let st2 = st.clone();
            std::thread::spawn(move || {
                let mut next = 13u32;
                let mut offers: HashMap<u32, Vec<String>> = HashMap::new();
                let mut current = 0u32;
                loop {
                    let Ok(msgs) = wire.read(Duration::from_millis(20)) else {
                        return;
                    };
                    for m in msgs {
                        let r = |o| read_u32(&m.body, o).unwrap();
                        let mut s = st2.lock().unwrap();
                        if m.object == layer && m.opcode == 0 {
                            sender.send(layer, 6, &u32s(&[r(0)])).unwrap();
                            let (w, h) = (r(4).max(1), r(8).max(1));
                            let size = (w * h * 4) as usize;
                            let fd =
                                rustix::fs::memfd_create("dst", rustix::fs::MemfdFlags::CLOEXEC)
                                    .unwrap();
                            let mut f = std::fs::File::from(fd);
                            f.write_all(&vec![0x80; size]).unwrap();
                            let (pool, buf) = (next, next + 1);
                            next += 2;
                            let mut p = u32s(&[pool]);
                            p.extend((size as i32).to_ne_bytes());
                            sender.send_fd(shm, 0, &p, f.as_fd()).unwrap();
                            let mut q = u32s(&[buf]);
                            for v in [0i32, w as i32, h as i32, (w * 4) as i32] {
                                q.extend(v.to_ne_bytes());
                            }
                            q.extend(u32s(&[0]));
                            sender.send(pool, 0, &q).unwrap();
                            let mut a = u32s(&[buf]);
                            a.extend([0u8; 8]);
                            sender.send(surface, 1, &a).unwrap();
                            sender.send(surface, 6, &[]).unwrap();
                            s.configured = true;
                        } else if m.object == pointer && m.opcode == 3 && r(12) == 1 {
                            s.presses += 1;
                        } else if m.object == device {
                            match m.opcode {
                                0 => {
                                    offers.insert(r(0), Vec::new());
                                }
                                1 => {
                                    // enter: take files, as a copy
                                    let (serial, offer) = (r(0), r(16));
                                    current = offer;
                                    s.entered += 1;
                                    s.offered = offers.get(&offer).cloned().unwrap_or_default();
                                    if s.offered.iter().any(|t| t == "text/uri-list") {
                                        let mut a = u32s(&[serial]);
                                        a.extend(string("text/uri-list"));
                                        sender.send(offer, 0, &a).unwrap();
                                        sender.send(offer, 4, &u32s(&[1, 1])).unwrap();
                                    }
                                }
                                4 => {
                                    // drop: read the list (and, as KDE, Ark's types), then
                                    // finish
                                    s.dropped += 1;
                                    let offer = current;
                                    let kde = s.kde_dest.clone().filter(|_| {
                                        [ARK_SERVICE, ARK_PATH]
                                            .iter()
                                            .all(|t| s.offered.iter().any(|o| o == t))
                                    });
                                    let mut types = vec!["text/uri-list"];
                                    if kde.is_some() {
                                        types.extend([ARK_SERVICE, ARK_PATH]);
                                    }
                                    let mut pipes = Vec::new();
                                    for t in types {
                                        let (rd, wr) = rustix::pipe::pipe().unwrap();
                                        sender.send_fd(offer, 1, &string(t), wr.as_fd()).unwrap();
                                        drop(wr);
                                        pipes.push(rd);
                                    }
                                    let (st3, sender3) = (st2.clone(), sender.clone());
                                    std::thread::spawn(move || {
                                        let read: Vec<Vec<u8>> = pipes
                                            .into_iter()
                                            .map(|rd| {
                                                let mut data = Vec::new();
                                                let _ = std::io::Read::read_to_end(
                                                    &mut std::fs::File::from(rd),
                                                    &mut data,
                                                );
                                                data
                                            })
                                            .collect();
                                        let mut called = None;
                                        if let Some(dest) = kde {
                                            let t = |i: usize| {
                                                String::from_utf8_lossy(&read[i]).into_owned()
                                            };
                                            let (service, path) = (t(1), t(2));
                                            // Dolphin calls during the drop, and waits
                                            called = Some(extract(
                                                &service,
                                                &path,
                                                &dest.to_string_lossy(),
                                            ));
                                            st3.lock().unwrap().ark = Some((service, path));
                                        }
                                        let ok = called.as_ref().is_some_and(|c| c.is_ok());
                                        if !read[0].is_empty() || ok {
                                            let _ = sender3.send(offer, 3, &[]); // finish
                                        }
                                        let _ = sender3.send(offer, 2, &[]);
                                        let mut s = st3.lock().unwrap();
                                        s.ark_call = called;
                                        s.data = Some(read[0].clone());
                                    });
                                }
                                _ => {}
                            }
                        } else if let Some(o) = offers.get_mut(&m.object) {
                            if m.opcode == 0 {
                                o.push(read_string(&m.body, 0).unwrap().0);
                            }
                        }
                    }
                }
            });
            Self { st }
        }

        pub fn wait(&self, secs: f64, f: impl Fn(&TargetState) -> bool) -> bool {
            let end = Instant::now() + Duration::from_secs_f64(secs);
            while Instant::now() < end {
                if f(&self.st.lock().unwrap()) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            f(&self.st.lock().unwrap())
        }
    }
}

#[derive(Default)]
struct DragRec {
    events: Mutex<Vec<String>>,
    drags: Mutex<Vec<(&'static str, Vec<std::path::PathBuf>)>>,
}

impl Sink for DragRec {
    fn edge(&self, side: &'static str, x: f64, y: f64, _r: Rect) {
        self.events
            .lock()
            .unwrap()
            .push(format!("edge {side} {x} {y}"));
    }
    fn leave(&self, side: &'static str) {
        self.events.lock().unwrap().push(format!("leave {side}"));
    }
    fn ctrl(&self, _held: bool) {}
    fn screens(&self, _r: Vec<Rect>) {}
    fn state(&self, state: &str, _m: Option<Text>) {
        self.events.lock().unwrap().push(format!("state {state}"));
    }
    fn file_drag(&self, side: &'static str, paths: Vec<std::path::PathBuf>) {
        self.drags.lock().unwrap().push((side, paths));
    }
}

#[test]
#[ignore = "needs the nested session of tests/live_desktop.sh"]
fn catcher_takes_a_file_drag_in_a_nested_session() {
    use dnd::{DragSource, FakeInput};
    assert!(nested(), "run through tests/live_desktop.sh");
    let inner = Arc::new(KWinBackend::new());
    inner.create_pointer.store(false, Ordering::SeqCst);
    let b = KWinHandle(inner);
    let sink = Arc::new(DragRec::default());
    b.start(sink.clone());
    assert!(wait(10.0, || b.state().0 == "ok"));
    assert!(
        b.drag_catcher_available(),
        "layer-shell catcher can't start"
    );
    let rect = b.screens()[0];
    let (w, h) = (rect[2] as f64, rect[3] as f64);
    let Some(mut input) = FakeInput::connect() else {
        panic!("no org_kde_kwin_fake_input (KWIN_WAYLAND_NO_PERMISSION_CHECKS=1 missing?)");
    };
    let uris = b"file:///tmp/glisa%20arrastre.txt\r\nhttps://example.com/x\r\n".to_vec();
    let src = DragSource::start(vec![("text/uri-list", uris), ("text/plain", b"x".to_vec())]);
    assert!(src.wait(5.0, |s| s.configured));
    // press on the source and drag towards the right edge
    input.move_to(w / 2.0, h / 2.0);
    input.nudge(1.0, 0.0);
    assert!(src.wait(5.0, |s| s.entered), "{:?}", src.st.lock().unwrap());
    input.button(true);
    assert!(src.wait(5.0, |s| s.press_serial.is_some()));
    let serial = src.st.lock().unwrap().press_serial.unwrap();
    src.start_drag(serial);
    std::thread::sleep(Duration::from_millis(200));
    for x in [w * 0.6, w * 0.8, w * 0.95, w - 1.0] {
        input.move_to(x, h / 2.0);
        std::thread::sleep(Duration::from_millis(40));
    }
    // the KWin script still reports the edge while a drag is going on
    let edged = wait(3.0, || {
        sink.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.starts_with("edge right"))
    });
    eprintln!(
        "live: edge event during a drag: {edged} ({:?})",
        sink.events.lock().unwrap()
    );
    // the engine would now ask for the catcher
    b.drag_catcher(Some(("right", rect)));
    let catcher = b.0.catcher().unwrap();
    assert!(wait(3.0, || catcher.mapped()), "{:?}", catcher.counters());
    // pushing on: the drag enters the catcher, which reads the files
    for _ in 0..10 {
        input.nudge(2.0, 0.0);
        if !sink.drags.lock().unwrap().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        wait(3.0, || !sink.drags.lock().unwrap().is_empty()),
        "{:?} {:?}",
        catcher.counters(),
        src.st.lock().unwrap()
    );
    assert_eq!(
        sink.drags.lock().unwrap()[0],
        (
            "right",
            vec![std::path::PathBuf::from("/tmp/glisa arrastre.txt")]
        )
    );
    eprintln!("live: the catcher mapped on the right edge and read the dragged file");
    // the crossing goes ahead: accept as a copy; the button is let go → the drop lands
    assert!(b.drag_accept(true));
    std::thread::sleep(Duration::from_millis(100));
    b.drag_catcher(None); // asked to go: it stays until the drop
    input.button(false);
    assert!(
        src.wait(5.0, |s| s.finished),
        "{:?} {:?}",
        src.st.lock().unwrap(),
        catcher.counters()
    );
    assert_eq!(src.st.lock().unwrap().action, 1, "the drop is a copy");
    assert!(wait(3.0, || !catcher.mapped()));
    eprintln!("live: the drop finished as a copy and the catcher went away");

    // a drag of text only (not files): refused, nothing reported
    src.reset();
    let src2 = DragSource::start(vec![("text/plain", b"hola".to_vec())]);
    assert!(src2.wait(5.0, |s| s.configured));
    input.move_to(w / 2.0, h / 2.0);
    input.nudge(1.0, 0.0);
    assert!(src2.wait(5.0, |s| s.entered));
    input.button(true);
    assert!(src2.wait(5.0, |s| s.press_serial.is_some()));
    src2.start_drag(src2.st.lock().unwrap().press_serial.unwrap());
    std::thread::sleep(Duration::from_millis(200));
    input.move_to(w - 1.0, h / 2.0);
    b.drag_catcher(Some(("right", rect)));
    assert!(wait(3.0, || catcher.mapped()));
    for _ in 0..6 {
        input.nudge(2.0, 0.0);
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!b.drag_accept(true)); // not a file drag
    input.button(false);
    assert!(
        src2.wait(5.0, |s| s.cancelled),
        "{:?}",
        src2.st.lock().unwrap()
    );
    assert_eq!(sink.drags.lock().unwrap().len(), 1);
    b.drag_catcher(None);
    assert!(wait(3.0, || !catcher.mapped()));
    eprintln!("live: a text drag is refused at the catcher");
    b.stop();
}

// ======================================== the glow while files are dragged anywhere
/// What the drag watch would hear from the Logitech mouse's evdev node: in the nested session
/// the pointer is KWin's fake input, so the test tells the watch what the mouse did.
struct ChanSource(
    std::sync::mpsc::Receiver<PointerEvent>,
    Arc<Mutex<Vec<bool>>>,
);

impl PointerSource for ChanSource {
    fn next(&mut self, timeout: Duration) -> std::io::Result<Vec<PointerEvent>> {
        match self.0.recv_timeout(timeout.min(Duration::from_millis(50))) {
            Ok(e) => Ok(vec![e]),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(Vec::new()),
            Err(_) => Err(std::io::Error::other("closed")),
        }
    }
    fn want_motion(&mut self, on: bool) {
        self.1.lock().unwrap().push(on);
    }
}

#[derive(Default)]
struct WatchRec {
    dragging: Mutex<Vec<bool>>,
    drags: Mutex<Vec<(&'static str, Vec<std::path::PathBuf>)>>,
    pickups: Mutex<Vec<PickupNews>>,
}

impl Sink for WatchRec {
    fn edge(&self, _side: &'static str, _x: f64, _y: f64, _r: Rect) {}
    fn leave(&self, _side: &'static str) {}
    fn ctrl(&self, _held: bool) {}
    fn screens(&self, _r: Vec<Rect>) {}
    fn state(&self, _state: &str, _m: Option<Text>) {}
    fn file_drag(&self, side: &'static str, paths: Vec<std::path::PathBuf>) {
        self.drags.lock().unwrap().push((side, paths));
    }
    fn drag_glow_wanted(&self) -> bool {
        true // a peer is on the right, online (the engine's part is tested in tests/drag.rs)
    }
    fn files_dragging(&self, on: bool) {
        self.dragging.lock().unwrap().push(on);
    }
    fn pickup(&self, news: PickupNews) {
        self.pickups.lock().unwrap().push(news);
    }
}

/// Moves the pointer `steps` times by (dx, 0), telling the watch about each move, until
/// `done` (checked between moves).
fn drag_along(
    input: &mut dnd::FakeInput,
    tx: &std::sync::mpsc::Sender<PointerEvent>,
    steps: usize,
    dx: f64,
    done: &dyn Fn() -> bool,
) -> bool {
    for _ in 0..steps {
        input.nudge(dx, 0.0);
        let _ = tx.send(PointerEvent::Motion(dx as i32, 0));
        std::thread::sleep(Duration::from_millis(15));
        if done() {
            return true;
        }
    }
    done()
}

#[test]
#[ignore = "needs the nested session of tests/live_desktop.sh"]
fn drag_glow_in_a_nested_session() {
    use dnd::{DragSource, DropTarget, FakeInput};
    use glisa_flow::geometry::GlowEdge;
    assert!(nested(), "run through tests/live_desktop.sh");
    let inner = Arc::new(KWinBackend::new());
    inner.create_pointer.store(false, Ordering::SeqCst);
    let (tx, rx) = std::sync::mpsc::channel();
    let motion_asks: Arc<Mutex<Vec<bool>>> = Arc::default();
    *inner.pointer_source.lock().unwrap() = Some(Box::new(ChanSource(rx, motion_asks.clone())));
    let b = KWinHandle(inner);
    let sink = Arc::new(WatchRec::default());
    b.start(sink.clone());
    assert!(wait(10.0, || b.state().0 == "ok"));
    assert!(b.drag_catcher_available());
    b.drag_watch(true);
    let catcher = b.0.catcher().unwrap();
    let rect = b.screens()[0];
    let (w, h) = (rect[2] as f64, rect[3] as f64);
    let glow = [GlowEdge {
        side: "right",
        output: rect,
        start: 0,
        len: rect[3],
    }];
    let mut input = FakeInput::connect().expect("fake input");
    let uris = b"file:///tmp/glisa%20arrastre.txt\r\n".to_vec();
    let src = DragSource::start(vec![
        ("text/uri-list", uris.clone()),
        ("text/plain", b"x".to_vec()),
    ]);
    let dst = DropTarget::start(8, (300, 200)); // a folder on the right
    assert!(src.wait(5.0, |s| s.configured) && dst.wait(5.0, |s| s.configured));
    let dragging = || sink.dragging.lock().unwrap().clone();
    let press_on_source = |input: &mut FakeInput, src: &DragSource| {
        input.move_to(10.0, 10.0); // off the source, so it sees the pointer come in again
        std::thread::sleep(Duration::from_millis(50));
        src.reset();
        input.move_to(w / 2.0, h / 2.0);
        input.nudge(1.0, 0.0);
        assert!(src.wait(5.0, |s| s.entered));
        input.button(true);
        tx.send(PointerEvent::Left(true)).unwrap();
        assert!(src.wait(5.0, |s| s.press_serial.is_some()));
    };

    // (c) a press and a move that isn't a drag (a selection): the probe looks twice, finds
    // nothing, and the press keeps its grab
    press_on_source(&mut input, &src);
    let before = src.st.lock().unwrap().motions;
    drag_along(&mut input, &tx, 40, 3.0, &|| false);
    assert!(
        wait(3.0, || catcher.counters().probe_missed == 2),
        "{:?}",
        catcher.counters()
    );
    let n = catcher.counters();
    assert_eq!((n.probes, n.probe_files, n.probe_other), (2, 0, 0));
    assert!(n.probe_mapped >= 2, "{n:?}");
    let motions = src.st.lock().unwrap().motions - before;
    input.button(false);
    tx.send(PointerEvent::Left(false)).unwrap();
    assert!(src.wait(3.0, |s| s.released), "the press lost its grab");
    assert!(
        motions >= 30,
        "the selection stopped getting motion: {motions}"
    );
    assert!(dragging().is_empty());
    eprintln!(
        "live: a plain press-and-move: {} probes, nothing found, the source kept its grab ({motions} motions, the release)",
        n.probes
    );

    // (a) a file drag starts: found by the first probe, the edge glows; (b) the drop lands
    // where it is let go
    press_on_source(&mut input, &src);
    src.start_drag(src.st.lock().unwrap().press_serial.unwrap());
    std::thread::sleep(Duration::from_millis(100));
    let t0 = std::time::Instant::now();
    assert!(
        drag_along(&mut input, &tx, 60, 2.0, &|| !dragging().is_empty()),
        "{:?}",
        catcher.counters()
    );
    let found_in = t0.elapsed();
    assert_eq!(dragging(), [true]);
    assert_eq!(catcher.counters().probe_files, 1);
    b.drag_glow(&glow); // what the engine does
    assert!(wait(3.0, || catcher.glowing() == 1));
    for x in [w * 0.7, w * 0.85, w - 60.0] {
        input.move_to(x, h / 2.0);
        std::thread::sleep(Duration::from_millis(40));
    }
    input.nudge(1.0, 0.0);
    assert!(
        dst.wait(3.0, |s| s.entered > 0
            && s.offered.contains(&"text/uri-list".to_string())),
        "the target under the glow didn't get the drag"
    );
    input.button(false);
    tx.send(PointerEvent::Left(false)).unwrap();
    assert!(
        dst.wait(5.0, |s| s.data.is_some()),
        "{:?}",
        src.st.lock().unwrap()
    );
    assert_eq!(dst.st.lock().unwrap().data.as_deref(), Some(&uris[..]));
    assert!(src.wait(5.0, |s| s.finished));
    assert_eq!(src.st.lock().unwrap().action, 1);
    assert!(wait(3.0, || dragging() == [true, false]));
    b.drag_glow(&[]);
    assert!(wait(3.0, || catcher.glowing() == 0));
    let targets = src.st.lock().unwrap().targets.clone();
    eprintln!(
        "live: file drag found by the probe after {} ms of motion; the right edge glowed; the drop landed on the folder under the glow and finished as a copy; the source saw targets {targets:?}",
        found_in.as_millis()
    );

    // (b) the probe right while the drag is over a folder: the folder loses it for a moment
    // and gets it back, and the drop lands there
    let entered = dst.st.lock().unwrap().entered;
    press_on_source(&mut input, &src);
    src.start_drag(src.st.lock().unwrap().press_serial.unwrap());
    std::thread::sleep(Duration::from_millis(100));
    for x in [w * 0.7, w * 0.85, w - 60.0] {
        input.move_to(x, h / 2.0);
        std::thread::sleep(Duration::from_millis(40));
    }
    input.nudge(1.0, 0.0);
    assert!(
        dst.wait(3.0, |s| s.entered > entered),
        "{:?} {:?} {:?}",
        dst.st.lock().unwrap(),
        src.st.lock().unwrap(),
        catcher.counters()
    );
    let mapped = catcher.counters().probe_mapped;
    let t0 = std::time::Instant::now();
    let targets_before = src.st.lock().unwrap().targets.len();
    tx.send(PointerEvent::Motion(20, 0)).unwrap(); // the watch's 12 units, all at once
    assert!(drag_along(&mut input, &tx, 60, 0.5, &|| dragging().len() == 3));
    let found_in = t0.elapsed();
    // the user keeps moving: the drag goes back to the folder at the next motion
    let t1 = std::time::Instant::now();
    let mut back = false;
    for _ in 0..20 {
        input.nudge(0.5, 0.0);
        if dst.st.lock().unwrap().entered >= entered + 2 {
            back = true;
            break;
        }
    }
    let flicker: Vec<String> = src.st.lock().unwrap().targets[targets_before..].to_vec();
    eprintln!(
        "live: a probe over the folder: files found in {} ms; the folder had the drag back {} ms later, at the next motion: {back}; the source saw targets {flicker:?} meanwhile",
        found_in.as_millis(),
        t1.elapsed().as_millis()
    );
    assert!(catcher.counters().probe_mapped > mapped);
    assert!(back);
    let dropped = {
        let mut d = dst.st.lock().unwrap();
        d.data = None;
        d.dropped
    };
    input.button(false);
    tx.send(PointerEvent::Left(false)).unwrap();
    assert!(dst.wait(5.0, |s| s.dropped > dropped && s.data.is_some()));
    assert_eq!(dst.st.lock().unwrap().data.as_deref(), Some(&uris[..]));
    assert!(
        src.wait(5.0, |s| s.finished),
        "{:?}",
        src.st.lock().unwrap()
    );
    assert_eq!(catcher.counters().probe_drops, 0);
    let targets = src.st.lock().unwrap().targets.clone();
    eprintln!("live: the drop still landed on the folder; the source saw targets {targets:?}");
    assert!(wait(3.0, || dragging() == [true, false, true, false]));

    // (d) with the glow up, the edge works as before: the catcher takes the files there
    press_on_source(&mut input, &src);
    src.start_drag(src.st.lock().unwrap().press_serial.unwrap());
    std::thread::sleep(Duration::from_millis(100));
    assert!(drag_along(&mut input, &tx, 60, 2.0, &|| dragging().len() == 5));
    b.drag_glow(&glow);
    assert!(wait(3.0, || catcher.glowing() == 1));
    input.move_to(w - 1.0, h / 4.0); // above the folder
    b.drag_catcher(Some(("right", rect)));
    assert!(wait(3.0, || catcher.mapped()));
    for _ in 0..10 {
        input.nudge(2.0, 0.0);
        if !sink.drags.lock().unwrap().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        wait(3.0, || !sink.drags.lock().unwrap().is_empty()),
        "{:?}",
        catcher.counters()
    );
    assert!(b.drag_accept(true));
    b.drag_catcher(None);
    input.button(false);
    tx.send(PointerEvent::Left(false)).unwrap();
    assert!(src.wait(5.0, |s| s.finished));
    assert!(catcher.counters().lit >= 1);
    b.drag_glow(&[]);
    assert!(wait(3.0, || catcher.glowing() == 0 && !catcher.mapped()));
    eprintln!("live: with the glow up, the edge catcher still took the files (lit, copy)");
    eprintln!(
        "live: the watch asked for motion {:?}",
        motion_asks.lock().unwrap()
    );
    b.drag_watch(false);
    b.stop();
}

// ================================= files dragged here: the drag goes on (pickup)
#[test]
#[ignore = "needs the nested session of tests/live_desktop.sh"]
fn pickup_in_a_nested_session() {
    use dnd::{ARK_PATH, ARK_SERVICE, DropTarget, FakeInput, extract};
    assert!(nested(), "run through tests/live_desktop.sh");
    let inner = Arc::new(KWinBackend::new());
    inner.create_pointer.store(false, Ordering::SeqCst);
    // no Logitech mouse in the nested session: this flag stands in for its evdev check
    // (false: a touchpad's press; true: our mouse's button is down)
    let ours = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let o2 = ours.clone();
    *inner.pickup_confirm.lock().unwrap() = Some(Arc::new(move || o2.load(Ordering::SeqCst)));
    let b = KWinHandle(inner);
    let sink = Arc::new(WatchRec::default());
    b.start(sink.clone());
    assert!(wait(10.0, || b.state().0 == "ok"));
    assert!(b.drag_pickup_available(), "no pickup here");
    let pickup = b.0.pickup().unwrap();
    let rect = b.screens()[0];
    let (w, h) = (rect[2] as f64, rect[3] as f64);
    let mut input = FakeInput::connect().expect("fake input");
    let dst = DropTarget::start(8, (300, 200));
    assert!(dst.wait(5.0, |s| s.configured));
    // the received items in a staging folder, and a folder of the user's to drop them on
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().to_string_lossy().into_owned();
    assert!(
        base.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
    );
    let folder = tmp.path().join("glisa staged");
    std::fs::create_dir_all(folder.join("Proyecto")).unwrap();
    std::fs::write(folder.join("Proyecto/notas.txt"), b"notas").unwrap();
    std::fs::write(folder.join("foto 1.jpg"), b"foto").unwrap();
    let staged = vec![folder.join("Proyecto"), folder.join("foto 1.jpg")];
    let want = format!(
        "file://{base}/glisa%20staged/Proyecto\r\nfile://{base}/glisa%20staged/foto%201.jpg\r\n"
    )
    .into_bytes();
    let desk = tmp.path().join("Escritorio");
    std::fs::create_dir(&desk).unwrap();
    std::fs::write(desk.join("foto 1.jpg"), b"mine").unwrap();
    let read = |p: std::path::PathBuf| std::fs::read(p).unwrap_or_default();
    let news = || sink.pickups.lock().unwrap().clone();
    let click_folder = |input: &mut FakeInput| {
        let n = dst.st.lock().unwrap().presses;
        input.move_to(w - 60.0, h / 2.0);
        input.nudge(1.0, 0.0);
        input.button(true);
        input.button(false);
        dst.wait(2.0, |s| s.presses > n)
    };
    assert!(click_folder(&mut input)); // the folder takes clicks

    // the switch after a drag notice: the pickup waits over the output
    b.drag_pickup(PickupStep::Expect {
        output: rect,
        count: 2,
    });
    assert!(wait(3.0, || pickup.pickup_waiting()));
    assert!(!click_folder(&mut input), "the pickup lets a click through");
    // a press before the mouse arrived, from a touchpad on this computer: nothing starts
    input.move_to(w / 2.0, h / 2.0);
    input.button(true);
    std::thread::sleep(Duration::from_millis(900));
    input.button(false);
    std::thread::sleep(Duration::from_millis(300));
    assert!(news().is_empty() && !pickup.pickup_dragging());
    // our mouse's press, before its arrival is even noticed (as behind a receiver), picks
    // the drag up: the mouse itself says its button is down
    ours.store(true, Ordering::SeqCst);
    b.drag_pickup(PickupStep::Ready(staged.clone()));
    input.move_to(w / 2.0 + 3.0, h / 2.0);
    input.button(true);
    assert!(
        wait(3.0, || news() == [PickupNews::Started]),
        "{:?}",
        pickup.counters()
    );
    for x in [w * 0.7, w * 0.85, w - 60.0] {
        input.move_to(x, h / 2.0);
        std::thread::sleep(Duration::from_millis(40));
    }
    input.nudge(1.0, 0.0);
    // the URI list for any drop target, and Ark's two types for KDE's
    assert!(
        dst.wait(3.0, |s| s.offered
            == ["text/uri-list", ARK_SERVICE, ARK_PATH]),
        "{:?} {:?}",
        dst.st.lock().unwrap(),
        pickup.counters()
    );
    // the folder takes it as KDE does: reads them all and, during the drop, asks the
    // drag's object to copy the files into it
    dst.st.lock().unwrap().kde_dest = Some(desk.clone());
    input.button(false);
    assert!(dst.wait(5.0, |s| s.data.is_some() && s.ark_call.is_some()));
    let (service, path) = {
        let s = dst.st.lock().unwrap();
        assert_eq!(s.data.as_deref(), Some(&want[..]));
        assert_eq!(s.ark_call, Some(Ok(())), "{s:?}");
        s.ark.clone().unwrap()
    };
    assert!(service.starts_with(':'), "{service}");
    let token = path.strip_prefix("/Flow/Drop/").unwrap();
    assert!(token.len() == 32 && token.bytes().all(|b| b.is_ascii_hexdigit()));
    // copied (nothing overwritten), and only once
    assert_eq!(read(desk.join("Proyecto/notas.txt")), b"notas");
    assert_eq!(read(desk.join("foto 1 (2).jpg")), b"foto");
    assert_eq!(read(desk.join("foto 1.jpg")), b"mine");
    assert!(staged.iter().all(|p| p.exists()));
    let again = extract(&service, &path, &desk.to_string_lossy());
    assert_eq!(
        again,
        Err("org.freedesktop.DBus.Error.UnknownObject".into())
    );
    assert!(wait(3.0, || news().len() == 2));
    assert_eq!(news()[1], PickupNews::Ended { taken: true });
    assert!(!pickup.pickup_waiting() && !pickup.pickup_dragging());
    assert!(click_folder(&mut input), "something of the pickup stayed");
    eprintln!(
        "live: pickup: the arriving press started a drag offering {:?}; the folder read the list and, as KDE, Ark's {service} {path}; its call copied the files there (foto 1 (2).jpg beside the one there), a second call found nothing; nothing stayed",
        dst.st.lock().unwrap().offered
    );

    // no press: the engine's timeout takes it away (the files go to the clipboard)
    b.drag_pickup(PickupStep::Expect {
        output: rect,
        count: 2,
    });
    assert!(wait(3.0, || pickup.pickup_waiting()));
    b.drag_pickup(PickupStep::Arrived);
    std::thread::sleep(Duration::from_millis(500));
    b.drag_pickup(PickupStep::Cancel);
    assert!(wait(3.0, || !pickup.pickup_waiting()));
    assert!(click_folder(&mut input));
    assert_eq!(news().len(), 2);
    eprintln!("live: pickup without a press: removed, clicks reach the folder again");

    // dropped before every file is here: nothing handed over after a second, not taken
    // (a drop target that isn't KDE's)
    dst.st.lock().unwrap().kde_dest = None;
    b.drag_pickup(PickupStep::Expect {
        output: rect,
        count: 1,
    });
    assert!(wait(3.0, || pickup.pickup_waiting()));
    b.drag_pickup(PickupStep::Arrived);
    input.move_to(w / 2.0, h / 2.0);
    input.nudge(1.0, 0.0);
    input.button(true);
    assert!(wait(3.0, || news().len() == 3));
    for x in [w * 0.7, w * 0.85, w - 60.0] {
        input.move_to(x, h / 2.0);
        std::thread::sleep(Duration::from_millis(40));
    }
    input.nudge(1.0, 0.0);
    let n = dst.st.lock().unwrap().entered;
    assert!(dst.wait(3.0, |s| s.entered >= n));
    dst.st.lock().unwrap().data = None;
    input.button(false);
    assert!(dst.wait(5.0, |s| s.data.is_some()));
    assert_eq!(dst.st.lock().unwrap().data.as_deref(), Some(&b""[..]));
    assert!(wait(5.0, || news().len() == 4));
    assert_eq!(news()[3], PickupNews::Ended { taken: false });
    assert!(click_folder(&mut input));
    eprintln!(
        "live: pickup dropped before the files were here: an empty list, not taken (Ctrl+V then)"
    );

    // the same on KDE's folder: the call waits for the files, and copies them once here
    let desk2 = tmp.path().join("Documentos");
    std::fs::create_dir(&desk2).unwrap();
    dst.st.lock().unwrap().kde_dest = Some(desk2.clone());
    b.drag_pickup(PickupStep::Expect {
        output: rect,
        count: 2,
    });
    assert!(wait(3.0, || pickup.pickup_waiting()));
    b.drag_pickup(PickupStep::Arrived);
    input.move_to(w / 2.0, h / 2.0);
    input.nudge(1.0, 0.0);
    input.button(true);
    assert!(wait(3.0, || news().len() == 5));
    for x in [w * 0.7, w * 0.85, w - 60.0] {
        input.move_to(x, h / 2.0);
        std::thread::sleep(Duration::from_millis(40));
    }
    input.nudge(1.0, 0.0);
    assert!(dst.wait(3.0, |s| s.offered.len() == 3));
    {
        let mut s = dst.st.lock().unwrap();
        (s.data, s.ark_call) = (None, None);
    }
    input.button(false);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(read(desk2.join("foto 1.jpg")).is_empty());
    b.drag_pickup(PickupStep::Ready(staged.clone()));
    assert!(dst.wait(5.0, |s| s.ark_call.is_some()));
    assert_eq!(dst.st.lock().unwrap().ark_call, Some(Ok(())));
    assert_eq!(dst.st.lock().unwrap().data.as_deref(), Some(&b""[..]));
    assert!(wait(3.0, || read(desk2.join("foto 1.jpg")) == b"foto"));
    assert_eq!(read(desk2.join("Proyecto/notas.txt")), b"notas");
    assert!(wait(5.0, || news().len() == 6));
    assert_eq!(news()[5], PickupNews::Ended { taken: true });
    assert!(click_folder(&mut input));
    eprintln!(
        "live: pickup dropped on KDE's folder before the files were here: copied there once they arrived, taken"
    );
    b.stop();
}
