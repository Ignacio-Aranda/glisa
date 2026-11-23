//! Is Ctrl held? On KDE Wayland a normal client only sees the keyboard while it has focus,
//! but KWin publishes the modifier state through the `org_kde_kwin_keystate` protocol
//! (version 5 adds alt/control/shift/meta with state "pressed"). This is a tiny read-only
//! Wayland client for it.

use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use super::wayland::{self, DISPLAY, Wire};
use crate::util::lock;

/// org_kde_kwin_keystate.key.control (since 5).
pub const KEY_CONTROL: u32 = 4;
/// unlocked = 0, latched = 1, locked = 2, pressed = 3.
pub const STATE_UNLOCKED: u32 = 0;
const REGISTRY: u32 = 2;
const SYNC: u32 = 3;
const KEYSTATE: u32 = 4;

pub type OnChange = Arc<dyn Fn(bool) + Send + Sync>;

/// `ctrl_held()` is None until known (or when unavailable).
pub struct KeyState {
    held: Arc<Mutex<Option<bool>>>,
    available: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    sender: Mutex<Option<Arc<wayland::Sender>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    pub error: Mutex<String>,
}

impl Default for KeyState {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyState {
    pub fn new() -> Self {
        Self {
            held: Arc::default(),
            available: Arc::default(),
            stop: Arc::default(),
            sender: Mutex::new(None),
            thread: Mutex::new(None),
            error: Mutex::new(String::new()),
        }
    }

    pub fn ctrl_held(&self) -> Option<bool> {
        *lock(&self.held)
    }

    pub fn available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }

    /// Connect and bind (to `sock`, or the session's compositor); false (with `error`)
    /// when not possible.
    pub fn start(&self, sock: Option<UnixStream>, on_change: OnChange) -> bool {
        let result = (|| -> std::io::Result<Option<Wire>> {
            let sock = match sock {
                Some(s) => s,
                None => wayland::connect(Duration::from_secs(2))?,
            };
            let mut wire = Wire::new(sock);
            let globals = wayland::globals(&mut wire, REGISTRY, SYNC, Duration::from_secs(10))?;
            match globals.get("org_kde_kwin_keystate") {
                Some((name, version)) if *version >= 5 => {
                    wayland::bind(&wire, REGISTRY, *name, "org_kde_kwin_keystate", 5, KEYSTATE)?;
                    wire.send(KEYSTATE, 0, &[])?; // fetchStates
                    Ok(Some(wire))
                }
                _ => Ok(None),
            }
        })();
        let mut wire = match result {
            Ok(Some(w)) => w,
            Ok(None) => {
                *lock(&self.error) = "the compositor does not report the Ctrl state".into();
                return false;
            }
            Err(e) => {
                *lock(&self.error) = e.to_string();
                return false;
            }
        };
        *lock(&self.sender) = wire.sender().ok();
        self.available.store(true, Ordering::SeqCst);
        self.stop.store(false, Ordering::SeqCst);
        let (held, available, stop) =
            (self.held.clone(), self.available.clone(), self.stop.clone());
        let t = std::thread::Builder::new()
            .name("flow-keystate".into())
            .spawn(move || {
                let result = (|| -> std::io::Result<()> {
                    while !stop.load(Ordering::SeqCst) {
                        for m in wire.read(Duration::from_millis(250))? {
                            if m.object == KEYSTATE && m.opcode == 0 && m.body.len() >= 8 {
                                let key = wayland::read_u32(&m.body, 0)?;
                                let state = wayland::read_u32(&m.body, 4)?;
                                if key == KEY_CONTROL {
                                    let now = state != STATE_UNLOCKED;
                                    let changed = lock(&held).replace(now) != Some(now);
                                    if changed {
                                        on_change(now);
                                    }
                                }
                            } else if m.object == DISPLAY && m.opcode == 0 {
                                return Err(wayland::bad("Wayland protocol error"));
                            }
                        }
                    }
                    Ok(())
                })();
                if let Err(e) = result {
                    if !stop.load(Ordering::SeqCst) {
                        log::info!("Ctrl state not available: {e}");
                    }
                }
                available.store(false, Ordering::SeqCst);
                *lock(&held) = None;
            });
        *lock(&self.thread) = t.ok();
        true
    }

    pub fn close(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(s) = lock(&self.sender).take() {
            s.shutdown();
        }
        if let Some(t) = lock(&self.thread).take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Read, Write};

    pub fn wstr(t: &str) -> Vec<u8> {
        wayland::string(t)
    }

    pub fn msg(obj: u32, op: u16, payload: &[u8]) -> Vec<u8> {
        let mut d = obj.to_ne_bytes().to_vec();
        d.extend_from_slice(&((((8 + payload.len()) as u32) << 16) | op as u32).to_ne_bytes());
        d.extend_from_slice(payload);
        d
    }

    /// Plays KWin on the other end of a socketpair for the keystate client.
    fn fake_compositor(mut sock: UnixStream, version: u32) -> Arc<AtomicBool> {
        let bound = Arc::new(AtomicBool::new(false));
        let b2 = bound.clone();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let Ok(n) = sock.read(&mut chunk) else { return };
                if n == 0 {
                    return;
                }
                buf.extend_from_slice(&chunk[..n]);
                while buf.len() >= 8 {
                    let obj = u32::from_ne_bytes(buf[0..4].try_into().unwrap());
                    let so = u32::from_ne_bytes(buf[4..8].try_into().unwrap());
                    let size = (so >> 16) as usize;
                    if buf.len() < size {
                        break;
                    }
                    let body: Vec<u8> = buf[8..size].to_vec();
                    buf.drain(..size);
                    let op = so & 0xFFFF;
                    if obj == 1 && op == 0 {
                        let new = u32::from_ne_bytes(body[0..4].try_into().unwrap());
                        let mut out = msg(
                            2,
                            0,
                            &[
                                &7u32.to_ne_bytes()[..],
                                &wstr("wl_seat"),
                                &9u32.to_ne_bytes(),
                            ]
                            .concat(),
                        );
                        out.extend(msg(
                            2,
                            0,
                            &[
                                &62u32.to_ne_bytes()[..],
                                &wstr("org_kde_kwin_keystate"),
                                &version.to_ne_bytes(),
                            ]
                            .concat(),
                        ));
                        out.extend(msg(new, 0, &1u32.to_ne_bytes()));
                        let _ = sock.write_all(&out);
                    } else if obj == 4 && op == 0 {
                        b2.store(true, Ordering::SeqCst);
                        let mut out = msg(4, 0, &[1u32, 2].map(u32::to_ne_bytes).concat()); // numlock locked
                        out.extend(msg(4, 0, &[4u32, 0].map(u32::to_ne_bytes).concat())); // control unlocked
                        let _ = sock.write_all(&out);
                    }
                }
            }
        });
        bound
    }

    fn wait(mut f: impl FnMut() -> bool) -> bool {
        for _ in 0..100 {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        f()
    }

    #[test]
    fn ctrl_from_keystate() {
        let (a, b) = UnixStream::pair().unwrap();
        let mut writer = b.try_clone().unwrap();
        let bound = fake_compositor(b, 5);
        let changes = Arc::new(Mutex::new(Vec::new()));
        let c2 = changes.clone();
        let ks = KeyState::new();
        assert!(ks.start(Some(a), Arc::new(move |h| c2.lock().unwrap().push(h))));
        assert!(wait(|| bound.load(Ordering::SeqCst)));
        assert!(wait(|| ks.ctrl_held() == Some(false)));
        writer
            .write_all(&msg(4, 0, &[4u32, 3].map(u32::to_ne_bytes).concat()))
            .unwrap(); // control pressed
        assert!(wait(|| ks.ctrl_held() == Some(true)));
        writer
            .write_all(&msg(4, 0, &[4u32, 0].map(u32::to_ne_bytes).concat()))
            .unwrap();
        assert!(wait(|| ks.ctrl_held() == Some(false)));
        assert_eq!(*changes.lock().unwrap(), vec![false, true, false]);
        ks.close();
        assert!(!ks.available());
    }

    #[test]
    fn old_compositor() {
        let (a, b) = UnixStream::pair().unwrap();
        fake_compositor(b, 4);
        let ks = KeyState::new();
        assert!(!ks.start(Some(a), Arc::new(|_| {})));
        assert!(ks.error.lock().unwrap().contains("Ctrl"));
    }
}
