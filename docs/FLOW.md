# Glisa Flow — design, protocol and security model

Flow lets one Logitech mouse (and, optionally, keyboard) move between computers that run Glisa
(or its predecessor, LinOptions). The device is paired with each computer on a different
Easy-Switch channel.

When the pointer is pushed against the screen edge where the other computer is, this computer:
1. asks the other one whether it's ready;
2. releases anything held;
3. sends the HID++ "change host" command (feature 0x1814) to the mouse, and to the keyboard
   if enabled;
4. sends what was copied: the clipboard text, or the copied files, folders or image (§6).

The other computer puts its cursor at the matching point of its opposite edge as soon as the
device arrives there.

Interoperability with Logitech Options+ is out of scope: its protocol is closed and tied to
per-device keys.

Protocol version: **1**.

## Glisa (Rust) notes

This document was written for the Python implementation (LinOptions). The Rust
implementation in `crates/flow` speaks exactly the same protocol: §3–§7 apply unchanged.

**Wire values stay the same.** Every value that goes on the wire keeps its original spelling
(`linoptions-flow` discovery app and certificate name, the commit and SAS contexts). They live
in `crates/flow/src/wire.rs`, pinned by a test. Before the Python implementation was retired,
an interop test paired a Rust and a Python computer and exercised crossings, text, files and
images both ways.

What differs:
- **No helper process.** The KWin script calls a D-Bus object Glisa exports itself with zbus,
  in-process. §1's Qt "bridge" is gone, and local names live in `crates/flow/src/local.rs`.
- **More desktops.**

  | Desktop | Edges | Cursor placement |
  |---|---|---|
  | Windows | `GetCursorPos` polling | `SetCursorPos` |
  | macOS | `CGEventGetLocation` polling | `CGWarpMouseCursorPosition` |

  The clipboard uses each system's API (Windows `CF_UNICODETEXT`, `CF_HDROP`, PNG/`CF_DIB`;
  macOS NSPasteboard). Images are always PNG on the wire.
- **No crossing while a mouse button is held**, on every backend.
  - KDE Wayland reads the button state with `EVIOCGKEY` on the Logitech mouse's input node
    (the udev rule grants read access to mice's event nodes only). While *Drag files across the
    edge* is on, it also reads that node's events, filtered by the kernel to the left button
    (and its motion only from a left press until the drag is decided), §6b.
  - X11, Windows and macOS ask their APIs.
  - The one exception: files dragged onto the edge catcher (KDE Wayland), §6b.
- **Received names are made safe for the receiving system:**
  - NFC normalization;
  - Windows reserved names and characters, trailing dots and spaces, long paths;
  - case-insensitive collisions on Windows and macOS;
  - never overwritten.

  The sender skips macOS metadata files. Received files are marked as downloaded (macOS
  quarantine attribute, Windows Mark of the Web).
- **The store** lives in Glisa's settings directory (`~/.config/glisa/flow` on Linux). The
  Python store is imported on first start.

## 1. Architecture

```
                        session D-Bus                       TLS 1.3 (mutual, pinned)
 KWin ── flow-kwin.js ──callDBus──▶ bridge (Qt helper) ◀─pipe─▶ daemon (FlowService) ◀───────▶ other computer
                                    · loads/unloads script        · edge engine                (same stack)
 klipper ◀──────────── D-Bus ──────── · klipper get/set           · peers, pairing, discovery
                                                                  · set_host on devices
 /dev/uinput ◀── absolute pointer ────────────────────────────── · places the cursor on arrival
 GUI (FlowPage, FlowWatcher) ◀── Unix socket JSON API (flow_*) ── ·
```

| Piece | Where | Notes |
|---|---|---|
| Edge detection, KDE Wayland | `data/flow-kwin.js`, loaded at runtime through `org.kde.KWin /Scripting` (`loadScript(path, "linoptions-flow")`, then `/Scripting/Script<N>` `run`) | never installed; unloaded when Flow is turned off, when the daemon stops, and on `uninstall.sh` |
| D-Bus endpoint | `linoptions/flow/bridge.py`: separate QtCore/QtDBus process started by the daemon | see below |
| Edge detection, X11 | `X11Backend`: `XQueryPointer` polled at 60 Hz through ctypes (also gives Ctrl and mouse-button state) | |
| Other Wayland desktops | not available; the page and `flow status` say *"Flow is not available on this desktop"* | |
| Ctrl state, KDE Wayland | `keystate.py`: tiny Wayland client for `org_kde_kwin_keystate` v5 (control key, state ≠ unlocked) | X11: `XQueryPointer` mask |
| Cursor placement | KDE Wayland: `uinput.AbsolutePointer` (2nd virtual device); X11: `XWarpPointer` | §5 |
| Clipboard text | klipper over D-Bus (through the bridge); fallback: the GUI process's `QClipboard` | §6 |
| Copied files and images | KDE Wayland: the Wayland data-control protocol from the daemon (`clipboard.py`, wire format like `keystate.py`); X11: `QClipboard` in the bridge (`--clipboard`, a `QGuiApplication` on xcb); fallback: the GUI process | §6 |
| Engine, network, pairing, API | `linoptions/flow/service.py` (`FlowService`) inside the daemon | Qt-free |

**Why the D-Bus endpoint is a helper process.** A KWin script can only talk to the outside world
with `callDBus()`, so something must own a D-Bus service. The daemon is Qt-free and runs as a
systemd user service without the GUI, so "QtDBus inside the daemon" would mean running a Qt event
loop in a thread of a non-Qt process, and "the GUI forwards it" would stop Flow whenever the window
is closed. The bridge is ~200 lines, owns `io.github.linoptions.Flow` (`/Flow`,
`io.github.linoptions.Flow1.Event(QString json)`), forwards events as JSON lines on stdout and
serves klipper requests on stdin. When its stdin closes (the daemon stopped, crashed or turned Flow
off) it unloads the script and exits, so nothing stays loaded. Every 5 s it re-checks
`isScriptLoaded` and reloads after a KWin restart. Script events are validated as strictly as
network messages (anything on the session bus could call `Event`).

The KWin script reports only *transitions*: `edge` (pointer reached an outer edge of the desktop —
an edge with no other output behind it — and again while moving ≥ 24 px or ≥ 120 ms along it),
`leave` (pointer is more than 32 px away from that edge), `screens` (output geometry) and a `hello`
when loaded. Moving or resizing a window (`activeWindow.move/resize`) suppresses edges, so quick
tiling by dragging a window to the edge never switches. On X11 a held mouse button does the same.
`geometry.EdgeTracker` implements the same rules in Python; a test runs the real script in `node`
with a fake `workspace` and checks both produce identical events.

Verified on Plasma 6.6 (KWin 6.6.6, Wayland): `workspace.cursorPos`, `workspace.screens[i].geometry`,
`activeWindow.move` (boolean), `callDBus` with a JSON string into a `QString` slot, and the
`/Scripting/Script<N>` `org.kde.kwin.Script.run()` path. `org_kde_kwin_keystate` v5 is advertised and
reports keys 0–7 (control = 4). KWin 6.6 advertises `ext_data_control_manager_v1` v1 (not the older
`zwlr_data_control_manager_v1`) to ordinary clients: a background process lists the selection's MIME
types and reads them through a pipe (verified read-only on the live session), and sets a selection
that a Qt Wayland application pastes as URLs or as an image (verified on a nested
`kwin_wayland --virtual` with its own D-Bus session, so the user's clipboard was never touched).

## 2. Edge engine (behaviour)

* Layout: each paired peer is `left | right | top | bottom` of this computer (one peer per side).
* Trigger: an `edge` event on a side with a peer, outside the **corner dead zones** (default 48 px,
  at most ¼ of the edge, at both ends of that output's edge).
* **Ctrl only** (option): the switch happens only while Ctrl is held; pressing Ctrl while resting
  at the edge triggers it. If the desktop cannot report Ctrl, nothing switches and the page says so.
* **Cooldown**: nothing switches for `cooldown_ms` (default 800 ms) after any switch or arrival.
* **Hysteresis**: one attempt per visit to an edge; a refused attempt (peer unreachable, no channel)
  re-arms only after the pointer has left the edge (32 px). An arriving cursor is placed 6 px inside
  the edge, so it never lands on the edge itself.
* Channel per peer and device: explicit choice (`flow_set_channel`), else a suggestion when an
  Easy-Switch channel name (HID++ 0x1815 `hosts.entries[].name`) matches the peer's host name
  (case and punctuation ignored, domain stripped; exact match first, then prefix). No suggestion →
  no switch, and a notice asks the user to choose.

Crossing, on the computer that has the device (A):

1. compute `t` = position along that side of A's whole desktop (0…1);
2. `switch {side, t, devices}` to the peer over the authenticated link (answer within 0.8 s,
   connecting first if needed). No answer, refusal or no channel → **the device stays**, and a
   notice (toast or tray notification) explains why;
3. `release_all()` on the virtual keyboard/mouse (keys/buttons held by remapped controls);
4. `set_host(channel)` on every target device in parallel (pointer devices + keyboards if enabled);
5. cooldown; then, if enabled, send what was copied: files, folders or an image on a separate
   transfer connection, else the clipboard text (§6).

On the receiving computer (B): `switch` stores a *pending arrival* (peer, entry side = where A is in
B's layout, else the opposite of A's side; `t`; device names) valid for 15 s, and starts a cooldown.
While it is pending the daemon looks for the device more often (rescan for Bluetooth nodes, ping
receiver devices every 0.3 s). When a matching pointer device comes online (`_register`,
`check_alive`, or the HID++ wireless-status event), the cursor is placed at
`entry_point(side, t)`: `t` along that side of B's desktop, on the outermost output there, 6 px
inside. While the device is on the other computer, this computer simply sees it go offline.

## 3. Ports and discovery

| Port | Use | Open |
|---|---|---|
| UDP 27461 | discovery announcements | while Flow is on |
| TCP 27461 | authenticated peer channel and transfer connections (TLS 1.3, mutual) | while Flow is on |
| TCP 27462 | pairing introduction | **only** while the user has pairing open (≤ 2 min) |

LinOptions never changes the firewall. When `ufw` is active (`/etc/ufw/ufw.conf` has
`ENABLED=yes`) the page and `flow status` show the rule to add:
`sudo ufw allow 27461/udp && sudo ufw allow 27461:27462/tcp`.

Discovery packet (UTF-8 JSON, ≤ 512 bytes, exactly these fields):

```json
{"app": "linoptions-flow", "v": 1, "type": "announce", "id": "0123456789abcdef",
 "name": "portatil-ana", "port": 27461}
```

`type` is `announce` (every 5 s, to the broadcast address of every IPv4 interface that is up) or
`query` (sent when the user looks for computers; others answer with an announcement, at most once
per second). Announcements are unauthenticated by nature: they carry nothing sensitive, malformed
ones are dropped, our own id is ignored, and they are only ever an **address hint** — nothing
happens until the pinned TLS session with that computer succeeds. A hint never overwrites the last
address where the pinned peer was actually reached. Daemon options `--flow-bind`, `--flow-port`
and `--flow-discovery IP:PORT` (repeatable) restrict where Flow listens and announces
(`--flow-discovery` replaces the broadcast addresses); tests use loopback only.

### Broadcast and routed networks

A broadcast never crosses a router. Two computers on **different subnets** (for example one on a
campus Wi-Fi `192.168.8.15/21`, the other wired at `192.168.2.20`, joined by a router that
forwards ICMP and TCP 27461) never hear each other's broadcasts, so discovery alone cannot find
them, and the address stored at pairing time goes stale when either one moves. Three things keep
them connected:

1. **Directed announcements.** Besides the broadcasts, the same announcement goes, unicast, to
   every address we know for a **paired** computer (the one the user typed, the last one that
   worked, the last announced one): IPv4 literals only (no name lookups on the discovery
   threads), deduplicated, at most 32 destinations per round. Only our own announcement, only to
   paired computers' addresses, never in answer to a packet — so it cannot be used to make us send
   traffic anywhere else (no amplification or reflection). `--flow-discovery` does not turn this
   off: it only replaces the broadcast addresses, and these addresses are where the TCP link goes
   anyway.
2. **Learning from incoming connections.** Each computer opens its own link to the other (to send
   its requests), so both need an address. When a paired computer reaches us, and only after its
   TLS pin and its `hello` id match, its source IP plus the listening port it advertises in
   `hello.port` (default 27461 for older peers that don't send it) become its learned address, and
   our own link retries at once. A link that is up keeps its address (the one that works for us
   wins). Nothing unauthenticated is ever learned.
3. **A manual address per paired computer** (`manual_address`, set with *Set address…* on the Flow
   page, `linoptions-ctl flow address`, or `flow_set_peer_address`): `host[:port]` with an IPv4
   address, an IPv6 address in brackets, or a host name; port 1–65535 (default 27461); empty or
   `auto` clears it. It is stored apart from the learned `address` and never overwritten by it.

A link tries, in this order: the **manual** address, the **learned** address (last reached, or
last seen connecting to us), the **discovery hint**. Whenever that list changes, the link retries
at once instead of waiting out its back-off.

**Computers on different networks, step by step:** pair them with *Pair by address…* (or
`linoptions-ctl flow pair IP[:PORT]`) — both then store the address the pairing used; if one of
them later shows *Offline*, set the other computer's address on **at least one** side. As soon as
that side's link connects, the other side learns its address from the `hello` (and from the
directed announcements), so both come online.

## 4. Wire protocol (v1)

Every message travels inside TLS 1.3 as one frame: `uint32 big-endian length ‖ UTF-8 JSON object`.

* The object has exactly `v` (protocol version, must equal 1), `type`, `seq` and the fields of its
  type — unknown types, unknown or missing fields, wrong types, duplicate keys, `NaN`/`Infinity`,
  non-printable names, lone surrogates: **protocol error** → `error {reason: "protocol"}` and the
  connection is closed.
* `seq` starts at 1 per connection and direction and must grow by exactly 1: a replayed, dropped
  or reordered frame closes the connection.
* Size limits: 8 KiB per frame for every type except `clipboard` (7 MiB frame, text ≤ 1 MiB of
  UTF-8), `transfer` (8 MiB: the list of files) and data frames (21-byte header + ≤ 256 KiB). The
  length prefix is checked, before reading the body, against the largest type that connection
  accepts at that moment: only the server side of the authenticated port reads frames over 8 KiB
  (its first message may be a `transfer`, then `clipboard` on a peer channel or data frames on a
  transfer connection); the pairing ports and every answer stay at 8 KiB. JSON nested too deeply
  to parse is a protocol error like any other.
* Each connection only accepts the types of its role (a pairing message on the peer channel is a
  protocol error, and vice versa). We validate what we send with the same rules.
* Timeouts: TLS handshake 5 s; once a frame has started, the whole frame within 10 s (a deadline
  for the frame, not for each read); server closes a peer connection idle for 45 s; clients ping
  every 10 s.

### Peer channel (TCP 27461, mutual TLS, pinned)

The client is the computer that wants something; the server only answers.

| Client → server | Fields | Answer |
|---|---|---|
| `hello` | `id` (16 hex), `name` (≤ 64), optional `port` (1–65534, the sender's listening TCP port), optional `caps` (≤ 16 names `[a-z][a-z0-9_]{0,23}`) | `hello {id, name}` + `caps` only if the client sent `caps` (ids must match the pinned peer) |
| `ping` | – | `pong {re}` |
| `switch` | `side` (left/right/top/bottom), `t` (0…1), `devices` (1–8 × `{name, kind}`) | `switch_ok {re}` or `switch_refused {re, reason}` |
| `clipboard` | `text` (1 B … 1 MiB UTF-8) | `ack {re}` |
| `drag` | `names` (1–8 unique single path components, as a transfer's top-level entries), `count` (1–10 000, ≥ the names listed), `bytes` (0 … 2⁵⁰); only to a peer that announced `drag` | `ack {re}` |
| `unpair` | – | `ack {re}`; the receiver forgets the sender's pin |
| | | `error {reason}` then close |

`re` is the `seq` of the request. `reason` ∈ `disabled, busy, not_pairing, rate_limited, version,
unknown_peer, no_layout, protocol, bye, rejected, expired, mismatch, error`.

`hello.port` and `hello.caps` are the only **optional** fields in the protocol (declared in
`proto.OPTIONAL`; every other field is still exactly-these-fields). Both were added within protocol
version 1: a `hello` without `port` means port 27461; without `caps`, no capabilities. Peers from
before a field reject a `hello` that has it (`error {reason: "protocol"}`); the client then says
`hello` again on a new connection with fewer fields: `port`+`caps`, then `port`, then neither. The
server answers with its own `caps` only when the client's `hello` had `caps` (an older client would
refuse the field). Capabilities: `transfer` = this computer accepts transfer connections (copied
files and images, below); `drag` = it takes `drag` notices and lets the drag go on there
(§6b; announced by the Linux build while that option and copied files are on); unknown names are
ignored, so later versions can add some. A peer without `transfer` gets the clipboard text exactly
as before; a peer without `drag` gets dragged files as copied files, with no notice. When this
computer's capabilities change (an option), it ends the peer channels peers have open with it, so
they say `hello` again; a client whose kept connection was closed reconnects once and repeats the
request.

The **first message** on the authenticated port says what the connection is for: `hello` (a peer
channel, above) or `transfer` (a transfer connection, below). Anything else is a protocol error.

### Transfer connection (TCP 27461, mutual TLS 1.3, pinned)

Copied files and images travel on a **separate connection per transfer**, opened by the sender
(the client) to the same address as its peer link, with the same TLS context, certificate check
and SPKI pin comparison. A big transfer therefore never delays `switch`, `ping` or the clipboard
text on the peer channel, and either computer can send the mouse back while it runs.

| Sender → receiver | Fields | Answer |
|---|---|---|
| `transfer` | `id` (sender's id, must match the pin), `kind` (`files` \| `image`), `entries` (below), `text` (`""`, or ≤ 1 MiB of UTF-8 that goes with an image) | `transfer_ok {re}` or `transfer_refused {re, reason}` then close |
| data frame (binary) | `0x00` ‖ `seq` uint64 ‖ entry index uint32 ‖ offset uint64 ‖ 1 … 262 144 bytes, big-endian; same `seq` counter as the JSON frames | – |
| `transfer_skip` | `index`: the current file couldn't be sent (changed or gone); the receiver deletes what it has of it | – |
| `transfer_end` | – | `transfer_done {re, failed}` then close |
| `transfer_cancel` | `reason` (either side, any time; then close) | – |

`entries` (1 … 10 000, in order):

* `kind: files` — `{"type": "dir", "path", "mtime"}` and `{"type": "file", "path", "size", "mtime",
  "sha256"}`. `path` is relative with `/` separators: no empty, `.` or `..` component, no leading or
  trailing `/`, no NUL, control, format (bidi overrides…), line or paragraph separator characters,
  each component ≤ 255 and the whole ≤ 4096 bytes of UTF-8, ≤ 32 components. Paths are unique and
  each one's parent folder appears earlier as a `dir`, so a file can never be a parent. There is
  no type for links, devices, fifos or sockets: they can't be expressed. `size` 0 … 2⁵⁰, `mtime`
  nanoseconds since the epoch, `sha256` 64 hex. `text` must be `""`.
* `kind: image` — exactly one `{"type": "image", "mime", "size" (> 0), "sha256"}` with `mime` ∈
  `image/png, image/jpeg, image/webp, image/bmp`.

Data frames carry the content of every non-empty file (or the image), entry after entry, each from
offset 0 without gaps: the receiver accepts a frame only for the current entry at exactly the
offset it has written so far, and never more bytes than declared. `transfer_end` before every byte
has arrived (or been skipped), data for another entry, a different offset or extra bytes are
protocol errors (`error {protocol}`, everything received so far is deleted). Each file's SHA-256 is
computed as it is written; a file that doesn't match is deleted and counted in `failed`.
Backpressure is TCP's own: the receiver writes each frame to disk before reading the next one. A
transfer connection with no frame for 30 s is dropped; `transfer_done` must come within 60 s of
`transfer_end`. Transfer reasons: `disabled, busy, too_large, too_many, no_space, cancelled,
failed, protocol, unknown_peer, error`.

Limits (both computers check them; the size limits are options of each computer): ≤ 10 000 files
and folders, ≤ 32 levels, ≤ `max_transfer_mb` (default 2048 MiB) per transfer of files,
≤ `max_image_mb` (default 50 MiB) per image, free space ≥ size + 64 MiB on the receiver. One
transfer in each direction at a time (`busy`).

### Pairing introduction (TCP 27462, server-authenticated TLS 1.3)

`pair_intro {id, name, cert (PEM ≤ 4 KiB), port}` → `pair_intro_ok {id, name, cert}` or
`pair_refused {reason}`. One message each way, then close. Certificates are public data; what
authenticates them is the SAS comparison below.

### Pairing session (TCP 27461, mutual TLS 1.3 with the introduced certificates)

`pair_hello {id, name}` · `pair_commit {commit (64 hex)}` · `pair_reveal {nonce (64 hex)}` ·
`pair_confirm {accept}` · `pair_refused {reason}` · `error {reason}`.

### States

* Pairing (per computer): `waiting` → `connecting` → `confirm` (SAS shown) → `done` |
  `failed` | `expired`. One attempt at a time; 3 attempts per window, ≥ 2 s apart; the window lasts
  120 s; the mutual session must start within 15 s of the introduction.
* Peer link: `offline` → `online`; `identity` when the computer at the peer's address presents a
  different key (hard failure, see §7).
* Engine: per-side `armed`, `cooldown_until`, `pending arrival` (15 s).

## 5. Cursor placement and multi-monitor

On KDE Wayland the cursor is placed with a second uinput device, `LinOptions Flow pointer`
(EV_ABS `ABS_X`/`ABS_Y` 0…65535 + `BTN_LEFT/RIGHT/MIDDLE`, no `BTN_TOUCH`): udev tags it
`ID_INPUT_MOUSE` and libinput treats it as an absolute pointer. It exists only while Flow is on
(never in tests or demo mode, which use a dry-run device and record the events).

What KWin does with it: libinput scales each axis with `value × size / (max − min + 1)` and KWin
passes the size of the bounding rectangle of **all** outputs (`workspace()->geometry().size()`),
in logical pixels, measured from the global origin (0, 0). So one absolute device addresses every
monitor, in the same logical coordinates as `workspace.cursorPos` and `output.geometry` (scaling
included). LinOptions converts a global position with
`value = (x + 0.5) × 65536 / (bbox.x + bbox.w)` (same for y) and clamps it; the screen geometry
comes from the KWin script (`hello` / `screens`). Limitation: KWin normalises output positions so
the layout starts at (0, 0); an output left of or above the origin could not be reached (KWin does
not create such layouts). Xorg maps absolute devices over the whole X screen the same way, but on
X11 LinOptions simply uses `XWarpPointer`.

## 6. Clipboard: text, files and images

**Text.** At most 1 MiB of UTF-8, opt-out per computer (`clipboard` option, applies to sending
and receiving). After a successful switch the sender reads its clipboard (klipper
`getClipboardContents`, 0.5 s) and sends it unless it is identical to what was last exchanged with
that peer (SHA-256 kept in memory). The receiver sets it with klipper `setClipboardContents`. If
klipper is not running, the daemon keeps the text for the GUI: `FlowWatcher` (in the main window,
also when it lives in the tray) polls `flow_events`, takes the text and sets `QClipboard`, and offers
`QClipboard` changes to the daemon. That fallback only works while the app is running, and on
Wayland an unfocused app may not see clipboard changes. Clipboard contents are never logged.

**Copied files, folders and images** (`files` option, on by default, applies to sending and
receiving; plus the size limits above). After a successful switch, when the option is on, the peer
announced `transfer` and the clipboard holds:

* **files** — `text/uri-list` with local `file:` URIs (an empty or `localhost` host; anything
  else, such as `http:` or another host, is ignored). This is what Dolphin and every other file
  manager put on the clipboard for Ctrl+C, whatever way the copy was made. KDE's
  `application/x-kde-cutselection` is read only to know it was a *cut*; it is **always treated as a
  copy**, the source is never deleted;
* else an **image** — one of `image/png, image/jpeg, image/webp, image/bmp`, PNG preferred when
  several are offered (Qt applications, Spectacle included, offer every format Qt can write). On
  this machine (Plasma 6.6) the clipboard holding an image — most likely an earlier region
  screenshot, re-offered by Klipper (`application/x-kde-onlyReplaceEmpty`) — listed
  `application/x-qt-image`, `image/png`, `image/jpeg`, `image/webp`, `image/bmp` and about 30 other
  `image/*` types; reading `image/png` through data-control gave a valid PNG.
  Spectacle's own copy (Meta+Shift+S, region) goes through Qt's `QMimeData::setImageData`, which
  offers the same list — that capture itself was not triggered, to leave the desktop alone. If the
  clipboard also has text (`text/plain;charset=utf-8`, `UTF8_STRING`, `text/plain`) and the
  `clipboard` option is on, the text travels inside the same transfer (`transfer.text`) so both end
  up on the other clipboard together;

…the sender starts a transfer instead of sending the text (for files the text is just their paths
on this computer). Otherwise — nothing copied but text, the option off here, an older peer — the
text goes as before.

*Reading the clipboard.* On KDE Wayland a background process can't use `QClipboard` and klipper's
D-Bus API only knows text, so the daemon is a tiny client of the Wayland **data-control** protocol
(`ext_data_control_manager_v1`, else `zwlr_data_control_manager_v1`): it binds the seat and the
manager, keeps the MIME types of the current selection, and reads one through a pipe passed with
`SCM_RIGHTS` (`receive`), with a byte limit and a deadline. On X11 the bridge runs as a
`QGuiApplication` (platform xcb, no window) and answers `clip_formats` / `clip_read` / `clip_put`
with `QClipboard`; the data goes through private files (0600) the daemon names, never through the
pipe. Without either (another compositor, the bridge failed), the GUI offers the file URLs it sees
(`flow_clipboard_offer.uris`) while it runs; images are then not sent.

*Sending.* Files: the copied paths are walked with `lstat`/`scandir` without following anything —
links, devices, fifos and sockets are skipped and counted, as are names the receiver would refuse
(not UTF-8, control characters…); top-level names that collide get ` (2)`; only what the user
copied, and what is inside the folders they copied, is ever read. The caps are checked while
walking, before anything is hashed or sent (a clear notice says which). Then every file is hashed
(SHA-256, read in 1 MiB chunks; its device, inode, size and mtime are remembered), the manifest is
sent, and after `transfer_ok` each file is opened again with `O_NOFOLLOW`, compared with what was
hashed and streamed in 256 KiB chunks. A file that changed, shrank or disappeared is skipped
(`transfer_skip`) and the transfer goes on; the notice says how many. An image is first read from the
clipboard into a private file (0600, `flow-received/.spool/`), its signature and dimensions are
checked (below; a mismatching label is corrected to what the bytes are), then sent the same way and
deleted. The same files (paths + type/size/mtime of each) or the same image (SHA-256) are not sent
twice in a row to the same computer.

*Receiving.* Into a new private folder `$XDG_CACHE_HOME/linoptions/flow-received/<transfer id>/`
(`~/.cache/…` by default; the transfer id is `<unix time>-<8 random hex>`, chosen by the receiver):
the root and every folder 0700, every file 0600 (`fchmod` after creating it, whatever the umask —
never an executable bit), created with `O_CREAT | O_EXCL | O_NOFOLLOW` in a parent opened component
by component with `O_DIRECTORY | O_NOFOLLOW` (nothing can lead outside the folder, even if a link
appeared in it), mtimes restored (folders last, deepest first). Before accepting: the size limits
and free space (`statvfs`) are checked (`too_large`, `no_space`). Any error deletes the whole folder.
An image is saved as `image-<date>.<ext>` and then checked **without decoding it**: its signature
must be the declared type (`imageinfo.sniff`, also on the first data frame) and the header's
dimensions (PNG IHDR, JPEG SOFn, WebP VP8/VP8L/VP8X, BMP DIB) must be ≤ 16 384 px per side and
≤ 64 megapixels; otherwise it is deleted and the notice says why. Nothing received is ever opened,
decoded or run by LinOptions.

*Placing it on the clipboard.* KDE Wayland: a data-control source offering `text/uri-list`,
`x-special/gnome-copied-files` (`copy`), `text/plain;charset=utf-8` and `text/plain` (the paths)
for files, or the image's own MIME type (served from the staged file when someone pastes) plus
the text if any; the daemon keeps serving it until another selection replaces it. Qt applications
see an image (`hasImage`) and paste it; file managers paste the files, copying them into place.
X11: the bridge's `QClipboard.setMimeData` with the same types. Otherwise the GUI takes it
(`flow_clipboard_take`: URLs + text, or the image bytes read in a worker thread and set under their
MIME type, never decoded). If the clipboard can't be set, the notice offers *Open folder*. What
was received is remembered so crossing back doesn't send it back.

*Clean-up.* Received transfers are deleted after 24 h (checked when Flow starts and every 10
minutes), and all of them when Flow is turned off. A pasted copy is the user's; the staged one
goes away anyway, so a paste after a day finds nothing.

*Progress.* `flow_status` / `flow_events` list the last 8 transfers (direction, kind, counts,
bytes, percent, state, folder). The app shows a card at the bottom of its window while it is in
front — *“Sending 3 files to portatil-ana… 45%”* with *Cancel*, *“Received 3 files from
portatil-ana — paste them with Ctrl+V”* with *Open folder* — otherwise a desktop notification
(`org.freedesktop.Notifications`, updated in place, same actions). Failures and refusals are
ordinary Flow notices that say why. `linoptions-ctl flow status` shows the transfer under way and
`flow cancel` cancels it.

## 6b. Dragging files across the edge

Drag a file or folder in the file manager to the edge that leads to another computer and keep
holding the button: the mouse crosses and, on the other computer, the drag goes on where the
pointer arrives; drop the files wherever you like. Always a **copy**. If the drag can't go on
there, the files arrive as copied files (Ctrl+V). Option `drag_files` (*Drag files across the
edge*, on by default; it needs `files` too), per computer: off, nothing glows, no catcher is shown
here, `drag` isn't announced and notices are ignored. Design and what is left for other desktops:
`docs/FLOW-DRAG-DROP.md`.

**The glow (KDE Plasma on Wayland).** As soon as files are picked up anywhere, every outer edge of
the desktop on a side whose peer could take them (the catcher's conditions below) glows with the
catcher's Iris glow, until the button is let go, the files cross, or 30 s. A Wayland client can't
see other clients' drags, so Glisa asks the compositor at the one moment one can have started:

1. **The left button.** While the option is on, Glisa reads the Logitech mouse's evdev node as
   events (never grabbed: the desktop gets every event as before). The kernel's per-client event
   mask (`EVIOCSMASK`) lets through only `BTN_LEFT`, and `REL_X`/`REL_Y` only from a left press
   until that press is decided; nothing else of the device reaches Glisa (no other button, no
   wheel, no key), and a node whose mask can't be set is not read. Nodes that declare letter keys
   (a keyboard) or no X/Y motion are never opened. Motion isn't even a wakeup otherwise.
2. **The probe.** After 12 device units of motion with the button held (the usual drag distance),
   and only if some edge would glow, an invisible `zwlr_layer_shell_v1` surface goes up over each
   output (overlay layer, a 1×1 transparent buffer stretched with `wp_viewporter`, the whole
   surface as input region, no keyboard) for at most 150 ms once it is on screen. A drag and drop
   in progress enters it at its next motion (`wl_data_device.enter`); its offer's MIME types are
   the only thing looked at: `text/uri-list` means files. It is destroyed the moment a drag enters
   it, and never accepts, receives data or finishes, so the drag goes on to whatever is under the
   pointer at the next motion. A press that isn't a drag (a text selection, a rubber band, a
   window being moved) keeps its implicit grab, never enters it, and nothing glows. A press whose
   drag started later gets one more probe at 48 units; never more than two per press.
3. **The glow strips.** Layer surfaces along those edges with an **empty input region**, so they
   never take part in input or drag and drop, showing `glow_pixels` (the catcher's look exactly).

Side effects, measured on a nested KWin 6.6: for the few milliseconds between the probe's enter
and the next motion, the drag has no target (its source sees `target` none, so the cursor may show
"no drop" for a frame); if the button is let go in that very instant, the drop lands on the probe
and is refused, so that one drag is cancelled (nothing is copied or moved). The probe also costs an
invisible full-screen surface for a moment on a press-and-move (for a fullscreen game, possibly a
frame of compositing).

**Sending (KDE Plasma on Wayland).**

1. A button is held at an edge with a peer behind it (outside the corners, not in the cooldown,
   with Ctrl if required), the peer is online and announced `transfer`, the mouse is here, both
   options are on → the **edge catcher** goes up: a `zwlr_layer_shell_v1` surface (overlay layer,
   exclusive zone −1, no keyboard), 120 px thick but taking input only on its outer pixel, fully
   transparent until files are dragged onto it (then its own glow along the edge, brighter over the
   ambient one), anchored to that edge of the output under the pointer (`zxdg_output_manager_v1`
   geometry). It binds no pointer or keyboard and is never focused, so it can't take an ordinary
   click (a press elsewhere keeps its implicit grab). It goes away when the button is let go
   (`EVIOCGKEY`, every 40 ms while it is up) or the pointer leaves the edge; while a drag is on it,
   only once that drag is over (or 15 s).
2. A drag entering it (`wl_data_device`) with `text/uri-list` is read through a pipe (≤ 4 MiB,
   within 300 ms; slower is a plain drag) and parsed: local `file:` URIs only (empty or
   `localhost` host), percent-decoded, no query or fragment, and **any `.` or `..` component drops
   the URI** (not normalized). The drop isn't accepted yet. Any other drag is refused and nothing
   crosses, as before.
3. Files on the catcher glow there for 0.3 s (pulling back from the edge cancels), then they are
   the only case where the pointer crosses with a button held: a short
   `drag` notice (the first names, as the transfer will name them; how many; the bytes counted in
   at most 0.25 s, a lower bound for a huge folder) to a peer that announced `drag` (≤ 0.6 s, a
   failure doesn't stop the crossing) → `switch` → the catcher accepts the drop with
   `text/uri-list` and the **copy** action only → `set_host`. If the drag left the catcher (the
   button was let go) before that, the mouse stays.
4. The mouse leaving makes the compositor let go of the button (the kernel releases what a gone
   device held), which drops on the catcher; it answers `finish` at once (copy), so the file
   manager never deletes anything. The paths go with `send_paths(peer, paths, Drag)`: the same
   transfer connection, walk (no links followed), caps, hashing and notices as copied files.

**Receiving: the drag goes on (KDE Plasma on Wayland).** A `switch` that follows a `drag` notice
from the same peer (within 5 s) puts up the **pickup**: an invisible layer surface over the output
where the pointer will enter (overlay, 1×1 transparent buffer stretched, the whole output as input
region, no keyboard), on a second Wayland connection that alone binds a `wl_pointer` (an ordinary
arrow through `wp_cursor_shape_v1` while the pointer is over it). The mouse reconnects with its
button still held; the first press of the left button on the pickup, **once our mouse has arrived**
and **only if its evdev node says its left button is down** (`EVIOCGKEY`, looked for again since
a mouse that came back has a new node; up to 0.4 s while that press lasts), starts a drag from the
pickup with that press's serial (`wl_data_device.start_drag`): `text/uri-list` of the received
items' `file://` URIs in the staging folder (each byte outside `[A-Za-z0-9/_.~-]`
percent-encoded), the **copy** action only, a small Iris document icon. The pickup then takes no
input (empty input region) and goes when that drag ends, so the drag goes wherever the user takes
it. A drop target's `send` gets the list once every file is here (waiting up to 1 s for the
transfer); otherwise it gets nothing and the files fall back to the clipboard.

**Dropped on KDE (Dolphin, the desktop): a copy, no menu.** KIO asks *Copy here / Link here /
Cancel* for any dropped files, and a link into the staging folder would break. So the drag also
offers Ark's two types, as Ark does: `application/x-kde-ark-dndextract-service` (Glisa's unique
name on the session bus) and `application/x-kde-ark-dndextract-path` (`/Flow/Drop/<32 random
hex>`, an object of that drag only). With them KIO and Dolphin skip the menu and call
`org.kde.ark.DndExtract.extractSelectedFilesTo(folder)` there (`linux/arkdrop.rs`): only a caller
of the same user is served, once (the call takes the object off the bus; it goes anyway when the
pickup ends), into an existing local folder the user can write to (canonicalized, never inside the
dragged files), once every file is here (up to 60 s): a copy, folders recursively, nothing
overwritten (`name (2).ext`), no link followed (everything opened and created relative to its
folder, `O_NOFOLLOW`, `O_EXCL`; links and special files skipped), modes as staged without execute
bits, times kept; on an error what it created is removed and the caller gets a D-Bus error. The
answer comes within 2 s, since Dolphin waits for it while dropping (a copy still going on then
finishes on its own). The drag is taken only if that copy succeeds; when the target read the
object but doesn't call within 3 s of the drop, its URI list decides as before. Other drop targets
just use the URI list.

The pickup never lingers, since it would swallow clicks of other pointing devices on that output:
at most 8 s waiting for the mouse, then 3 s for its press (the user had let go); a pickup whose
mouse never came holds the next one off for 30 s, and none goes up while a Logitech pointer of
ours is online here (no drag can be coming with it then). A press counts only if the mouse's
own evdev node confirms its left button is down, so a touchpad press is ignored; it may come
before the mouse's arrival is noticed (behind a receiver, or with a slower Bluetooth stack), and
then still counts. Whether the held button of a particular mouse
arrives as a press when it reconnects depends on its firmware and on how fast KWin opens its new
node; `GLISA_LOG=debug` logs every step (`Flow: pickup: …`).

The next `transfer` from that peer within 120 s whose first top-level name is one of the announced
names carries the dragged files (`reason: drag`); anything else is ordinary copied files. It is
received exactly like copied files (private staging folder, every check of §6 and §7). While the
pickup may still take them they wait (the transfer shows *Receiving… 100%*); a drop that got the
list, or a KDE drop whose copy succeeded, ends the transfer as `placed: drop` (*Received 2 items
from A*, nothing on the clipboard). Otherwise (no press, the drag cancelled, the files not all
here when dropped, a KDE copy that failed, or a desktop without the pickup) they go on the
clipboard with the usual *paste them with Ctrl+V* notice.

## 7. Security model

**Identity.** One long-lived X.509 identity per installation: ECDSA P-256 key + self-signed
certificate generated with the `openssl` CLI (`req -x509 -newkey ec`, notBefore in 2020 so a peer
with a lagging clock accepts it, 25-year validity). Files in `~/.config/linoptions/flow/`
(directory 0700, `key.pem`, `cert.pem`, `flow.json` all 0600, written atomically). A peer is
identified by the **pin**: SHA-256 of its certificate's SubjectPublicKeyInfo, stored at pairing
together with its certificate. `flow.json` entries whose pin does not match their certificate are
dropped on load.

**Transport.** TLS 1.3 only (`minimum_version = TLSv1_3`; TLS 1.2 is refused), no compression, no
session tickets, ECDHE (forward secrecy). Both sides `CERT_REQUIRED`:

* the server's trust store holds only the pinned peers' certificates (plus, for one pairing
  attempt only, the introduced candidate);
* a client trusts only the certificate of the peer it is calling;
* hostnames are irrelevant; after every handshake the peer's SPKI pin is compared with the stored
  one — mandatory on every connection. A mismatch is a hard failure: no connection, the peer shows
  *"Identity changed"*, the user gets a notice, and the mouse is never switched.

**Why certificates rather than TLS-PSK keyed by the 6-digit code**: a low-entropy PSK can be
brute-forced offline from one captured handshake (the PSK binder is a MAC keyed by it). Per-
installation certificates give standard mutual TLS 1.3, need no secret shared over the network,
and work with Python ≥ 3.10 (`ssl` PSK callbacks need 3.13).

**Why a separate introduction listener**: Python's `ssl` cannot accept a not-yet-known client
certificate under `CERT_REQUIRED` and has no per-connection verify callback; switching contexts in an
SNI callback keeps the original verify mode (verified with OpenSSL 3.5). So the (public)
certificates are exchanged once over a server-authenticated TLS connection on TCP 27462, which is
only open while the user has pairing open; everything authenticated, including the pairing
session itself, runs on TCP 27461 with `CERT_REQUIRED` on both sides.

**Pairing (short authentication string with commitment, as in ZRTP RFC 6189 / Bluetooth numeric
comparison).**

1. The user opens *Pair a computer* on both computers (B must have it open; A opens it by
   choosing B in the list).
2. Introduction: A → B:27462, `pair_intro {id, name, cert, port}` → `pair_intro_ok {id, name,
   cert}`. A checks that the certificate in the answer is the one B used in that handshake.
3. Mutual TLS 1.3 session A → B:27461 with exactly those two certificates; both pins are checked.
4. `pair_hello` both ways (ids must match the introduction).
5. Commit, then reveal — both commitments are sent before either nonce:
   `A: pair_commit = H("linoptions-flow-commit-v1"‖"initiator"‖pinA‖nA)`,
   `B: pair_commit = H(…‖"responder"‖pinB‖nB)`, `A: pair_reveal nA` (B verifies),
   `B: pair_reveal nB` (A verifies). Nonces: 32 random bytes; H = SHA-256.
6. `SAS = int(SHA-256("linoptions-flow-sas-v1"‖pinA‖pinB‖nA‖nB)[0:8]) mod 10⁶`, shown as six
   digits on **both** computers.
7. The user compares and confirms on **both**: each side sends `pair_confirm {accept}`. A computer
   stores the peer only after it has sent its own "yes" and received the other's. Any "no",
   cancellation, timeout or expiry aborts and stores nothing.

A man in the middle has to present its own certificate on each leg, so the two SAS values cover
different pins; because every nonce is committed before any is revealed, it cannot steer either
SAS after seeing the other side's values, so the two numbers match with probability 10⁻⁶ per
attempt — and attempts are limited (3 per 2-minute window, one at a time, with the user at both
computers). A relaying attacker that forwards commitments verbatim fails earlier: commitments bind
the sender's own pin and role.

**Local surface.** The daemon's Unix socket stays 0600 and the daemon checks the client's UID with
`SO_PEERCRED`, refusing any other user; the GUI and `linoptions-ctl` likewise refuse a socket whose
listener is another user. Runtime files (that socket, the GUI's single-instance socket, the KWin
script) live in `$XDG_RUNTIME_DIR`; without it, in `/tmp/linoptions-<uid>`, used only if it is a
real directory of ours with mode 0700 (otherwise nothing starts and the error says why). The KWin
script is created with `O_EXCL | O_NOFOLLOW`. Same-user processes are trusted (they could already
read `key.pem`); this includes the session bus, where any same-user process could call the
bridge's `Event`. Never logged: clipboard contents, keys, nonces, SAS values. Received files live in
`$XDG_CACHE_HOME/linoptions/flow-received/` (0700, checked to be a real directory of ours; files
0600).

**A paired computer can now send you files.** Only while *Bring copied files and images along* is
on (the default; turn it off to refuse every transfer), only up to your size limits, and only into
the private staging folder: they are never executed, opened or decoded by LinOptions, nothing can be
created outside that folder (paths are checked before anything is written, every component is
opened without following links), no link, device, fifo or executable file can be created, and they
stay there until you paste them (Ctrl+V copies them where you paste) or until they are deleted after
a day. A received image is checked by its signature and header dimensions before it goes on your
clipboard, where the application you paste into will decode it.

**Names are untrusted.** Computer names come from announcements, the introduction and `hello`
(anyone on the LAN can choose them). Non-printable and markup characters (`< > &`) are dropped
before they are stored or shown, and the GUI shows every label as plain text, so a name cannot
restyle the pairing dialog (for instance, put a big fake six-digit code next to the real one).

**Housekeeping.** Removing a peer deletes its pin, certificate, layout and channel choices, ends
any session it has open with this computer (every message on a served session is checked against
the current pins), and best-effort tells the peer (`unpair`), which forgets us too. *Crear una
identidad nueva* (`flow_reset_identity`) makes a new id and key pair and unpairs everyone.

### Threat model

| Attacker | Can | Cannot |
|---|---|---|
| Passive, on the LAN | see announcements (id, name, version, port) and that two computers talk | read or alter anything inside TLS 1.3 (forward secrecy) |
| Active, on the LAN, after pairing | spoof announcements (address hints), drop/flood traffic → Flow refuses to switch (the mouse is never lost), make the peer look offline, trigger an "Identity changed" notice by answering at a hinted address | impersonate a paired peer without its private key, inject, replay or reorder messages, use any other port (only 27461 listens), change a stored address (only a connection that passed the pinned TLS handshake teaches one), make us send directed announcements anywhere but to paired computers' addresses |
| Man in the middle during pairing | relay with its own certificate | get paired without the users seeing different six-digit codes (10⁻⁶ per attempt, 3 attempts per window) — **if the users actually compare them** |
| Stolen laptop (disk readable) | use `key.pem` (unencrypted, 0600) to impersonate that laptop to its paired peers: push clipboard text, files and images to them (into their staging folders, within their limits) and receive yours when you move the mouse there | anything once the owner removes it on the other computers (deletes the pin) or creates a new identity there; use disk encryption |
| Malicious paired peer | push clipboard text at any moment, not only after a switch (≤ 1 MiB, only if clipboard sharing is on; never executed); push files or an image at any moment into the staging folder, up to your size limits and free space minus 64 MiB, one transfer at a time, and put them on your clipboard (only if the files option is on; never executed or opened); receive your clipboard (text, the files or image you copied) when you switch to it; refuse switches; make the cursor jump to an edge only when a device actually arrives; after a `drag` notice and a `switch`, put up the invisible pickup on one output for up to 8 s (11 s once a device arrives), during which clicks of other pointing devices there go nowhere (not while our Logitech pointer is here; at most once per 30 s when no device arrives; only while the drag and files options are on) | send key presses or pointer motion, switch your devices (only this computer sends `set_host`, only on a local edge event), read files you didn't copy, write outside the staging folder, create links, devices or executable files, overwrite anything, crash the daemon with malformed or oversized messages |
| Other local users | – | use the daemon socket (0600 + `SO_PEERCRED`) or read `~/.config/linoptions/flow/` |

Addresses — typed, learned or announced — are only **hints** for where to connect: the pinned
TLS session decides who is at the other end, so a wrong or spoofed address can at most make a peer
look offline (or show *Identity changed*), never pair, impersonate or receive anything.

Not protected: availability (anyone on the LAN can disrupt Flow), metadata (who talks to whom and
when, host names in announcements, which now also travel unicast to paired computers' addresses, and
the size and timing of transfers), a user who confirms the code without comparing, a compromised
OS or root, clipboard text and copied files once they are on the other computer, and what an
application does with an image or file the user pastes into it (a decoder bug there is that
application's). The channel itself is authenticated
end to end, but HID++ "change host" over Bluetooth/Unifying radio is whatever the device firmware
provides.

## 8. Daemon API and CLI

API methods (JSON over the Unix socket; see DESIGN.md §6 for the table): `flow_status`,
`flow_enable`, `flow_peers`, `flow_discover`, `flow_pair_start`, `flow_pair_request`,
`flow_pair_confirm`, `flow_pair_cancel`, `flow_remove_peer`, `flow_set_peer_address`, `flow_set_layout`,
`flow_set_options`,
`flow_set_channel`, `flow_events`, `flow_clipboard_take`, `flow_clipboard_offer`, `flow_transfer_cancel`,
`flow_reset_identity`, `flow_simulate` (fake
backend only). `flow_*` calls do not hold the daemon's
global lock (some wait on the network).

```
linoptions-ctl flow status [--json]          linoptions-ctl flow pair [EQUIPO|IP:PUERTO]
linoptions-ctl flow on | off                 linoptions-ctl flow unpair EQUIPO
linoptions-ctl flow peers | discover         linoptions-ctl flow layout EQUIPO left|right|top|bottom|none
linoptions-ctl flow options [--ctrl on|off] [--clipboard on|off] [--files on|off] [--drag on|off]
                            [--keyboard on|off] [--cooldown MS] [--corner PX] [--max-size MB]
                            [--image-max MB]
linoptions-ctl flow cancel                   (the copied files or image being sent or received)
linoptions-ctl flow channel DISPOSITIVO EQUIPO N|auto
linoptions-ctl flow address EQUIPO [HOST[:PORT]|auto]   (no value: show the address in use)
linoptions-ctl flow reset-identity           linoptions-ctl flow test-edge SIDE [T]   (demo only)
```

`flow pair` without argument opens pairing on this computer and waits; with an argument it pairs
with that computer. Either way it prints the six digits and asks *"Do they match? [y/N]"*.

## 9. Testing

`tests/flowutil.py` runs several LinOptions "computers" in one process: each is a daemon with its
own config dir, Unix socket, loopback-only TCP/UDP ports, a `FakeBackend` (fake edges, dry-run
absolute pointer, in-memory clipboard). A `SimHub` moves **one** `SimulatedMouse` between them when
it receives `set_host`: its connection to the old computer closes and a fresh `SimEndpoint` attaches
to the computer registered for that channel. `tests/test_flow.py` covers pairing (matching SAS,
rejection, expiry, busy/rate limit, MITM relay with its own certificate), discovery, crossing in both
directions with entry positions and absolute values, clipboard (transfer, opt-out, size cap),
unreachable/disabled peer, cooldown, hysteresis, corners, Ctrl-only (and without Ctrl support),
channel suggestion and explicit choice, keyboard follows, held keys released, TLS 1.2 refused,
unknown certificate refused, pin mismatch, replayed/out-of-order/oversized/malformed messages,
other-UID socket client refused, unpair and identity reset; plus the KWin loader with a fake bus,
the real script in `node` against the Python tracker, the keystate client against a fake
compositor, and the X11/KWin backends with fakes. `tests/test_gui_flow.py` drives the page.
`tests/test_security.py` holds the regression tests of the security review (frame caps and
deadlines, nested JSON, sessions of removed peers, markup in names, private runtime files).

Copied files and images (`tests/test_flow.py`): manifest validation (absolute paths, `..`, control
and bidi characters, long names, depth, duplicates, orphans, link/device/fifo types, lying sizes,
kind mismatches), `hello.caps`, binary data frames (replay, wrong channel, oversize), image headers
(PNG, JPEG, three WebP variants, BMP, truncated), `text/uri-list` parsing, the staging folder
(permissions, clean-up) and `Incoming` (order, offsets, extra or missing bytes, checksums, a link
planted mid-transfer), the sender's walk (links and fifos skipped, caps, name collisions), the
data-control client against a fake compositor (formats, pipes and `SCM_RIGHTS`, own selection
served, cancellation), and between two daemons: files and nested folders arrive intact (hashes,
mtimes, 0600/0700) with the receiver clipboard set, no echo back nor resend, caps with clear
notices on both sides, malicious manifests and streams from a raw authenticated client, a switch
and a ping during a slow transfer, cancellation on either side, a file deleted mid-transfer, an
older peer without `caps` (text only), the option off on either side, staging clean-up, an image
round trip (bytes identical, text along), image type/magic mismatch and over-size/over-dimension
images. `tests/test_gui_flow.py`: the option and size limit in English, the progress card (Cancel,
Open folder), desktop notifications (with a fake notifier) and the clipboard fallback for files and
images.

Dragging files (Rust): `crates/flow/src/drag.rs` (drag URI lists: percent-encoding, other schemes
and hosts, `.`/`..`; the notice summary), `proto/tests.rs` (`drag` validation), `geometry.rs`
(the outer edges that glow, partial ones included), `linux/input.rs` (the evdev reader: left
button, motion only when asked, `SYN_DROPPED`, a node that goes away with the button held, the
wake-up; only mice are opened; the `EVIOCSMASK` number), `linux/dragwatch.rs` (the press → probe
→ report state machine: nothing without motion or without an edge to glow, text drags, a second
chance, a missed release), `linux/catcher.rs` against a fake compositor (placement, transparent
buffer, copy-only acceptance after the engine says so, plain and slow drags refused, lingering
until the drop; the probe maps over every output and is destroyed as soon as a drag enters it,
never accepting or reading, a uri-list drag is files, text or no drag is nothing, a drop on it is
refused; glow strips with an empty input region and exactly `glow_pixels`, gone when asked; the
pickup: an arrow cursor, presses before the arrival or not confirmed by the mouse ignored, the
drag started with the press's serial, copy only, with its icon, no input afterwards, the list
handed over only once ready and within 1 s, finished / cancelled / late reported as taken or not,
extra MIME types written at once), `linux/arkdrop.rs` (the KDE drop: the copy into a folder with
names already taken, modes and times, links never followed on either side, destination checks,
all or nothing, waiting for the files, the answer within 2 s, taken only if the copy succeeded,
and the D-Bus object on a private `dbus-daemon`: an error for a bad folder, one call only),
`tests/drag.rs` between two services (the glow on every outer edge of the peer's side while files
are dragged, only under the catcher's conditions, gone on release, after its timeout or when the
option goes off, and the edge still crossing with it up; a file drag crosses, a plain drag doesn't,
no peer or option off means no catcher, released before the switch; on the other computer the
pickup on the entry output, armed by the arrival, the files held until the drop takes them
(nothing on the clipboard) or put there when no press comes or the drag is cancelled; an older
peer without `drag`, the notice and its matching, the option round trip), and
`tests/live_desktop.rs` in a nested `kwin_wayland --virtual` with fake input: a real drag reaches
the edge, the KWin script still reports it, the catcher maps, reads the files, and the drop
finishes as a copy; a text drag is refused; a plain press-and-move gets two probes, nothing, and
keeps its grab; a real file drag is found by the first probe, the edge glows, and the drop still
lands on a folder under the glow, also when the probe went up right over that folder; with the
glow up the catcher still takes the files; the pickup starts a real drag from a fake press that a
folder client reads as the right uri-list, ignores a press before the arrival, goes away without
a press (clicks reach the folder again), and hands over nothing when the files aren't ready; a
folder that behaves like KDE's reads Ark's two types and calls the drag's object over the nested
session bus during the drop: the files land there (`foto 1 (2).jpg` beside the one already
there), a second call finds nothing, and dropped before the files are here they are copied once
they arrive.

### What needs two real computers (manual test plan)

1. On both computers: install, open LinOptions, check the mouse is paired with both (channel 1 on
   A, channel 2 on B); ideally give each Easy-Switch channel the computer's host name.
2. On both: *Flow* (⇄ in the top bar) → turn it on. If `ufw` is active, add the rule the page shows.
3. On B: *Pair a computer*. On A: *Pair a computer* → B appears → *Pair*. Both show the
   same six digits → *They match* on both. Both list each other as *Online*.
4. On A drag B to the right; on B drag A to the left. Check *Channels* suggests the right channel
   (or choose it).
5. Move the pointer to A's right edge, mid-height: the mouse goes to B within ~1–3 s and B's cursor
   appears on its left edge at the same height. Repeat from B's left edge back to A. Try near a
   corner (nothing happens) and immediately back (nothing during the cooldown).
6. Copy text on A, cross to B, paste: the text is there. Turn off *Bring the clipboard along* on B and
   repeat: it is not.
7. Turn off B's Flow (or unplug its network) and push A's edge: the mouse stays on A with a notice.
8. *Cambiar solo con Ctrl pulsado*: pushing the edge does nothing until Ctrl is pressed.
9. With a Logitech keyboard paired on the same channels and *Keyboard switches too* on, it
   follows the mouse; with it off, it stays.
10. Drag a window to the shared edge (quick tiling): the mouse does not switch.
11. Multi-monitor / scaling: with two outputs (or a scaled output) on B, check the cursor lands on
    the outermost output at the right height.
12. *Remove* B on A: B also drops A. Pair again. On B, *Create a new identity*: A shows
    *Identity changed* and refuses to switch until paired again.
13. Copied files: on A, select a folder with subfolders and a couple of files in Dolphin, Ctrl+C,
    cross to B. A shows *Sending … to B… N%* (in the window, or as a desktop notification when
    it's in the background) and B *Received … — paste them with Ctrl+V*. In a Dolphin window on B,
    Ctrl+V: the files and folders appear with the same contents and dates. Check
    `~/.cache/linoptions/flow-received/` on B (0700) and that nothing there is executable.
14. Screenshot: on A press Meta+Shift+S, select a region, cross to B and Ctrl+V into Gwenview,
    a chat or LibreOffice: the same image is pasted. Copy an image in Firefox on B, cross back to
    A and paste it into an application there.
15. Big copy (a few GB, under the size limit): while it is sending, push the mouse back from B to
    A — it switches at once — and press *Cancel* on either computer: both say it was cancelled and
    B's staging folder has nothing of it. Copy something over the size limit: A explains why it
    wasn't sent. Lower the limit only on B: A says B has a lower limit.
16. Delete or edit one of the copied files right after crossing with a large copy: the others
    arrive, and A says how many couldn't be sent.
17. On B turn off *Bring copied files and images along*: copying files on A and crossing sends
    nothing (A says it's turned off there); copied text still follows. Turn Flow off on B: its
    staging folder is emptied.
18. Different subnets (one on Wi-Fi, one wired, behind a router): pair with *Pair by address…*.
    Move one computer to another network so its stored address goes stale: it shows *Offline*,
    and after a few seconds the row says it may need its address. *Set address…* on **one** side
    only: both come back *Online* (the other side learns the address from the connection), and
    the row shows the address with *set manually* / *found automatically*.
19. Dragging files (KDE Plasma on Wayland, both computers, `GLISA_LOG=debug` on B): in Dolphin on
    A, pick up a file and a folder: A's right edge glows softly at once (and only then). Drag them
    to that edge and keep pushing, **without letting go**: the mouse goes to B, and the drag goes on
    there: an Iris document icon follows the pointer; drop them into a Dolphin folder on B. They
    are copied there (the originals stay on A), B's clipboard is untouched and its card says
    *Received 2 items from A*. Let go at B's arrival instead: after 3 s the files are on B's
    clipboard (*paste them with Ctrl+V*). B's log tells which happened (`Flow: pickup: …`: the press
    received, the left button confirmed on the mouse, the drop). Drag text from a browser, a
    window, or select text: nothing glows. Let go of the button at the edge before it crosses:
    nothing crosses. Turn *Drag files across the edge* off on B: the files still arrive (Ctrl+V);
    off on A: nothing glows and nothing crosses with a button held.
