// Texts derived from API data (kinds, connections, battery, sizes, Flow transfers).

import { N_, t, tc, tn, tr } from './i18n/index.svelte';
import type { Battery, DeviceSummary, Transfer } from './types';

const KIND_NAMES: Record<string, string> = {
  mouse: N_('Mouse'), keyboard: N_('Keyboard'), trackball: N_('Trackball'), trackpad: N_('Trackpad'),
  touchpad: N_('Trackpad'), numpad: N_('Numeric keypad'), presenter: N_('Presenter'), remote: N_('Remote'),
  receiver: N_('Receiver'),
};

export function kindName(kind: string | undefined): string {
  return tr(KIND_NAMES[kind ?? ''] ?? N_('Device'));
}

/** The device as a lower-case noun inside a sentence ("This mouse can…"). */
export function kindNoun(kind: string | undefined): string {
  switch (kind) {
    case 'mouse': return tc('device noun', 'mouse');
    case 'keyboard': return tc('device noun', 'keyboard');
    case 'trackball': return tc('device noun', 'trackball');
    case 'trackpad': case 'touchpad': return tc('device noun', 'trackpad');
    case 'numpad': return tc('device noun', 'numeric keypad');
    case 'presenter': return tc('device noun', 'presenter');
    case 'remote': return tc('device noun', 'remote');
    default: return tc('device noun', 'device');
  }
}

export function connectionName(connection: string | undefined): string {
  switch (connection) {
    case 'bluetooth': return 'Bluetooth';
    case 'receiver': return t('USB receiver');
    case 'usb': return t('USB cable');
    default: return tc('connection', 'Unknown');
  }
}

export function levelName(level: string): string {
  switch (level) {
    case 'full': return tc('battery level', 'Full');
    case 'good': return tc('battery level', 'Good');
    case 'low': return tc('battery level', 'Low');
    case 'critical': return tc('battery level', 'Very low');
    default: return '';
  }
}

export function percentText(percent: number): string {
  return t('{percent}%', { percent });
}

/** '78%', '40% · Charging', '9% · Very low battery', 'Good'… ('' when unknown). */
export function batteryText(b: Battery | null | undefined): string {
  if (!b) return '';
  const parts: string[] = [];
  if (b.percent) parts.push(percentText(b.percent));
  else if (levelName(b.level)) parts.push(levelName(b.level));
  if (b.charging) parts.push(t('Charging'));
  else if (b.status === 'full') parts.push(t('Fully charged'));
  else if ((b.level === 'low' || b.level === 'critical') && b.percent) {
    parts.push(b.level === 'low' ? t('Low battery') : t('Very low battery'));
  }
  return parts.join(' · ');
}

/** 'ok' | 'warn' | 'danger' tone of a battery reading. */
export function batteryTone(b: Battery | null | undefined): 'ok' | 'warn' | 'danger' | 'muted' {
  if (!b) return 'muted';
  if (b.charging) return 'ok';
  const pct = b.percent ?? (b.level === 'critical' ? 5 : b.level === 'low' ? 15 : 60);
  if (pct <= 10 || b.level === 'critical') return 'danger';
  if (pct <= 20 || b.level === 'low') return 'warn';
  return 'ok';
}

export function isLowBattery(d: DeviceSummary): boolean {
  const b = d.battery;
  return !!b && !b.charging && ((b.percent ?? 100) <= 15 || b.level === 'low' || b.level === 'critical');
}

/** 2048 → '2 GB', 500 → '500 MB' (the units read the same in every language). */
export function sizeText(mb: number): string {
  if (mb >= 1024 && mb % 1024 === 0) return `${mb / 1024} GB`;
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${mb} MB`;
}

/** '3 files', '1 folder', '2 items', 'an image' (mid-sentence). */
export function transferWhat(tr_: Transfer): string {
  if (tr_.kind === 'image') return t('an image');
  const n = Number(tr_.items || 0);
  const folders = Number(tr_.top_folders || 0);
  if (n && folders === n) return tn('{count} folder', '{count} folders', n, { count: n });
  if (!folders) return tn('{count} file', '{count} files', n, { count: n });
  return tn('{count} item', '{count} items', n, { count: n });
}

/** What a transfer notice says. */
export function transferText(x: Transfer): string {
  const what = transferWhat(x);
  const name = x.name ?? '';
  const percent = Number(x.percent || 0);
  const sending = x.direction === 'send';
  switch (x.state) {
    case 'preparing': return t('Getting {what} ready for {name}…', { what, name });
    case 'waiting': case 'sending': return t('Sending {what} to {name}… {percent}%', { what, name, percent });
    case 'receiving': return t('Receiving {what} from {name}… {percent}%', { what, name, percent });
    case 'done':
      if (sending) return t('Sent {what} to {name}', { what, name });
      if (x.placed === 'drop') return t('Received {what} from {name}', { what, name });
    {
      const n = x.kind === 'image' ? 1 : Number(x.items || 0);
      return x.placed
        ? tn('Received {what} from {name} — paste it with Ctrl+V', 'Received {what} from {name} — paste them with Ctrl+V', n, { what, name })
        : tn("Received {what} from {name}. It's in the Flow folder", "Received {what} from {name}. They're in the Flow folder", n, { what, name });
    }
    case 'cancelled':
      return sending ? t('Cancelled: {what} for {name}', { what, name }) : t('Cancelled: {what} from {name}', { what, name });
    default:
      return sending ? t("Didn't arrive: {what} for {name}", { what, name }) : t("Didn't arrive: {what} from {name}", { what, name });
  }
}

/** 95 → '1:35'. */
export function clock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

/** '372649' → '372 649'. */
export function formatCode(code: string | null | undefined): string {
  return code && code.length === 6 ? `${code.slice(0, 3)} ${code.slice(3)}` : code ?? '';
}
