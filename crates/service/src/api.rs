//! The JSON API: `Service::handle(method, params, lang)`, answered in the client's language.
//!
//! | method | params | result |
//! |---|---|---|
//! | `ping` | – | `{version}` |
//! | `status` | – | `{version, denied, input_error, simulate, rules_installed, devices, platform, permissions}` |
//! | `devices` | – | short snapshots |
//! | `device` | `id` | full snapshot |
//! | `set` | `id, key, value` | the value stored |
//! | `set_button` | `id, cid, action` | `true` |
//! | `set_host` | `id, host` | `true` |
//! | `reset` | `id` | `true` |
//! | `rename` | `id, name` | short snapshot |
//! | `forget` | `id` | `true` |
//! | `sim` | `id, op, …` | `{events}` (simulated devices only) |
//! | `presets` | – | the action presets of this OS (see `actions::presets_json`) |
//! | `fix_permission` | `id` (from `status.permissions`) | `{ok}` |
//! | `flow_*` | | Flow's (see `crate::Service::set_flow`) |
//!
//! Snapshots also carry `family` and `photo` (docs/ARCHITECTURE.md, Device pictures).

use glisa_common::{ApiError, ApiResult};
use glisa_hidpp::Error as HidppError;
use glisa_i18n::tr;
use serde_json::{Map, Value, json};

use crate::device::{DevError, ManagedDevice, int_of};
use crate::service::{BATTERY_FRESH, Service};

fn err(msg: String) -> ApiError {
    ApiError::new(msg)
}

fn invalid() -> ApiError {
    err(tr!("Invalid parameters"))
}

fn dev_error(e: DevError) -> ApiError {
    match e {
        DevError::Invalid(m) => err(m),
        DevError::Unsupported => err(tr!("This device doesn't support that setting")),
        DevError::Save(e) => err(tr!(
            "Couldn't save the settings: {error}",
            error = crate::fsutil::error_text(&e)
        )),
        DevError::Hidpp(HidppError::Hidpp { text, .. }) => err(tr!(
            "The device rejected the change ({error})",
            error = text
        )),
        DevError::Hidpp(HidppError::FeatureNotSupported(_)) => {
            err(tr!("This device doesn't support that setting"))
        }
        DevError::Hidpp(HidppError::Malformed(what)) => err(tr!(
            "The device rejected the change ({error})",
            error = what
        )),
        DevError::Hidpp(HidppError::NoResponse(_) | HidppError::Closed(_)) => err(tr!(
            "The device didn't respond. Check that it's on and connected"
        )),
    }
}

fn int_param(p: &Map<String, Value>, key: &str) -> Result<i64, ApiError> {
    p.get(key).and_then(int_of).ok_or_else(invalid)
}

impl Service {
    /// Answers one API request, in `lang` (the client's language; `None`: the
    /// process default).
    pub fn handle(&self, method: &str, params: &Value, lang: Option<&str>) -> ApiResult {
        glisa_i18n::with_language(lang, || self.handle_here(method, params))
    }

    fn handle_here(&self, m: &str, params: &Value) -> ApiResult {
        let empty = Map::new();
        let p = match params {
            Value::Null => &empty,
            Value::Object(o) => o,
            _ => return Err(invalid()),
        };
        if m.starts_with("flow_") {
            // Flow locks for itself (network calls).
            return match self.flow() {
                Some(flow) => flow.api(m, params),
                None => Err(err(tr!("Flow isn't available"))),
            };
        }
        if m == "fix_permission" {
            // May wait minutes for a password: outside the request lock.
            let id = p.get("id").and_then(Value::as_str).ok_or_else(invalid)?;
            return match crate::permissions::fix(id) {
                Ok(outcome) => {
                    if outcome.ok {
                        self.retry_input(true);
                    }
                    self.emit("status", json!({}));
                    Ok(outcome.to_json())
                }
                Err(_) => Err(err(tr!("Unknown permission: {id}", id = id))),
            };
        }
        let _serial = self.api_lock.lock().unwrap_or_else(|e| e.into_inner());
        match m {
            "ping" => return Ok(json!({"version": glisa_common::VERSION})),
            "status" => return Ok(self.status()),
            "devices" => {
                return Ok(Value::Array(
                    self.devices().iter().map(|d| d.snapshot(false)).collect(),
                ));
            }
            "presets" => return Ok(crate::actions::presets_json()),
            _ => {}
        }
        let id = p.get("id").and_then(Value::as_str).unwrap_or("");
        let md = self
            .device(id)
            .ok_or_else(|| err(tr!("Device not found")))?;
        match m {
            "device" => {
                if md.online() && md.battery_age().is_none_or(|a| a > BATTERY_FRESH) {
                    md.refresh_battery();
                }
                return Ok(md.snapshot(true));
            }
            // Also while disconnected: only Glisa stores it.
            "rename" => {
                md.rename(p.get("name").unwrap_or(&Value::Null))
                    .map_err(dev_error)?;
                self.emit_devices();
                self.emit_device(&md.id);
                return Ok(md.snapshot(false));
            }
            "forget" => {
                self.forget(&md);
                return Ok(json!(true));
            }
            _ => {}
        }
        if !md.online() {
            return Err(err(tr!("The device is disconnected")));
        }
        let out = self.device_request(&md, m, p)?;
        self.emit_device(&md.id);
        Ok(out)
    }

    fn device_request(&self, md: &ManagedDevice, m: &str, p: &Map<String, Value>) -> ApiResult {
        match m {
            "set" => {
                let key = p.get("key").and_then(Value::as_str).ok_or_else(invalid)?;
                let value = p.get("value").ok_or_else(invalid)?;
                md.set(key, value).map_err(dev_error)
            }
            "set_button" => {
                let cid = u16::try_from(int_param(p, "cid")?).map_err(|_| invalid())?;
                md.set_button(cid, p.get("action")).map_err(dev_error)?;
                Ok(json!(true))
            }
            "set_host" => {
                md.set_host(int_param(p, "host")?).map_err(dev_error)?;
                Ok(json!(true))
            }
            "reset" => {
                self.config
                    .reset_device(&md.key)
                    .map_err(|e| dev_error(DevError::Save(e)))?;
                md.factory_reset();
                Ok(json!(true))
            }
            "sim" if self.sim(&md.source).is_some() => {
                let sim = self.sim(&md.source).expect("checked");
                let u16_param =
                    |k: &str| int_param(p, k).and_then(|v| u16::try_from(v).map_err(|_| invalid()));
                let i16_param =
                    |k: &str| int_param(p, k).and_then(|v| i16::try_from(v).map_err(|_| invalid()));
                match p.get("op").and_then(Value::as_str) {
                    Some("press") => sim.press(u16_param("cid")?),
                    Some("release") => sim.release(u16_param("cid")?),
                    Some("move") => sim.move_by(i16_param("dx")?, i16_param("dy")?),
                    Some("battery") => {
                        let pct = u8::try_from(int_param(p, "percent")?).map_err(|_| invalid())?;
                        sim.set_battery(pct, crate::actions::truthy(p.get("charging")));
                    }
                    _ => {}
                }
                let events = self.input_events();
                let start = events.len().saturating_sub(50);
                Ok(json!({"events": events[start..]}))
            }
            _ => Err(err(tr!("Unknown method: {method}", method = m))),
        }
    }

    /// The `status` result.
    pub fn status(&self) -> Value {
        let facts = crate::permissions::Facts {
            denied: !self.denied().is_empty(),
            opened_any: self.opened_any(),
            input_error: !self.input_error().is_empty(),
            dry_run_input: self.dry_run_input(),
        };
        json!({
            "version": glisa_common::VERSION,
            "denied": self.denied(),
            "input_error": self.input_error(),
            "simulate": self.simulate(),
            // Only Linux has something to install.
            "rules_installed": !cfg!(target_os = "linux") || crate::setup::rules_installed(),
            "devices": self.devices().len(),
            "platform": glisa_common::platform(),
            "permissions": crate::permissions::list(&facts),
        })
    }
}
