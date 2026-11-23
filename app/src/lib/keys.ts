// Key combinations: platform-neutral strings such as "Ctrl+Shift+T" or "Meta+W"
// (docs/ARCHITECTURE.md, "Input"). Same key names as the service; "Cmd" is accepted as an
// alias of Meta.

import { bt } from './i18n/index.svelte';
import type { Platform } from './types';

/** Key names the service understands (upper case). */
export const KEYS = new Set([
  'ESC', '1', '2', '3', '4', '5', '6', '7', '8', '9', '0', 'MINUS', 'EQUAL', 'BACKSPACE', 'TAB',
  'Q', 'W', 'E', 'R', 'T', 'Y', 'U', 'I', 'O', 'P', 'LEFTBRACE', 'RIGHTBRACE', 'ENTER', 'CTRL',
  'A', 'S', 'D', 'F', 'G', 'H', 'J', 'K', 'L', 'SEMICOLON', 'APOSTROPHE', 'GRAVE', 'SHIFT',
  'BACKSLASH', 'Z', 'X', 'C', 'V', 'B', 'N', 'M', 'COMMA', 'DOT', 'SLASH', 'RIGHTSHIFT', 'ALT',
  'SPACE', 'CAPSLOCK', 'F1', 'F2', 'F3', 'F4', 'F5', 'F6', 'F7', 'F8', 'F9', 'F10', 'F11', 'F12',
  'RIGHTCTRL', 'PRINT', 'ALTGR', 'HOME', 'UP', 'PAGEUP', 'LEFT', 'RIGHT', 'END', 'DOWN',
  'PAGEDOWN', 'INSERT', 'DELETE', 'MUTE', 'VOLUMEDOWN', 'VOLUMEUP', 'PAUSE', 'META', 'RIGHTMETA',
  'MENU', 'CALC', 'BACK', 'FORWARD', 'NEXTSONG', 'PLAYPAUSE', 'PREVIOUSSONG', 'STOPCD',
  'HOMEPAGE', 'F13', 'F14', 'F15', 'F16', 'F17', 'F18', 'F19', 'F20', 'F21', 'F22', 'F23', 'F24',
  'SEARCH', 'BRIGHTNESSDOWN', 'BRIGHTNESSUP', 'MICMUTE', 'ZOOMIN', 'ZOOMOUT',
]);

export const ALIASES: Record<string, string> = {
  CONTROL: 'CTRL', LEFTCTRL: 'CTRL', SUPER: 'META', WIN: 'META', CMD: 'META', COMMAND: 'META',
  LEFTMETA: 'META', LEFTALT: 'ALT', OPTION: 'ALT', LEFTSHIFT: 'SHIFT', RETURN: 'ENTER',
  DEL: 'DELETE', PGUP: 'PAGEUP', PGDOWN: 'PAGEDOWN', ESCAPE: 'ESC', PLUS: 'EQUAL', PERIOD: 'DOT',
};

export const MODIFIERS = new Set(['CTRL', 'SHIFT', 'ALT', 'META', 'ALTGR', 'RIGHTCTRL', 'RIGHTSHIFT', 'RIGHTMETA']);

/** How each key is written in a stored combination. */
const PRETTY: Record<string, string> = Object.fromEntries([
  'Ctrl', 'Shift', 'Alt', 'Meta', 'AltGr', 'Esc', 'Tab', 'Backspace', 'Enter', 'Insert', 'Delete',
  'Pause', 'Print', 'Home', 'End', 'Left', 'Up', 'Right', 'Down', 'PageUp', 'PageDown', 'Space',
  'Minus', 'Plus', 'Equal', 'Comma', 'Dot', 'Slash', 'Semicolon', 'Apostrophe', 'LeftBrace',
  'RightBrace', 'Backslash', 'Grave', 'VolumeUp', 'VolumeDown', 'Mute', 'PlayPause', 'NextSong',
  'PreviousSong', 'CapsLock', 'MicMute', 'BrightnessUp', 'BrightnessDown', 'ZoomIn', 'ZoomOut',
  'RightCtrl', 'RightShift', 'RightMeta', 'StopCD', 'HomePage', 'PgUp', 'PgDown', 'LeftCtrl',
  'LeftAlt', 'LeftShift', 'LeftMeta', 'Menu', 'Calc', 'Search', 'Back', 'Forward',
].map((n) => [n.toUpperCase(), n]));

export class ComboError extends Error {}

function canonical(part: string): string {
  const up = part.toUpperCase();
  return ALIASES[up] ?? up;
}

/** Validates a combination the way the service does (throws ComboError, translated). */
export function parseCombo(combo: string): string[] {
  const names: string[] = [];
  for (const part of combo.replace(/\s+/g, '').split('+')) {
    if (!part) continue;
    const name = canonical(part);
    if (!KEYS.has(name)) throw new ComboError(bt('Unknown key: {key}', { key: part }));
    names.push(name);
  }
  if (!names.length) throw new ComboError(bt('Empty shortcut'));
  return names;
}

/** 'meta + tab' → 'Meta+Tab' (modifiers first). Throws ComboError when not valid. */
export function normalizeCombo(text: string): string {
  parseCombo(text);
  const mods: string[] = [];
  const keys: string[] = [];
  for (const part of text.replace(/\s+/g, '').split('+')) {
    if (!part) continue;
    const up = part.toUpperCase();
    const isAliasOfMeta = ['CMD', 'COMMAND', 'WIN', 'SUPER'].includes(up);
    const pretty = isAliasOfMeta ? 'Meta'
      : PRETTY[up] ?? (up.length === 1 || /^F\d+$/.test(up) ? up : part[0].toUpperCase() + part.slice(1).toLowerCase());
    (MODIFIERS.has(canonical(up)) ? mods : keys).push(pretty);
  }
  return [...mods, ...keys].join('+');
}

/** Whether `text` is a valid combination. */
export function isValidCombo(text: string): boolean {
  try {
    parseCombo(text);
    return true;
  } catch {
    return false;
  }
}

/** KeyboardEvent.code → key name of the service (letters, digits, function keys…). */
const CODE_NAMES: Record<string, string> = {
  Escape: 'Esc', Tab: 'Tab', Backspace: 'Backspace', Enter: 'Enter', NumpadEnter: 'Enter', Space: 'Space',
  Minus: 'Minus', Equal: 'Equal', BracketLeft: 'LeftBrace', BracketRight: 'RightBrace',
  Semicolon: 'Semicolon', Quote: 'Apostrophe', Backquote: 'Grave', Backslash: 'Backslash',
  Comma: 'Comma', Period: 'Dot', Slash: 'Slash', CapsLock: 'CapsLock', PrintScreen: 'Print',
  Pause: 'Pause', Insert: 'Insert', Delete: 'Delete', Home: 'Home', End: 'End', PageUp: 'PageUp',
  PageDown: 'PageDown', ArrowLeft: 'Left', ArrowRight: 'Right', ArrowUp: 'Up', ArrowDown: 'Down',
  AudioVolumeMute: 'Mute', AudioVolumeDown: 'VolumeDown', AudioVolumeUp: 'VolumeUp',
  VolumeMute: 'Mute', VolumeDown: 'VolumeDown', VolumeUp: 'VolumeUp', MediaPlayPause: 'PlayPause',
  MediaTrackNext: 'NextSong', MediaTrackPrevious: 'PreviousSong', MediaStop: 'StopCD',
  ContextMenu: 'Menu', NumpadAdd: 'Plus', NumpadSubtract: 'Minus', NumpadDecimal: 'Dot',
  NumpadDivide: 'Slash', IntlBackslash: 'Backslash', BrowserBack: 'Back', BrowserForward: 'Forward',
  BrowserHome: 'HomePage', BrowserSearch: 'Search', LaunchApp2: 'Calc',
};

const MODIFIER_CODES = new Set([
  'ControlLeft', 'ControlRight', 'ShiftLeft', 'ShiftRight', 'AltLeft', 'AltRight', 'MetaLeft',
  'MetaRight', 'OSLeft', 'OSRight',
]);

/** Key name for a KeyboardEvent.code, or '' when the service has no such key. */
export function keyNameFromCode(code: string): string {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit\d$/.test(code)) return code.slice(5);
  if (/^Numpad\d$/.test(code)) return code.slice(6);
  if (/^F\d{1,2}$/.test(code)) return Number(code.slice(1)) <= 24 ? code : '';
  return CODE_NAMES[code] ?? '';
}

export interface ComboEventLike {
  code: string;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  metaKey: boolean;
}

/** The combination being pressed. `done` is false while only modifiers are held. */
export function comboFromEvent(e: ComboEventLike): { combo: string; done: boolean } {
  const parts: string[] = [];
  if (e.ctrlKey) parts.push('Ctrl');
  if (e.altKey) parts.push('Alt');
  if (e.shiftKey) parts.push('Shift');
  if (e.metaKey) parts.push('Meta');
  if (MODIFIER_CODES.has(e.code)) return { combo: parts.join('+'), done: false };
  const key = keyNameFromCode(e.code);
  if (!key) return { combo: parts.join('+'), done: false };
  return { combo: [...parts, key].join('+'), done: true };
}

/** How a key is shown on a key cap on this platform. */
export function keyLabel(name: string, platform: Platform): string {
  if (name.toUpperCase() === 'PLUS') return '+';
  const up = canonical(name);
  if (up === 'META' || up === 'RIGHTMETA') return platform === 'macos' ? '⌘ Cmd' : platform === 'windows' ? 'Win' : 'Meta';
  if (up === 'ALT' && platform === 'macos') return '⌥ Option';
  if (up === 'CTRL' && platform === 'macos') return '⌃ Control';
  if (up === 'SHIFT' && platform === 'macos') return '⇧ Shift';
  const symbols: Record<string, string> = {
    LEFT: '←', RIGHT: '→', UP: '↑', DOWN: '↓', EQUAL: '=', MINUS: '-', DOT: '.', COMMA: ',',
    SLASH: '/', PAGEUP: 'PgUp', PAGEDOWN: 'PgDn', ESC: 'Esc',
  };
  if (symbols[up]) return symbols[up];
  return PRETTY[up] ?? name;
}

/** Splits a stored combination into its keys. */
export function comboKeys(combo: string): string[] {
  return combo.replace(/\s+/g, '').split('+').filter(Boolean);
}
