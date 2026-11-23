//! The Glisa app.
//!
//! One process holds the device service (`glisa-service`), Flow
//! (`glisa-flow`) and a tray icon. The window is a WebView that exists only while it
//! is open: closing it destroys it and frees its memory, and the service keeps running.
//! The tray's *Quit* ends everything.
//!
//! ```text
//! glisa                  start (or wake the running instance) and open the window
//! glisa --background     service + tray only (what autostart runs)
//! glisa --simulate       add simulated devices (demo, no hardware)
//! ```
//!
//! The window talks to the service through the `api` command (the same JSON API as
//! `glisa-ctl`) plus a few shell commands (settings, pictures, links), and listens
//! to `lo://event` for changes. See docs/ARCHITECTURE.md.

mod commands;
mod fsutil;
mod notify;
mod photos;
mod pictures;
mod settings;
mod tray;

use std::sync::{Arc, Mutex};

use glisa_common::{APP_NAME, Event, EventSink, FlowHost, VERSION};
use glisa_flow::{FlowOptions, FlowService};
use glisa_service::{Service, ServiceOptions};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

pub const MAIN_WINDOW: &str = "main";
/// Tauri event carrying a [`glisa_common::Event`] to the window.
pub const EVENT: &str = "lo://event";

pub struct AppState {
    pub service: Arc<Service>,
    pub flow: Arc<FlowService>,
    pub settings: Mutex<settings::Settings>,
    pub photos: photos::Photos,
    pub simulate: bool,
    notifier: notify::Notifier,
}

impl AppState {
    /// Language the window and tray use now ("en", "es").
    pub fn language(&self) -> String {
        let s = self.settings.lock().unwrap_or_else(|e| e.into_inner());
        s.language_code()
            .map(str::to_string)
            .unwrap_or_else(|| glisa_i18n::system_language().to_string())
    }
}

#[derive(Debug, Default)]
struct Args {
    background: bool,
    simulate: bool,
}

fn parse_args() -> Option<Args> {
    let mut args = Args::default();
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--background" | "--minimized" => args.background = true,
            "--simulate" | "--demo" => args.simulate = true,
            "--version" | "-V" => {
                println!("{APP_NAME} {VERSION}");
                return None;
            }
            "--help" | "-h" => {
                println!(
                    "{APP_NAME} {VERSION}\n\nUsage: glisa [--background] [--simulate]\n\n  \
                     --background  start in the tray without opening the window\n  \
                     --simulate    add simulated devices (no hardware needed)\n\n\
                     Command line control: glisa-ctl --help"
                );
                return None;
            }
            // Ignore what desktop launchers or the OS add (file paths, -psn_… on macOS).
            _ => {}
        }
    }
    Some(args)
}

fn init_logging() {
    let env = std::env::var("GLISA_LOG").unwrap_or_else(|_| "info".into());
    let level = match env.to_ascii_lowercase().as_str() {
        "trace" => log::LevelFilter::Trace,
        "debug" => log::LevelFilter::Debug,
        "warn" => log::LevelFilter::Warn,
        "error" => log::LevelFilter::Error,
        "off" => log::LevelFilter::Off,
        _ => log::LevelFilter::Info,
    };
    struct Stderr;
    impl log::Log for Stderr {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, r: &log::Record) {
            if self.enabled(r.metadata()) {
                eprintln!("{} {}: {}", r.level(), r.target(), r.args());
            }
        }
        fn flush(&self) {}
    }
    static LOGGER: Stderr = Stderr;
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(level);
    }
}

/// Opens the window, or brings it to the front when it already exists.
pub fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let dark = app
        .try_state::<AppState>()
        .map(|s| {
            s.settings
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .prefers_dark()
        })
        .unwrap_or(false);
    let bg = if dark { (15, 17, 21) } else { (246, 247, 249) };
    let built = WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::App("index.html".into()))
        .title(APP_NAME)
        .inner_size(1180.0, 780.0)
        .min_inner_size(900.0, 600.0)
        .center()
        .background_color(tauri::window::Color(bg.0, bg.1, bg.2, 255))
        .build();
    match built {
        Ok(w) => {
            let app = app.clone();
            w.on_window_event(move |e| {
                if let WindowEvent::Destroyed = e {
                    settings::tray_notice_once(&app);
                }
            });
        }
        Err(e) => log::error!("can't open the window: {e}"),
    }
}

/// Every service and Flow event: straight to the window, and to the notifier thread
/// (tray, notifications), which may call the service back. Never blocks.
fn forward_event(app: &AppHandle, ev: Event) {
    let _ = app.emit(EVENT, &ev);
    if let Some(state) = app.try_state::<AppState>() {
        state.notifier.push(ev);
    }
}

/// Quitting: Flow and the service stop on their own thread (nothing here may wait on the
/// main thread), and if that takes longer than [`QUIT_TIMEOUT`] the process ends anyway:
/// Quit must never leave Glisa hanging. The kernel closes the virtual input device (and so
/// lets go of anything held) and the sockets when the process ends; a KWin script left
/// loaded is replaced at the next start.
const QUIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

fn shut_down(flow: Arc<FlowService>, service: Arc<Service>) {
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("app-quit".into())
        .spawn(move || {
            log::info!("quitting: stopping Flow");
            flow.stop();
            log::info!("quitting: stopping the device service");
            service.stop();
            let _ = done_tx.send(());
        });
    if spawned.is_err() || done_rx.recv_timeout(QUIT_TIMEOUT).is_err() {
        log::warn!("quitting: shutting down took too long; exiting anyway");
        std::process::exit(0);
    }
    log::info!("quitting: done");
}

pub fn run() {
    let Some(args) = parse_args() else { return };
    init_logging();
    match glisa_common::paths::migrate_legacy() {
        Ok(true) => log::info!("imported the settings and Flow pairings of LinOptions"),
        Ok(false) => {}
        Err(e) => log::warn!("couldn't import the LinOptions settings: {e}"),
    }
    let initial = settings::load();
    glisa_i18n::set_language(initial.language_code());
    let Args {
        background,
        simulate,
    } = args;

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if !argv.iter().any(|a| a == "--background") {
                show_main_window(app);
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--background"]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            commands::api,
            commands::app_info,
            commands::settings_get,
            commands::settings_set,
            commands::picture_choose,
            commands::picture_from_url,
            commands::picture_layout_get,
            commands::picture_layout_set,
            commands::picture_get,
            commands::picture_remove,
            commands::photo_get,
            commands::open_url,
            commands::open_path,
            commands::setup_permission,
            commands::open_bluetooth,
            commands::quit,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let sink_app = handle.clone();
            let events: EventSink = Arc::new(move |ev: Event| forward_event(&sink_app, ev));
            let service = Service::new(ServiceOptions {
                config_path: None,
                simulate,
                dry_run_input: false,
                ipc: None,
                events: events.clone(),
            })
            .map_err(|e| format!("{e}"))?;
            let host: Arc<dyn FlowHost> = service.clone();
            let flow = FlowService::new(
                Arc::downgrade(&host),
                events,
                FlowOptions {
                    demo: simulate,
                    ..FlowOptions::default()
                },
            );
            service.set_flow(flow.clone());
            app.manage(AppState {
                service: service.clone(),
                flow: flow.clone(),
                settings: Mutex::new(initial),
                photos: photos::Photos::load(),
                simulate,
                notifier: notify::Notifier::start(handle.clone()),
            });
            service.start().map_err(|e| format!("{e}"))?;
            flow.start();
            settings::first_run(&handle);
            tray::create(&handle)?;
            if !background {
                show_main_window(&handle);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the Glisa app");

    app.run(|app, event| match event {
        // Closing the last window keeps the service and the tray running; only Quit
        // (`app.exit(0)`, which carries an exit code) ends the process.
        RunEvent::ExitRequested { api, code, .. } if code.is_none() => api.prevent_exit(),
        RunEvent::Exit => {
            if let Some(state) = app.try_state::<AppState>() {
                shut_down(state.flow.clone(), state.service.clone());
            }
        }
        _ => {}
    });
}
