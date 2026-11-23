//! macOS desktop: the pointer polled at ~125 Hz (`CGEventCreate` + `CGEventGetLocation`),
//! Ctrl from `CGEventSourceFlagsState`, mouse buttons from `CGEventSourceButtonState`,
//! displays from `CGGetActiveDisplayList` / `CGDisplayBounds` (global coordinates, points),
//! `CGWarpMouseCursorPosition` (+ `CGAssociateMouseAndMouseCursorPosition`) for placement,
//! and NSPasteboard translated to MIME types at the edge:
//!
//! | MIME type | NSPasteboard |
//! |---|---|
//! | `text/plain;charset=utf-8`, `text/plain` | `public.utf8-plain-text` |
//! | `text/uri-list` | the items' `public.file-url` (written back as file URLs) |
//! | `image/png` | `public.png`, else `public.tiff` converted with NSBitmapImageRep |
//! | `image/jpeg` | `public.jpeg` |
//!
//! Received files get the `com.apple.quarantine` attribute, as AirDrop's do.
//!
//! NOT COMPILED on the Linux machine where this was written (no macOS SDK): it uses only
//! documented C functions and Objective-C messages, through `objc2::msg_send!`.

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};

use crate::backends::{Base, EdgeBackend, Sink, report};
use crate::clipboard::{RichClipboard, TooLarge, copy_out};
use crate::geometry::{EdgeTracker, Rect, clean_rects, now_s};
use crate::text::Text;
use crate::transfer::{ClipData, Offers, parse_uri_list};
use crate::util::lock;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

const COMBINED_SESSION_STATE: i32 = 0; // kCGEventSourceStateCombinedSessionState
const FLAG_MASK_CONTROL: u64 = 1 << 18; // kCGEventFlagMaskControl
const BUTTON_LEFT: u32 = 0;
const BUTTON_RIGHT: u32 = 1;
const BUTTON_CENTER: u32 = 2;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreate(source: *const c_void) -> *mut c_void;
    fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
    fn CGEventSourceFlagsState(state: i32) -> u64;
    fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
    fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayBounds(display: u32) -> CGRect;
    fn CGWarpMouseCursorPosition(p: CGPoint) -> i32;
    fn CGAssociateMouseAndMouseCursorPosition(connected: i32) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

// NSPasteboard, NSBitmapImageRep, NSURL
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

fn location() -> Option<(f64, f64)> {
    // SAFETY: a NULL source is allowed; the event is released below.
    unsafe {
        let ev = CGEventCreate(std::ptr::null());
        if ev.is_null() {
            return None;
        }
        let p = CGEventGetLocation(ev);
        CFRelease(ev);
        Some((p.x, p.y))
    }
}

fn displays() -> Vec<Rect> {
    let mut ids = [0u32; 16];
    let mut n = 0u32;
    // SAFETY: room for 16 ids.
    if unsafe { CGGetActiveDisplayList(16, ids.as_mut_ptr(), &mut n) } != 0 {
        return Vec::new();
    }
    let rects: Vec<Rect> = ids[..n.min(16) as usize]
        .iter()
        .map(|id| {
            // SAFETY: an active display id.
            let r = unsafe { CGDisplayBounds(*id) };
            [
                r.origin.x as i64,
                r.origin.y as i64,
                r.size.width as i64,
                r.size.height as i64,
            ]
        })
        .collect();
    clean_rects(&rects)
}

fn ctrl_now() -> bool {
    // SAFETY: no preconditions.
    unsafe { CGEventSourceFlagsState(COMBINED_SESSION_STATE) & FLAG_MASK_CONTROL != 0 }
}

fn buttons_now() -> bool {
    // SAFETY: no preconditions.
    [BUTTON_LEFT, BUTTON_RIGHT, BUTTON_CENTER]
        .iter()
        .any(|b| unsafe { CGEventSourceButtonState(COMBINED_SESSION_STATE, *b) })
}

/// Received files are marked like AirDrop's (Gatekeeper asks before opening them).
pub fn quarantine(f: &std::fs::File) {
    use std::os::fd::AsRawFd;
    let value = format!("0083;{:08x};Glisa;", crate::util::unix_time() as u64);
    let name = c"com.apple.quarantine";
    // SAFETY: valid fd, NUL-terminated name, `value` bytes.
    let r = unsafe {
        libc::fsetxattr(
            f.as_raw_fd(),
            name.as_ptr(),
            value.as_ptr() as *const c_void,
            value.len(),
            0,
            0,
        )
    };
    if r != 0 {
        log::debug!("Flow: couldn't mark a received file as downloaded");
    }
}

// ======================================================================== backend
struct Shared {
    base: Base,
    screens: Mutex<Vec<Rect>>,
    ctrl: Mutex<Option<bool>>,
    stop: AtomicBool,
}

pub struct MacBackend {
    sh: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
    clipboard: Arc<MacClipboard>,
}

impl Default for MacBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MacBackend {
    pub fn new() -> Self {
        Self {
            sh: Arc::new(Shared {
                base: Base::new(),
                screens: Mutex::new(Vec::new()),
                ctrl: Mutex::new(None),
                stop: AtomicBool::new(false),
            }),
            thread: Mutex::new(None),
            clipboard: Arc::new(MacClipboard),
        }
    }

    fn run(sh: Arc<Shared>) {
        sh.base.set_state("ok", None);
        let mut tracker = EdgeTracker::new(32.0);
        let mut last_screens: Option<std::time::Instant> = None;
        let mut last_pos: Option<(f64, f64)> = None;
        while !sh.stop.load(Ordering::SeqCst) {
            if last_screens.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
                let rects = displays();
                if !rects.is_empty() && *lock(&sh.screens) != rects {
                    *lock(&sh.screens) = rects.clone();
                    if let Some(s) = sh.base.sink() {
                        s.screens(rects);
                    }
                }
                last_screens = Some(std::time::Instant::now());
            }
            if let Some((x, y)) = location() {
                let ctrl = ctrl_now();
                if lock(&sh.ctrl).replace(ctrl) != Some(ctrl) {
                    if let Some(s) = sh.base.sink() {
                        s.ctrl(ctrl);
                    }
                }
                if last_pos != Some((x, y)) {
                    last_pos = Some((x, y));
                    let rects = lock(&sh.screens).clone();
                    let events = tracker.feed(x.floor(), y.floor(), &rects, now_s(), buttons_now());
                    if let Some(s) = sh.base.sink() {
                        report(s.as_ref(), events);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(8));
        }
    }
}

impl EdgeBackend for MacBackend {
    fn kind(&self) -> &'static str {
        "macos"
    }

    fn start(&self, sink: Arc<dyn Sink>) {
        self.sh.base.set_sink(Some(sink));
        self.sh.stop.store(false, Ordering::SeqCst);
        let sh = self.sh.clone();
        *lock(&self.thread) = std::thread::Builder::new()
            .name("flow-macos".into())
            .spawn(move || Self::run(sh))
            .ok();
    }

    fn stop(&self) {
        self.sh.stop.store(true, Ordering::SeqCst);
        if let Some(t) = lock(&self.thread).take() {
            let _ = t.join();
        }
        self.sh.base.set_state("off", None);
        self.sh.base.set_sink(None);
    }

    fn state(&self) -> (String, Option<Text>) {
        self.sh.base.get_state()
    }

    fn screens(&self) -> Vec<Rect> {
        lock(&self.sh.screens).clone()
    }

    fn ctrl_held(&self) -> Option<bool> {
        *lock(&self.sh.ctrl)
    }

    fn ctrl_available(&self) -> bool {
        true
    }

    fn buttons_held(&self) -> Option<bool> {
        Some(buttons_now())
    }

    fn clipboard_kind(&self) -> &'static str {
        "macos"
    }

    fn clipboard_get(&self, timeout: Duration) -> Option<String> {
        String::from_utf8(
            self.clipboard
                .read("text/plain;charset=utf-8", crate::CLIPBOARD_MAX, timeout)
                .ok()??,
        )
        .ok()
    }

    fn clipboard_set(&self, text: &str) -> bool {
        let mut o = Offers::new();
        o.insert(
            "text/plain;charset=utf-8".into(),
            ClipData::Bytes(text.as_bytes().to_vec()),
        );
        self.clipboard.put(&o)
    }

    fn can_place(&self) -> bool {
        true
    }

    fn place_cursor(&self, x: i64, y: i64) -> bool {
        // SAFETY: no preconditions; re-associating keeps the pointer responsive after a warp.
        unsafe {
            let ok = CGWarpMouseCursorPosition(CGPoint {
                x: x as f64,
                y: y as f64,
            }) == 0;
            CGAssociateMouseAndMouseCursorPosition(1);
            ok
        }
    }

    fn rich(&self) -> Option<Arc<dyn RichClipboard>> {
        Some(self.clipboard.clone() as Arc<dyn RichClipboard>)
    }
}

// ====================================================================== pasteboard
const UTF8_TEXT: &str = "public.utf8-plain-text";
const FILE_URL: &str = "public.file-url";
const PNG: &str = "public.png";
const TIFF: &str = "public.tiff";
const JPEG: &str = "public.jpeg";
const PNG_FILE_TYPE: usize = 4; // NSBitmapImageFileTypePNG

fn ns_string(s: &str) -> Option<Retained<AnyObject>> {
    let c = CString::new(s).ok()?;
    // SAFETY: a NUL-terminated UTF-8 string.
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

fn rust_string(s: &AnyObject) -> String {
    // SAFETY: `s` is an NSString; UTF8String lives while `s` does.
    let p: *const c_char = unsafe { msg_send![s, UTF8String] };
    if p.is_null() {
        return String::new();
    }
    // SAFETY: a NUL-terminated C string.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

fn data_bytes(d: &AnyObject) -> Vec<u8> {
    // SAFETY: `d` is an NSData; `bytes` is valid for `length` bytes while it lives.
    unsafe {
        let len: usize = msg_send![d, length];
        let p: *const c_void = msg_send![d, bytes];
        if p.is_null() || len == 0 {
            return Vec::new();
        }
        std::slice::from_raw_parts(p as *const u8, len).to_vec()
    }
}

fn ns_data(b: &[u8]) -> Option<Retained<AnyObject>> {
    // SAFETY: `b` is valid for its length; NSData copies it.
    unsafe {
        msg_send![class!(NSData), dataWithBytes: b.as_ptr() as *const c_void, length: b.len()]
    }
}

fn general() -> Option<Retained<AnyObject>> {
    // SAFETY: the shared general pasteboard.
    unsafe { msg_send![class!(NSPasteboard), generalPasteboard] }
}

fn array_items(arr: &AnyObject) -> Vec<Retained<AnyObject>> {
    // SAFETY: `arr` is an NSArray.
    let n: usize = unsafe { msg_send![arr, count] };
    (0..n.min(crate::MAX_FILES))
        .filter_map(|i| {
            // SAFETY: an index below count.
            let o: Option<Retained<AnyObject>> = unsafe { msg_send![arr, objectAtIndex: i] };
            o
        })
        .collect()
}

fn types(pb: &AnyObject) -> Vec<String> {
    // SAFETY: NSPasteboard -types returns an NSArray of NSString (or nil).
    let t: Option<Retained<AnyObject>> = unsafe { msg_send![pb, types] };
    t.map(|a| array_items(&a).iter().map(|s| rust_string(s)).collect())
        .unwrap_or_default()
}

fn data_for(pb: &AnyObject, ty: &str) -> Option<Vec<u8>> {
    let t = ns_string(ty)?;
    // SAFETY: -dataForType: returns NSData or nil.
    let d: Option<Retained<AnyObject>> = unsafe { msg_send![pb, dataForType: &*t] };
    d.map(|d| data_bytes(&d))
}

/// The file URLs of the pasteboard items ("file:///…").
fn file_urls(pb: &AnyObject) -> Vec<String> {
    // SAFETY: -pasteboardItems returns an NSArray of NSPasteboardItem (or nil).
    let items: Option<Retained<AnyObject>> = unsafe { msg_send![pb, pasteboardItems] };
    let Some(items) = items else {
        return Vec::new();
    };
    let Some(ty) = ns_string(FILE_URL) else {
        return Vec::new();
    };
    array_items(&items)
        .iter()
        .filter_map(|item| {
            // SAFETY: -stringForType: returns NSString or nil.
            let s: Option<Retained<AnyObject>> = unsafe { msg_send![&**item, stringForType: &*ty] };
            s.map(|s| rust_string(&s))
        })
        .filter(|u| u.starts_with("file://"))
        .collect()
}

fn tiff_to_png(tiff: &[u8]) -> Option<Vec<u8>> {
    let data = ns_data(tiff)?;
    // SAFETY: +imageRepWithData: returns a bitmap rep or nil; the PNG representation is NSData.
    unsafe {
        let rep: Option<Retained<AnyObject>> =
            msg_send![class!(NSBitmapImageRep), imageRepWithData: &*data];
        let rep = rep?;
        let w: isize = msg_send![&*rep, pixelsWide];
        let h: isize = msg_send![&*rep, pixelsHigh];
        if !crate::imageinfo::acceptable(w.max(0) as u64, h.max(0) as u64) {
            return None;
        }
        let props: Option<Retained<AnyObject>> = msg_send![class!(NSDictionary), dictionary];
        let props = props?;
        let png: Option<Retained<AnyObject>> =
            msg_send![&*rep, representationUsingType: PNG_FILE_TYPE, properties: &*props];
        png.map(|d| data_bytes(&d))
    }
}

/// NSPasteboard (thread-safe for this use; each call in its own autorelease pool).
pub struct MacClipboard;

impl RichClipboard for MacClipboard {
    fn kind(&self) -> &'static str {
        "macos"
    }

    fn available(&self) -> bool {
        true
    }

    fn formats(&self, _timeout: Duration) -> Option<Vec<String>> {
        objc2::rc::autoreleasepool(|_| {
            let pb = general()?;
            let t = types(&pb);
            let mut out: Vec<String> = Vec::new();
            let mut add = |m: &str| {
                if !out.iter().any(|x| x == m) {
                    out.push(m.to_string());
                }
            };
            for ty in &t {
                match ty.as_str() {
                    UTF8_TEXT => {
                        add("text/plain;charset=utf-8");
                        add("text/plain");
                    }
                    FILE_URL => add("text/uri-list"),
                    PNG | TIFF => add("image/png"),
                    JPEG => add("image/jpeg"),
                    _ => {}
                }
            }
            Some(out)
        })
    }

    fn read_to(
        &self,
        mime: &str,
        out: &mut dyn std::io::Write,
        limit: usize,
        _timeout: Duration,
    ) -> Result<Option<u64>, TooLarge> {
        let data = objc2::rc::autoreleasepool(|_| -> Option<Vec<u8>> {
            let pb = general()?;
            match mime {
                "text/plain;charset=utf-8" | "text/plain" | "UTF8_STRING" => {
                    data_for(&pb, UTF8_TEXT)
                }
                "text/uri-list" => {
                    let urls = file_urls(&pb);
                    (!urls.is_empty()).then(|| {
                        urls.iter()
                            .map(|u| format!("{u}\r\n"))
                            .collect::<String>()
                            .into_bytes()
                    })
                }
                "image/png" => {
                    data_for(&pb, PNG).or_else(|| data_for(&pb, TIFF).and_then(|t| tiff_to_png(&t)))
                }
                "image/jpeg" => data_for(&pb, JPEG),
                _ => None,
            }
        });
        match data {
            None => Ok(None),
            Some(d) if d.len() > limit => Err(TooLarge),
            Some(d) => Ok(out.write_all(&d).ok().map(|()| d.len() as u64)),
        }
    }

    fn put(&self, offers: &Offers) -> bool {
        let bytes = |d: &ClipData| -> Option<Vec<u8>> {
            let mut out = Vec::new();
            copy_out(d, &mut |c| {
                out.extend_from_slice(c);
                Ok(())
            })
            .ok()?;
            Some(out)
        };
        objc2::rc::autoreleasepool(|_| -> Option<bool> {
            let pb = general()?;
            // SAFETY: NSPasteboard messages with valid objects.
            unsafe {
                let _: isize = msg_send![&*pb, clearContents];
                let mut any = false;
                if let Some(list) = offers.get("text/uri-list").and_then(&bytes) {
                    let urls: Vec<Retained<AnyObject>> = parse_uri_list(&list, crate::MAX_FILES)
                        .iter()
                        .filter_map(|p| {
                            let s = ns_string(&p.to_string_lossy())?;
                            let u: Option<Retained<AnyObject>> = msg_send![class!(NSURL), fileURLWithPath: &*s];
                            u
                        })
                        .collect();
                    if !urls.is_empty() {
                        let ptrs: Vec<*const AnyObject> = urls.iter().map(|u| Retained::as_ptr(u)).collect();
                        let arr: Option<Retained<AnyObject>> =
                            msg_send![class!(NSArray), arrayWithObjects: ptrs.as_ptr(), count: ptrs.len()];
                        if let Some(arr) = arr {
                            let ok: bool = msg_send![&*pb, writeObjects: &*arr];
                            any |= ok;
                        }
                    }
                }
                let text = offers.get("text/plain;charset=utf-8").or_else(|| offers.get("text/plain"));
                if let (Some(t), Some(ty)) = (text.and_then(&bytes), ns_string(UTF8_TEXT)) {
                    if let Some(s) = ns_string(&String::from_utf8_lossy(&t)) {
                        let ok: bool = msg_send![&*pb, setString: &*s, forType: &*ty];
                        any |= ok;
                    }
                }
                for (mime, uti) in [("image/png", PNG), ("image/jpeg", JPEG)] {
                    if let (Some(b), Some(ty)) = (offers.get(mime).and_then(&bytes), ns_string(uti)) {
                        if let Some(d) = ns_data(&b) {
                            let ok: bool = msg_send![&*pb, setData: &*d, forType: &*ty];
                            any |= ok;
                        }
                    }
                }
                Some(any)
            }
        })
        .unwrap_or(false)
    }
}
