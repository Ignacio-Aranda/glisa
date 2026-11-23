// Shapes of the service's JSON API and of the shell commands
// (app/src-tauri/src/commands.rs).

export type Platform = 'linux' | 'windows' | 'macos';
export type Connection = 'bluetooth' | 'receiver' | 'usb' | string;
export type Kind =
  | 'mouse' | 'keyboard' | 'trackball' | 'trackpad' | 'touchpad' | 'numpad' | 'presenter'
  | 'remote' | 'receiver' | 'unknown' | string;

export interface Battery {
  percent: number | null;
  level: 'critical' | 'low' | 'good' | 'full' | string;
  charging: boolean;
  status: string;
}

/** Short snapshot (`devices`, `rename`). */
export interface DeviceSummary {
  id: string;
  name: string;
  model: string;
  kind: Kind;
  online: boolean;
  product_id: string;
  source: string;
  connection: Connection;
  battery: Battery | null;
  /** Illustration family (ARCHITECTURE "Device pictures"); may be missing on older services. */
  family?: string | null;
  /** Key of the official product photo, or null. */
  photo?: string | null;
}

export interface HostEntry {
  index: number;
  paired: boolean | null;
  name: string;
}

export interface Hosts {
  count: number;
  current: number | null;
  entries: HostEntry[];
}

export interface DpiInfo {
  current: number;
  default: number;
  min: number;
  max: number;
  step: number;
  choices: number[];
  presets: number[];
}

export interface WheelInfo {
  hires: boolean;
  inverted: boolean;
  has_invert: boolean;
}

export interface SmartShiftInfo {
  ratchet: boolean;
  threshold: number;
  enhanced: boolean;
}

export type Direction = 'left' | 'right' | 'up' | 'down';
export type MouseButton = 'left' | 'right' | 'middle' | 'back' | 'forward';

export type SimpleAction =
  | { type: 'default' }
  | { type: 'disabled' }
  | { type: 'mouse'; button: MouseButton | string }
  | { type: 'keys'; combo: string }
  | { type: 'command'; cmd: string }
  | { type: 'scroll'; dir: Direction | string }
  | { type: 'dpi_cycle' };

export type GestureSlot = 'tap' | 'up' | 'down' | 'left' | 'right';

export interface GestureAction {
  type: 'gesture';
  tap?: SimpleAction;
  up?: SimpleAction;
  down?: SimpleAction;
  left?: SimpleAction;
  right?: SimpleAction;
}

export type Action = SimpleAction | GestureAction;

export interface Control {
  cid: number;
  name: string;
  divertable: boolean;
  reprogrammable: boolean;
  gestures: boolean;
  action: Action;
}

/** Full snapshot (`device`). */
export interface Device extends DeviceSummary {
  firmware: string[];
  features: string[];
  hosts: Hosts;
  dpi: DpiInfo | null;
  wheel: WheelInfo | null;
  smartshift: SmartShiftInfo | null;
  controls: Control[];
  settings: Record<string, unknown>;
}

export interface Permission {
  id: string;
  state: 'ok' | 'missing' | 'unknown';
  /** How it is granted (opaque to the window; non-empty when `setup_permission` can fix it). */
  fix?: string | null;
}

export interface Status {
  version: string;
  denied: string[];
  input_error: string;
  simulate: boolean;
  rules_installed: boolean;
  devices: number;
  platform?: Platform;
  permissions?: Permission[];
}

/** A preset as the window keeps it: `label` is the English msgid (buttons.po), `title` the
 *  service's translation of it (used when the window's catalogue doesn't have it). */
export interface PresetItem {
  label: string;
  action: SimpleAction;
  title?: string;
}

export interface PresetCategory {
  category: string;
  items: PresetItem[];
  title?: string;
}

/** The service's `presets` answer (crates/service/src/actions.rs presets_json). */
export interface PresetsAnswer {
  platform?: Platform;
  categories: { id: string; label: string; items: { id: string; label: string; action: SimpleAction }[] }[];
  default_gestures?: GestureAction;
  gesture_slots?: { id: GestureSlot; label: string }[];
}

// ------------------------------------------------------------------ Flow

export type Side = 'left' | 'right' | 'top' | 'bottom';

export interface FlowOptions {
  require_ctrl: boolean;
  clipboard: boolean;
  files: boolean;
  /** Files dragged across the edge go along (the drag goes on there, or Ctrl+V). */
  drag_files: boolean;
  keyboard: boolean;
  cooldown_ms: number;
  corner_px: number;
  max_transfer_mb: number;
  max_image_mb: number;
}

export interface FlowPeer {
  id: string;
  name: string;
  online: boolean;
  state: 'online' | 'offline' | 'identity';
  side: Side | null;
  fingerprint: string;
  address: string;
  address_source: 'manual' | 'learned' | 'discovered' | '';
  manual_address: string;
  learned_address: string;
  offline_for: number;
  last_error: string;
  files: boolean;
}

export interface Discovered {
  id: string;
  name: string;
  address: string;
  compatible: boolean;
  paired: boolean;
}

export interface Pairing {
  state: 'waiting' | 'connecting' | 'confirm' | 'done' | 'failed' | 'expired';
  role: 'initiator' | 'responder' | null;
  expires_in: number;
  peer: { id: string; name: string } | null;
  sas: string | null;
  local: boolean | null;
  remote: boolean | null;
  reason: string;
  message: string;
  attempts_left: number;
}

export interface Transfer {
  id: string;
  direction: 'send' | 'receive';
  peer: string;
  name: string;
  kind: 'files' | 'image';
  state: 'preparing' | 'waiting' | 'sending' | 'receiving' | 'done' | 'failed' | 'cancelled' | 'refused';
  active: boolean;
  items: number;
  top_folders: number;
  files: number;
  folders: number;
  bytes: number;
  done: number;
  percent: number;
  failed: number;
  skipped: number;
  first: string;
  folder: string;
  /** 'drop': dragged here and dropped by the user (nothing to paste). */
  placed: 'clipboard' | 'gui' | 'drop' | '';
  cut: boolean;
  ago: number;
}

export interface FlowDevice {
  id: string;
  name: string;
  kind: Kind;
  online: boolean;
  hosts: Hosts;
  channels: Record<string, { channel: number | null; suggested: number | null }>;
}

export interface FlowStatus {
  version: number;
  enabled: boolean;
  running: boolean;
  demo: boolean;
  error: string;
  backend: string;
  backend_state: 'off' | 'starting' | 'ok' | 'error' | 'unavailable';
  backend_message: string;
  available: boolean;
  ctrl_available: boolean;
  can_place: boolean;
  /** Files can be dragged across the edge here (KDE Plasma on Wayland). */
  drag_available?: boolean;
  clipboard_backend: string;
  files_backend: string;
  transfers: Transfer[];
  staging: string;
  id: string;
  name: string;
  fingerprint: string;
  ports: { udp: number; tcp: number; pairing: number };
  firewall: { ufw?: boolean; [k: string]: unknown };
  options: FlowOptions;
  peers: FlowPeer[];
  discovered: Discovered[];
  pairing: Pairing | null;
  devices: FlowDevice[];
  screens: number[][];
  last_switch: unknown;
  last_arrival: unknown;
  events_seq: number;
  clipboard_seq: number;
}

export interface FlowNotice {
  seq: number;
  text: string;
  kind: 'ok' | 'info' | 'warn' | 'error' | string;
}

export interface FlowEvents {
  seq: number;
  enabled: boolean;
  clipboard_seq: number;
  clipboard_backend: string;
  files_backend: string;
  transfers: Transfer[];
  events: FlowNotice[];
}

// ------------------------------------------------------------- shell / events

export type Theme = 'system' | 'light' | 'dark';

export interface AppSettings {
  theme: Theme;
  language: 'system' | 'en' | 'es';
  autostart: boolean;
  notifications: boolean;
  official_photos: boolean;
  [extra: string]: unknown;
}

export interface AppInfo {
  name: string;
  version: string;
  platform: Platform;
  simulate: boolean;
}

export interface PictureResult {
  ok: boolean;
  error?: string;
}

/** Where the controls are on the user's own picture (fractions 0..1 of its width and height). */
export interface PictureLayout {
  use_on_pages: boolean;
  /** Keyed by the control id as a decimal string. */
  anchors: Record<string, { x: number; y: number }>;
}

export interface SetupResult {
  ok: boolean;
  /** macOS: a System Settings pane was opened (the user grants it there). */
  opened?: boolean;
  cancelled?: boolean;
  error?: string;
  /** A command to run by hand when the window can't do it. */
  manual?: string;
}

export type Topic = 'devices' | 'device' | 'status' | 'flow' | 'transfer';

export interface LoEvent {
  topic: Topic | string;
  data?: unknown;
}
