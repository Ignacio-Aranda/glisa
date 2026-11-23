// Action presets per OS, as the service's `presets` method returns them:
// [{category, items: [{label, action}]}], with category and label as English msgids of
// locale/es/buttons.po (the window translates them). Linux uses KDE Plasma shortcuts;
// Windows and macOS use their own system shortcuts (crates/service/src/actions.rs).

import type { GestureAction, Platform, PresetCategory, PresetsAnswer, SimpleAction } from '../types';
import type { Translator } from './catalog';

/** Marks a buttons.po msgid for the i18n checker. */
export function NB_(msgid: string): string {
  return msgid;
}

const keys = (combo: string): SimpleAction => ({ type: 'keys', combo });

const MOUSE: PresetCategory = {
  category: NB_('Mouse'),
  items: [
    { label: NB_('Left click'), action: { type: 'mouse', button: 'left' } },
    { label: NB_('Right click'), action: { type: 'mouse', button: 'right' } },
    { label: NB_('Middle click'), action: { type: 'mouse', button: 'middle' } },
    { label: 'Back', action: { type: 'mouse', button: 'back' } },          // translated in context
    { label: 'Forward', action: { type: 'mouse', button: 'forward' } },    // translated in context
    { label: NB_('Scroll left'), action: { type: 'scroll', dir: 'left' } },
    { label: NB_('Scroll right'), action: { type: 'scroll', dir: 'right' } },
    { label: NB_('Change DPI'), action: { type: 'dpi_cycle' } },
  ],
};

const MEDIA: PresetCategory = {
  category: NB_('Media'),
  items: [
    { label: NB_('Play / pause'), action: keys('PlayPause') },
    { label: NB_('Next track'), action: keys('NextSong') },
    { label: NB_('Previous track'), action: keys('PreviousSong') },
    { label: NB_('Volume up'), action: keys('VolumeUp') },
    { label: NB_('Volume down'), action: keys('VolumeDown') },
    { label: NB_('Mute'), action: keys('Mute') },
    { label: NB_('Mute microphone'), action: keys('MicMute') },
  ],
};

function editing(mod: string, redo: string): PresetCategory {
  return {
    category: NB_('Editing'),
    items: [
      { label: NB_('Copy'), action: keys(`${mod}+C`) },
      { label: NB_('Paste'), action: keys(`${mod}+V`) },
      { label: NB_('Cut'), action: keys(`${mod}+X`) },
      { label: NB_('Undo'), action: keys(`${mod}+Z`) },
      { label: NB_('Redo'), action: keys(redo) },
      { label: NB_('Select all'), action: keys(`${mod}+A`) },
      { label: NB_('Find'), action: keys(`${mod}+F`) },
      { label: NB_('Zoom in'), action: keys(`${mod}+Plus`) },
      { label: NB_('Zoom out'), action: keys(`${mod}+Minus`) },
    ],
  };
}

function browser(mod: string, reload: string): PresetCategory {
  return {
    category: NB_('Browser'),
    items: [
      { label: NB_('New tab'), action: keys(`${mod}+T`) },
      { label: NB_('Close tab'), action: keys(`${mod}+W`) },
      { label: NB_('Reopen closed tab'), action: keys(`${mod}+Shift+T`) },
      { label: NB_('Next tab'), action: keys('Ctrl+Tab') },
      { label: NB_('Previous tab'), action: keys('Ctrl+Shift+Tab') },
      { label: NB_('Reload page'), action: keys(reload) },
    ],
  };
}

const LINUX: PresetCategory[] = [
  MOUSE,
  editing('Ctrl', 'Ctrl+Shift+Z'),
  {
    category: NB_('Windows and desktops'),
    items: [
      { label: NB_('Switch apps'), action: keys('Alt+Tab') },
      { label: NB_('Overview'), action: keys('Meta+W') },
      { label: NB_('Show desktop'), action: keys('Meta+D') },
      { label: NB_('Next desktop'), action: keys('Ctrl+Meta+Right') },
      { label: NB_('Previous desktop'), action: keys('Ctrl+Meta+Left') },
      { label: NB_('Maximize window'), action: keys('Meta+PageUp') },
      { label: NB_('Minimize window'), action: keys('Meta+PageDown') },
      { label: NB_('Snap window left'), action: keys('Meta+Left') },
      { label: NB_('Snap window right'), action: keys('Meta+Right') },
      { label: NB_('Close window'), action: keys('Alt+F4') },
    ],
  },
  browser('Ctrl', 'F5'),
  MEDIA,
  {
    category: NB_('System'),
    items: [
      { label: NB_('App launcher'), action: keys('Meta') },
      { label: NB_('Search and run'), action: keys('Alt+Space') },
      { label: NB_('Screenshot'), action: keys('Print') },
      { label: NB_('Capture a region'), action: keys('Meta+Shift+Print') },
      { label: NB_('Lock screen'), action: keys('Meta+L') },
      { label: NB_('Emoji picker'), action: keys('Meta+Dot') },
      { label: NB_('Clipboard history'), action: keys('Meta+V') },
      { label: NB_('Open terminal'), action: keys('Ctrl+Alt+T') },
      { label: NB_('File manager'), action: keys('Meta+E') },
      { label: NB_('Brightness up'), action: keys('BrightnessUp') },
      { label: NB_('Brightness down'), action: keys('BrightnessDown') },
    ],
  },
];

const WINDOWS: PresetCategory[] = [
  MOUSE,
  editing('Ctrl', 'Ctrl+Y'),
  {
    category: NB_('Windows and desktops'),
    items: [
      { label: NB_('Switch apps'), action: keys('Alt+Tab') },
      { label: NB_('Overview'), action: keys('Meta+Tab') },
      { label: NB_('Show desktop'), action: keys('Meta+D') },
      { label: NB_('Next desktop'), action: keys('Ctrl+Meta+Right') },
      { label: NB_('Previous desktop'), action: keys('Ctrl+Meta+Left') },
      { label: NB_('Maximize window'), action: keys('Meta+Up') },
      { label: NB_('Minimize window'), action: keys('Meta+Down') },
      { label: NB_('Snap window left'), action: keys('Meta+Left') },
      { label: NB_('Snap window right'), action: keys('Meta+Right') },
      { label: NB_('Close window'), action: keys('Alt+F4') },
    ],
  },
  browser('Ctrl', 'F5'),
  MEDIA,
  {
    category: NB_('System'),
    items: [
      { label: NB_('App launcher'), action: keys('Meta') },
      { label: NB_('Search and run'), action: keys('Meta+S') },
      { label: NB_('Screenshot'), action: keys('Print') },
      { label: NB_('Capture a region'), action: keys('Meta+Shift+S') },
      { label: NB_('Lock screen'), action: keys('Meta+L') },
      { label: NB_('Emoji picker'), action: keys('Meta+Dot') },
      { label: NB_('Clipboard history'), action: keys('Meta+V') },
      { label: NB_('File manager'), action: keys('Meta+E') },
      { label: NB_('Brightness up'), action: keys('BrightnessUp') },
      { label: NB_('Brightness down'), action: keys('BrightnessDown') },
    ],
  },
];

const MACOS: PresetCategory[] = [
  MOUSE,
  editing('Meta', 'Meta+Shift+Z'),
  {
    category: NB_('Windows and desktops'),
    items: [
      { label: NB_('Switch apps'), action: keys('Meta+Tab') },
      { label: NB_('Overview'), action: keys('Ctrl+Up') },
      { label: NB_('Show desktop'), action: keys('F11') },
      { label: NB_('Next desktop'), action: keys('Ctrl+Right') },
      { label: NB_('Previous desktop'), action: keys('Ctrl+Left') },
      { label: NB_('Minimize window'), action: keys('Meta+M') },
      { label: NB_('Close window'), action: keys('Meta+Shift+W') },
    ],
  },
  browser('Meta', 'Meta+R'),
  MEDIA,
  {
    category: NB_('System'),
    items: [
      { label: NB_('Search and run'), action: keys('Meta+Space') },
      { label: NB_('Screenshot'), action: keys('Meta+Shift+3') },
      { label: NB_('Capture a region'), action: keys('Meta+Shift+4') },
      { label: NB_('Lock screen'), action: keys('Ctrl+Meta+Q') },
      { label: NB_('Emoji picker'), action: keys('Ctrl+Meta+Space') },
      { label: NB_('File manager'), action: keys('Meta+Alt+Space') },
      { label: NB_('Brightness up'), action: keys('BrightnessUp') },
      { label: NB_('Brightness down'), action: keys('BrightnessDown') },
    ],
  },
];

export function presetsFor(platform: Platform): PresetCategory[] {
  return JSON.parse(JSON.stringify(platform === 'windows' ? WINDOWS : platform === 'macos' ? MACOS : LINUX));
}

const GESTURES: Record<Platform, GestureAction> = {
  linux: { type: 'gesture', tap: keys('Meta+W'), up: keys('Meta+W'), down: keys('Meta+D'), left: keys('Ctrl+Meta+Left'), right: keys('Ctrl+Meta+Right') },
  windows: { type: 'gesture', tap: keys('Meta+Tab'), up: keys('Meta+Tab'), down: keys('Meta+D'), left: keys('Ctrl+Meta+Left'), right: keys('Ctrl+Meta+Right') },
  macos: { type: 'gesture', tap: keys('Ctrl+Up'), up: keys('Ctrl+Up'), down: keys('F11'), left: keys('Ctrl+Left'), right: keys('Ctrl+Right') },
};

/** The `presets` answer, translated like the service does (crates/service/src/actions.rs). */
export function presetsAnswer(platform: Platform, tr: Translator): PresetsAnswer {
  const label = (id: string) => (id === 'Back' || id === 'Forward' ? tr.trc('mouse button', id) : tr.tr(id));
  const slot = (id: string, name: string) => ({ id, label: tr.trc('gesture', name) });
  return {
    platform,
    categories: presetsFor(platform).map((c) => ({
      id: c.category, label: label(c.category),
      items: c.items.map((i) => ({ id: i.label, label: label(i.label), action: i.action })),
    })),
    default_gestures: JSON.parse(JSON.stringify(GESTURES[platform])),
    gesture_slots: [slot('tap', 'Press'), slot('up', 'Up'), slot('down', 'Down'), slot('left', 'Left'), slot('right', 'Right')] as PresetsAnswer['gesture_slots'],
  };
}
