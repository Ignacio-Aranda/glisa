# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [0.1.0] — Unreleased

First release of **Glisa**: a configurator for Logitech mice and keyboards for Linux, Windows
and macOS, one installer per system.

### Added
- **Platforms and installers.**
  - Linux: `.deb` and `.rpm` (which install the device permissions themselves) and AppImage.
  - Windows: `.exe`/`.msi`.
  - macOS: universal `.dmg`.
- **One app, light on memory.** The device service, Flow and the tray run in one small
  process. The window (the system WebView) exists only while it is open. Single instance,
  starts at login, low-battery notifications.
- **Devices and controls.** HID++ 2.0 over Bluetooth and receivers:
  - battery and DPI;
  - hi-res wheel, SmartShift;
  - reprogrammable controls with diversion and gesture movement;
  - change host and hosts info.

  Linux uses hidraw with no libudev; Windows and macOS use hidapi.
- **Actions.**
  - mouse buttons, horizontal scroll;
  - shortcuts with a recorder;
  - commands, media, window/desktop, browser, DPI cycle;
  - gesture button with five directions.

  Presets per system.
- **The window.** A new design in Svelte, with light, dark and system themes:
  - devices;
  - buttons as a list, or as hotspots on the device's photo;
  - pointer and scrolling;
  - Easy-Switch;
  - device settings;
  - Flow;
  - app settings;
  - permissions guide per system.

  English and Spanish, switchable live.
- **Device photos.** Optional official product photos, downloaded on your computer from
  logitech.com, or your own picture from a file or a link; button positions placed once in
  an editor.
- **Flow on every system.**
  - Edges: KDE Plasma 6 Wayland (KWin script over D-Bus, in-process), X11, Windows, macOS.
  - Clipboard text, files, folders and images.
  - Cross-subnet addressing.
  - Drag files and folders across the edge (KDE Plasma): as soon as files are picked up, the
    edges that lead to another computer glow softly in Iris; keep holding the button as the
    mouse crosses and the drag goes on there, or paste them with Ctrl+V (option, on by
    default).
  - Never switches while a mouse button is held, except for that file drag.
  - Received names made safe for each system, and received files marked as downloaded.
- **`glisa ctl`** (also `glisa-ctl` on Linux): everything the window does, plus `probe` for
  hardware diagnostics.

### Security
- Reviewed before release; every finding has a regression test (see [SECURITY.md](SECURITY.md)).
