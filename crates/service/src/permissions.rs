//! What Glisa needs from the OS, for `status.permissions`: a list of
//! `{id, state: "ok" | "missing" | "unknown", fix}` where `fix` names what
//! `fix_permission` does about it (`null`: nothing can be done from here).
//!
//! | OS | ids | fix |
//! |---|---|---|
//! | Linux | `hid` (hidraw access), `uinput` | `install_rules` (udev rule through pkexec) |
//! | macOS | `accessibility`, `input_monitoring`, `local_network` | `request` / `open_settings` |
//! | Windows | none | |

use serde_json::Value;

/// What the service knows that the checks need.
pub struct Facts {
    /// hidraw nodes we were refused.
    pub denied: bool,
    /// Logitech nodes opened successfully.
    pub opened_any: bool,
    /// The virtual input device could not be created.
    pub input_error: bool,
    /// Input is only recorded (tests, `--simulate`).
    pub dry_run_input: bool,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn entry(id: &str, state: &str, fix: Option<&str>) -> Value {
    serde_json::json!({"id": id, "state": state, "fix": fix})
}

pub fn list(facts: &Facts) -> Vec<Value> {
    #[cfg(target_os = "linux")]
    {
        use crate::setup;
        // Offered even without polkit: the fix then answers with the manual command.
        let fix = Some("install_rules");
        let hid = if facts.denied {
            "missing"
        } else if facts.opened_any || setup::rules_installed() {
            "ok"
        } else {
            "unknown"
        };
        let uinput = if (!facts.dry_run_input && !facts.input_error) || setup::uinput_accessible() {
            "ok"
        } else {
            "missing"
        };
        vec![
            entry("hid", hid, if hid == "ok" { None } else { fix }),
            entry("uinput", uinput, if uinput == "ok" { None } else { fix }),
        ]
    }
    #[cfg(target_os = "macos")]
    {
        let _ = facts;
        let ax = if macos::accessibility_trusted() {
            "ok"
        } else {
            "missing"
        };
        let (im, im_fix) = match macos::input_monitoring() {
            macos::Access::Granted => ("ok", None),
            macos::Access::Denied => ("missing", Some("open_settings")),
            macos::Access::Unknown => ("unknown", Some("request")),
        };
        vec![
            entry("accessibility", ax, (ax != "ok").then_some("open_settings")),
            entry("input_monitoring", im, im_fix),
            // No API tells whether Local Network access was granted.
            entry("local_network", "unknown", Some("open_settings")),
        ]
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = facts;
        Vec::new()
    }
}

/// `fix` was asked for a permission this OS doesn't have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownPermission;

/// What asking for a missing permission did (the window shows it).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FixOutcome {
    /// Granted (or it already was).
    pub ok: bool,
    /// A System Settings pane was opened: the user grants it there (macOS).
    pub opened: bool,
    /// The user dismissed the password prompt.
    pub cancelled: bool,
    pub error: Option<String>,
    /// A command to paste into a terminal instead (Linux).
    pub manual: Option<String>,
}

impl FixOutcome {
    pub fn to_json(&self) -> Value {
        serde_json::json!({"ok": self.ok, "opened": self.opened, "cancelled": self.cancelled,
                           "error": self.error, "manual": self.manual})
    }
}

/// Carries out the fix for permission `id`.
pub fn fix(id: &str) -> Result<FixOutcome, UnknownPermission> {
    #[cfg(target_os = "linux")]
    {
        use glisa_i18n::tr;
        match id {
            "hid" | "uinput" => {
                let manual = Some(crate::setup::manual_install_hint());
                let failed = |error: String| FixOutcome {
                    error: Some(error),
                    manual: manual.clone(),
                    ..Default::default()
                };
                if !crate::setup::can_elevate() {
                    return Ok(failed(tr!(
                        "This system has no polkit (pkexec) to ask for your password: run the command below in a terminal instead."
                    )));
                }
                Ok(match crate::setup::install_rules() {
                    Ok(0) => FixOutcome {
                        ok: true,
                        ..Default::default()
                    },
                    Ok(126) => FixOutcome {
                        cancelled: true,
                        manual: manual.clone(),
                        ..Default::default()
                    },
                    Ok(127) => failed(tr!(
                        "Your account isn't allowed to install it (polkit refused)."
                    )),
                    Ok(code) => failed(tr!(
                        "The installation failed (exit code {code}).",
                        code = code
                    )),
                    Err(e) => {
                        log::warn!("could not run pkexec: {e}");
                        failed(tr!("Couldn't ask for your password ({error}).", error = e))
                    }
                })
            }
            _ => Err(UnknownPermission),
        }
    }
    #[cfg(target_os = "macos")]
    {
        match id {
            "accessibility" => {
                if macos::accessibility_trusted() {
                    return Ok(FixOutcome {
                        ok: true,
                        ..Default::default()
                    });
                }
                macos::open_settings("Privacy_Accessibility");
                Ok(FixOutcome {
                    opened: true,
                    ..Default::default()
                })
            }
            "input_monitoring" => {
                if macos::request_input_monitoring() {
                    return Ok(FixOutcome {
                        ok: true,
                        ..Default::default()
                    });
                }
                macos::open_settings("Privacy_ListenEvent");
                Ok(FixOutcome {
                    opened: true,
                    ..Default::default()
                })
            }
            "local_network" => {
                macos::open_settings("Privacy_LocalNetwork");
                Ok(FixOutcome {
                    opened: true,
                    ..Default::default()
                })
            }
            _ => Err(UnknownPermission),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = id;
        Err(UnknownPermission)
    }
}

#[cfg(target_os = "macos")]
mod macos {
    //! Accessibility (AXIsProcessTrusted) and Input Monitoring (IOHIDCheckAccess /
    //! IOHIDRequestAccess, macOS 10.15+). Untested in this port: written against
    //! Apple's documented C API.

    const K_IOHID_REQUEST_TYPE_LISTEN_EVENT: u32 = 1;
    const K_IOHID_ACCESS_TYPE_GRANTED: u32 = 0;
    const K_IOHID_ACCESS_TYPE_DENIED: u32 = 1;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> u8;
    }

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOHIDCheckAccess(request_type: u32) -> u32;
        fn IOHIDRequestAccess(request_type: u32) -> u8;
    }

    pub enum Access {
        Granted,
        Denied,
        Unknown,
    }

    pub fn accessibility_trusted() -> bool {
        // SAFETY: no arguments, no preconditions.
        unsafe { AXIsProcessTrusted() != 0 }
    }

    pub fn input_monitoring() -> Access {
        // SAFETY: plain query with a valid request type.
        match unsafe { IOHIDCheckAccess(K_IOHID_REQUEST_TYPE_LISTEN_EVENT) } {
            K_IOHID_ACCESS_TYPE_GRANTED => Access::Granted,
            K_IOHID_ACCESS_TYPE_DENIED => Access::Denied,
            _ => Access::Unknown,
        }
    }

    /// Shows the system prompt (first time only); true when granted.
    pub fn request_input_monitoring() -> bool {
        // SAFETY: plain request with a valid request type.
        unsafe { IOHIDRequestAccess(K_IOHID_REQUEST_TYPE_LISTEN_EVENT) != 0 }
    }

    /// Opens System Settings → Privacy & Security at `anchor`.
    pub fn open_settings(anchor: &str) {
        let url = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
        if let Err(e) = std::process::Command::new("/usr/bin/open").arg(url).spawn() {
            log::warn!("could not open System Settings: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shape() {
        let facts = Facts {
            denied: true,
            opened_any: false,
            input_error: true,
            dry_run_input: false,
        };
        for p in list(&facts) {
            assert!(p["id"].is_string());
            assert!(["ok", "missing", "unknown"].contains(&p["state"].as_str().unwrap()));
            assert!(p["fix"].is_null() || p["fix"].is_string());
        }
        if cfg!(target_os = "linux") {
            assert_eq!(list(&facts)[0]["state"], json!("missing"));
        }
    }
}
