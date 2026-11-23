//! Turns diverted button presses, releases and raw movement into actions.
//!
//! Keys and mouse buttons are held for as long as the control is held. A gesture
//! control accumulates raw movement while held and, on release, fires the action of
//! the direction moved (or `tap` under [`GESTURE_THRESHOLD`]).

use std::collections::HashMap;

use glisa_input::{VirtualInput, parse_combo};
use serde_json::{Value, json};

use crate::actions::{GESTURE_THRESHOLD, action_type, mouse_button_code};

/// Actions the service itself carries out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Special {
    DpiCycle,
}

pub struct Executor {
    pub vinput: VirtualInput,
    held: HashMap<u16, Vec<u16>>,
    gesture: HashMap<u16, (i32, i32)>,
}

fn codes_for(action: &Value) -> Option<Vec<u16>> {
    match action_type(action) {
        Some("keys") => {
            let combo = action.get("combo").and_then(Value::as_str).unwrap_or("");
            match parse_combo(combo) {
                Ok(c) => Some(c),
                Err(e) => {
                    log::warn!("bad shortcut {combo:?}: {e}");
                    None
                }
            }
        }
        Some("mouse") => action
            .get("button")
            .and_then(Value::as_str)
            .and_then(mouse_button_code)
            .map(|c| vec![c]),
        _ => None,
    }
}

impl Executor {
    pub fn new(vinput: VirtualInput) -> Self {
        Executor {
            vinput,
            held: HashMap::new(),
            gesture: HashMap::new(),
        }
    }

    pub fn pressed(&mut self, cid: u16, action: &Value) -> Option<Special> {
        match action_type(action) {
            Some("gesture") => {
                self.gesture.insert(cid, (0, 0));
                None
            }
            Some("keys" | "mouse") => {
                if let Some(codes) = codes_for(action) {
                    self.vinput.press(&codes);
                    self.held.insert(cid, codes);
                }
                None
            }
            _ => self.fire(action),
        }
    }

    pub fn released(&mut self, cid: u16, action: &Value) -> Option<Special> {
        if let Some((dx, dy)) = self.gesture.remove(&cid) {
            let dir = Self::gesture_direction(dx, dy);
            let sub = action
                .get(dir)
                .filter(|v| crate::actions::truthy(Some(v)))
                .cloned()
                .unwrap_or_else(|| json!({"type": "disabled"}));
            return self.fire(&sub);
        }
        if let Some(codes) = self.held.remove(&cid) {
            self.vinput.release(&codes);
        }
        None
    }

    pub fn moved(&mut self, dx: i32, dy: i32) {
        for acc in self.gesture.values_mut() {
            acc.0 = acc.0.saturating_add(dx);
            acc.1 = acc.1.saturating_add(dy);
        }
    }

    pub fn gesture_direction(dx: i32, dy: i32) -> &'static str {
        if dx.abs().max(dy.abs()) < GESTURE_THRESHOLD {
            "tap"
        } else if dx.abs() > dy.abs() {
            if dx > 0 { "right" } else { "left" }
        } else if dy > 0 {
            "down"
        } else {
            "up"
        }
    }

    /// Lets go of everything (device gone, Flow switching away).
    pub fn release_all(&mut self) {
        let held: Vec<Vec<u16>> = self.held.drain().map(|(_, c)| c).collect();
        for codes in held {
            self.vinput.release(&codes);
        }
        self.gesture.clear();
        self.vinput.release_all();
    }

    fn fire(&mut self, action: &Value) -> Option<Special> {
        match action_type(action) {
            Some("keys" | "mouse") => {
                if let Some(codes) = codes_for(action) {
                    self.vinput.tap(&codes);
                }
            }
            Some("scroll") => {
                let dir = action.get("dir").and_then(Value::as_str).unwrap_or("");
                let v = match dir {
                    "up" => 1,
                    "down" => -1,
                    _ => 0,
                };
                let h = match dir {
                    "right" => 1,
                    "left" => -1,
                    _ => 0,
                };
                self.vinput.scroll(v, h);
            }
            Some("command") => {
                if let Some(cmd) = action.get("cmd").and_then(Value::as_str) {
                    run_command(cmd);
                }
            }
            Some("dpi_cycle") => return Some(Special::DpiCycle),
            _ => {}
        }
        None
    }
}

/// Starts `cmd` through the system shell, detached (its own session, no stdio), and
/// reaps it in the background. The command is the user's own (see SECURITY.md:
/// same-user processes can assign commands).
pub fn run_command(cmd: &str) {
    use std::process::{Command, Stdio};
    #[cfg(unix)]
    let mut c = {
        use std::os::unix::process::CommandExt;
        let mut c = Command::new("/bin/sh");
        c.arg("-c").arg(cmd);
        // SAFETY: setsid is async-signal-safe and touches no Rust state.
        unsafe {
            c.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        c
    };
    #[cfg(windows)]
    let mut c = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        let mut c = Command::new("cmd");
        c.arg("/C").raw_arg(cmd);
        c.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
        c
    };
    #[cfg(not(any(unix, windows)))]
    let mut c = Command::new(cmd);
    c.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match c.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => log::warn!("could not run {cmd:?}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gesture_direction() {
        let g = Executor::gesture_direction;
        assert_eq!(g(5, 5), "tap");
        assert_eq!(g(-200, 20), "left");
        assert_eq!(g(10, 300), "down");
        assert_eq!(g(0, -90), "up");
        assert_eq!(g(90, 0), "right");
    }

    fn keys_only(e: &Executor) -> Vec<(u16, u16, i32)> {
        e.vinput
            .events()
            .iter()
            .copied()
            .filter(|e| e.0 != 0)
            .collect()
    }

    #[test]
    fn keys_held_while_pressed_and_released_on_release_all() {
        let mut e = Executor::new(VirtualInput::dry_run());
        let a = json!({"type": "keys", "combo": "Ctrl+C"});
        e.pressed(0x53, &a);
        assert_eq!(keys_only(&e), vec![(1, 29, 1), (1, 46, 1)]);
        e.release_all();
        assert_eq!(keys_only(&e)[2..], [(1, 46, 0), (1, 29, 0)]);
        e.released(0x53, &a); // nothing more to release
        assert_eq!(keys_only(&e).len(), 4);
    }

    #[test]
    fn gestures_and_specials() {
        let mut e = Executor::new(VirtualInput::dry_run());
        let g = crate::actions::default_gestures_for(glisa_common::Platform::Linux);
        e.pressed(0xFD, &g);
        e.moved(-50, 3);
        e.moved(-50, 3);
        e.released(0xFD, &g);
        let codes: Vec<u16> = keys_only(&e).iter().take(3).map(|e| e.1).collect();
        assert_eq!(codes, vec![29, 125, 105]);
        assert_eq!(
            e.pressed(0xFD, &json!({"type": "dpi_cycle"})),
            Some(Special::DpiCycle)
        );
        e.vinput.clear_events();
        e.pressed(0x56, &json!({"type": "scroll", "dir": "left"}));
        assert_eq!(e.vinput.events()[0], (2, 6, -1));
        e.vinput.clear_events();
        e.pressed(0x56, &json!({"type": "gesture"}));
        e.released(0x56, &json!({"type": "gesture"})); // tap with no action: nothing
        assert!(e.vinput.events().is_empty());
    }
}
