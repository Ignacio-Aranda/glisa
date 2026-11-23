// What a button does: labels, icons and matching against the per-OS presets the service
// returns (`presets`).

import { bt, btc, currentLang } from './i18n/index.svelte';
import type {
  Action, GestureAction, GestureSlot, Platform, PresetCategory, SimpleAction,
} from './types';

export const DEFAULT_ACTION: SimpleAction = { type: 'default' };
export const DISABLED_ACTION: SimpleAction = { type: 'disabled' };
export const SLOTS: GestureSlot[] = ['tap', 'up', 'down', 'left', 'right'];

/** Gesture set chosen when a button becomes a gesture button (Overview, desktops). */
export function defaultGestures(platform: Platform): GestureAction {
  const k = (combo: string): SimpleAction => ({ type: 'keys', combo });
  if (platform === 'windows') {
    return { type: 'gesture', tap: k('Meta+Tab'), up: k('Meta+Tab'), down: k('Meta+D'),
      left: k('Ctrl+Meta+Left'), right: k('Ctrl+Meta+Right') };
  }
  if (platform === 'macos') {
    return { type: 'gesture', tap: k('Ctrl+Up'), up: k('Ctrl+Up'), down: k('F11'),
      left: k('Ctrl+Left'), right: k('Ctrl+Right') };
  }
  return { type: 'gesture', tap: k('Meta+W'), up: k('Meta+W'), down: k('Meta+D'),
    left: k('Ctrl+Meta+Left'), right: k('Ctrl+Meta+Right') };
}

export function isDefault(action: Action | null | undefined): boolean {
  return !action || (action.type ?? 'default') === 'default';
}

/** Deep equality for the small JSON objects actions are. */
export function sameAction(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (!a || !b || typeof a !== 'object' || typeof b !== 'object') return false;
  const ka = Object.keys(a as object).filter((k) => (a as Record<string, unknown>)[k] !== undefined);
  const kb = Object.keys(b as object).filter((k) => (b as Record<string, unknown>)[k] !== undefined);
  if (ka.length !== kb.length) return false;
  return ka.every((k) => sameAction((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]));
}

export function clone<T>(v: T): T {
  return v === undefined ? v : JSON.parse(JSON.stringify(v));
}

/** Words that mean something else elsewhere: translated with a context (buttons.po). */
function inContext(text: string): string | null {
  switch (text) {
    case 'Back': return btc('mouse button', 'Back');
    case 'Forward': return btc('mouse button', 'Forward');
    default: return null;
  }
}

/** A preset label or category (English msgid) in the window's language; `title` is the
 *  service's own translation, used when buttons.po doesn't have the msgid. */
export function presetLabel(text: string, title?: string): string {
  const shown = inContext(text) ?? bt(text);
  if (shown !== text || currentLang() === 'en' || !title) return shown;
  return title;
}

export interface FoundPreset { category: string; label: string; title?: string }

export function findPreset(action: Action | null | undefined, presets: PresetCategory[]): FoundPreset | null {
  if (!action) return null;
  for (const cat of presets) {
    for (const item of cat.items) {
      if (sameAction(item.action, action)) return { category: cat.category, label: item.label, title: item.title };
    }
  }
  return null;
}

/** The key combination or the command of an action, or ''. */
export function hint(action: Action | null | undefined): string {
  if (!action) return '';
  if (action.type === 'keys') return action.combo ?? '';
  if (action.type === 'command') return action.cmd ?? '';
  return '';
}

const MOUSE_LABELS: Record<string, () => string> = {
  left: () => bt('Left click'),
  right: () => bt('Right click'),
  middle: () => bt('Middle click'),
  back: () => btc('mouse button', 'Back'),
  forward: () => btc('mouse button', 'Forward'),
};

const SCROLL_LABELS: Record<string, () => string> = {
  up: () => bt('Scroll up'),
  down: () => bt('Scroll down'),
  left: () => bt('Scroll left'),
  right: () => bt('Scroll right'),
};

/** What an action does, in a few words, in the window's language. */
export function describe(action: Action | null | undefined, presets: PresetCategory[]): string {
  if (isDefault(action)) return btc('button action', 'Default');
  const found = findPreset(action, presets);
  if (found) return presetLabel(found.label, found.title);
  const a = action as Action;
  switch (a.type) {
    case 'disabled': return btc('button action', 'Disabled');
    case 'keys': return bt('Shortcut: {combo}', { combo: a.combo ?? '' });
    case 'command': return bt('Run: {command}', { command: a.cmd ?? '' });
    case 'gesture': return bt('Gesture button');
    case 'mouse': return MOUSE_LABELS[a.button]?.() ?? bt('Button {name}', { name: a.button });
    case 'scroll': return SCROLL_LABELS[a.dir]?.() ?? bt('Scroll');
    case 'dpi_cycle': return bt('Change DPI');
    default: return (a as { type?: string }).type ?? '?';
  }
}

/** A gesture direction's action ("None" when it does nothing). */
export function slotLabel(action: Action | null | undefined, presets: PresetCategory[]): string {
  if (!action || action.type === 'disabled' || action.type === 'default') return btc('button action', 'None');
  return describe(action, presets);
}

export function slotAction(gesture: GestureAction | Action, slot: GestureSlot): SimpleAction {
  if (gesture.type !== 'gesture') return DISABLED_ACTION;
  return (gesture as GestureAction)[slot] ?? DISABLED_ACTION;
}

/** Short name of a gesture direction ("Up"). */
export function slotName(slot: GestureSlot): string {
  switch (slot) {
    case 'tap': return btc('gesture', 'Press');
    case 'up': return btc('gesture', 'Up');
    case 'down': return btc('gesture', 'Down');
    case 'left': return btc('gesture', 'Left');
    case 'right': return btc('gesture', 'Right');
  }
}

/** Short text for toasts. */
export function short(text: string, limit = 36): string {
  return text.length <= limit ? text : text.slice(0, limit - 1).trimEnd() + '…';
}

// ------------------------------------------------------------------ icons

/** Icon names (see components/Icon.svelte) per category and preset label. */
const CATEGORY_ICONS: Record<string, string> = {
  Mouse: 'mouse', Editing: 'pencil', 'Windows and desktops': 'app-window', Browser: 'globe',
  Media: 'play', System: 'monitor-cog',
};

const PRESET_ICONS: Record<string, string> = {
  'Left click': 'mouse-left', 'Right click': 'mouse-right', 'Middle click': 'mouse',
  Back: 'circle-arrow-left', Forward: 'circle-arrow-right', 'Scroll left': 'arrow-left-to-line',
  'Scroll right': 'arrow-right-to-line', 'Change DPI': 'gauge',
  Copy: 'copy', Paste: 'clipboard-paste', Cut: 'scissors', Undo: 'undo-2', Redo: 'redo-2',
  'Select all': 'square-dashed-text', Find: 'search', 'Zoom in': 'zoom-in', 'Zoom out': 'zoom-out',
  'Switch apps': 'layers', Overview: 'layout-grid', 'Mission Control': 'layout-grid',
  'Task view': 'layout-grid', 'Show desktop': 'monitor', 'Next desktop': 'chevrons-right',
  'Previous desktop': 'chevrons-left', 'Maximize window': 'maximize-2', 'Minimize window': 'minimize-2',
  'Snap window left': 'panel-left', 'Snap window right': 'panel-right', 'Close window': 'x',
  'New tab': 'plus', 'Close tab': 'x', 'Reopen closed tab': 'rotate-ccw', 'Next tab': 'arrow-right',
  'Previous tab': 'arrow-left', 'Reload page': 'refresh-cw',
  'Play / pause': 'play', 'Next track': 'skip-forward', 'Previous track': 'skip-back',
  'Volume up': 'volume-2', 'Volume down': 'volume-1', Mute: 'volume-x', 'Mute microphone': 'mic-off',
  'App launcher': 'layout-grid', 'Search and run': 'search', Screenshot: 'camera',
  'Capture a region': 'scan', 'Lock screen': 'lock', 'Emoji picker': 'face-slightly-smiling',
  'Clipboard history': 'clipboard-list', 'Open terminal': 'terminal', 'File manager': 'folder',
  'Brightness up': 'sun', 'Brightness down': 'sun-dim',
};

const TYPE_ICONS: Record<string, string> = {
  default: 'rotate-ccw', disabled: 'ban', mouse: 'mouse', keys: 'keyboard', command: 'terminal',
  scroll: 'arrow-down-up', dpi_cycle: 'gauge', gesture: 'move',
};

export function categoryIcon(category: string): string {
  return CATEGORY_ICONS[category] ?? 'square-mouse-pointer';
}

export function typeIcon(type: string): string {
  return TYPE_ICONS[type] ?? 'square-mouse-pointer';
}

/** Icon that best represents an action. */
export function iconFor(action: Action | null | undefined, presets: PresetCategory[]): string {
  if (isDefault(action)) return TYPE_ICONS.default;
  const found = findPreset(action, presets);
  if (found) return PRESET_ICONS[found.label] ?? categoryIcon(found.category);
  return typeIcon((action as Action).type);
}

/** Icon of a physical control, by HID++ control id. */
export function controlIcon(cid: number): string {
  switch (cid) {
    case 0x50: return 'mouse-left';
    case 0x51: return 'mouse-right';
    case 0x52: return 'mouse';
    case 0x53: return 'circle-arrow-left';
    case 0x56: return 'circle-arrow-right';
    case 0x59: return 'arrow-left-to-line';
    case 0x5b: return 'arrow-right-to-line';
    case 0xc3: return 'move';
    case 0xc4: return 'disc-3';
    case 0xd7: return 'arrow-left-right';
    case 0xfd: return 'gauge';
    default: return 'circle-dot';
  }
}
