//! KDE Plasma on Wayland.
//!
//! * Edges: a KWin script (`data/flow-kwin.js`, embedded) loaded at runtime through
//!   `org.kde.KWin /Scripting` (`loadScript(path, plugin)` then `/Scripting/Script<N>.run`),
//!   never installed. It reports transitions with `callDBus` to a D-Bus object this process
//!   exports itself (zbus, in-process: the Python version needed a Qt helper process for
//!   this). Every event is validated as strictly as network messages (anything on the
//!   session bus could call it). The script is unloaded when Flow stops, and reloaded after
//!   a KWin restart (checked every 5 s).
//! * Ctrl: `org_kde_kwin_keystate` (keystate.rs). Mouse buttons: evdev (input.rs).
//! * Placement: a uinput absolute pointer (input.rs).
//! * Clipboard: klipper over D-Bus for text when it runs; the Wayland data-control protocol
//!   for files and images (and for text without klipper), reconnected while Flow is on (the
//!   service may start before the Wayland session is ready, and KWin may restart).
//! * Files dragged to the edge: the edge catcher (catcher.rs), a layer-shell surface on its
//!   own Wayland connection, (re)connected when it is needed.
//! * Files dragged anywhere: the drag watch (dragwatch.rs) on the Logitech mouse's left
//!   button, with the catcher's probe; the glow is the catcher's too.
//! * Files dragged here from another computer: the pickup, on a second catcher connection
//!   (the only one with a pointer), confirmed by the mouse's evdev node. Its drag also
//!   offers KDE's way of copying without the Copy/Link menu: an `org.kde.ark.DndExtract`
//!   object of that drag on our D-Bus connection (arkdrop.rs).

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use glisa_i18n::n_;
use serde_json::Value;

use super::arkdrop::{self, ArkDrop, DndExtract};
use super::catcher::{Catcher, PickupExtra, PickupHooks};
use super::datacontrol::DataControl;
use super::dragwatch::{DragEvents, DragWatch};
use super::input::{AbsolutePointer, EvdevPointer, MouseButtons, PointerSource, Wake};
use super::keystate::KeyState;
use crate::backends::{Base, EdgeBackend, PickupNews, PickupStep, Sink, abs_value_for};
use crate::clipboard::RichClipboard;
use crate::geometry::{GlowEdge, Rect, bounding, clean_rects};
use crate::local::{
    DBUS_DROP_PATH, DBUS_INTERFACE, DBUS_PATH, DBUS_SERVICE, KWIN_PLUGIN, KWIN_SCRIPT_PREFIX,
    POINTER_NAME,
};
use crate::text::Text;
use crate::util::lock;

pub const TEMPLATE: &str = include_str!("../../data/flow-kwin.js");
const KLIPPER: (&str, &str, &str) = ("org.kde.klipper", "/klipper", "org.kde.klipper.klipper");

pub fn render_script(service: &str, path: &str, iface: &str, release_px: i64) -> String {
    let q = |s: &str| serde_json::to_string(s).expect("string");
    TEMPLATE
        .replace("@SERVICE@", &q(service))
        .replace("@PATH@", &q(path))
        .replace("@IFACE@", &q(iface))
        .replace("@RELEASE@", &release_px.to_string())
}

/// A validated event of the KWin script.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptEvent {
    Edge {
        side: &'static str,
        x: f64,
        y: f64,
        screen: Rect,
    },
    Leave {
        side: &'static str,
    },
    Screens(Vec<Rect>),
    Hello(Vec<Rect>),
}

fn num(v: &Value) -> Option<f64> {
    if v.is_boolean() {
        return None;
    }
    v.as_f64().filter(|f| f.abs() < (1 << 20) as f64)
}

fn rect(d: &Value) -> Option<Rect> {
    let o = d.as_object()?;
    if o.keys()
        .any(|k| !["x", "y", "w", "h", "name"].contains(&k.as_str()))
    {
        return None;
    }
    let get = |k: &str| o.get(k).and_then(num).map(|f| f.trunc() as i64);
    let r = [get("x")?, get("y")?, get("w")?, get("h")?];
    clean_rects(&[r]).first().copied()
}

/// Validate an event of the KWin script (anything on the session bus can call our object,
/// so this is as strict as the network protocol).
pub fn parse_event(payload: &str) -> Option<ScriptEvent> {
    if payload.len() > 16384 {
        return None;
    }
    let e: Value = serde_json::from_str(payload).ok()?;
    let o = e.as_object()?;
    if o.get("v").and_then(Value::as_i64) != Some(1) || o["v"].is_f64() {
        return None;
    }
    let keys = |allowed: &[&str]| o.keys().all(|k| allowed.contains(&k.as_str()));
    let exact = |want: &[&str]| o.len() == want.len() && keys(want);
    match o.get("type")?.as_str()? {
        "edge" => {
            if !exact(&["v", "type", "side", "x", "y", "screen"]) {
                return None;
            }
            let side = crate::side_of(o["side"].as_str()?)?;
            Some(ScriptEvent::Edge {
                side,
                x: num(&o["x"])?,
                y: num(&o["y"])?,
                screen: rect(&o["screen"])?,
            })
        }
        "leave" => {
            if !exact(&["v", "type", "side"]) {
                return None;
            }
            Some(ScriptEvent::Leave {
                side: crate::side_of(o["side"].as_str()?)?,
            })
        }
        t @ ("screens" | "hello") => {
            let allowed: &[&str] = if t == "hello" {
                &["v", "type", "screens", "cursor", "api"]
            } else {
                &["v", "type", "screens"]
            };
            if !keys(allowed) {
                return None;
            }
            let list = o.get("screens")?.as_array()?;
            let rects: Vec<Rect> = list.iter().take(16).filter_map(rect).collect();
            Some(if t == "hello" {
                ScriptEvent::Hello(rects)
            } else {
                ScriptEvent::Screens(rects)
            })
        }
        _ => None,
    }
}

/// What KWin's scripting interface needs from D-Bus (zbus here, a fake in tests).
pub trait ScriptBus: Send + Sync {
    fn is_loaded(&self, plugin: &str) -> Result<bool, String>;
    fn load(&self, path: &str, plugin: &str) -> Result<i32, String>;
    fn run(&self, id: i32) -> Result<(), String>;
    fn unload(&self, plugin: &str) -> Result<bool, String>;
}

/// Load / run / unload one script through org.kde.KWin /Scripting.
pub struct KWinScripting<B: ScriptBus> {
    pub bus: B,
    pub plugin: String,
    pub script_id: Option<i32>,
}

impl<B: ScriptBus> KWinScripting<B> {
    pub fn new(bus: B, plugin: &str) -> Self {
        Self {
            bus,
            plugin: plugin.to_string(),
            script_id: None,
        }
    }

    pub fn load(&mut self, path: &str) -> Result<i32, String> {
        if self.bus.is_loaded(&self.plugin)? {
            self.unload()?; // a leftover from a crash: replace it
        }
        let id = self.bus.load(path, &self.plugin)?;
        if id < 0 {
            return Err("KWin refused the script".into());
        }
        self.bus.run(id)?;
        self.script_id = Some(id);
        Ok(id)
    }

    pub fn unload(&mut self) -> Result<bool, String> {
        self.script_id = None;
        self.bus.unload(&self.plugin)
    }

    /// Reload after a KWin restart. True if it had to.
    pub fn ensure(&mut self, path: &str) -> Result<bool, String> {
        if self.script_id.is_some() && !self.bus.is_loaded(&self.plugin)? {
            self.script_id = None;
            self.load(path)?;
            return Ok(true);
        }
        Ok(false)
    }
}

/// [`ScriptBus`] on the session bus.
pub struct ZbusScripting(pub zbus::blocking::Connection);

const KWIN: &str = "org.kde.KWin";
const SCRIPTING: &str = "/Scripting";
const SCRIPTING_IFACE: &str = "org.kde.kwin.Scripting";

impl ZbusScripting {
    fn call<B, R>(&self, path: &str, iface: &str, method: &str, body: &B) -> Result<R, String>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    {
        let reply = self
            .0
            .call_method(Some(KWIN), path, Some(iface), method, body)
            .map_err(|e| e.to_string())?;
        reply.body().deserialize::<R>().map_err(|e| e.to_string())
    }
}

impl ScriptBus for ZbusScripting {
    fn is_loaded(&self, plugin: &str) -> Result<bool, String> {
        self.call(SCRIPTING, SCRIPTING_IFACE, "isScriptLoaded", &(plugin,))
    }
    fn load(&self, path: &str, plugin: &str) -> Result<i32, String> {
        self.call(SCRIPTING, SCRIPTING_IFACE, "loadScript", &(path, plugin))
    }
    fn run(&self, id: i32) -> Result<(), String> {
        self.0
            .call_method(
                Some(KWIN),
                format!("{SCRIPTING}/Script{id}").as_str(),
                Some("org.kde.kwin.Script"),
                "run",
                &(),
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    fn unload(&self, plugin: &str) -> Result<bool, String> {
        self.call(SCRIPTING, SCRIPTING_IFACE, "unloadScript", &(plugin,))
    }
}

/// The D-Bus object the script calls: `Event(json)`. The interface name below must be
/// `local::DBUS_INTERFACE` (the macro needs a literal; a test checks they agree).
struct FlowObject {
    on_event: Arc<dyn Fn(ScriptEvent) + Send + Sync>,
}

#[zbus::interface(name = "io.github.ignacioaranda.glisa.Flow1")]
impl FlowObject {
    #[zbus(name = "Event")]
    fn event(&self, payload: String) {
        if let Some(e) = parse_event(&payload) {
            (self.on_event)(e);
        }
    }
}

fn has_service(conn: &zbus::blocking::Connection, name: &str) -> bool {
    conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "NameHasOwner",
        &(name,),
    )
    .ok()
    .and_then(|r| r.body().deserialize::<bool>().ok())
    .unwrap_or(false)
}

/// The script file: private runtime directory, a new file we created (never a link).
fn write_script(text: &str) -> std::io::Result<PathBuf> {
    write_script_in(&glisa_common::paths::runtime_dir()?, text)
}

fn write_script_in(dir: &std::path::Path, text: &str) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = dir.join(format!("{KWIN_SCRIPT_PREFIX}-{}.js", std::process::id()));
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)?;
    f.write_all(text.as_bytes())?;
    Ok(path)
}

/// The service usually starts at login, sometimes before the Wayland session is ready, and
/// KWin may restart: keep (re)connecting the clipboard.
pub struct RichRetry {
    pub first: Duration,
    pub max: Duration,
    pub check_every: Duration,
}

impl Default for RichRetry {
    fn default() -> Self {
        Self {
            first: Duration::from_secs(2),
            max: Duration::from_secs(60),
            check_every: Duration::from_secs(5),
        }
    }
}

/// Makes a data-control client (tests pass fakes).
pub type RichFactory = Arc<dyn Fn() -> Option<Arc<DataControl>> + Send + Sync>;

pub struct KWinBackend {
    base: Arc<Base>,
    screens: Arc<Mutex<Vec<Rect>>>,
    bus: Mutex<Option<zbus::blocking::Connection>>,
    scripting: Arc<Mutex<Option<KWinScripting<ZbusScripting>>>>,
    script_path: Mutex<Option<PathBuf>>,
    klipper: Arc<AtomicBool>,
    keys: Mutex<Option<Arc<KeyState>>>,
    pointer: Mutex<Option<AbsolutePointer>>,
    buttons: MouseButtons,
    rich: Arc<Mutex<Option<Arc<DataControl>>>>,
    rich_factory: RichFactory,
    pub retry: Mutex<RichRetry>,
    /// Create the uinput absolute pointer (tests in a nested session don't).
    pub create_pointer: AtomicBool,
    catcher: Mutex<Option<Arc<Catcher>>>,
    /// When the catcher last couldn't connect (not retried for a while).
    catcher_failed: Mutex<Option<std::time::Instant>>,
    drag_watch: Mutex<Option<DragWatch>>,
    /// Tests: the next drag watch hears these events instead of the mouse.
    pub pointer_source: Mutex<Option<Box<dyn PointerSource>>>,
    pickup: Mutex<Option<Arc<Catcher>>>,
    pickup_failed: Mutex<Option<std::time::Instant>>,
    /// Tests: whether the press on the pickup is our mouse's (instead of its evdev node).
    pub pickup_confirm: Mutex<Option<Arc<dyn Fn() -> bool + Send + Sync>>>,
    /// The KDE drop of the pickup's drag (its D-Bus object is up while it is here).
    ark: Mutex<Option<Arc<ArkDrop>>>,
    stop: Arc<AtomicBool>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl Default for KWinBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl KWinBackend {
    pub fn new() -> Self {
        Self::with_rich(Arc::new(|| {
            let dc = Arc::new(DataControl::new());
            if dc.start(None) {
                Some(dc)
            } else {
                log::debug!(
                    "Flow: copied files and images can't be read yet ({})",
                    lock(&dc.error)
                );
                None
            }
        }))
    }

    pub fn with_rich(rich_factory: RichFactory) -> Self {
        Self {
            base: Arc::new(Base::new()),
            screens: Arc::default(),
            bus: Mutex::new(None),
            scripting: Arc::default(),
            script_path: Mutex::new(None),
            klipper: Arc::default(),
            keys: Mutex::new(None),
            pointer: Mutex::new(None),
            buttons: MouseButtons::default(),
            rich: Arc::default(),
            rich_factory,
            retry: Mutex::new(RichRetry::default()),
            create_pointer: AtomicBool::new(true),
            catcher: Mutex::new(None),
            catcher_failed: Mutex::new(None),
            drag_watch: Mutex::new(None),
            pointer_source: Mutex::new(None),
            pickup: Mutex::new(None),
            pickup_failed: Mutex::new(None),
            pickup_confirm: Mutex::new(None),
            ark: Mutex::new(None),
            stop: Arc::default(),
            threads: Mutex::new(Vec::new()),
        }
    }

    fn on_event(base: &Base, screens: &Mutex<Vec<Rect>>, e: ScriptEvent) {
        let Some(sink) = base.sink() else { return };
        match e {
            ScriptEvent::Hello(r) | ScriptEvent::Screens(r) if r.is_empty() => {
                if base.get_state().0 != "ok" {
                    base.set_state("ok", None);
                }
            }
            ScriptEvent::Hello(r) => {
                *lock(screens) = r.clone();
                sink.screens(r);
                if base.get_state().0 != "ok" {
                    base.set_state("ok", None);
                }
            }
            ScriptEvent::Screens(r) => {
                *lock(screens) = r.clone();
                sink.screens(r);
            }
            ScriptEvent::Edge { side, x, y, screen } => sink.edge(side, x, y, screen),
            ScriptEvent::Leave { side } => sink.leave(side),
        }
    }

    fn start_bus(&self) -> Result<(), Text> {
        let script = render_script(DBUS_SERVICE, DBUS_PATH, DBUS_INTERFACE, 32);
        let path = write_script(&script).map_err(|e| {
            Text::new(n_!("Couldn't start the Flow helper: {error}")).arg("error", e.to_string())
        })?;
        *lock(&self.script_path) = Some(path.clone());
        let (base, screens) = (self.base.clone(), self.screens.clone());
        let obj = FlowObject {
            on_event: Arc::new(move |e| Self::on_event(&base, &screens, e)),
        };
        let conn = zbus::blocking::connection::Builder::session()
            .and_then(|b| {
                b.method_timeout(Duration::from_secs(3))
                    .serve_at(DBUS_PATH, obj)
            })
            .and_then(|b| b.name(DBUS_SERVICE))
            .and_then(|b| b.build())
            .map_err(|e| match e {
                zbus::Error::NameTaken => {
                    Text::new(n_!("Another process is already using the Flow service"))
                }
                zbus::Error::Address(_) => Text::new(n_!("There is no D-Bus session bus")),
                e => Text::new(n_!("Couldn't start the Flow helper: {error}"))
                    .arg("error", e.to_string()),
            })?;
        self.klipper
            .store(has_service(&conn, KLIPPER.0), Ordering::SeqCst);
        let mut scripting = KWinScripting::new(ZbusScripting(conn.clone()), KWIN_PLUGIN);
        scripting.load(&path.to_string_lossy()).map_err(|e| {
            Text::new(n_!("KWin didn't load the Flow script ({error})")).arg("error", e)
        })?;
        *lock(&self.scripting) = Some(scripting);
        *lock(&self.bus) = Some(conn);
        self.base
            .set_state("starting", Some(Text::new(n_!("Waiting for KWin…")))); // until the script says hello
        Ok(())
    }

    fn connect_rich(&self, first: bool) -> bool {
        match (self.rich_factory)() {
            Some(dc) => {
                *lock(&self.rich) = Some(dc);
                if !first {
                    log::info!(
                        "Flow: clipboard connected; copied files and images can travel again"
                    );
                }
                true
            }
            None => false,
        }
    }

    fn spawn_watchers(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        let stop = self.stop.clone();
        // KWin restarted? klipper there?
        let watch = std::thread::Builder::new()
            .name("flow-kwin-watch".into())
            .spawn({
                let me = me.clone();
                let stop = stop.clone();
                move || {
                    while !stop.load(Ordering::SeqCst) {
                        for _ in 0..50 {
                            if stop.load(Ordering::SeqCst) {
                                return;
                            }
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        let Some(b) = me.upgrade() else { return };
                        let path = lock(&b.script_path).clone();
                        if let (Some(s), Some(path)) = (lock(&b.scripting).as_mut(), path) {
                            if let Err(e) = s.ensure(&path.to_string_lossy()) {
                                b.base.set_state(
                                    "error",
                                    Some(
                                        Text::new(n_!(
                                            "KWin didn't load the Flow script ({error})"
                                        ))
                                        .arg("error", e),
                                    ),
                                );
                            }
                        }
                        if let Some(conn) = lock(&b.bus).as_ref() {
                            b.klipper
                                .store(has_service(conn, KLIPPER.0), Ordering::SeqCst);
                        }
                    }
                }
            });
        let rich = std::thread::Builder::new()
            .name("flow-clipboard-connect".into())
            .spawn(move || {
                let first = me
                    .upgrade()
                    .map(|b| lock(&b.retry).first)
                    .unwrap_or_default();
                let mut delay = first;
                while !stop.load(Ordering::SeqCst) {
                    let Some(b) = me.upgrade() else { return };
                    let (check, max) = {
                        let r = lock(&b.retry);
                        (r.check_every, r.max)
                    };
                    let current = lock(&b.rich).clone();
                    let wait = if current.as_ref().is_some_and(|c| c.available()) {
                        delay = first;
                        check
                    } else {
                        if let Some(c) = current {
                            // the connection died (e.g. KWin restarted)
                            *lock(&b.rich) = None;
                            c.close();
                        }
                        if b.connect_rich(false) {
                            check
                        } else {
                            let d = delay;
                            delay = (delay * 2).min(max);
                            d
                        }
                    };
                    drop(b);
                    let end = std::time::Instant::now() + wait;
                    while std::time::Instant::now() < end {
                        if stop.load(Ordering::SeqCst) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(20).min(wait));
                    }
                }
            });
        lock(&self.threads).extend(watch.into_iter().chain(rich));
    }

    /// The edge catcher, connected (again) if needed; None when this compositor can't
    /// have one (tried again after a while).
    pub fn catcher(&self) -> Option<Arc<Catcher>> {
        let mut slot = lock(&self.catcher);
        if let Some(c) = slot.as_ref().filter(|c| c.available()) {
            return Some(c.clone());
        }
        if let Some(old) = slot.take() {
            old.close(); // the connection died (KWin restarted)
        }
        let mut failed = lock(&self.catcher_failed);
        if failed.is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
            return None;
        }
        let base = self.base.clone();
        let c = Arc::new(Catcher::new(Arc::new(move |side, paths| {
            if let Some(sink) = base.sink() {
                sink.file_drag(side, paths);
            }
        })));
        if c.start(None) {
            *failed = None;
            *slot = Some(c.clone());
            Some(c)
        } else {
            log::info!(
                "Flow: files can't be dragged across the edge here ({})",
                lock(&c.error)
            );
            *failed = Some(std::time::Instant::now());
            None
        }
    }

    /// The pickup's connection, connected (again) if needed; None when this compositor
    /// can't have one (tried again after a while).
    pub fn pickup(self: &Arc<Self>) -> Option<Arc<Catcher>> {
        let mut slot = lock(&self.pickup);
        if let Some(c) = slot.as_ref().filter(|c| c.available()) {
            return Some(c.clone());
        }
        if let Some(old) = slot.take() {
            old.close();
        }
        let mut failed = lock(&self.pickup_failed);
        if failed.is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
            return None;
        }
        let me = Arc::downgrade(self);
        let confirm: Arc<dyn Fn() -> bool + Send + Sync> = match lock(&self.pickup_confirm).clone()
        {
            Some(f) => f,
            None => {
                let me = me.clone();
                Arc::new(move || {
                    me.upgrade()
                        .is_some_and(|b| b.buttons.left_held_now() == Some(true))
                })
            }
        };
        let c = Arc::new(Catcher::for_pickup(PickupHooks {
            confirm,
            news: Arc::new(move |n| {
                if let Some(b) = me.upgrade() {
                    b.pickup_news(n);
                }
            }),
        }));
        if c.start(None) && c.pickup_possible() {
            *failed = None;
            *slot = Some(c.clone());
            Some(c)
        } else {
            log::info!(
                "Flow: drags from another computer can't go on here ({})",
                lock(&c.error)
            );
            c.close();
            *failed = Some(std::time::Instant::now());
            None
        }
    }

    /// How the pickup's drag went, for the engine. After a drop whose target read the KDE
    /// offer, the copy it asked for decides (on a thread: the target calls right after the
    /// drop, or during it).
    fn pickup_news(self: &Arc<Self>, n: PickupNews) {
        let tell = |b: &Self, n| {
            if let Some(s) = b.base.sink() {
                s.pickup(n);
            }
        };
        let PickupNews::Ended { taken } = n else {
            tell(self, n);
            return;
        };
        let ark = lock(&self.ark).clone().filter(|a| a.offered());
        let Some(a) = ark else {
            self.retire_ark(None);
            tell(self, n);
            return;
        };
        let me = Arc::downgrade(self);
        let a2 = a.clone();
        let spawned = std::thread::Builder::new()
            .name("flow-ark-drop".into())
            .spawn(move || {
                let taken = a2.taken_after_drop(taken, arkdrop::CALL_WAIT, arkdrop::OUTCOME_WAIT);
                // Unless the pickup was cancelled or replaced meanwhile (the engine has
                // moved on then).
                if let Some(b) = me.upgrade().filter(|b| b.retire_ark(Some(&a2))) {
                    tell(&b, PickupNews::Ended { taken });
                }
            });
        if spawned.is_err() {
            self.retire_ark(Some(&a));
            tell(self, n);
        }
    }

    /// A new drag may be picked up: its KDE drop object goes up on our D-Bus connection,
    /// and the drag will offer where it is. Nothing extra without D-Bus.
    fn new_ark(&self) -> PickupExtra {
        self.retire_ark(None);
        let Some(conn) = lock(&self.bus).clone() else {
            return PickupExtra::default();
        };
        let Some(service) = conn.unique_name().map(|n| n.to_string()) else {
            return PickupExtra::default();
        };
        let drop = ArkDrop::new(DBUS_DROP_PATH);
        match conn
            .object_server()
            .at(drop.path(), DndExtract(drop.clone()))
        {
            Ok(true) => {}
            other => {
                log::info!("Flow: pickup: KDE's copy without asking isn't offered ({other:?})");
                return PickupExtra::default();
            }
        }
        *lock(&self.ark) = Some(drop.clone());
        let d2 = drop.clone();
        PickupExtra {
            offers: vec![
                (arkdrop::SERVICE_MIME.into(), service.into_bytes()),
                (arkdrop::PATH_MIME.into(), drop.path().as_bytes().to_vec()),
            ],
            on_send: Some(Arc::new(move |m| d2.sent(m))),
        }
    }

    /// The KDE drop (`which`, or whichever is here) is over: a call still waiting for the
    /// files gives up, and its object goes unless a call took it (it went then). False if
    /// `which` isn't the one here any more.
    fn retire_ark(&self, which: Option<&Arc<ArkDrop>>) -> bool {
        let a = {
            let mut slot = lock(&self.ark);
            match (slot.as_ref(), which) {
                (None, _) => return false,
                (Some(cur), Some(w)) if !Arc::ptr_eq(cur, w) => return false,
                _ => slot.take(),
            }
        };
        let Some(a) = a else { return false };
        a.retire();
        if !a.claimed() {
            if let Some(conn) = lock(&self.bus).clone() {
                let _ = conn.object_server().remove::<DndExtract, _>(a.path());
            }
        }
        true
    }

    fn klipper_call(&self, method: &str, body: Option<&str>) -> Option<zbus::Message> {
        let bus = lock(&self.bus).clone()?;
        match body {
            Some(t) => bus
                .call_method(Some(KLIPPER.0), KLIPPER.1, Some(KLIPPER.2), method, &(t,))
                .ok(),
            None => bus
                .call_method(Some(KLIPPER.0), KLIPPER.1, Some(KLIPPER.2), method, &())
                .ok(),
        }
    }

    fn start_arc(self: &Arc<Self>, sink: Arc<dyn Sink>) {
        self.base.set_sink(Some(sink));
        self.stop.store(false, Ordering::SeqCst);
        self.base
            .set_state("starting", Some(Text::new(n_!("Connecting to KWin…"))));
        if let Err(e) = self.start_bus() {
            self.base.set_state("error", Some(e));
        }
        let me = self.clone();
        let keys = Arc::new(KeyState::new());
        let ok = keys.start(
            None,
            Arc::new(move |held| {
                if let Some(s) = me.base.sink() {
                    s.ctrl(held);
                }
            }),
        );
        if !ok {
            log::info!("Flow: Ctrl not available ({})", lock(&keys.error));
        }
        *lock(&self.keys) = Some(keys);
        if self.create_pointer.load(Ordering::SeqCst) {
            match AbsolutePointer::new(POINTER_NAME) {
                Ok(p) => *lock(&self.pointer) = Some(p),
                Err(e) => log::warn!("Flow: can't create the absolute pointer: /dev/uinput: {e}"),
            }
        }
        if !self.connect_rich(true) {
            log::info!("Flow: copied files and images can't be read yet; will retry");
        }
        *lock(&self.catcher_failed) = None;
        let _ = self.catcher(); // ready before the first drag
        self.spawn_watchers();
    }
}

/// The drag watch's questions and news, through the backend: the engine (sink) decides
/// whether to look and what glows; the catcher probes.
struct WatchEvents(Weak<KWinBackend>);

impl DragEvents for WatchEvents {
    fn wanted(&self) -> bool {
        self.0
            .upgrade()
            .and_then(|b| b.base.sink())
            .is_some_and(|s| s.drag_glow_wanted())
    }

    fn probe(&self, window: Duration) -> Option<bool> {
        self.0.upgrade()?.catcher()?.probe(window)
    }

    fn dragging(&self, on: bool) {
        if let Some(s) = self.0.upgrade().and_then(|b| b.base.sink()) {
            s.files_dragging(on);
        }
    }
}

/// The backend is shared (`Arc`) so its threads can reach it; the trait is implemented on
/// a thin handle.
pub struct KWinHandle(pub Arc<KWinBackend>);

impl KWinHandle {
    pub fn new() -> Self {
        Self(Arc::new(KWinBackend::new()))
    }
}

impl Default for KWinHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgeBackend for KWinHandle {
    fn kind(&self) -> &'static str {
        "kwin"
    }

    fn start(&self, sink: Arc<dyn Sink>) {
        self.0.start_arc(sink);
    }

    fn stop(&self) {
        let b = &self.0;
        b.stop.store(true, Ordering::SeqCst);
        let watch = lock(&b.drag_watch).take();
        if let Some(w) = watch {
            w.stop();
        }
        log::debug!("Flow: KWin backend: unloading the script");
        if let Some(mut s) = lock(&b.scripting).take() {
            let _ = s.unload();
        }
        b.retire_ark(None);
        log::debug!("Flow: KWin backend: releasing D-Bus");
        lock(&b.bus).take(); // releases the name and the objects
        if let Some(k) = lock(&b.keys).take() {
            k.close();
        }
        lock(&b.pointer).take();
        log::debug!("Flow: KWin backend: waiting for its threads");
        for t in lock(&b.threads).drain(..).collect::<Vec<_>>() {
            let name = t.thread().name().unwrap_or("?").to_string();
            let _ = t.join();
            log::debug!("Flow: KWin backend: thread {name} ended");
        }
        if let Some(r) = lock(&b.rich).take() {
            r.close();
        }
        if let Some(c) = lock(&b.catcher).take() {
            c.close();
        }
        if let Some(c) = lock(&b.pickup).take() {
            c.close();
        }
        if let Some(p) = lock(&b.script_path).take() {
            let _ = std::fs::remove_file(p);
        }
        b.base.set_state("off", None);
        b.base.set_sink(None);
    }

    fn state(&self) -> (String, Option<Text>) {
        self.0.base.get_state()
    }

    fn screens(&self) -> Vec<Rect> {
        lock(&self.0.screens).clone()
    }

    fn ctrl_held(&self) -> Option<bool> {
        lock(&self.0.keys)
            .as_ref()
            .filter(|k| k.available())
            .and_then(|k| k.ctrl_held())
    }

    fn ctrl_available(&self) -> bool {
        lock(&self.0.keys).as_ref().is_some_and(|k| k.available())
    }

    fn buttons_held(&self) -> Option<bool> {
        self.0.buttons.held()
    }

    fn clipboard_kind(&self) -> &'static str {
        if self.0.klipper.load(Ordering::SeqCst) && lock(&self.0.bus).is_some() {
            "klipper"
        } else if lock(&self.0.rich).as_ref().is_some_and(|r| r.available()) {
            "data-control"
        } else {
            "none"
        }
    }

    fn clipboard_get(&self, timeout: Duration) -> Option<String> {
        if self.clipboard_kind() == "klipper" {
            return self
                .0
                .klipper_call("getClipboardContents", None)
                .and_then(|m| m.body().deserialize::<String>().ok());
        }
        lock(&self.0.rich).clone().and_then(|r| r.text(timeout))
    }

    fn clipboard_set(&self, text: &str) -> bool {
        if self.clipboard_kind() == "klipper" {
            return self
                .0
                .klipper_call("setClipboardContents", Some(text))
                .is_some();
        }
        lock(&self.0.rich).clone().is_some_and(|r| r.set_text(text))
    }

    fn can_place(&self) -> bool {
        lock(&self.0.pointer).is_some()
    }

    fn place_cursor(&self, x: i64, y: i64) -> bool {
        // KWin maps the absolute range onto the size of the bounding box of all outputs,
        // starting at the global origin (0, 0).
        let [bx, by, bw, bh] = bounding(&lock(&self.0.screens));
        let p = lock(&self.0.pointer);
        let Some(p) = p.as_ref() else { return false };
        p.move_to(
            abs_value_for(x as f64, (bx + bw) as f64, 0.0),
            abs_value_for(y as f64, (by + bh) as f64, 0.0),
        )
        .is_ok()
    }

    fn rich(&self) -> Option<Arc<dyn RichClipboard>> {
        lock(&self.0.rich)
            .clone()
            .map(|r| r as Arc<dyn RichClipboard>)
    }

    fn drag_catcher_available(&self) -> bool {
        self.0.catcher().is_some()
    }

    fn drag_catcher(&self, at: Option<(&'static str, Rect)>) {
        match at {
            Some((side, rect)) => {
                if let Some(c) = self.0.catcher() {
                    c.show(side, rect);
                }
            }
            None => {
                let c = lock(&self.0.catcher).clone();
                if let Some(c) = c {
                    c.hide();
                }
            }
        }
    }

    fn drag_accept(&self, accept: bool) -> bool {
        let c = lock(&self.0.catcher).clone();
        c.is_some_and(|c| c.accept(accept))
    }

    fn drag_watch(&self, on: bool) {
        let b = &self.0;
        let mut slot = lock(&b.drag_watch);
        if !on {
            let w = slot.take();
            drop(slot);
            if let Some(w) = w {
                w.stop();
            }
            return;
        }
        if slot.is_some() || b.stop.load(Ordering::SeqCst) || b.catcher().is_none() {
            return;
        }
        let test_source = lock(&b.pointer_source).take();
        let (source, wake): (Box<dyn PointerSource>, Option<Wake>) = match test_source {
            Some(s) => (s, None),
            None => match Wake::new() {
                Ok(w) => (
                    Box::new(EvdevPointer::new(
                        PathBuf::from("/sys/class/input"),
                        w.clone(),
                    )),
                    Some(w),
                ),
                Err(e) => {
                    log::info!("Flow: files dragged anywhere can't be watched: {e}");
                    return;
                }
            },
        };
        *slot = Some(DragWatch::start(
            source,
            wake,
            Arc::new(WatchEvents(Arc::downgrade(b))),
        ));
    }

    fn drag_pickup_available(&self) -> bool {
        self.0.pickup().is_some()
    }

    fn drag_pickup(&self, step: PickupStep) {
        let c = match step {
            PickupStep::Expect { .. } => self.0.pickup(),
            _ => lock(&self.0.pickup).clone(),
        };
        let Some(c) = c else { return };
        match step {
            PickupStep::Expect { output, count } => {
                if c.pickup_dragging() {
                    log::info!("Flow: the pickup couldn't be put up (a drag is going on)");
                    return;
                }
                if !c.pickup_expect(output, count, self.0.new_ark()) {
                    log::info!("Flow: the pickup couldn't be put up");
                    self.0.retire_ark(None);
                }
            }
            PickupStep::Arrived => c.pickup_arrived(),
            PickupStep::Ready(paths) => {
                let uris: String = paths
                    .iter()
                    .map(|p| crate::transfer::uri_of(p) + "\r\n")
                    .collect();
                if let Some(a) = lock(&self.0.ark).clone() {
                    a.ready(paths);
                }
                c.pickup_ready(uris.into_bytes());
            }
            PickupStep::Cancel => {
                c.pickup_cancel();
                self.0.retire_ark(None);
            }
        }
    }

    fn drag_glow(&self, edges: &[GlowEdge]) {
        let c = if edges.is_empty() {
            lock(&self.0.catcher).clone()
        } else {
            self.0.catcher()
        };
        if let Some(c) = c {
            c.glow(edges);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Default)]
    struct FakeBus {
        calls: Mutex<Vec<(String, String)>>,
        loaded: Mutex<bool>,
    }

    impl ScriptBus for &FakeBus {
        fn is_loaded(&self, _p: &str) -> Result<bool, String> {
            self.calls
                .lock()
                .unwrap()
                .push(("/Scripting".into(), "isScriptLoaded".into()));
            Ok(*self.loaded.lock().unwrap())
        }
        fn load(&self, path: &str, plugin: &str) -> Result<i32, String> {
            assert_eq!((path, plugin), ("/run/x.js", "glisa-flow"));
            self.calls
                .lock()
                .unwrap()
                .push(("/Scripting".into(), "loadScript".into()));
            *self.loaded.lock().unwrap() = true;
            Ok(3)
        }
        fn run(&self, id: i32) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push((format!("/Scripting/Script{id}"), "run".into()));
            Ok(())
        }
        fn unload(&self, _p: &str) -> Result<bool, String> {
            self.calls
                .lock()
                .unwrap()
                .push(("/Scripting".into(), "unloadScript".into()));
            let mut l = self.loaded.lock().unwrap();
            let was = *l;
            *l = false;
            Ok(was)
        }
    }

    #[test]
    fn script_loader() {
        let bus = FakeBus::default();
        *bus.loaded.lock().unwrap() = true; // a leftover from a crash
        let mut k = KWinScripting::new(&bus, "glisa-flow");
        assert_eq!(k.load("/run/x.js"), Ok(3));
        let calls: Vec<(String, String)> = bus.calls.lock().unwrap().clone();
        let want: Vec<(String, String)> = [
            ("/Scripting", "isScriptLoaded"),
            ("/Scripting", "unloadScript"),
            ("/Scripting", "loadScript"),
            ("/Scripting/Script3", "run"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        assert_eq!(calls, want);
        assert_eq!(k.ensure("/run/x.js"), Ok(false));
        *bus.loaded.lock().unwrap() = false; // KWin restarted
        assert_eq!(k.ensure("/run/x.js"), Ok(true));
        assert_eq!(k.unload(), Ok(true));
        assert!(!*bus.loaded.lock().unwrap());
    }

    #[test]
    fn events() {
        let e = parse_event(
            &json!({"v": 1, "type": "edge", "side": "left", "x": 0, "y": 5,
                                    "screen": {"x": 0, "y": 0, "w": 100, "h": 50}})
            .to_string(),
        );
        assert_eq!(
            e,
            Some(ScriptEvent::Edge {
                side: "left",
                x: 0.0,
                y: 5.0,
                screen: [0, 0, 100, 50]
            })
        );
        assert_eq!(
            parse_event(r#"{"v":1,"type":"leave","side":"top"}"#),
            Some(ScriptEvent::Leave { side: "top" })
        );
        let huge = "x".repeat(20000);
        for bad in [
            r#"{"v":1,"type":"edge","side":"up","x":0,"y":0,"screen":{"x":0,"y":0,"w":1,"h":1}}"#,
            r#"{"v":1,"type":"leave","side":"top","x":1}"#,
            r#"{"v":2,"type":"leave","side":"top"}"#,
            r#"{"v":1,"type":"edge","side":"left","x":"0","y":0,"screen":{}}"#,
            "nope",
            "[]",
            &huge,
        ] {
            assert!(parse_event(bad).is_none(), "{bad}");
        }
        let deep = format!("{}{}", "[".repeat(16000), "]".repeat(16000));
        assert!(parse_event(&deep).is_none());
        let hello = json!({"v": 1, "type": "hello", "cursor": {"x": 1, "y": 2},
                           "screens": [{"x": 0, "y": 0, "w": 1920, "h": 1080}], "api": {"screens": true}});
        assert_eq!(
            parse_event(&hello.to_string()),
            Some(ScriptEvent::Hello(vec![[0, 0, 1920, 1080]]))
        );
    }

    #[test]
    fn clipboard_reconnects() {
        // At login the service can start before the Wayland session is ready, and KWin can
        // restart: the data-control clipboard keeps (re)connecting (no D-Bus, no uinput here).
        use std::os::unix::net::UnixStream;
        let attempts = Arc::new(Mutex::new(0));
        let peers: Arc<Mutex<Vec<UnixStream>>> = Arc::default();
        let (a2, p2) = (attempts.clone(), peers.clone());
        let b = Arc::new(KWinBackend::with_rich(Arc::new(move || {
            let mut n = a2.lock().unwrap();
            *n += 1;
            if *n <= 2 {
                return None; // no Wayland session yet
            }
            let (a, b) = UnixStream::pair().unwrap();
            // a compositor that only answers the registry round trip with data-control
            let comp = b.try_clone().unwrap();
            std::thread::spawn(move || {
                let mut w = super::super::wayland::Wire::new(comp);
                while let Ok(msgs) = w.read(Duration::from_millis(20)) {
                    for m in msgs {
                        let r = super::super::wayland::read_u32(&m.body, 0).unwrap_or(0);
                        let send = |obj: u32, op: u16, p: &[u8]| {
                            let _ = (&w.sock)
                                .write_all(&crate::linux::keystate::tests::msg(obj, op, p));
                        };
                        if m.object == 1 && m.opcode == 1 {
                            send(
                                r,
                                0,
                                &[
                                    &7u32.to_ne_bytes()[..],
                                    &super::super::wayland::string("wl_seat"),
                                    &9u32.to_ne_bytes(),
                                ]
                                .concat(),
                            );
                            send(
                                r,
                                0,
                                &[
                                    &8u32.to_ne_bytes()[..],
                                    &super::super::wayland::string("ext_data_control_manager_v1"),
                                    &1u32.to_ne_bytes(),
                                ]
                                .concat(),
                            );
                        } else if m.object == 1 && m.opcode == 0 {
                            send(r, 0, &1u32.to_ne_bytes());
                        }
                    }
                }
            });
            p2.lock().unwrap().push(b);
            let dc = Arc::new(DataControl::new());
            dc.start(Some(a)).then_some(dc)
        })));
        *b.retry.lock().unwrap() = RichRetry {
            first: Duration::from_millis(50),
            max: Duration::from_millis(100),
            check_every: Duration::from_millis(50),
        };
        assert!(!b.connect_rich(true)); // first try failed
        b.spawn_watchers();
        let wait = |f: &dyn Fn() -> bool| {
            (0..150).any(|_| {
                f() || {
                    std::thread::sleep(Duration::from_millis(20));
                    false
                }
            })
        };
        assert!(wait(&|| b
            .rich
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|r| r.available())));
        let first = b.rich.lock().unwrap().clone().unwrap();
        for p in peers.lock().unwrap().iter() {
            let _ = p.shutdown(std::net::Shutdown::Both); // KWin restarted
        }
        assert!(wait(&|| b
            .rich
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|r| r.available() && !Arc::ptr_eq(r, &first))));
        KWinHandle(b.clone()).stop();
        let n = *attempts.lock().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(*attempts.lock().unwrap(), n); // no retries after stop()
        assert!(b.rich.lock().unwrap().is_none());
    }

    #[test]
    fn interface_name_is_the_local_one() {
        use zbus::object_server::Interface;
        assert_eq!(FlowObject::name().as_str(), DBUS_INTERFACE);
        assert_eq!(DndExtract::name().as_str(), arkdrop::INTERFACE);
    }

    #[test]
    fn render() {
        let src = render_script("a.b.C", "/P", "a.b.I", 40);
        for p in ["@SERVICE@", "@PATH@", "@IFACE@", "@RELEASE@"] {
            assert!(!src.contains(p));
        }
        assert!(src.contains("var SERVICE = \"a.b.C\";"));
        assert!(src.contains("var RELEASE = 40;"));
    }

    #[test]
    fn script_behaves_like_the_tracker() {
        // Run the real KWin script in node with a fake `workspace` and compare its events
        // with geometry::EdgeTracker.
        if std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("node not available: skipped");
            return;
        }
        let screens: [Rect; 2] = [[0, 0, 1920, 1080], [1920, 0, 1280, 1024]];
        let moves = [
            (900, 500, 0),
            (1919, 500, 0),
            (3199, 500, 0),
            (3199, 540, 0),
            (3150, 540, 0),
            (3199, 600, 1),
            (3100, 600, 0),
            (0, 300, 0),
            (0, 300, 0),
            (0, 350, 0),
            (2500, 1023, 0),
            (2500, 990, 0),
            (1500, 1079, 0),
            (1500, 1000, 0),
        ];
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("flow.js"),
            render_script("s", "/p", "i", 32),
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("in.json"),
            json!({"screens": screens, "moves": moves}).to_string(),
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("run.js"),
            r#"
const fs = require('fs'), vm = require('vm');
const src = fs.readFileSync(process.argv[2], 'utf8');
const input = JSON.parse(fs.readFileSync(process.argv[3], 'utf8'));
const out = []; let handler = null;
const ws = {cursorPos: {x: 0, y: 0}, activeWindow: {move: false, resize: false},
  screens: input.screens.map(r => ({geometry: {x: r[0], y: r[1], width: r[2], height: r[3]}})),
  cursorPosChanged: {connect: f => { handler = f; }}, screensChanged: {connect: f => {}}};
const ctx = {workspace: ws, callDBus: (s, p, i, m, payload) => out.push(JSON.parse(payload)),
  JSON: JSON, Math: Math, Date: {now: () => 1000}};
vm.createContext(ctx); vm.runInContext(src, ctx);
for (const [x, y, moving] of input.moves) { ws.cursorPos = {x: x, y: y}; ws.activeWindow.move = !!moving; handler(); }
console.log(JSON.stringify(out));
"#,
        )
        .unwrap();
        let out = std::process::Command::new("node")
            .arg(tmp.path().join("run.js"))
            .arg(tmp.path().join("flow.js"))
            .arg(tmp.path().join("in.json"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let js: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(js[0]["type"], "hello");
        for e in &js[1..] {
            assert!(parse_event(&e.to_string()).is_some(), "{e}");
        }
        let js_events: Vec<String> = js[1..]
            .iter()
            .map(|e| match e["type"].as_str().unwrap() {
                "edge" => format!("edge {} {} {}", e["side"].as_str().unwrap(), e["x"], e["y"]),
                t => format!("{t} {}", e["side"].as_str().unwrap()),
            })
            .collect();
        let mut tr = crate::geometry::EdgeTracker::new(32.0);
        let mut rs_events = Vec::new();
        for (x, y, moving) in moves {
            for ev in tr.feed(x as f64, y as f64, &screens, 1000.0, moving == 1) {
                rs_events.push(match ev {
                    crate::geometry::EdgeEvent::Edge { side, x, y, .. } => {
                        format!("edge {side} {x} {y}")
                    }
                    crate::geometry::EdgeEvent::Leave { side } => format!("leave {side}"),
                });
            }
        }
        assert_eq!(js_events, rs_events);
        assert!(js_events.contains(&"edge right 3199 500".to_string()));
        assert!(!js_events.contains(&"edge right 1919 500".to_string())); // inner edge
    }

    #[test]
    fn script_file_is_private_and_never_follows_a_link() {
        let run = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(run.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let victim = run.path().join("victim.txt");
        std::fs::write(&victim, "keep me").unwrap();
        let planted = run
            .path()
            .join(format!("{KWIN_SCRIPT_PREFIX}-{}.js", std::process::id()));
        std::os::unix::fs::symlink(&victim, &planted).unwrap();
        let path = write_script_in(run.path(), "// script").unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        let meta = std::fs::symlink_metadata(&path).unwrap();
        assert!(meta.file_type().is_file());
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }
}
