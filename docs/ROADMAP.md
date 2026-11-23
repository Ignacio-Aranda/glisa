# Roadmap and TODO

Planned work, known gaps and ideas, roughly in priority order. Items marked *(help wanted)*
need hardware or setups the maintainer doesn't have.

## Platforms and packaging

- [ ] Test on **Windows 10/11** and **macOS** hardware *(help wanted)*:
  - HID access to Bluetooth and receiver devices;
  - SendInput / CGEvent actions;
  - Flow edges and the clipboard;
  - permissions flow on macOS.
- [ ] Flatpak / Flathub, AUR.
- [ ] Auto-update (Tauri updater with signed update manifests).

## Devices

- [ ] LIFT **virtual thumb wheel**: hold back/forward and turn the wheel to scroll
      sideways (`data/models.json` already flags the devices).
- [ ] Validate **Unifying / Bolt / Lightspeed** receivers on real hardware. *(help wanted)*
- [ ] **Thumb wheel** (`0x2150`) of MX mice: direction, resolution, diversion to actions.
- [ ] Keyboards: backlight (`0x1982`), Fn inversion (`0x40A3`), configurable keys.
- [ ] Receiver pairing (Unifying/Bolt) from the window.
- [ ] Battery history and remaining-time estimate.

## Features

- [ ] **Per-application profiles**: different actions per active window.
- [ ] More actions: lock screen, emoji picker, precision mode (low DPI while held).
- [ ] Import/export settings.
- [ ] macOS media and brightness keys as actions (they need NX system events).

## Flow

- [x] **Drag files across the edge** on KDE Plasma (Wayland).
- [ ] The same on X11, Windows and macOS ([notes](FLOW-DRAG-DROP.md)).
- [ ] More than two computers in the layout.
- [ ] GNOME Wayland (needs a GNOME Shell extension to follow the pointer).
- [ ] Optional real *move* (delete the original after the receiver confirms the hash).

## Languages

- [x] English and Spanish.
- [ ] More languages through a translation platform once the project is public.

## Known limitations

- Only the Logitech LIFT over Bluetooth on Linux has been tested on real hardware so far.
- Flow can't pair with Logitech Options+ (closed protocol tied to device keys).
- The installers aren't code-signed (an open-source project's choice): Windows SmartScreen
  asks once, and macOS needs *Open Anyway* in Privacy & Security the first time.
