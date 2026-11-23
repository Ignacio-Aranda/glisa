# Flow: drag files across the edge

**Status: implemented on KDE Plasma (Wayland).** Behaviour, protocol and security notes:
`docs/FLOW.md` §6b. X11, Windows and macOS build and run without it (see *Other desktops*).

**Goal.** Pick up a file or folder in the file manager: the edges that lead to another computer
glow, saying "drag it here and it goes over". Drag it to such an edge and keep holding the
button: the mouse crosses, and on the other computer the drag simply goes on; drop it wherever
you like. It always **copies**, never moves. Option *Drag files across the edge* (`drag_files`,
on by default; `glisa-ctl flow options --drag on|off`).

## 1. While files are dragged anywhere: the glow

A Wayland client can't see other clients' drags, so Glisa asks the compositor at the one moment
a drag can have started (`crates/flow/src/linux/dragwatch.rs`, `catcher.rs`):

- **The left button, as events.** The Logitech mouse's evdev node, read as events, never grabbed.
  The kernel's event mask (`EVIOCSMASK`) lets through only `BTN_LEFT`, and `REL_X`/`REL_Y` only
  from a press until that press is decided; motion isn't even a wakeup otherwise. Nodes with
  letter keys (keyboards) or without X/Y motion are never opened; a node whose mask can't be set
  isn't read. Only while the option is on.
- **The probe.** 12 device units into a press-and-move (and only if some edge would glow), an
  invisible layer surface over every output, for at most 150 ms on screen: a drag and drop going on
  enters it at its next motion. `text/uri-list` in the offer means files; nothing else is looked
  at, nothing accepted or read, and the probe goes the moment a drag enters. Selections, rubber
  bands and window moves keep their implicit grab and never enter it. A second probe at 48 units
  catches a drag that started late; never more than two per press.
- **The glow.** Layer surfaces along every outer edge of the desktop on a side whose peer is
  online and takes transfers (the mouse here, both options on), per output, partial stretches
  included, with the catcher's `glow_pixels` exactly, and an **empty input region**: they never take
  part in input or drag and drop. They go when the button is let go, the files cross, or 30 s.
- **Side effects** (nested KWin 6.6): between the probe's enter and the next motion (a few ms) the
  drag has no target, so its source sees `target` none and the cursor may show "no drop" for a
  frame; a release in that very instant drops on the probe, which refuses it (that drag is
  cancelled, nothing copied). A fullscreen game may lose a frame of direct scanout to the probe.

## 2. At the edge: the catcher

- **What it is.** A `zwlr_layer_shell_v1` surface (overlay layer) anchored to the edge of the
  output under the pointer, on its own Wayland connection (`crates/flow/src/linux/catcher.rs`,
  wire-level like the rest of `linux/`, no new crates). It is 120 px thick but its **input region
  is only the outer pixel**, so it never covers anything; it stays fully transparent until files
  are dragged onto it.
- **What it shows.** While files are on it, its own **Iris glow** along that edge, on top of the
  ambient one, so brighter there: a soft purple shadow fading inwards with a thin light line on
  the edge, fading out towards both ends (`glow_pixels`). The files stay there, glowing, for 0.3 s
  before the mouse crosses (`DRAG_DWELL`): the glow is seen, and pulling back cancels.
- **When it exists.** Only while a button is held at an edge that leads to a paired, online peer
  that takes transfers (and the mouse is here, both options on). It goes when the button is let go
  (`EVIOCGKEY`, polled every 40 ms while it is up) or the pointer leaves the edge. It binds no
  pointer or keyboard and is never focused, so it never takes an ordinary click.
- **What it reads.** On drag-enter, `text/uri-list` (≤ 4 MiB, within 300 ms): local `file:` paths
  only, percent-decoded, `.`/`..` components refused. Anything else, or nothing within 300 ms, is
  a plain drag: the crossing stays suppressed, as before.
- **The drop.** Accepted only once the crossing goes ahead (after `switch_ok`), with the **copy**
  action only. The switch makes the compositor let go of the button (the mouse is gone), so the
  drop lands on the catcher, which answers `finish` at once. A drag that left the catcher before
  that means nothing crosses.

## 3. Transfer

- Reuses the pinned-TLS transfer (`send_paths(peer, paths, Drag)`): SHA-256, size, count and
  depth caps, private staging, the same notices.
- A `drag` notice (first names, count, bytes) goes out *before* the switch, only to peers that
  announce the `drag` capability; older peers get the files as copied files. Small files usually
  finish during the 1–3 s Bluetooth reconnect.

## 4. On the other computer: the drag goes on (the pickup)

The user keeps holding the button while the mouse changes host. When it reconnects there, its
first reports carry the held button, which the compositor can see as a fresh press:

- **The pickup.** The `switch` that follows a `drag` notice puts up an invisible layer surface
  over the output where the pointer will enter (the whole output as input region, no keyboard), on
  a second connection that alone binds a `wl_pointer`.
- **The press.** Once our mouse has arrived, its left press on the pickup, confirmed by the
  mouse's own evdev node (`EVIOCGKEY`: so a touchpad click isn't taken for it), starts
  `wl_data_device.start_drag` with that press's serial: `text/uri-list` of the received items'
  `file://` URIs, copy only, a small Iris document icon. The pickup then takes no input, so the
  drag goes wherever the user takes it.
- **The data.** A drop target's `send` gets the list once the transfer is complete (waiting up
  to 1 s). Otherwise nothing is handed over and the files go on the clipboard instead.
- **Never lingering.** The pickup would swallow clicks of other pointing devices, so it is up for
  at most 8 s waiting for the mouse and 3 s for its press; then it goes, and the files are put on
  the clipboard exactly like copied files, with the usual *paste them with Ctrl+V* notice. The
  same happens when the drag is cancelled, or on a desktop without the pickup.
- **What only real hardware tells.** Whether a given mouse's held button arrives as a press on
  reconnect (firmware, Bluetooth or receiver, how fast KWin opens the new node) can't be tested in
  a nested compositor. `GLISA_LOG=debug` logs each step (`Flow: pickup: …`).

Verified on a nested `kwin_wayland --virtual` (KWin 6.6.6, `crates/flow/tests/live_desktop.sh`):
the KWin script keeps reporting the edge during a drag; layer-shell surfaces are drop targets; the
probe finds a real file drag at its first look and a plain press-and-move keeps its grab; drops
still land on the client under the glow (also right after a probe over it); the catcher reads the
uri-list and the drop finishes as a copy; a press on the pickup starts a real drag that another
client drops and reads as the right uri-list.

## 5. Other desktops (left to do)

- **X11.** Catcher: an override-redirect XDND-aware 1-pixel window on the edge, mapped while a
  button is held (XQueryPointer already reports buttons); read `text/uri-list` on `XdndEnter` /
  `XdndPosition`, answer `XdndStatus` with `XdndActionCopy` once the crossing goes ahead. Glow:
  XFixes/XInput could see the drag; the pickup would need an XDND source started from a grab.
  Until then X11 receives dragged files as copied files.
- **Windows.** A topmost layered window with `IDropTarget` (`CF_HDROP`), `DROPEFFECT_COPY`;
  buttons from `GetAsyncKeyState`. Pickup: `DoDragDrop` with a `CF_HDROP` data object while the
  button is held. Until then Windows doesn't announce `drag` and receives dragged files as copied
  files.
- **macOS.** An `NSWindow` registered for file URLs (`NSPasteboardTypeFileURL`), copy operation
  only; pickup with `NSDraggingSession`. Also still needs the HID++ host switch over BLE.
- **Other Wayland compositors** with layer-shell (wlroots, Hyprland) would only need edge
  detection; GNOME has no layer-shell.

## 6. Later

A real **move**: delete the original only after the target confirms the SHA-256. Off by default.
