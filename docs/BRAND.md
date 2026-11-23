# Brand and design language

This is the visual contract for the Tauri UI. The tokens live in
`app/src/lib/brand/tokens.css` (generated from `brand/tools/tokens.cjs`); this page says how
to use them. The product is named **Glisa** (formerly LinOptions; see `brand/NAMES.md`).
Never hard-code the name in artwork: use the logo files, which are generated from the name.

## 1. Principles

1. **Calm precision.** A tool you open for a minute and trust. Neutral surfaces, one accent,
   exact alignment, nothing decorative that doesn't carry meaning.
2. **The device is the hero.** Pages are built around the device: its product photo when
   there is one, otherwise its buttons as a clean list. No drawings of devices. Chrome stays
   quiet: thin borders, soft shadows, muted labels.
3. **Immediate and acknowledged.** Changes apply at once. Every action gets visible feedback
   (state change, inline confirmation or a short toast), never a silent click.
4. **Same product on three OSes.** It must not look like a skin of any one desktop. Our own
   neutrals, our own accent, Inter everywhere, the same layout on Linux, Windows and macOS.
5. **Original.** Nothing that resembles Logitech or Options+ (no teal/black scheme, no product
   renders, no Logitech glyphs). Device pictures are our own drawings.
6. **One flat family.** App icon, tray, mark, wordmark, favicon, banner, UI and device art
   share one language: flat shapes, generous rounded geometry, one stroke weight (1.75 px,
   the same as the Lucide icons), Iris as the only accent. No gradients used as lighting,
   no glows, no pseudo-3D shading, no realistic drop shadows on artwork. (The UI keeps its
   subtle elevation tokens for cards, menus and dialogs.)

## 2. Colour

Neutrals are cool with a hint of violet so they sit well with the accent. The accent is
**Iris**, a blue-violet: unused by the big peripheral brands, legible, calm.

| Token | Light ("Paper") | Dark ("Ink") | Use |
|---|---|---|---|
| `--bg` | `#F5F5F8` | `#0F0F14` | Window background, sidebar |
| `--surface` | `#FFFFFF` | `#17171E` | Cards, panels |
| `--surface-2` | `#F0F0F5` | `#1F1F28` | Insets in a card: segmented track, input fill, code, list hover |
| `--surface-hover` | `#E9E9F0` | `#272731` | Hover on rows/ghost buttons placed on `--surface-2` or `--bg` |
| `--bg-elevated` | `#FFFFFF` | `#1F1F29` | Menus, popovers, dialogs, toasts |
| `--border` | `#E3E3EB` | `#2A2A35` | Card outlines, dividers |
| `--border-strong` | `#C6C5D3` | `#3E3D4C` | Inputs, secondary buttons, off toggles, dashed "add" tiles |
| `--text` | `#17161F` | `#EDEDF3` | Primary text |
| `--text-muted` | `#515065` | `#AAA9BA` | Secondary text, setting descriptions |
| `--text-subtle` | `#666579` | `#8F8EA2` | Captions, placeholders, units, disabled labels |
| `--accent` | `#5B45E5` | `#8E80FF` | Primary buttons, selection, links, active hotspots |
| `--accent-hover` | `#4C36D2` | `#A195FF` | Hover/pressed of accent fills |
| `--accent-soft` | `#EEEBFF` | `#25213F` | Selected row/nav item, chip "on", hotspot halo |
| `--text-on-accent` | `#FFFFFF` | `#0E0B26` | Text and icons on accent **and on solid status fills** |
| `--focus-ring` | Iris 42% | Iris 50% | `box-shadow: 0 0 0 3px var(--focus-ring)` |
| `--success` / `-soft` | `#137A4A` / `#E2F5EB` | `#43C285` / `#12291F` | Connected, saved, battery OK |
| `--warning` / `-soft` | `#A35400` / `#FFF1DC` | `#F2A93B` / `#2E2311` | Low battery, needs attention |
| `--danger` / `-soft` | `#C8303D` / `#FDE8EA` | `#FF6B73` / `#33171B` | Errors, remove, critical battery |
| `--info` / `-soft` | `#1F69C7` / `#E4EFFC` | `#5EA4FF` / `#132438` | Neutral notices, demo mode |

Contrast (WCAG 2.2): `--text`, `--text-muted` and `--text-subtle` pass AA (≥ 4.5:1) on
`--bg`, `--surface`, `--surface-2` and `--surface-hover` in both themes. `--accent` and every
status colour pass AA as text on `--surface` and on their own `-soft` fill.
`--text-on-accent` passes AA on `--accent`, `--accent-hover` and every solid status colour.
In dark mode the accent is light, so text on it is **dark ink**, not white; always use
`--text-on-accent`, never `#fff`.

Rules:

- One accent per view does the talking: the primary action, the selection, the active hotspot.
  Don't tint whole sections with it.
- Status colours mean status. Never use `--danger` for emphasis or `--success` for decoration.
- Don't add colours. If something needs a new role, ask the brand owner for a token.
- Battery: ≥ 30 % `--success`, 15–29 % `--warning`, < 15 % `--danger`, charging `--accent`.

## 3. Type

**Inter** (variable, bundled). Use `font-variant-numeric: tabular-nums` for every number that
changes (DPI, percentages, sliders, battery). Sentence case everywhere; no trailing periods in
labels and buttons; no ALL CAPS except overlines.

| Token | Size | Weight · line height | Use |
|---|---|---|---|
| `--text-2xl` | 28px | 650 · 1.15, tracking −0.02em | Page title ("Devices", device name) |
| `--text-xl` | 20px | 600 · 1.25, −0.01em | Panel title, dialog title, empty-state title |
| `--text-lg` | 16px | 600 · 1.35 | Card title, section heading |
| `--text-md` | 14px | 400/500 · 1.45 | Body, controls, setting titles (500) |
| `--text-sm` | 12.5px | 400/500 · 1.4 | Descriptions, metadata, button text in dense rows |
| `--text-xs` | 11.5px | 500 · 1.3 | Captions, badges; overline = 600, uppercase, +0.06em |

Button labels: `--text-md` 500 (default size) or `--text-sm` 500 (small). `--font-mono` only
for diagnostics, product ids and key combos typed by the user.

## 4. Space, radius, elevation, motion

- **Spacing** is the 4 px scale `--space-1` … `--space-8` (4, 8, 12, 16, 20, 24, 32, 40).
  Page padding 32 (`--space-7`); gap between cards 16; card padding 20 (24 for large panels);
  rows inside a card 12–16 apart; label↔control 8.
- **Radius**: `--radius-xs` 4 (kbd keys, tiny badges) · `--radius-s` 6 (chips, small buttons,
  menu items) · `--radius-m` 10 (buttons, inputs, segmented controls, nav items) ·
  `--radius-l` 14 (cards, panels) · `--radius-xl` 20 (dialogs, the device stage).
  Nested corners: inner radius = outer radius − padding (never larger than the outer one).
- **Elevation**: `--shadow-1` resting cards (with a `--border` outline) · `--shadow-2`
  hover-lifted cards, popovers, dropdowns · `--shadow-3` dialogs and toasts. In dark mode
  the shadow is subtle, so elevated things also get the lighter `--bg-elevated` fill.
- **Motion**: `--dur-fast` 120 ms for hover/press/colour; `--dur-med` 200 ms for panels,
  tabs, toggles, popovers; `--dur-slow` 340 ms for page transitions and the device stage.
  `--ease-out` for almost everything; `--ease-spring` only for small "physical" things
  (toggle thumb, hotspot pop, toast entry). Animate `transform` and `opacity` only.
  Reduced-motion users get ~0 ms durations from the tokens automatically.

## 5. Iconography

Lucide (`@lucide/svelte`), outline, **stroke 1.75**, sizes 16 (dense rows, buttons), 20 (nav, setting
rows), 24 (empty states inside a 48 px tinted circle). Icons inherit `currentColor`; give them
`--text-muted` next to text and `--accent` only when the item is active. Icon-only buttons
need an `aria-label` and a tooltip. No emoji, no OS icon themes.

Suggested mapping: Devices `mouse`, Keyboard `keyboard`, Buttons `mouse-pointer-click`,
Pointer & scrolling `move` / `gauge`, Easy-Switch `monitor-smartphone`, Flow `waypoints`,
Settings `settings-2`, Battery `battery-*`, Bluetooth `bluetooth`, Receiver `usb`,
Gesture `move`, Keyboard shortcut `command`, Run command `terminal`, Reset `rotate-ccw`,
Remove `trash-2`, Rename `pencil`, Picture `image`.

## 6. Components

**App frame and sidebar.** Window `--bg`. Left sidebar 232 px on `--bg`, separated from the
content by a `--border` hairline. Top: the logo mark (24 px) + product name (`--text-md` 650).
Nav items: 36 px tall, `--radius-m`, 20 px icon + label, `--text-muted`; hover
`--surface-hover`; selected `--accent-soft` fill, `--accent` icon, `--text` label 500.
Devices are listed under an overline "Devices" with a 28 px thumbnail (the device's photo,
or its kind's icon on an `--accent-soft` tile), name and a status dot (online `--success`, offline
`--text-subtle`). Footer: demo chip (neutral), Flow status and settings. Pages share one left
edge and top padding (`--space-6` / `--space-7`); Home is up to 1120 px wide, settings-like
pages up to 960 px.

**Cards.** `--surface`, 1 px `--border`, `--radius-l`, `--shadow-1`, padding 20. Clickable
cards: hover lifts (`translateY(-1px)`, `--shadow-2`, border `--border-strong`), focus shows
the focus ring. Device card: a 160–200 px picture band on a flat `--surface-2` (the photo, or the kind's icon
on an 88 px `--accent-soft` tile), then name (`--text-lg`), "Mouse · Bluetooth"
(`--text-sm --text-muted`), battery at the right. The "Add device" tile is a dashed
`--border-strong` card with a 48 px `--accent-soft` circle and `--accent` plus icon.

**Buttons.** Height 36 (default) / 30 (small), padding 0 16 / 0 12, `--radius-m`,
`--text-md` 500.
- Primary: `--accent` fill, `--text-on-accent`; hover `--accent-hover`; press scale 0.98.
- Secondary: `--surface` fill, 1 px `--border-strong`, `--text`; hover `--surface-hover`.
- Ghost: transparent, `--text-muted`; hover `--surface-hover` + `--text`.
- Danger: like secondary with `--danger` text; the confirming button in a dialog is a solid
  `--danger` fill with `--text-on-accent`.
- Disabled: 45 % opacity, no hover. Loading: keep the width, swap the label for a spinner.

**Toggles.** 36×20 track, `--radius` full, 16 px thumb. Off: `--border-strong` track, white
thumb (`--surface` in dark) with `--shadow-1`. On: `--accent` track, thumb
`--text-on-accent` in dark / white in light. Thumb slides with `--ease-spring`. The whole
setting row is the click target.

**Sliders.** 4 px track `--surface-2` with a 1 px inset `--border`; filled part `--accent`;
18 px thumb `--surface` with 2 px `--accent` border and `--shadow-1`; focus ring on the thumb.
Value shown to the right in tabular numbers (`--text-md` 500) with the unit in
`--text-subtle`. Presets (DPI) are small ticks under the track you can click.

**Segmented controls.** Track `--surface-2`, `--radius-m`, 3 px inner padding. Segments
30 px tall, `--text-sm` 500 `--text-muted`. The selected segment is a `--surface` pill
(`--radius-s`, `--shadow-1`) with `--text`; it slides between positions (`--dur-med`).

**Inputs, search, select.** 36 px, `--surface` fill, 1 px `--border-strong`, `--radius-m`;
hover border `--text-subtle`; focus border `--accent` + focus ring. Placeholder
`--text-subtle`. Search has a 16 px `search` icon inside on the left.

**Lists of actions** (button assignment panel). Rows 36 px, `--radius-s`, icon 16
`--text-muted`, label `--text-md`. Hover `--surface-hover`. Current action: `--accent-soft`
fill, `--accent` icon and a trailing `check`. Group headers are overlines in `--text-subtle`.

**Device stage and hotspots.** With a photo (official or the user's), the Buttons page shows
it in a large stage, scaled to fit, and draws a hotspot for each button at the position the
user placed it (fractions of the picture). Without a photo there's no stage: the list of
buttons takes the page.
- Resting: 12 px dot, `--surface` fill, 2 px `--accent` ring, `--shadow-1`.
- Hover/focus: grows to 16 px with a 6 px `--accent-soft` halo (`--ease-spring`).
- Selected: solid `--accent` dot with a `--text-on-accent` 4 px centre and a soft halo that
  pulses once.
- Labels: a callout pill (`--bg-elevated`, `--border`, `--radius-m`, `--shadow-1`) with the
  control name (`--text-sm` 600) and current action (`--text-xs --text-muted`), joined to the
  dot by a 1 px `--border-strong` leader. Put callouts on the side of the stage nearest to
  the hotspot; keep the leaders from crossing by ordering callouts by height.
- Every hotspot is a real button (Tab order follows the callout order, Enter selects).

**Banners.** Inline, full width of the content column: `--{kind}-soft` fill, 1 px border
of the same colour at ~25 % (`color-mix(in srgb, var(--info) 25%, transparent)`),
`--radius-m`, 16 px leading icon in `--{kind}`, title `--text-md` 600 + text
`--text-md --text-muted`, optional ghost action at the right.

**Toasts.** Bottom centre, 12 px above the edge, `--bg-elevated`, `--border`, `--radius-m`,
`--shadow-3`, max 420 px. 16 px status icon in its colour + `--text-md`. Enter: fade + 8 px
rise with `--ease-spring`; auto-dismiss after 3.5 s (errors stay until closed).

**Empty states.** Centred in the area they replace: 48 px circle `--accent-soft` with a
24 px `--accent` icon, title `--text-xl`, one sentence `--text-md --text-muted` (max 44 ch),
one primary button. No illustrations of people, no jokes in error states.

**Dialogs.** 440 px wide, `--bg-elevated`, `--radius-xl`, `--shadow-3`, padding 24; scrim
`rgb(10 9 20 / 45%)`. Title `--text-xl`, body `--text-md --text-muted`, buttons right-aligned
(cancel = secondary, confirm = primary or danger).

**Badges and pills.** 20 px tall, `--radius-s`, `--text-xs` 600, `--{kind}-soft` fill with
`--{kind}` text ("Connected", "Bluetooth", "Beta"). Neutral pills: `--surface-2` +
`--text-muted`.

## 7. Device pictures

There are no drawings of devices. A device is shown by, in order:
1. the user's own picture, if they chose to use it on the device pages;
2. its official product photo, when *Show product photos* is on;
3. otherwise, its kind's Lucide icon (`mouse`, `keyboard`, `usb`) on an `--accent-soft`
   tile, and on the Buttons page a plain list.

Photos are never altered; transparent pictures get a soft themed drop shadow so they read on
both themes.

## 8. Logo, icons and naming

The mark is a mouse seen from above, tilted, with its left button picked out in Iris and a
hotspot ring on it: the product's core gesture ("pick a button, choose what it does") and
the same ring the app draws on device photos. It doesn't depend on the name.

| File | Use |
|---|---|
| `app/src/lib/brand/logo.svg` (= `brand/logo/mark.svg`) | The mark in colour, transparent; sidebar, about box |
| `brand/logo/mark-mono.svg` | One colour (`currentColor`) for single-colour contexts |
| `app/src/lib/brand/logo-wordmark.svg` (= `brand/logo/wordmark.svg`) | Mark + name; the name is `currentColor` |
| `brand/logo/wordmark-on-light.svg` / `-on-dark.svg` (+ PNG) | Fixed-colour versions for documents |
| `brand/icon/app-icon.png` (+ `.svg`) | 1024 px app icon on the macOS grid (824 px flat Iris squircle, flat mark); source for `tauri icon` |
| `brand/icon/app-icon-full.png` (+ `.svg`) | Full-bleed variant (no padding) for Windows tiles or Linux docks if preferred |
| `brand/tray/tray-template-*.png` | macOS menu bar template (black + alpha; 16, 22, 24, 32, and 16@2x, 22@2x) |
| `brand/tray/tray-light-*.png` / `tray-dark-*.png` | Dark glyph for light panels / light glyph for dark panels (Linux, Windows) |
| `brand/tray/tray-color-*.png` | Iris glyph that reads on both (an option for Windows) |
| `app/src/lib/brand/favicon.svg` | Window/tab icon for the UI |
| `brand/readme-banner.svg` / `.png` | Top of the README: flat Iris, mark, wordmark, tagline, platforms; no images |

The tray glyph is drawn per pixel size (seams on whole pixels), so use the PNG that matches
the size rather than scaling another one.

- Clear space around the mark = ¼ of its width. Minimum size 16 px (tray), 20 px in UI.
- Don't recolour, stretch, rotate further, outline, add effects, or put the colour mark on
  busy or mid-violet backgrounds (use the mono mark there).
- In the UI the product name is plain text in Inter 650, not the wordmark image.
- Name proposals: `brand/NAMES.md`. After a rename, run
  `node brand/tools/wordmark.cjs "<Name>"` to regenerate the wordmark and the banner.

## Regenerating the assets

Everything in the brand area is generated from scripts in `brand/tools/` (Node ≥ 18):

```
npm i --prefix brand/tools          # resvg, opentype.js, Inter (dev only; not committed)
node brand/tools/tokens.cjs         # app/src/lib/brand/tokens.css
node brand/tools/devices.cjs        # device SVGs, index.ts, devices.css, brand/preview.html
node brand/tools/devices.cjs --png <dir>   # + a contact sheet to review
node brand/tools/logo.cjs           # mark, app icon, tray icons, favicon
node brand/tools/wordmark.cjs "Glisa"      # wordmark + README banner
```

## 9. Do and don't

| Do | Don't |
|---|---|
| Use tokens for every colour, radius, shadow and duration | Hard-code hex values or `#fff` on accent |
| One primary button per view | Several accent-filled buttons side by side |
| Sentence case, short labels ("Pointer speed") | Title Case, ALL CAPS, trailing periods |
| Tabular numbers for values | Numbers that jitter while dragging a slider |
| Hairline borders + soft shadow for cards | Heavy borders, coloured card backgrounds, glows |
| Lucide at 1.75 stroke, `currentColor` | Mixed icon sets, filled icons, emoji |
| Product photos with hotspots, or a clean list | Drawings of devices, Logitech UI art, 3D or shaded art |
| Keep layouts usable at 980×640 | Content that clips or scrolls horizontally |
| Animate transform/opacity, ≤ 340 ms | Bouncy page transitions, animating layout |
