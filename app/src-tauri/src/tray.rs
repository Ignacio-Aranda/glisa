//! The tray icon and its menu: open the window, each device's battery at a glance, Flow
//! on/off, quit. On Linux (AppIndicator) clicks on the icon itself aren't reported, so
//! everything is reachable from the menu.

use glisa_common::{APP_NAME, FlowApi};
use glisa_i18n::tr;
use serde_json::{Value, json};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::{AppState, show_main_window};

pub const TRAY_ID: &str = "main";

/// "LIFT — 78 %", "MX Keys S — charging", or just the name.
pub fn battery_line(dev: &Value) -> String {
    let name = dev.get("name").and_then(Value::as_str).unwrap_or("?");
    let b = dev.get("battery").filter(|b| b.is_object());
    let mut txt = match b.and_then(|b| b.get("percent")).and_then(Value::as_u64) {
        Some(p) if p > 0 => format!("{p} %"),
        _ => match b.and_then(|b| b.get("level")).and_then(Value::as_str) {
            Some("critical") => tr!("Critical"),
            Some("low") => tr!("Low"),
            Some("good") => tr!("Good"),
            Some("full") => tr!("Full"),
            _ => String::new(),
        },
    };
    if b.and_then(|b| b.get("charging")).and_then(Value::as_bool) == Some(true) {
        txt = if txt.is_empty() {
            tr!("charging")
        } else {
            tr!("{battery}, charging", battery = txt)
        };
    }
    if dev.get("online").and_then(Value::as_bool) == Some(false) {
        return tr!("{name} — disconnected", name = name);
    }
    if txt.is_empty() {
        name.to_string()
    } else {
        format!("{name} — {txt}")
    }
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let state = app.state::<AppState>();
    let lang = state.language();
    let menu = Menu::new(app)?;
    menu.append(&MenuItem::with_id(
        app,
        "open",
        tr!("Open {app}", app = APP_NAME),
        true,
        None::<&str>,
    )?)?;

    let devices = state
        .service
        .handle("devices", &json!({}), Some(&lang))
        .unwrap_or(Value::Null);
    let list = devices.as_array().cloned().unwrap_or_default();
    if !list.is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        for (i, dev) in list.iter().take(8).enumerate() {
            menu.append(&MenuItem::with_id(
                app,
                format!("dev{i}"),
                battery_line(dev),
                false,
                None::<&str>,
            )?)?;
        }
    }

    let flow = state
        .flow
        .api("flow_status", &json!({}))
        .unwrap_or(Value::Null);
    if flow.get("available").and_then(Value::as_bool) == Some(true) {
        let enabled = flow
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        menu.append(&CheckMenuItem::with_id(
            app,
            "flow",
            "Flow",
            true,
            enabled,
            None::<&str>,
        )?)?;
    }

    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        "quit",
        tr!("Quit {app}", app = APP_NAME),
        true,
        None::<&str>,
    )?)?;
    Ok(menu)
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app)?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(APP_NAME)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main_window(app),
            "quit" => app.exit(0),
            "flow" => toggle_flow(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    // macOS draws a monochrome "template" image in the menu bar's own colour; Linux
    // panels and the Windows tray get the colour glyph, readable on light and dark.
    let bytes: &[u8] = if cfg!(target_os = "macos") {
        include_bytes!("../icons/tray/template.png")
    } else {
        include_bytes!("../icons/tray/color.png")
    };
    match tauri::image::Image::from_bytes(bytes) {
        Ok(icon) => {
            builder = builder
                .icon(icon)
                .icon_as_template(cfg!(target_os = "macos"))
        }
        Err(_) => {
            if let Some(icon) = app.default_window_icon() {
                builder = builder.icon(icon.clone());
            }
        }
    }
    builder.build(app)?;
    Ok(())
}

fn toggle_flow(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let lang = state.language();
        let enabled = state
            .flow
            .api("flow_status", &json!({}))
            .ok()
            .and_then(|s| s.get("enabled").and_then(Value::as_bool))
            .unwrap_or(false);
        let r = glisa_i18n::with_language(Some(&lang), || {
            state
                .flow
                .api("flow_enable", &json!({ "enabled": !enabled }))
        });
        if let Err(e) = r {
            log::warn!("Flow on/off from the tray: {e}");
        }
        refresh(&app);
    });
}

/// Rebuilds the menu (devices, batteries, Flow, language). Cheap; called on changes.
pub fn refresh(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    match build_menu(app) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(e) => log::warn!("tray menu: {e}"),
    }
}
