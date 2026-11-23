//! Linux: a uinput virtual device created with ioctls on `/dev/uinput`.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Duration;

use crate::keys::KEYS;
use crate::{
    Backend, EV_KEY, EV_REL, EV_SYN, REL_HWHEEL, REL_HWHEEL_HI_RES, REL_WHEEL, REL_WHEEL_HI_RES,
    REL_X, REL_Y, SYN_REPORT,
};

const UI_SET_EVBIT: u64 = 0x4004_5564;
const UI_SET_KEYBIT: u64 = 0x4004_5565;
const UI_SET_RELBIT: u64 = 0x4004_5566;
const UI_DEV_SETUP: u64 = 0x405C_5503;
const UI_DEV_CREATE: u64 = 0x5501;
const UI_DEV_DESTROY: u64 = 0x5502;
const BUS_VIRTUAL: u16 = 0x06;

/// `struct uinput_setup`
#[repr(C)]
struct UinputSetup {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
    name: [u8; 80],
    ff_effects_max: u32,
}

/// `struct input_event` (the kernel ignores the time of events written to uinput).
#[repr(C)]
struct InputEvent {
    sec: libc::c_long,
    usec: libc::c_long,
    type_: u16,
    code: u16,
    value: i32,
}

pub struct Uinput {
    fd: OwnedFd,
}

fn check(r: libc::c_int) -> io::Result<()> {
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

impl Uinput {
    pub fn create(name: &str) -> io::Result<Self> {
        let path = CString::new("/dev/uinput").expect("no NUL");
        // SAFETY: path is a valid NUL-terminated string.
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_WRONLY | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd was just returned by open and is owned by nobody else.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let raw = fd.as_raw_fd();
        // SAFETY (all ioctls below): raw is an open uinput descriptor; integer requests
        // take an int argument, UI_DEV_SETUP a pointer to a live uinput_setup.
        unsafe {
            check(libc::ioctl(
                raw,
                UI_SET_EVBIT as _,
                libc::c_int::from(EV_KEY),
            ))?;
            check(libc::ioctl(
                raw,
                UI_SET_EVBIT as _,
                libc::c_int::from(EV_REL),
            ))?;
            let mut codes: Vec<u16> = KEYS.iter().map(|(_, c)| *c).collect();
            codes.sort_unstable();
            codes.dedup();
            for code in codes {
                check(libc::ioctl(
                    raw,
                    UI_SET_KEYBIT as _,
                    libc::c_int::from(code),
                ))?;
            }
            for rel in [
                REL_X,
                REL_Y,
                REL_WHEEL,
                REL_HWHEEL,
                REL_WHEEL_HI_RES,
                REL_HWHEEL_HI_RES,
            ] {
                check(libc::ioctl(raw, UI_SET_RELBIT as _, libc::c_int::from(rel)))?;
            }
            let mut setup = UinputSetup {
                bustype: BUS_VIRTUAL,
                vendor: 0x1209,
                product: 0x4C4F,
                version: 1,
                name: [0; 80],
                ff_effects_max: 0,
            };
            let bytes = name.as_bytes();
            let n = bytes.len().min(79);
            setup.name[..n].copy_from_slice(&bytes[..n]);
            check(libc::ioctl(
                raw,
                UI_DEV_SETUP as _,
                &setup as *const UinputSetup,
            ))?;
            check(libc::ioctl(raw, UI_DEV_CREATE as _))?;
        }
        // Let the compositor pick the device up.
        std::thread::sleep(Duration::from_millis(200));
        Ok(Uinput { fd })
    }

    fn emit(&self, type_: u16, code: u16, value: i32) -> io::Result<()> {
        let ev = InputEvent {
            sec: 0,
            usec: 0,
            type_,
            code,
            value,
        };
        let size = std::mem::size_of::<InputEvent>();
        // SAFETY: ev is a live, fully initialised repr(C) struct of `size` bytes.
        let n =
            unsafe { libc::write(self.fd.as_raw_fd(), (&ev as *const InputEvent).cast(), size) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn syn(&self) -> io::Result<()> {
        self.emit(EV_SYN, SYN_REPORT, 0)
    }
}

impl Backend for Uinput {
    fn press(&mut self, codes: &[u16]) -> io::Result<()> {
        for &c in codes {
            self.emit(EV_KEY, c, 1)?;
        }
        self.syn()
    }

    fn release(&mut self, codes: &[u16]) -> io::Result<()> {
        for &c in codes {
            self.emit(EV_KEY, c, 0)?;
        }
        self.syn()
    }

    fn scroll(&mut self, vertical: i32, horizontal: i32) -> io::Result<()> {
        if vertical != 0 {
            self.emit(EV_REL, REL_WHEEL, vertical)?;
            self.emit(EV_REL, REL_WHEEL_HI_RES, vertical * 120)?;
        }
        if horizontal != 0 {
            self.emit(EV_REL, REL_HWHEEL, horizontal)?;
            self.emit(EV_REL, REL_HWHEEL_HI_RES, horizontal * 120)?;
        }
        self.syn()
    }
}

impl Drop for Uinput {
    fn drop(&mut self) {
        // SAFETY: the descriptor is open until OwnedFd drops right after this.
        unsafe { libc::ioctl(self.fd.as_raw_fd(), UI_DEV_DESTROY as _) };
    }
}
