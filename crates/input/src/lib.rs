//! Virtual keyboard and mouse used to run button actions: key combos, mouse buttons and
//! (high-resolution) scrolling.
//!
//! | OS | Backend |
//! |---|---|
//! | Linux | a uinput device (`/dev/uinput`, ioctls; no python-evdev, no libevdev) |
//! | Windows | `SendInput` |
//! | macOS | `CGEventPost` (needs the Accessibility permission) |
//!
//! Keys are evdev codes on every OS (see [`keys`]). A dry-run instance records the
//! events instead, as `(type, code, value)` evdev triples, identical on every OS: the
//! tests and `--simulate` use it. Flow's absolute pointer is not here (Flow owns it).

use std::io;

pub mod keys;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub use keys::parse_combo;

pub const EV_SYN: u16 = 0;
pub const EV_KEY: u16 = 1;
pub const EV_REL: u16 = 2;
pub const SYN_REPORT: u16 = 0;
pub const REL_X: u16 = 0;
pub const REL_Y: u16 = 1;
pub const REL_HWHEEL: u16 = 6;
pub const REL_WHEEL: u16 = 8;
pub const REL_WHEEL_HI_RES: u16 = 11;
pub const REL_HWHEEL_HI_RES: u16 = 12;

/// `(type, code, value)` as Linux evdev events.
pub type Event = (u16, u16, i32);

/// What an OS backend does. Codes are evdev codes; `release` gets them in the order to
/// release them.
pub(crate) trait Backend: Send {
    fn press(&mut self, codes: &[u16]) -> io::Result<()>;
    fn release(&mut self, codes: &[u16]) -> io::Result<()>;
    /// Notches; positive = up / right.
    fn scroll(&mut self, vertical: i32, horizontal: i32) -> io::Result<()>;
}

/// Name of the virtual device when the caller gives none (the service passes
/// "<app name> virtual input").
pub const DEFAULT_NAME: &str = "Virtual input";

pub struct VirtualInput {
    backend: Option<Box<dyn Backend>>,
    recorded: Vec<Event>,
    /// Codes held down now (a multiset, in press order).
    held: Vec<u16>,
}

impl VirtualInput {
    /// The real backend of this OS.
    pub fn new(name: &str) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        let backend: Box<dyn Backend> = Box::new(linux::Uinput::create(name)?);
        #[cfg(windows)]
        let backend: Box<dyn Backend> = {
            let _ = name;
            Box::new(windows::SendInputBackend)
        };
        #[cfg(target_os = "macos")]
        let backend: Box<dyn Backend> = {
            let _ = name;
            Box::new(macos::CgEventBackend::new()?)
        };
        #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
        {
            let _ = name;
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "no virtual input on this OS",
            ));
        }
        #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
        Ok(VirtualInput {
            backend: Some(backend),
            recorded: Vec::new(),
            held: Vec::new(),
        })
    }

    /// Records events instead of sending them.
    pub fn dry_run() -> Self {
        VirtualInput {
            backend: None,
            recorded: Vec::new(),
            held: Vec::new(),
        }
    }

    /// `dry_run()` or `new(name)`.
    pub fn open(name: &str, dry_run: bool) -> io::Result<Self> {
        if dry_run {
            Ok(Self::dry_run())
        } else {
            Self::new(name)
        }
    }

    pub fn is_dry_run(&self) -> bool {
        self.backend.is_none()
    }

    /// Events recorded so far (dry run only).
    pub fn events(&self) -> &[Event] {
        &self.recorded
    }

    pub fn clear_events(&mut self) {
        self.recorded.clear();
    }

    /// Codes held down now.
    pub fn held(&self) -> &[u16] {
        &self.held
    }

    fn syn(&mut self) {
        self.recorded.push((EV_SYN, SYN_REPORT, 0));
    }

    pub fn press(&mut self, codes: &[u16]) {
        self.held.extend_from_slice(codes);
        match self.backend.as_mut() {
            Some(b) => {
                if let Err(e) = b.press(codes) {
                    log::warn!("virtual input: press failed: {e}");
                }
            }
            None => {
                for &c in codes {
                    self.recorded.push((EV_KEY, c, 1));
                }
                self.syn();
            }
        }
    }

    /// Releases `codes` in reverse order (modifiers last).
    pub fn release(&mut self, codes: &[u16]) {
        for c in codes {
            if let Some(i) = self.held.iter().rposition(|h| h == c) {
                self.held.remove(i);
            }
        }
        let reversed: Vec<u16> = codes.iter().rev().copied().collect();
        match self.backend.as_mut() {
            Some(b) => {
                if let Err(e) = b.release(&reversed) {
                    log::warn!("virtual input: release failed: {e}");
                }
            }
            None => {
                for c in reversed {
                    self.recorded.push((EV_KEY, c, 0));
                }
                self.syn();
            }
        }
    }

    pub fn tap(&mut self, codes: &[u16]) {
        self.press(codes);
        self.release(codes);
    }

    /// Scrolls whole notches (positive: up / right).
    pub fn scroll(&mut self, vertical: i32, horizontal: i32) {
        match self.backend.as_mut() {
            Some(b) => {
                if let Err(e) = b.scroll(vertical, horizontal) {
                    log::warn!("virtual input: scroll failed: {e}");
                }
            }
            None => {
                if vertical != 0 {
                    self.recorded.push((EV_REL, REL_WHEEL, vertical));
                    self.recorded
                        .push((EV_REL, REL_WHEEL_HI_RES, vertical * 120));
                }
                if horizontal != 0 {
                    self.recorded.push((EV_REL, REL_HWHEEL, horizontal));
                    self.recorded
                        .push((EV_REL, REL_HWHEEL_HI_RES, horizontal * 120));
                }
                self.syn();
            }
        }
    }

    /// Releases everything held down (the device went away mid-press, Flow switched).
    pub fn release_all(&mut self) {
        if self.held.is_empty() {
            return;
        }
        let held = self.held.clone();
        self.release(&held);
    }

    /// Releases what is held and destroys the virtual device.
    pub fn close(&mut self) {
        self.release_all();
        self.backend = None;
    }
}

impl Drop for VirtualInput {
    fn drop(&mut self) {
        self.release_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_run_records_evdev_events() {
        let mut v = VirtualInput::dry_run();
        v.press(&[29, 46]);
        assert_eq!(v.events(), &[(1, 29, 1), (1, 46, 1), (0, 0, 0)]);
        v.release(&[29, 46]);
        assert_eq!(&v.events()[3..], &[(1, 46, 0), (1, 29, 0), (0, 0, 0)]);
        v.clear_events();
        v.scroll(1, -1);
        assert_eq!(
            v.events(),
            &[
                (2, 8, 1),
                (2, 11, 120),
                (2, 6, -1),
                (2, 12, -120),
                (0, 0, 0)
            ]
        );
    }

    #[test]
    fn release_all_releases_what_is_held() {
        let mut v = VirtualInput::dry_run();
        v.press(&[29, 42]);
        v.press(&[keys::BTN_SIDE]);
        v.release(&[keys::BTN_SIDE]);
        assert_eq!(v.held(), &[29, 42]);
        v.clear_events();
        v.release_all();
        assert_eq!(v.events(), &[(1, 42, 0), (1, 29, 0), (0, 0, 0)]);
        assert!(v.held().is_empty());
        v.release_all();
        assert_eq!(v.events().len(), 3);
    }
}
