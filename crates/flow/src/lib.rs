//! Flow: move the mouse (and keyboard) between computers that run Glisa by pushing the
//! pointer against a screen edge, with the clipboard text, copied files, folders and images
//! following it.
//!
//! The protocol and the security model are in `docs/FLOW.md`. Layout of the crate:
//!
//! ```text
//!     crypto ...... identity (ECDSA P-256, rcgen), SPKI pins, commitments, SAS
//!     tls ......... rustls configurations: TLS 1.3 only, pinned custom verifiers, no tickets
//!     store ....... <config>/flow/ (identity, peers, options), 0600 files in a 0700 folder
//!     proto ....... framed, versioned, schema-checked messages with sequence numbers
//!     transfer .... copied files and images: manifest, staging folder, sending and receiving
//!     names ....... received file names made valid and unique on this computer's file system
//!     imageinfo ... image type and size from the header bytes (never decodes)
//!     clipboard ... the whole clipboard (MIME types): fake, Wayland data-control, X11, …
//!     drag ........ files dragged across the edge: URI lists, the notice
//!     discovery ... UDP announcements (address hints only): broadcast + directed to paired peers
//!     address ..... host[:port] typed by the user: validation and formatting
//!     geometry .... screen geometry, edge tracking, entry points
//!     backends .... where the pointer is: KWin script, X11, Windows, macOS, fake (tests)
//!     service ..... FlowService: the `flow_*` API, TLS server and links, pairing, switching
//!     text ........ notices kept untranslated until a client reads them (per-client language)
//! ```

pub mod address;
pub mod backends;
pub mod clipboard;
pub mod crypto;
pub mod dib;
pub mod discovery;
pub mod drag;
pub mod geometry;
pub mod imageinfo;
pub mod local;
pub mod names;
pub mod proto;
pub mod service;
pub mod store;
pub mod text;
pub mod tls;
pub mod transfer;
pub(crate) mod util;
pub mod wire;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod windows;

pub use service::{BackendChoice, FlowOptions, FlowService, NetConfig, SendReason, free_ports};

pub use wire::{CAP_DRAG, CAP_TRANSFER, PAIR_PORT_OFFSET, PORT, PROTOCOL_VERSION};

pub const SIDES: [&str; 4] = ["left", "right", "top", "bottom"];
pub const POINTER_KINDS: [&str; 3] = ["mouse", "trackball", "trackpad"];
/// Bytes of UTF-8 clipboard text.
pub const CLIPBOARD_MAX: usize = 1 << 20;
/// What this computer announces in `hello.caps`. Only the Linux build takes `drag` notices
/// (on KDE Plasma the drag goes on there; the other systems get them as copied files).
#[cfg(target_os = "linux")]
pub const CAPS: &[&str] = &[CAP_TRANSFER, CAP_DRAG];
#[cfg(not(target_os = "linux"))]
pub const CAPS: &[&str] = &[CAP_TRANSFER];
/// Entries (files + folders) in one transfer.
pub const MAX_FILES: usize = 10_000;
/// Nesting of folders.
pub const MAX_DEPTH: usize = 32;
/// Bytes of UTF-8 of a relative path.
pub const MAX_PATH: usize = 4096;
/// Bytes of UTF-8 of one path component.
pub const MAX_NAME: usize = 255;
/// Image types a transfer may carry, preferred first.
pub const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/bmp"];
pub const PAIRING_SECONDS: f64 = 120.0;

pub fn pair_port(port: u16) -> u16 {
    port.wrapping_add(PAIR_PORT_OFFSET)
}

/// The side opposite to `side` ("left" ↔ "right", "top" ↔ "bottom").
pub fn opposite(side: &str) -> &'static str {
    match side {
        "left" => "right",
        "right" => "left",
        "top" => "bottom",
        _ => "top",
    }
}

/// `side` as one of [`SIDES`] (a `&'static str`), or None.
pub fn side_of(side: &str) -> Option<&'static str> {
    SIDES.iter().copied().find(|s| *s == side)
}
