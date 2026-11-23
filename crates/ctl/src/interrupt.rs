//! Ctrl+C while pairing: the pairing is cancelled on the service instead of being left
//! open (Python's KeyboardInterrupt handling).

use std::sync::atomic::{AtomicBool, Ordering};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn triggered() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

#[cfg(unix)]
extern "C" fn on_sigint(_: libc::c_int) {
    if INTERRUPTED.swap(true, Ordering::SeqCst) {
        // Second Ctrl+C: leave now.
        // SAFETY: _exit is async-signal-safe.
        unsafe { libc::_exit(130) };
    }
}

#[cfg(windows)]
unsafe extern "system" fn on_ctrl(ctrl_type: u32) -> windows_sys::core::BOOL {
    if ctrl_type == 0 || ctrl_type == 1 {
        // CTRL_C_EVENT, CTRL_BREAK_EVENT
        if INTERRUPTED.swap(true, Ordering::SeqCst) {
            return 0; // second one: default handling (exit)
        }
        return 1;
    }
    0
}

/// From now on Ctrl+C sets [`triggered`] (a second one exits).
pub fn install() {
    #[cfg(unix)]
    {
        let handler: extern "C" fn(libc::c_int) = on_sigint;
        // SAFETY: the handler only touches an atomic and calls _exit.
        unsafe { libc::signal(libc::SIGINT, handler as libc::sighandler_t) };
    }
    #[cfg(windows)]
    {
        // SAFETY: the handler only touches an atomic.
        unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(on_ctrl), 1) };
    }
}
