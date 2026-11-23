//! LOCAL NAMES: only this computer uses them (they never travel to another computer), so
//! they may follow a product rename. Kept in one place for that.

/// Folder of the Flow store inside the config directory.
pub const STORE_DIR: &str = "flow";
/// Folder of received transfers inside the cache directory.
pub const STAGING_DIR: &str = "flow-received";
/// D-Bus name, object and interface the KWin script calls (`callDBus`).
pub const DBUS_SERVICE: &str = "io.github.ignacioaranda.glisa.Flow";
pub const DBUS_PATH: &str = "/Flow";
pub const DBUS_INTERFACE: &str = "io.github.ignacioaranda.glisa.Flow1";
/// Under it, one object per drag picked up here, for KDE drops that copy without asking
/// (`linux/arkdrop.rs`): `<DBUS_DROP_PATH>/<random token>`.
pub const DBUS_DROP_PATH: &str = "/Flow/Drop";
/// Name KWin knows the script by (`loadScript(path, plugin)`).
pub const KWIN_PLUGIN: &str = "glisa-flow";
/// File name prefix of the script written to the runtime directory.
pub const KWIN_SCRIPT_PREFIX: &str = "glisa-flow";
/// Name of the virtual absolute pointer (uinput) that places the cursor.
pub const POINTER_NAME: &str = "Glisa Flow pointer";
/// X11 property our clipboard window receives selections in.
pub const X11_PROPERTY: &str = "GLISA_FLOW_SELECTION";
