# Security policy

## Reporting a vulnerability

Please **do not open a public issue** for security problems. Use GitHub's private
vulnerability reporting (*Security → Report a vulnerability*) on this repository. Include:
- the version or commit;
- your system and desktop;
- steps to reproduce.

You should get an answer within 7 days. Fixes are released as soon as they're ready, and
credited if you wish.

## Supported versions

Only the latest release (and `main`) receives security fixes.

## Security model

Glisa runs entirely as your user. The only privileged operation is installing the Linux
udev rule.

| Surface | Protection |
|---|---|
| **udev rule** (Linux) | It does only `TAG+="uaccess"` (active local session user) for:<br>• Logitech `hidraw` nodes;<br>• `/dev/uinput`;<br>• Logitech **mice's** input event nodes (Glisa only reads them, so Flow can tell whether a button is held; `uaccess` itself grants the session read and write, see *Accepted risks*).<br>The `.deb`/`.rpm` install it as a root-owned file. The AppImage's in-app setup passes the rule text inside the polkit request. The text is checked to contain only allowed match keys and `uaccess`: no `RUN`, `PROGRAM`, `MODE`, `ENV` assignments… |
| **Local control** | Linux/macOS: a Unix socket, mode 0600, in a private runtime directory. Peer UID checked (`SO_PEERCRED` / `getpeereid`); the client refuses a service owned by another user.<br>Windows: a named pipe per user SID with an owner-only DACL and `PIPE_REJECT_REMOTE_CLIENTS`; the client checks the pipe's owner.<br>Request lines are capped at 16 MiB and requests validated. |
| **Window ↔ backend** | Strict content security policy (no remote code, no inline scripts). The page's capability is only `core:event` (listening to Glisa's events): no direct access to files, images, the tray, dialogs, links or notifications, only Glisa's own commands, which validate their arguments. Opening folders is limited to Glisa's folders and Downloads; links must be https. |
| **Flow network** | TLS 1.3 only, through rustls; mutual authentication.<br>ECDSA P-256 identity per installation: key 0600, directory 0700.<br>Peers pinned by the SHA-256 of their public key, checked by custom verifiers on every connection. No session tickets or resumption.<br>Pairing with commit-then-reveal and a 6-digit SAS confirmed on both computers.<br>Versioned strict schemas, per-connection sequence numbers, size limits, per-frame deadlines, connection rate limits. Unauthenticated discovery is only a hint.<br>Removing a peer ends its sessions. |
| **Mouse input read by Flow** (KDE Wayland) | Only the Logitech mouse's own event nodes (vendor 046d, a left button and X/Y motion; a node that declares letter keys is never opened, and the udev rule excludes keyboard nodes). Never grabbed: the desktop gets every event as before.<br>• Button state (`EVIOCGKEY`), only while the pointer is at an edge, or to confirm the press that lets a drag go on after the mouse arrives.<br>• While *Drag files across the edge* is on: that node's events, filtered by the kernel (`EVIOCSMASK`) to the **left button** and, only from a left press until it is known whether files are being dragged (under a second), its **X/Y motion**. No other button, wheel, key or event ever reaches Glisa; a node that can't be filtered isn't read. Nothing of it is stored or sent. |
| **Flow transfers** | Separate pinned connection per transfer; SHA-256 per file; strict manifests (no absolute paths, `..` or links); caps on size, count and depth.<br>Names normalized and made valid for the receiving OS (Windows reserved names and characters, case-insensitive collisions); nothing overwritten.<br>Files go to a private staging folder (0700/0600), are never opened or executed, are marked as downloaded (macOS quarantine, Windows Mark of the Web), and are cleaned up after 24 h.<br>Images are checked by signature and dimensions without decoding them.<br>Files dragged across the edge use the same transfer. The edge catcher (KDE Wayland) exists only while a button is held at a peer's edge, takes no clicks, accepts only local `file:` URIs without `.`/`..` and only the copy action. The probe that tells a file drag has started looks only at the offer's MIME types (never accepts, reads or finishes) and is up for at most a moment; the glow takes no input. On the receiving computer, the drag that goes on offers only the received items in staging, copy only, and the invisible surface that waits for the arriving mouse's press is up for at most 11 s (8 s for the mouse, 3 s for its press) and only after a `drag` notice and a `switch` from a paired peer; presses its own mouse doesn't confirm are ignored. |
| **KDE drops** (D-Bus, KDE Wayland) | A drag picked up on the receiving computer offers Ark's `DndExtract` object so Dolphin and the desktop copy without their *Copy / Link* menu. The object path carries a 128-bit random token and exists only for that drag (gone at its first call or when the pickup ends). Only a caller of the same user (asked to the session bus) is served, once. The destination must be an existing local folder the user can write to: canonicalized, opened once, never inside the dragged files. The copy creates everything relative to opened folders with `O_EXCL` and `O_NOFOLLOW`: nothing is overwritten (`name (2)`), no link is followed in the staging folder or the destination, links and special files are skipped, and nothing is made executable. Errors undo what the call created. |
| **Pictures and photos** | The user's picture (file or link) is read with a size cap, decoded with pixel and memory limits by a memory-safe decoder, and re-encoded as a fresh PNG.<br>Picture links: https only, public addresses only (loopback, private, link-local and similar are refused), redirects re-checked.<br>Official photos: an allowlist of one host, no redirects, 5 MB cap, signature and dimension checks.<br>Everything stored privately. |
| **Processes** | No untrusted data ever reaches a shell. The privileged setup runs a fixed `pkexec /bin/sh -c` script and passes the rule text as `$1`. Opening links and folders goes through the system's default handler with the target passed as an argument. The user's own *Run a command* actions run as the user. Secrets and clipboard contents are never logged. |

Full Flow threat model: [docs/FLOW.md §7](docs/FLOW.md).

## Accepted risks

- **udev access.** With the rule installed, any program you run as your user can read Logitech
  `hidraw` devices (including keystrokes of a Logitech keyboard) and create virtual input
  devices. Glisa can't work without it; Solaar and Steam ship equivalent rules.
- **Input nodes.** `uaccess` gives your session read and write access to the Logitech mouse's
  input event nodes, so any program you run could also read that mouse's events or grab it.
  Glisa only reads (the left button and, during a press, motion, filtered by the kernel).
- **Same-user processes** can drive the local control channel, assign *Run a command*
  actions, read the Flow private key and edit the settings. A process running as your user
  is not a security boundary on a desktop.
- **A paired Flow computer is trusted.** It can replace your clipboard and ask to switch the
  device at any moment; with a `drag` notice and a `switch` it can also make clicks of other
  pointing devices on one of your screens go nowhere for up to 11 s (not while your Logitech
  mouse is on this computer; at most once every 30 s when no mouse arrives). Pair only computers you control; unpair or reset the identity if one
  is lost.
- **Availability.** An attacker on the network can exhaust connection slots or pairing
  attempts. Flow protects confidentiality and integrity, not availability.
- **Unsigned builds.** The installers aren't code-signed: download them only from this
  repository's Releases page and check them against the published SHA-256 sums. On macOS, permissions granted to an unsigned build
  may need to be granted again after an update.

## Review history

| Date | Scope | Result |
|---|---|---|
| 2026-11 | Whole codebase before the first release: local control, udev rule, Flow (TLS, pairing, transfers, cross-system file names), window commands, picture links and official photos | Findings fixed, each with a regression test (`crates/service/tests`, `crates/flow/tests/security.rs`, `app/src-tauri` unit tests); accepted risks documented above |
