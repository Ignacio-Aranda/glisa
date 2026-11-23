//! Reacts to service and Flow events away from the threads that emit them (which may
//! hold service locks): refreshes the tray and shows desktop notifications for low
//! batteries and for transfers that end while the window is closed.
//!
//! Low battery (as the Python app did): once per device at or below 15 %, re-armed when
//! it charges or climbs back above 20 %.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use glisa_common::{APP_NAME, Event};
use glisa_i18n::tr;
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::{AppState, MAIN_WINDOW, tray};

pub const LOW_AT: u64 = 15;
pub const RECOVERED_AT: u64 = 20;

pub struct Notifier {
    tx: Sender<Event>,
}

impl Notifier {
    pub fn start(app: AppHandle) -> Self {
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("app-notifier".into())
            .spawn(move || worker(app, rx))
            .expect("spawn the notifier thread");
        Self { tx }
    }

    pub fn push(&self, ev: Event) {
        let _ = self.tx.send(ev);
    }
}

/// Decides when a low-battery notice is due.
#[derive(Default)]
pub struct BatteryWatch {
    warned: HashSet<String>,
}

impl BatteryWatch {
    /// Feed the latest device list; returns the devices to warn about now.
    pub fn feed(&mut self, devices: &[Value]) -> Vec<Value> {
        let mut due = Vec::new();
        for dev in devices {
            let Some(id) = dev.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(b) = dev.get("battery").filter(|b| b.is_object()) else {
                continue;
            };
            let pct = b.get("percent").and_then(Value::as_u64).filter(|p| *p > 0);
            let charging = b.get("charging").and_then(Value::as_bool).unwrap_or(false);
            let level_low = matches!(
                b.get("level").and_then(Value::as_str),
                Some("low" | "critical")
            );
            let online = dev.get("online").and_then(Value::as_bool).unwrap_or(true);
            let low = pct.map(|p| p <= LOW_AT).unwrap_or(level_low);
            let recovered = pct.map(|p| p > RECOVERED_AT).unwrap_or(!level_low);
            if charging || recovered {
                self.warned.remove(id);
            } else if low && online && !self.warned.contains(id) {
                self.warned.insert(id.to_string());
                due.push(dev.clone());
            }
        }
        due
    }
}

fn worker(app: AppHandle, rx: Receiver<Event>) {
    let mut battery = BatteryWatch::default();
    while let Ok(first) = rx.recv() {
        // Coalesce bursts (a reconnection sends several events at once).
        let mut batch = vec![first];
        while let Ok(ev) = rx.recv_timeout(Duration::from_millis(150)) {
            batch.push(ev);
            if batch.len() > 64 {
                break;
            }
        }
        let Some(state) = app.try_state::<AppState>() else {
            continue;
        };
        let lang = state.language();
        let notifications = state
            .settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .notifications;
        let devices_changed = batch
            .iter()
            .any(|e| matches!(e.topic.as_str(), "devices" | "device" | "status"));
        if devices_changed || batch.iter().any(|e| e.topic == "flow") {
            tray::refresh(&app);
        }
        if devices_changed {
            let list = state
                .service
                .handle("devices", &json!({}), Some(&lang))
                .unwrap_or(Value::Null);
            let due = battery.feed(list.as_array().map(Vec::as_slice).unwrap_or(&[]));
            if notifications {
                glisa_i18n::with_language(Some(&lang), || {
                    for dev in due {
                        let line = tray::battery_line(&dev);
                        let name = dev.get("name").and_then(Value::as_str).unwrap_or(APP_NAME);
                        let body = tr!("{device}. Charge it soon.", device = line);
                        let _ = app
                            .notification()
                            .builder()
                            .title(tr!("Low battery: {name}", name = name))
                            .body(body)
                            .show();
                    }
                });
            }
        }
        if notifications && app.get_webview_window(MAIN_WINDOW).is_none() {
            for ev in batch.iter().filter(|e| e.topic == "transfer") {
                if let Some(text) = ev.data.get("notice").and_then(Value::as_str) {
                    let _ = app.notification().builder().title("Flow").body(text).show();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, pct: u64, charging: bool) -> Value {
        json!({"id": id, "name": id, "online": true, "battery": {"percent": pct, "charging": charging}})
    }

    #[test]
    fn warns_once_and_rearms() {
        let mut w = BatteryWatch::default();
        assert!(w.feed(&[dev("a", 50, false)]).is_empty());
        assert_eq!(w.feed(&[dev("a", 15, false)]).len(), 1);
        assert!(w.feed(&[dev("a", 10, false)]).is_empty());
        assert!(w.feed(&[dev("a", 18, true)]).is_empty());
        assert_eq!(w.feed(&[dev("a", 12, false)]).len(), 1);
        assert!(w.feed(&[dev("a", 21, false)]).is_empty());
        assert_eq!(w.feed(&[dev("a", 14, false)]).len(), 1);
    }
}
