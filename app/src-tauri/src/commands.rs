//! Commands the window can call. Everything the page asks for goes through here: the
//! page has no direct access to files, dialogs, links or notifications (see
//! capabilities/default.json).

use std::path::PathBuf;

use glisa_common::{APP_NAME, VERSION, paths, platform};
use glisa_i18n::tr;
use serde_json::{Value, json};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::{AppState, pictures, settings};

/// Runs `f` on the blocking pool (HID I/O, dialogs, downloads), off the main thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())
}

/// The service's JSON API (same methods as `glisa-ctl`), answered in `lang`.
#[tauri::command]
pub async fn api(
    state: State<'_, AppState>,
    method: String,
    params: Option<Value>,
    lang: Option<String>,
) -> Result<Value, String> {
    let service = state.service.clone();
    let lang = lang.unwrap_or_else(|| state.language());
    let params = params.unwrap_or_else(|| json!({}));
    blocking(move || service.handle(&method, &params, Some(&lang)))
        .await?
        .map_err(|e| e.0)
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> Value {
    json!({ "name": APP_NAME, "version": VERSION, "platform": platform(), "simulate": state.simulate })
}

#[tauri::command]
pub fn settings_get(app: AppHandle) -> settings::Settings {
    settings::current(&app)
}

#[tauri::command]
pub fn settings_set(app: AppHandle, patch: Value) -> Result<settings::Settings, String> {
    let photos_off = patch.get("official_photos").and_then(Value::as_bool) == Some(false);
    let out = settings::update(&app, &patch)?;
    if photos_off {
        app.state::<AppState>().photos.clear_cache();
    }
    Ok(out)
}

/// Native file dialog, then the picture is checked and stored (see `pictures`).
#[tauri::command]
pub async fn picture_choose(app: AppHandle, device_id: String) -> Result<Value, String> {
    let lang = app.state::<AppState>().language();
    blocking(move || {
        glisa_i18n::with_language(Some(&lang), || {
            let picked = app
                .dialog()
                .file()
                .set_title(tr!("Choose a picture"))
                .add_filter(tr!("Pictures"), pictures::EXTENSIONS)
                .blocking_pick_file();
            let Some(file) = picked else {
                return json!({ "ok": false });
            };
            let Ok(path) = file.into_path() else {
                return json!({ "ok": false, "error": tr!("Can't read the file") });
            };
            match pictures::import(&path, &device_id) {
                Ok(()) => json!({ "ok": true }),
                Err(e) => json!({ "ok": false, "error": e }),
            }
        })
    })
    .await
}

/// Downloads a picture from an https link the user pasted.
#[tauri::command]
pub async fn picture_from_url(
    app: AppHandle,
    device_id: String,
    url: String,
) -> Result<Value, String> {
    let lang = app.state::<AppState>().language();
    blocking(move || {
        glisa_i18n::with_language(Some(&lang), || {
            match pictures::import_url(&url, &device_id) {
                Ok(()) => json!({ "ok": true }),
                Err(e) => json!({ "ok": false, "error": e }),
            }
        })
    })
    .await
}

#[tauri::command]
pub fn picture_layout_get(device_id: String) -> pictures::Layout {
    pictures::layout_get(&device_id)
}

#[tauri::command]
pub fn picture_layout_set(
    device_id: String,
    layout: pictures::Layout,
) -> Result<pictures::Layout, String> {
    pictures::layout_set(&device_id, layout)
}

#[tauri::command]
pub async fn picture_get(device_id: String) -> Result<Option<String>, String> {
    blocking(move || pictures::get(&device_id)).await
}

#[tauri::command]
pub fn picture_remove(device_id: String) -> Result<(), String> {
    pictures::remove(&device_id).map_err(|e| e.to_string())
}

/// The official product photo of a device, when the setting is on and one exists.
#[tauri::command]
pub async fn photo_get(app: AppHandle, device_id: String) -> Result<Option<String>, String> {
    blocking(move || {
        let state = app.state::<AppState>();
        if !state
            .settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .official_photos
        {
            return None;
        }
        let dev = state
            .service
            .handle("device", &json!({ "id": device_id }), None)
            .ok()?;
        let key = dev.get("photo").and_then(Value::as_str)?;
        state.photos.get(key)
    })
    .await
}

/// Opens a web page in the browser: https only.
#[tauri::command]
pub fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    if !url.starts_with("https://")
        || url.len() > 2048
        || !url.bytes().all(|b| b.is_ascii_graphic())
    {
        return Err(tr!("Invalid parameters"));
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

/// Opens a folder in the file manager: only Glisa' own folders (Flow's received
/// files live in the cache directory) and the user's Downloads.
#[tauri::command]
pub fn open_path(app: AppHandle, path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    let real = std::fs::canonicalize(&p).map_err(|e| e.to_string())?;
    let mut roots = vec![paths::cache_dir(), paths::config_dir()];
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        roots.push(PathBuf::from(home).join("Downloads"));
    }
    let allowed = roots
        .iter()
        .filter_map(|r| std::fs::canonicalize(r).ok())
        .any(|r| real.starts_with(&r));
    if !allowed || !real.is_dir() {
        return Err(tr!("Invalid parameters"));
    }
    app.opener()
        .open_path(real.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Grants a missing permission (`status.permissions[].id`) through the service's
/// `fix_permission`: the udev rule through pkexec on Linux, the system prompt or the
/// right System Settings pane on macOS. May wait for a password: off the main thread.
#[tauri::command]
pub async fn setup_permission(app: AppHandle, id: String) -> Result<Value, String> {
    let state = app.state::<AppState>();
    let lang = state.language();
    let service = state.service.clone();
    blocking(move || service.handle("fix_permission", &json!({ "id": id }), Some(&lang)))
        .await?
        .map_err(|e| e.0)
}

/// Opens the system's Bluetooth settings (to pair a device).
#[tauri::command]
pub fn open_bluetooth(app: AppHandle) -> Result<(), String> {
    if cfg!(target_os = "windows") {
        return app
            .opener()
            .open_url("ms-settings:bluetooth", None::<&str>)
            .map_err(|e| e.to_string());
    }
    if cfg!(target_os = "macos") {
        return app
            .opener()
            .open_url(
                "x-apple.systempreferences:com.apple.BluetoothSettings",
                None::<&str>,
            )
            .map_err(|e| e.to_string());
    }
    // Linux: the desktop's own Bluetooth settings, first one that exists.
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_ascii_uppercase();
    let mut candidates: Vec<(&str, &[&str])> = Vec::new();
    if desktop.contains("KDE") {
        candidates.push(("systemsettings", &["kcm_bluetooth"]));
        candidates.push(("kcmshell6", &["kcm_bluetooth"]));
    }
    if desktop.contains("GNOME") || desktop.contains("UNITY") || desktop.contains("BUDGIE") {
        candidates.push(("gnome-control-center", &["bluetooth"]));
    }
    candidates.extend_from_slice(&[
        ("systemsettings", &["kcm_bluetooth"][..]),
        ("gnome-control-center", &["bluetooth"]),
        ("blueman-manager", &[]),
        ("blueberry", &[]),
    ]);
    for (cmd, args) in candidates {
        if std::process::Command::new(cmd)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok()
        {
            return Ok(());
        }
    }
    Err(tr!("Can't find this desktop's Bluetooth settings"))
}

#[tauri::command]
pub fn quit(app: AppHandle) {
    app.exit(0);
}
