# Architecture

Glisa is a Rust workspace plus a Tauri 2 app whose window is written in Svelte 5 and
TypeScript. It runs on Linux, Windows and macOS, and ships as one installer per system.

Glisa started as *LinOptions*, a Linux-only Python/Qt app. Code comments that mention "the
Python version" refer to it. Glisa keeps two things compatible with it: Flow's wire format
(a LinOptions computer still pairs and links) and the Linux settings, which are imported on
the first start.

## Process model

There is one app binary, `glisa`. It is the Tauri app with the device service and Flow embedded.

| Command | Effect |
|---|---|
| `glisa` | Starts the service and the tray, and opens the window. If an instance is already running (single instance), that instance opens its window instead. |
| `glisa --background` | Service and tray, no window. This is what autostart runs. |
| `glisa --simulate` | Adds three simulated devices, for demos and development without hardware. |

Closing the window **destroys the WebView**. The service and the tray keep running, and the
tray's *Quit* ends everything. `glisa-ctl` is a small separate CLI that talks to the running
service over local IPC.

Threading:
- The crates are **synchronous**: std threads and channels, no async.
- Only the Tauri shell uses tokio, and it calls the service from `spawn_blocking`.

## Repository layout

```
crates/common/      app identity, per-OS paths, ApiError, Event, the service ↔ Flow traits
crates/i18n/        runtime .po catalogues, tr!/trn!/trc!, per-thread language
crates/hidpp/       HID++ 1.0/2.0 protocol, features, device simulator
crates/hid/         HID transports: Linux hidraw + sysfs (no libudev); Windows/macOS via hidapi
crates/input/       virtual input: Linux uinput, Windows SendInput, macOS CGEvent
crates/service/     device manager, settings, actions and presets, JSON API, IPC, setup
crates/flow/        Flow: protocol, TLS, pairing, discovery, store, transfers, desktop backends
crates/ctl/         glisa-ctl
app/                the window: Svelte 5 + TypeScript (Vite), with a mock of the service
app/src-tauri/      the Tauri shell: tray, windows, commands, pictures, bundling
app/src/lib/brand/  design tokens, logo, favicon
brand/              brand sources and generators (logo, icons, banner)
locale/es/          Spanish catalogues (English is the source language)
data/models.json    our own table of known models (name, kind, extras)
data/photos.json    public product-photo URLs per model (URLs only, never images)
```

## The JSON API

The service exposes one JSON API. The window reaches it through the Tauri command
`api(method, params, lang)`, `glisa-ctl` over local IPC, and both get the same answers:

| Area | Methods |
|---|---|
| Service | `ping`, `status`, `presets`, `fix_permission` |
| Devices | `devices`, `device`, `set`, `set_button`, `set_host`, `rename`, `forget`, `reset`, `sim` |
| Flow | `flow_status`, `flow_enable`, `flow_peers`, `flow_discover`, `flow_pair_*`, `flow_set_*`, `flow_transfer_cancel`, … |

How a request travels:
- **Wire format:** the request is `{"id", "method", "params", "lang"}`; the reply is
  `{"id", "result"}` or `{"id", "error": "<localized message>"}`. Every request is answered
  in its client's language.
- **IPC on Linux and macOS:** a Unix socket, mode `0600`, in a private per-user directory,
  with a peer-UID check.
- **IPC on Windows:** a named pipe `\\.\pipe\glisa-<user SID>` with an owner-only DACL and
  `PIPE_REJECT_REMOTE_CLIENTS`.

**Events.** The service and Flow push `Event { topic, data }`:
- `devices`, `device` and `status` for devices and the service;
- `flow` and `transfer` for Flow.

The shell forwards them to the window as the Tauri event `lo://event`, and the window
re-fetches what it shows. Nothing polls.

Messages produced in background threads (Flow notices) are kept as a msgid plus arguments
(`i18n::Text`) and rendered in the language of whoever reads them.

**The service ↔ Flow boundary** is two traits in `crates/common`:
- `FlowHost` is implemented by the service: device list, change host, release held input,
  poll arrivals.
- `FlowApi` is implemented by Flow: the `flow_*` methods, and the device appearing,
  online and forgotten hooks.

The shell builds the service, then Flow with a weak reference to it, then
`service.set_flow(flow)`.

## The window

- **Access to the backend:** only through `app/src/lib/backend.ts`. That means the `api`
  command, a handful of shell commands (settings, pictures and photos, links, permissions,
  Bluetooth settings, quit) and the `lo://event` listener.
- **Mock backend:** outside Tauri the same module answers from a mock of the service. The
  window therefore runs, is tested and screenshotted in a plain browser.
- **Styling:** colours, spacing, type and motion come only from the design tokens in
  `app/src/lib/brand/tokens.css` (see [BRAND.md](BRAND.md)).
- **Icons:** Lucide.
- **Font:** Inter, bundled.
- **Network:** nothing is loaded from it at runtime.
- **Content security policy:** strict. The page has no direct access to files, dialogs,
  links or notifications; it goes through Glisa's commands.

**Device pictures.** A device is shown with, in order of preference:
1. the user's own picture, from a file or an https link, when they chose to use it on the
   device pages;
2. its official product photo, when *Show product photos* is on;
3. otherwise, its kind's icon. On the Buttons page that means a plain list of buttons.

Placing the buttons:
- The user places each button on the picture once, in an editor.
- Positions are stored as fractions of the picture's size.
- Pictures are untrusted: size caps, a decoder with limits, and re-encoding as PNG.
- Official photos are downloaded on the user's machine from an allowlisted host and cached.

## Translations

- **Source:** English strings in the code.
- **Spanish:** gettext catalogues in `locale/es/`, parsed at runtime. Rust embeds them with
  `include_str!`; the window imports them with `?raw`.

| Catalogue | Contents |
|---|---|
| `core.po` | service, CLI |
| `flow.po` | Flow |
| `app.po` | tray, notifications |
| `buttons.po` | control and action names |
| `ui.po` | the window |

`app/scripts/i18n.mjs check` finds missing or stale entries across `.rs`, `.ts` and `.svelte`.

## Permissions per system

`status.permissions` lists `{id, state, fix}`. `fix_permission {id}` grants what can be
granted from the app.

| System | Needs | How |
|---|---|---|
| Linux | `hidraw` and `uinput` access, and access to Logitech mice's input nodes, which Glisa only reads (Flow: button state; while dragging files is on, the left button and, during a press, motion, filtered by the kernel) | The `.deb`/`.rpm` install `/usr/lib/udev/rules.d/70-glisa.rules` (`uaccess` only). The AppImage installs the same rule through `pkexec`. |
| Windows | Nothing for HID or SendInput | Flow's ports trigger the usual firewall prompt. |
| macOS | Accessibility, Input Monitoring, Local Network | The window links each missing one to its System Settings pane. |

## Platform notes

**HID++**
- Long reports (`0x11`) are preferred.
- On Windows every top-level collection is its own device path: Bluetooth HID++ is usage
  page `0xFF43`; receivers expose short and long collections separately. Glisa opens the
  long one and also reads the short one.
- **Hot-plug:** the sysfs scan on Linux; `hidapi` enumeration every 2 s elsewhere.

**Input**
- Key combos are platform-neutral strings (`Ctrl+Shift+T`, `Meta+W`; `Cmd` is accepted for
  Meta).
- Action presets are per system.

**Flow**

| Desktop | Edges | Placement |
|---|---|---|
| KDE Plasma 6 (Wayland) | KWin script calling Glisa's own D-Bus object (zbus, in-process) | uinput absolute pointer |
| X11 | polling | XWarpPointer |
| Windows | `GetCursorPos` polling | `SetCursorPos` |
| macOS | CoreGraphics location polling | `CGWarpMouseCursorPosition` |

- **Clipboard:** Wayland data-control, X11 selections, Win32 clipboard and NSPasteboard
  carry text, files and images. Images are PNG on the wire.
- **Protocol and security model:** [FLOW.md](FLOW.md).

## Quality bar

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --workspace --exclude glisa-app --target x86_64-pc-windows-gnu
cd app && npm run check && npm test && node scripts/i18n.mjs check
```

CI runs these on Linux for every push and pull request.

- Platform code stays small and behind `cfg(target_os = …)`.
- macOS code is compiled by the release build on a macOS runner.

## Packaging and releases

- **Bundler targets:**
  - Linux: `deb`, `rpm`, `appimage`;
  - Windows: `nsis`, `msi`;
  - macOS: `dmg`, universal binary.
- **Release workflow:** `.github/workflows/release.yml` builds the three systems on a `v*`
  tag and attaches the installers to a draft release.
- **Signing:** none. The installers aren't code-signed; the README explains the first-start
  prompts on Windows and macOS.
