// No console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `glisa ctl …`: the command-line tool from the app itself, so it exists wherever the
    // app is installed (the .deb and .rpm also install `glisa-ctl`).
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("ctl") {
        std::process::exit(i32::from(ctl(&args[1..])));
    }
    #[cfg(target_os = "linux")]
    linux_webview_defaults();
    glisa_app::run();
}

fn ctl(argv: &[String]) -> u8 {
    #[cfg(windows)]
    attach_parent_console();
    glisa_i18n::set_language(None);
    let argv = argv.to_vec();
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let mut ask = glisa_ctl::ask_terminal;
    let mut io = glisa_ctl::Io {
        out: &mut out,
        err: &mut err,
        ask: &mut ask,
    };
    glisa_ctl::run(&argv, &mut io)
}

/// The app is a GUI program on Windows: print into the terminal it was started from.
#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: plain Win32 call; failing (no parent console) just leaves output unseen.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// WebKitGTK's GPU path (accelerated compositing, Skia on the GPU) can crash in Mesa's EGL
/// teardown when the window's web process exits, which happens every time the window is
/// closed. The window is a plain 2D interface: draw it on the CPU, which is just as smooth
/// here and uses less memory. `GLISA_GPU=1`, or setting WebKit's own variables, keeps the
/// GPU path.
#[cfg(target_os = "linux")]
fn linux_webview_defaults() {
    if std::env::var_os("GLISA_GPU").is_some() {
        return;
    }
    for (key, value) in [
        ("WEBKIT_DISABLE_COMPOSITING_MODE", "1"),
        ("WEBKIT_SKIA_ENABLE_CPU_RENDERING", "1"),
    ] {
        if std::env::var_os(key).is_none() {
            // SAFETY: the first thing main does, before any other thread exists.
            unsafe { std::env::set_var(key, value) };
        }
    }
}
