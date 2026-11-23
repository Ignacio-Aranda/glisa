//! App settings (the window's *App settings* page): theme, language, start at login,
//! notifications and official product photos. Stored in `<config dir>/app.json`
//! (private). Device settings live in the service's config, not here.
//!
//! Keys the window adds on its own are kept as they are (`extra`), so the UI can store
//! small preferences without a shell change.

use glisa_common::paths;
use glisa_i18n::tr;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_notification::NotificationExt;

use crate::AppState;
use crate::fsutil::write_private;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// "system" | "light" | "dark"
    pub theme: String,
    /// "system" | "en" | "es"
    pub language: String,
    /// Start Glisa (in the tray) at login. Read back from the OS on every get.
    pub autostart: bool,
    pub notifications: bool,
    /// Download product photos from logitech.com for the Devices cards.
    pub official_photos: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            language: "system".into(),
            autostart: true,
            notifications: true,
            official_photos: false,
            extra: Map::new(),
        }
    }
}

/// Keys of `extra` that belong to the shell, not to the window.
const FIRST_RUN_DONE: &str = "first_run_done";
const TRAY_NOTICE_SHOWN: &str = "tray_notice_shown";

impl Settings {
    /// The chosen language code, or None for "follow the system".
    pub fn language_code(&self) -> Option<&'static str> {
        if self.language == "system" {
            None
        } else {
            glisa_i18n::normalize(&self.language)
        }
    }

    pub fn prefers_dark(&self) -> bool {
        self.theme == "dark"
    }

    fn flag(&self, key: &str) -> bool {
        self.extra
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    fn sanitize(&mut self) {
        if !matches!(self.theme.as_str(), "system" | "light" | "dark") {
            self.theme = "system".into();
        }
        if self.language != "system" && glisa_i18n::normalize(&self.language).is_none() {
            self.language = "system".into();
        }
        // Bound what the window may park here.
        self.extra
            .retain(|k, v| k.len() <= 64 && v.to_string().len() <= 4096);
        while self.extra.len() > 64 {
            let Some(k) = self.extra.keys().next().cloned() else {
                break;
            };
            self.extra.remove(&k);
        }
    }
}

fn path() -> std::path::PathBuf {
    paths::config_dir().join("app.json")
}

pub fn load() -> Settings {
    let mut s = std::fs::read(path())
        .ok()
        .and_then(|d| serde_json::from_slice::<Settings>(&d).ok())
        .unwrap_or_default();
    s.sanitize();
    s
}

pub fn save(s: &Settings) {
    match serde_json::to_vec_pretty(s) {
        Ok(data) => {
            if let Err(e) = write_private(&path(), &data) {
                log::warn!("can't save the app settings: {e}");
            }
        }
        Err(e) => log::warn!("can't encode the app settings: {e}"),
    }
}

/// The settings as the window sees them, with the real start-at-login state.
pub fn current(app: &AppHandle) -> Settings {
    let state = app.state::<AppState>();
    let mut s = state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Ok(on) = app.autolaunch().is_enabled() {
        s.autostart = on;
    }
    s.extra.remove(FIRST_RUN_DONE);
    s.extra.remove(TRAY_NOTICE_SHOWN);
    s
}

/// Applies a partial update from the window and returns the result.
pub fn update(app: &AppHandle, patch: &Value) -> Result<Settings, String> {
    let Some(patch) = patch.as_object() else {
        return Err(tr!("Invalid parameters"));
    };
    let state = app.state::<AppState>();
    let (before, after) = {
        let mut s = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        let before = s.clone();
        let mut merged = serde_json::to_value(&*s).map_err(|e| e.to_string())?;
        if let Some(obj) = merged.as_object_mut() {
            for (k, v) in patch {
                if k == FIRST_RUN_DONE || k == TRAY_NOTICE_SHOWN {
                    continue;
                }
                obj.insert(k.clone(), v.clone());
            }
        }
        let mut next: Settings =
            serde_json::from_value(merged).map_err(|_| tr!("Invalid parameters"))?;
        next.sanitize();
        *s = next.clone();
        (before, next)
    };
    if patch.contains_key("autostart") {
        set_autostart(app, after.autostart);
    }
    if before.language != after.language {
        glisa_i18n::set_language(after.language_code());
        crate::tray::refresh(app);
    }
    save(&after);
    Ok(current(app))
}

fn set_autostart(app: &AppHandle, on: bool) {
    let al = app.autolaunch();
    let r = if on { al.enable() } else { al.disable() };
    if let Err(e) = r {
        log::warn!("can't change start at login: {e}");
    }
}

/// First start: turn on start-at-login (the buttons only work while Glisa runs).
/// Not for development builds, which would register a binary from the build tree.
pub fn first_run(app: &AppHandle) {
    let state = app.state::<AppState>();
    let mut s = state.settings.lock().unwrap_or_else(|e| e.into_inner());
    if s.flag(FIRST_RUN_DONE) {
        return;
    }
    s.extra.insert(FIRST_RUN_DONE.into(), Value::Bool(true));
    if !cfg!(debug_assertions) && !state.simulate {
        set_autostart(app, true);
    }
    save(&s);
}

/// The first time the window is closed, explain that Glisa keeps running.
pub fn tray_notice_once(app: &AppHandle) {
    let state = app.state::<AppState>();
    let notify = {
        let mut s = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        if s.flag(TRAY_NOTICE_SHOWN) || !s.notifications {
            false
        } else {
            s.extra.insert(TRAY_NOTICE_SHOWN.into(), Value::Bool(true));
            save(&s);
            true
        }
    };
    if notify {
        let body = tr!(
            "{app} keeps running in the background so your buttons keep working. Open it again from its tray icon or the app menu; quit it from the tray.",
            app = glisa_common::APP_NAME
        );
        let _ = app
            .notification()
            .builder()
            .title(glisa_common::APP_NAME)
            .body(body)
            .show();
    }
}
