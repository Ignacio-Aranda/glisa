//! Windows desktop: the pointer polled at ~125 Hz (`GetCursorPos`, Ctrl and mouse buttons
//! from `GetAsyncKeyState`), `SetCursorPos` for placement, monitors from
//! `EnumDisplayMonitors` (virtual-screen coordinates, which may be negative), and the
//! clipboard translated to MIME types at the edge:
//!
//! | MIME type | Windows clipboard |
//! |---|---|
//! | `text/plain;charset=utf-8`, `text/plain` | `CF_UNICODETEXT` (UTF-16; CRLF untouched) |
//! | `text/uri-list` | `CF_HDROP` (+ "Preferred DropEffect" = copy when we set it) |
//! | `image/png` | the registered "PNG" format, else `CF_DIBV5`/`CF_DIB` converted to PNG |
//! | `image/jpeg` | the registered "JFIF" format |
//! | `image/bmp` (from an older peer) | `CF_DIB` (the file header dropped, never decoded) |
//!
//! Also the few Windows-only helpers the other modules need (computer name, broadcast
//! addresses, free disk space). Written against the documented Win32 API; it is
//! type-checked on Linux (x86_64-pc-windows-gnu) but has not run on Windows yet.

use std::io;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardFormatNameW, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows_sys::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON,
};
use windows_sys::Win32::UI::Shell::{DROPFILES, DragQueryFileW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, GetCursorPos, HWND_MESSAGE, MSG, PM_REMOVE,
    PeekMessageW, SetCursorPos, TranslateMessage,
};

use crate::backends::{Base, EdgeBackend, Sink, report};
use crate::clipboard::{RichClipboard, TooLarge, copy_out};
use crate::geometry::{EdgeTracker, Rect, clean_rects, now_s};
use crate::text::Text;
use crate::transfer::{ClipData, Offers, parse_uri_list, uri_of};
use crate::util::lock;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ======================================================================== helpers
/// This computer's DNS host name (as `socket.gethostname()` gives on Windows).
pub fn computer_name() -> String {
    use windows_sys::Win32::System::SystemInformation::{
        ComputerNamePhysicalDnsHostname, GetComputerNameExW,
    };
    let mut buf = [0u16; 256];
    let mut n = buf.len() as u32;
    // SAFETY: `buf` has room for `n` UTF-16 units.
    if unsafe { GetComputerNameExW(ComputerNamePhysicalDnsHostname, buf.as_mut_ptr(), &mut n) } != 0
    {
        return String::from_utf16_lossy(&buf[..n as usize]);
    }
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

/// Broadcast address of every IPv4 interface that is up (not loopback).
pub fn broadcast_addresses() -> Vec<Ipv4Addr> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
        GetAdaptersAddresses, IF_TYPE_SOFTWARE_LOOPBACK, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};
    let mut out = Vec::new();
    let mut size: u32 = 16 * 1024;
    let mut buf: Vec<u64> = Vec::new();
    for _ in 0..3 {
        buf = vec![0u64; (size as usize).div_ceil(8)];
        // SAFETY: `buf` is `size` bytes, 8-aligned.
        let r = unsafe {
            GetAdaptersAddresses(
                AF_INET as u32,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                null(),
                buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
                &mut size,
            )
        };
        if r == 0 {
            break;
        }
        if r != 111 {
            // not ERROR_BUFFER_OVERFLOW
            return out;
        }
    }
    let mut cur = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !cur.is_null() {
        // SAFETY: a list filled by GetAdaptersAddresses inside `buf`.
        let a = unsafe { &*cur };
        cur = a.Next;
        if a.OperStatus != IfOperStatusUp || a.IfType == IF_TYPE_SOFTWARE_LOOPBACK {
            continue;
        }
        let mut u = a.FirstUnicastAddress;
        while !u.is_null() {
            // SAFETY: same list.
            let ua = unsafe { &*u };
            u = ua.Next;
            let sa = ua.Address.lpSockaddr;
            // SAFETY: lpSockaddr points to a sockaddr of iSockaddrLength bytes.
            if sa.is_null() || unsafe { (*sa).sa_family } != AF_INET {
                continue;
            }
            // SAFETY: an AF_INET sockaddr is a SOCKADDR_IN.
            let sin = unsafe { &*(sa as *const SOCKADDR_IN) };
            // SAFETY: reading the u32 view of the address union.
            let ip = u32::from_be(unsafe { sin.sin_addr.S_un.S_addr });
            let bits = ua.OnLinkPrefixLength.min(32) as u32;
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            let b = Ipv4Addr::from(ip | !mask);
            if bits < 31 && !out.contains(&b) {
                out.push(b);
            }
        }
    }
    out
}

pub fn free_bytes(path: &Path) -> io::Result<u64> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let p = wide(&path.to_string_lossy());
    let mut free = 0u64;
    // SAFETY: a NUL-terminated path and a valid out pointer.
    if unsafe { GetDiskFreeSpaceExW(p.as_ptr(), &mut free, null_mut(), null_mut()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(free)
}

fn key_down(vk: u16) -> bool {
    // SAFETY: no preconditions.
    (unsafe { GetAsyncKeyState(vk as i32) } as u16) & 0x8000 != 0
}

unsafe extern "system" fn monitor(
    hm: HMONITOR,
    _hdc: HDC,
    _r: *mut RECT,
    data: LPARAM,
) -> windows_sys::core::BOOL {
    // SAFETY: `data` is the &mut Vec<Rect> passed to EnumDisplayMonitors below.
    let out = unsafe { &mut *(data as *mut Vec<Rect>) };
    let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    // SAFETY: `mi` is a MONITORINFO with its size set.
    if unsafe { GetMonitorInfoW(hm, &mut mi) } != 0 {
        let r = mi.rcMonitor;
        out.push([
            r.left as i64,
            r.top as i64,
            (r.right - r.left) as i64,
            (r.bottom - r.top) as i64,
        ]);
    }
    1
}

/// Every monitor, in virtual-screen coordinates.
pub fn monitors() -> Vec<Rect> {
    let mut out: Vec<Rect> = Vec::new();
    // SAFETY: the callback only uses `out` during this call.
    unsafe {
        EnumDisplayMonitors(
            null_mut(),
            null(),
            Some(monitor),
            &mut out as *mut Vec<Rect> as LPARAM,
        )
    };
    clean_rects(&out)
}

// ======================================================================== backend
struct Shared {
    base: Base,
    screens: Mutex<Vec<Rect>>,
    ctrl: Mutex<Option<bool>>,
    buttons: Mutex<Option<bool>>,
    stop: AtomicBool,
}

pub struct WindowsBackend {
    sh: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
    clipboard: Mutex<Option<Arc<WinClipboard>>>,
}

impl Default for WindowsBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsBackend {
    pub fn new() -> Self {
        Self {
            sh: Arc::new(Shared {
                base: Base::new(),
                screens: Mutex::new(Vec::new()),
                ctrl: Mutex::new(None),
                buttons: Mutex::new(None),
                stop: AtomicBool::new(false),
            }),
            thread: Mutex::new(None),
            clipboard: Mutex::new(None),
        }
    }

    fn run(sh: Arc<Shared>) {
        sh.base.set_state("ok", None);
        let mut tracker = EdgeTracker::new(32.0);
        let mut last_screens: Option<std::time::Instant> = None;
        let mut last_pos: Option<(i32, i32)> = None;
        while !sh.stop.load(Ordering::SeqCst) {
            if last_screens.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
                let rects = monitors();
                if !rects.is_empty() && *lock(&sh.screens) != rects {
                    *lock(&sh.screens) = rects.clone();
                    if let Some(s) = sh.base.sink() {
                        s.screens(rects);
                    }
                }
                last_screens = Some(std::time::Instant::now());
            }
            let mut p = POINT { x: 0, y: 0 };
            // SAFETY: a valid POINT to fill.
            if unsafe { GetCursorPos(&mut p) } != 0 {
                let ctrl = key_down(VK_CONTROL);
                if lock(&sh.ctrl).replace(ctrl) != Some(ctrl) {
                    if let Some(s) = sh.base.sink() {
                        s.ctrl(ctrl);
                    }
                }
                let held = key_down(VK_LBUTTON) || key_down(VK_RBUTTON) || key_down(VK_MBUTTON);
                *lock(&sh.buttons) = Some(held);
                if last_pos != Some((p.x, p.y)) {
                    last_pos = Some((p.x, p.y));
                    let rects = lock(&sh.screens).clone();
                    let events = tracker.feed(p.x as f64, p.y as f64, &rects, now_s(), held);
                    if let Some(s) = sh.base.sink() {
                        report(s.as_ref(), events);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(8));
        }
    }
}

impl EdgeBackend for WindowsBackend {
    fn kind(&self) -> &'static str {
        "windows"
    }

    fn start(&self, sink: Arc<dyn Sink>) {
        self.sh.base.set_sink(Some(sink));
        self.sh.stop.store(false, Ordering::SeqCst);
        let sh = self.sh.clone();
        *lock(&self.thread) = std::thread::Builder::new()
            .name("flow-windows".into())
            .spawn(move || Self::run(sh))
            .ok();
        *lock(&self.clipboard) = WinClipboard::start();
    }

    fn stop(&self) {
        self.sh.stop.store(true, Ordering::SeqCst);
        if let Some(t) = lock(&self.thread).take() {
            let _ = t.join();
        }
        if let Some(c) = lock(&self.clipboard).take() {
            c.close();
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
        Some(key_down(VK_LBUTTON) || key_down(VK_RBUTTON) || key_down(VK_MBUTTON))
    }

    fn clipboard_kind(&self) -> &'static str {
        if lock(&self.clipboard).is_some() {
            "windows"
        } else {
            "none"
        }
    }

    fn clipboard_get(&self, timeout: Duration) -> Option<String> {
        let c = lock(&self.clipboard).clone()?;
        String::from_utf8(
            c.read("text/plain;charset=utf-8", crate::CLIPBOARD_MAX, timeout)
                .ok()??,
        )
        .ok()
    }

    fn clipboard_set(&self, text: &str) -> bool {
        let Some(c) = lock(&self.clipboard).clone() else {
            return false;
        };
        let mut o = Offers::new();
        o.insert(
            "text/plain;charset=utf-8".into(),
            ClipData::Bytes(text.as_bytes().to_vec()),
        );
        c.put(&o)
    }

    fn can_place(&self) -> bool {
        true
    }

    fn place_cursor(&self, x: i64, y: i64) -> bool {
        // SAFETY: no preconditions.
        unsafe { SetCursorPos(x as i32, y as i32) != 0 }
    }

    fn rich(&self) -> Option<Arc<dyn RichClipboard>> {
        lock(&self.clipboard)
            .clone()
            .map(|c| c as Arc<dyn RichClipboard>)
    }
}

// ====================================================================== clipboard
enum Cmd {
    Formats(mpsc::Sender<Vec<String>>),
    Read(
        String,
        usize,
        mpsc::Sender<Result<Option<Vec<u8>>, TooLarge>>,
    ),
    Put(Offers, mpsc::Sender<bool>),
}

/// The Windows clipboard, used from one thread that owns a message-only window (an owner
/// window is needed for SetClipboardData).
pub struct WinClipboard {
    tx: Mutex<mpsc::Sender<Cmd>>,
    alive: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

struct Open;

impl Open {
    fn new(hwnd: HWND) -> Option<Self> {
        for _ in 0..10 {
            // SAFETY: `hwnd` is our window.
            if unsafe { OpenClipboard(hwnd) } != 0 {
                return Some(Open);
            }
            std::thread::sleep(Duration::from_millis(20)); // another program has it open
        }
        None
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: we opened it.
        unsafe { CloseClipboard() };
    }
}

fn format_id(name: &str) -> u32 {
    let w = wide(name);
    // SAFETY: a NUL-terminated name.
    unsafe { RegisterClipboardFormatW(w.as_ptr()) }
}

fn format_name(f: u32) -> Option<String> {
    let mut buf = [0u16; 128];
    // SAFETY: `buf` has room for 128 units.
    let n = unsafe { GetClipboardFormatNameW(f, buf.as_mut_ptr(), buf.len() as i32) };
    (n > 0).then(|| String::from_utf16_lossy(&buf[..n as usize]))
}

/// The bytes of a clipboard format (the clipboard is open).
fn get_bytes(format: u32, limit: usize) -> Result<Option<Vec<u8>>, TooLarge> {
    // SAFETY: the clipboard is open; the handle is valid until it is closed.
    let h = unsafe { GetClipboardData(format) };
    if h.is_null() {
        return Ok(None);
    }
    // SAFETY: a global memory handle owned by the clipboard.
    let size = unsafe { GlobalSize(h) };
    if size > limit {
        return Err(TooLarge);
    }
    // SAFETY: locking it gives `size` readable bytes until unlocked.
    let p = unsafe { GlobalLock(h) } as *const u8;
    if p.is_null() {
        return Ok(None);
    }
    // SAFETY: as above.
    let data = unsafe { std::slice::from_raw_parts(p, size) }.to_vec();
    // SAFETY: matching the lock above.
    unsafe { GlobalUnlock(h) };
    Ok(Some(data))
}

fn set_bytes(format: u32, data: &[u8]) -> bool {
    // SAFETY: allocate, copy and hand over a global memory block.
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, data.len().max(1));
        if h.is_null() {
            return false;
        }
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            GlobalFree(h);
            return false;
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
        GlobalUnlock(h);
        if SetClipboardData(format, h as HANDLE).is_null() {
            GlobalFree(h);
            return false;
        }
    }
    true
}

fn utf16_bytes(text: &str) -> Vec<u8> {
    wide(text).iter().flat_map(|u| u.to_le_bytes()).collect()
}

fn hdrop(paths: &[PathBuf]) -> Vec<u8> {
    let header = std::mem::size_of::<DROPFILES>();
    let mut out = vec![0u8; header];
    out[0..4].copy_from_slice(&(header as u32).to_le_bytes()); // pFiles
    out[16..20].copy_from_slice(&1i32.to_le_bytes()); // fWide
    for p in paths {
        out.extend(utf16_bytes(&p.to_string_lossy()));
    }
    out.extend_from_slice(&[0, 0]);
    out
}

fn dropped_paths() -> Vec<PathBuf> {
    // SAFETY: the clipboard is open; CF_HDROP data is an HDROP.
    let h = unsafe { GetClipboardData(CF_HDROP as u32) };
    if h.is_null() {
        return Vec::new();
    }
    // SAFETY: querying the count.
    let n = unsafe { DragQueryFileW(h as _, u32::MAX, null_mut(), 0) };
    let mut out = Vec::new();
    for i in 0..n.min(crate::MAX_FILES as u32) {
        // SAFETY: the length of name i, then the name into a buffer one longer.
        let len = unsafe { DragQueryFileW(h as _, i, null_mut(), 0) };
        let mut buf = vec![0u16; len as usize + 1];
        let got = unsafe { DragQueryFileW(h as _, i, buf.as_mut_ptr(), len + 1) };
        out.push(PathBuf::from(String::from_utf16_lossy(
            &buf[..got as usize],
        )));
    }
    out
}

impl WinClipboard {
    fn start() -> Option<Arc<Self>> {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let alive = Arc::new(AtomicBool::new(true));
        let a2 = alive.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("flow-clipboard".into())
            .spawn(move || {
                let class = wide("STATIC");
                let title = wide("");
                // SAFETY: a message-only window of a system class; destroyed below.
                let hwnd = unsafe {
                    CreateWindowExW(
                        0,
                        class.as_ptr(),
                        title.as_ptr(),
                        0,
                        0,
                        0,
                        0,
                        0,
                        HWND_MESSAGE,
                        null_mut(),
                        null_mut(),
                        null(),
                    )
                };
                let _ = ready_tx.send(!hwnd.is_null());
                if hwnd.is_null() {
                    a2.store(false, Ordering::SeqCst);
                    return;
                }
                while a2.load(Ordering::SeqCst) {
                    match rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(cmd) => serve(hwnd, cmd),
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    let mut msg: MSG = unsafe { std::mem::zeroed() };
                    // SAFETY: pumping this thread's messages.
                    while unsafe { PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) } != 0 {
                        unsafe {
                            TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                }
                // SAFETY: our window, on its own thread.
                unsafe { DestroyWindow(hwnd) };
                a2.store(false, Ordering::SeqCst);
            })
            .ok()?;
        if !ready_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or(false)
        {
            return None;
        }
        Some(Arc::new(Self {
            tx: Mutex::new(tx),
            alive,
            thread: Mutex::new(Some(thread)),
        }))
    }
}

fn serve(hwnd: HWND, cmd: Cmd) {
    match cmd {
        Cmd::Formats(tx) => {
            let mut out: Vec<String> = Vec::new();
            if let Some(_open) = Open::new(hwnd) {
                let mut f = 0u32;
                loop {
                    // SAFETY: enumerating the open clipboard.
                    f = unsafe { EnumClipboardFormats(f) };
                    if f == 0 {
                        break;
                    }
                    let names: Vec<String> = match f as u16 {
                        CF_UNICODETEXT => {
                            vec!["text/plain;charset=utf-8".into(), "text/plain".into()]
                        }
                        CF_HDROP => vec!["text/uri-list".into()],
                        CF_DIB | CF_DIBV5 => vec!["image/png".into()],
                        _ => match format_name(f).as_deref() {
                            Some("PNG") => vec!["image/png".into()],
                            Some("JFIF") => vec!["image/jpeg".into()],
                            _ => vec![],
                        },
                    };
                    for n in names {
                        if !out.contains(&n) {
                            out.push(n);
                        }
                    }
                }
            }
            let _ = tx.send(out);
        }
        Cmd::Read(mime, limit, tx) => {
            let r = match Open::new(hwnd) {
                None => Ok(None),
                Some(_open) => read(&mime, limit),
            };
            let _ = tx.send(r);
        }
        Cmd::Put(offers, tx) => {
            let ok = match Open::new(hwnd) {
                None => false,
                Some(_open) => put(&offers),
            };
            let _ = tx.send(ok);
        }
    }
}

fn read(mime: &str, limit: usize) -> Result<Option<Vec<u8>>, TooLarge> {
    match mime {
        "text/plain;charset=utf-8" | "text/plain" | "UTF8_STRING" => {
            let Some(raw) = get_bytes(CF_UNICODETEXT as u32, limit.saturating_mul(3))? else {
                return Ok(None);
            };
            let units: Vec<u16> = raw
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|u| *u != 0)
                .collect();
            let text = String::from_utf16_lossy(&units).into_bytes();
            if text.len() > limit {
                Err(TooLarge)
            } else {
                Ok(Some(text))
            }
        }
        "text/uri-list" => {
            let list: String = dropped_paths().iter().map(|p| uri_of(p) + "\r\n").collect();
            if list.is_empty() {
                Ok(None)
            } else if list.len() > limit {
                Err(TooLarge)
            } else {
                Ok(Some(list.into_bytes()))
            }
        }
        "image/png" => {
            let png = format_id("PNG");
            // SAFETY: the clipboard is open.
            if unsafe { IsClipboardFormatAvailable(png) } != 0 {
                return get_bytes(png, limit);
            }
            for f in [CF_DIBV5, CF_DIB] {
                if let Some(dib) = get_bytes(f as u32, limit.saturating_mul(8))? {
                    let Some(png) = crate::dib::dib_to_png(&dib) else {
                        continue;
                    };
                    return if png.len() > limit {
                        Err(TooLarge)
                    } else {
                        Ok(Some(png))
                    };
                }
            }
            Ok(None)
        }
        "image/jpeg" => get_bytes(format_id("JFIF"), limit),
        _ => Ok(None),
    }
}

fn put(offers: &Offers) -> bool {
    // SAFETY: the clipboard is open (with our window as the owner).
    if unsafe { EmptyClipboard() } == 0 {
        return false;
    }
    let bytes = |d: &ClipData| -> Option<Vec<u8>> {
        let mut out = Vec::new();
        copy_out(d, &mut |c| {
            out.extend_from_slice(c);
            Ok(())
        })
        .ok()?;
        Some(out)
    };
    let mut any = false;
    let text = offers
        .get("text/plain;charset=utf-8")
        .or_else(|| offers.get("text/plain"));
    if let Some(t) = text.and_then(bytes) {
        any |= set_bytes(
            CF_UNICODETEXT as u32,
            &utf16_bytes(&String::from_utf8_lossy(&t)),
        );
    }
    if let Some(list) = offers.get("text/uri-list").and_then(bytes) {
        let paths = parse_uri_list(&list, crate::MAX_FILES);
        if !paths.is_empty() {
            any |= set_bytes(CF_HDROP as u32, &hdrop(&paths));
            set_bytes(format_id("Preferred DropEffect"), &1u32.to_le_bytes()); // DROPEFFECT_COPY
        }
    }
    if let Some(png) = offers.get("image/png").and_then(bytes) {
        any |= set_bytes(format_id("PNG"), &png);
    }
    if let Some(jpeg) = offers.get("image/jpeg").and_then(bytes) {
        any |= set_bytes(format_id("JFIF"), &jpeg);
    }
    if let Some(bmp) = offers.get("image/bmp").and_then(bytes) {
        if let Some(dib) = crate::dib::bmp_to_dib(&bmp) {
            any |= set_bytes(CF_DIB as u32, dib);
        }
    }
    any
}

impl RichClipboard for WinClipboard {
    fn kind(&self) -> &'static str {
        "windows"
    }

    fn available(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn formats(&self, timeout: Duration) -> Option<Vec<String>> {
        let (tx, rx) = mpsc::channel();
        lock(&self.tx).send(Cmd::Formats(tx)).ok()?;
        rx.recv_timeout(timeout + Duration::from_millis(300)).ok()
    }

    fn read_to(
        &self,
        mime: &str,
        out: &mut dyn std::io::Write,
        limit: usize,
        timeout: Duration,
    ) -> Result<Option<u64>, TooLarge> {
        let (tx, rx) = mpsc::channel();
        if lock(&self.tx)
            .send(Cmd::Read(mime.to_string(), limit, tx))
            .is_err()
        {
            return Ok(None);
        }
        match rx.recv_timeout(timeout + Duration::from_millis(300)) {
            Ok(Ok(Some(d))) => Ok(out.write_all(&d).ok().map(|()| d.len() as u64)),
            Ok(Err(e)) => Err(e),
            _ => Ok(None),
        }
    }

    fn put(&self, offers: &Offers) -> bool {
        let (tx, rx) = mpsc::channel();
        lock(&self.tx).send(Cmd::Put(offers.clone(), tx)).is_ok()
            && rx.recv_timeout(Duration::from_secs(3)).unwrap_or(false)
    }

    fn close(&self) {
        self.alive.store(false, Ordering::SeqCst);
        if let Some(t) = lock(&self.thread).take() {
            let _ = t.join();
        }
    }
}
