// The service's three simulated devices (crates/hidpp/src/sim.rs: "lift", "mx", "keys"), with the
// daemon's behaviour for each API method: snapshots, settings, buttons, Easy-Switch,
// rename, reset and forget.

import type {
  Action, Battery, Control, Device, DeviceSummary, HostEntry, SimpleAction,
} from '../types';
import { S_, type Translator } from './catalog';

export const DISPLAY_NAME_MAX = 40;

interface ControlSpec { cid: number; name: string }

interface Profile {
  key: string;
  name: string;
  pid: number;
  kind: string;
  family: string;
  battery: number;
  dpi: [number, number, number] | null;
  smartshift: boolean;
  controls: ControlSpec[];
}

const LIFT_CONTROLS: ControlSpec[] = [
  { cid: 0x52, name: 'Middle button' },
  { cid: 0x53, name: 'Back button' },
  { cid: 0x56, name: 'Forward button' },
  { cid: 0xfd, name: 'DPI button' },
];

export const PROFILES: Record<string, Profile> = {
  lift: { key: 'lift', name: 'LIFT', pid: 0xb031, kind: 'mouse', family: 'mouse-vertical', battery: 78,
    dpi: [400, 4000, 100], smartshift: false, controls: LIFT_CONTROLS },
  mx: { key: 'mx', name: 'Wireless Mouse MX-Sim', pid: 0xb034, kind: 'mouse', family: 'mouse-performance',
    battery: 42, dpi: [200, 8000, 50], smartshift: true,
    controls: [...LIFT_CONTROLS.slice(0, 3), { cid: 0xc3, name: 'Gesture button' }, { cid: 0xc4, name: 'Wheel mode button' }] },
  keys: { key: 'keys', name: 'Wireless Keyboard K-Sim', pid: 0xb35b, kind: 'keyboard', family: 'keyboard-full',
    battery: 64, dpi: null, smartshift: false, controls: [] },
};

const BASE_FEATURES = ['FEATURE_SET', 'DEVICE_FW_VERSION', 'DEVICE_NAME', 'UNIFIED_BATTERY', 'CHANGE_HOST', 'HOSTS_INFO'];

/** Control names as the service reports them (msgids of buttons.po, translated per request). */
export const CONTROL_NAMES = [S_('Middle button'), S_('Back button'), S_('Forward button'), S_('DPI button'),
  S_('Gesture button'), S_('Wheel mode button')];

export class SimDevice {
  readonly profile: Profile;
  readonly id: string;
  online = true;
  customName = '';
  battery: number;
  charging = false;
  dpi = 1000;
  dpiPresets: number[] | null = null;
  hires = false;
  inverted = false;
  ssRatchet = true;
  ssThreshold = 30;
  host = 0;
  hostNames: string[];
  buttons = new Map<number, Action>();
  stored: Record<string, unknown> = {};

  constructor(profileKey: string, hostname: string) {
    this.profile = PROFILES[profileKey];
    const p = this.profile;
    this.id = `${p.pid.toString(16).padStart(4, '0')}-${p.name.replace(/ /g, '_')}`.toLowerCase();
    this.battery = p.battery;
    this.hostNames = [hostname, 'MacBook de trabajo', ''];
  }

  get modelName(): string {
    return this.profile.name;
  }

  get displayName(): string {
    return this.customName || this.modelName;
  }

  private choices(): number[] {
    const d = this.profile.dpi;
    if (!d) return [];
    const out: number[] = [];
    for (let v = d[0]; v <= d[1]; v += d[2]) out.push(v);
    return out;
  }

  clampDpi(value: number): number {
    const c = this.choices();
    if (!c.length) return value;
    return c.reduce((best, v) => (Math.abs(v - value) < Math.abs(best - value) ? v : best), c[0]);
  }

  batteryInfo(): Battery {
    const b = this.battery;
    const level = b > 80 ? 'full' : b > 30 ? 'good' : b > 10 ? 'low' : 'critical';
    return { percent: b, level, charging: this.charging, status: this.charging ? 'recharging' : 'discharging' };
  }

  /** Short snapshot. The battery stays as last read while the device is away. */
  summary(): DeviceSummary {
    const p = this.profile;
    return {
      id: this.id, name: this.displayName, model: this.modelName, kind: p.kind,
      online: this.online, product_id: p.pid.toString(16).toUpperCase().padStart(4, '0'),
      source: `sim://${p.key}`, connection: 'bluetooth', battery: this.batteryInfo(),
      family: p.family, photo: null,
    };
  }

  snapshot(tr: Translator): Device {
    const p = this.profile;
    const features = [...BASE_FEATURES];
    if (p.controls.length) features.push('REPROG_CONTROLS_V4');
    features.push('WIRELESS_DEVICE_STATUS');
    if (p.smartshift) features.push('SMART_SHIFT_ENHANCED');
    if (p.dpi) features.push('HIRES_WHEEL', 'ADJUSTABLE_DPI');
    const entries: HostEntry[] = this.hostNames.map((name, index) => ({ index, paired: !!name, name }));
    const controls: Control[] = p.controls.map((c) => ({
      cid: c.cid, name: tr.tr(c.name), divertable: true, reprogrammable: true, gestures: true,
      action: this.buttons.get(c.cid) ?? { type: 'default' },
    }));
    return {
      ...this.summary(),
      firmware: ['FW RBM16.01.B0017'],
      features,
      hosts: { count: 3, current: this.host, entries },
      dpi: p.dpi ? {
        current: this.dpi, default: 1000, min: p.dpi[0], max: p.dpi[1], step: p.dpi[2],
        choices: this.choices().slice(0, 200), presets: this.dpiPresets ?? [800, 1600, 2400],
      } : null,
      wheel: p.dpi ? { hires: this.hires, inverted: this.inverted, has_invert: true } : null,
      smartshift: p.smartshift ? { ratchet: this.ssRatchet, threshold: this.ssThreshold, enhanced: true } : null,
      controls,
      settings: { ...this.stored },
    };
  }

  /** `set`. */
  set(key: string, value: unknown, tr: Translator): unknown {
    const p = this.profile;
    if (key === 'dpi' && p.dpi) {
      this.dpi = this.clampDpi(Number(value));
      value = this.dpi;
    } else if (key === 'dpi_presets' && p.dpi && Array.isArray(value)) {
      value = [...new Set(value.map((v) => this.clampDpi(Number(v))))].sort((a, b) => a - b).slice(0, 5);
      this.dpiPresets = value as number[];
    } else if ((key === 'wheel_hires' || key === 'wheel_inverted') && p.dpi) {
      value = Boolean(value);
      if (key === 'wheel_hires') this.hires = value as boolean;
      else this.inverted = value as boolean;
    } else if ((key === 'ss_ratchet' || key === 'ss_threshold') && p.smartshift) {
      if (key === 'ss_ratchet') this.ssRatchet = Boolean(value);
      else this.ssThreshold = Math.max(1, Math.min(255, Number(value)));
      value = key === 'ss_ratchet' ? this.ssRatchet : Number(value);
    } else {
      throw new Error(tr.tr(S_('Unknown setting: {key}'), { key }));
    }
    this.stored[key] = value;
    return value;
  }

  setButton(cid: number, action: Action | null, tr: Translator): void {
    if (action) validate(action, tr);
    if (!this.profile.controls.some((c) => c.cid === cid)) throw new Error(tr.tr(S_('Unknown control')));
    if (!action || action.type === 'default') this.buttons.delete(cid);
    else this.buttons.set(cid, JSON.parse(JSON.stringify(action)));
  }

  rename(name: unknown, tr: Translator): void {
    if (name != null && typeof name !== 'string') throw new Error(tr.tr(S_('The name has to be text')));
    const clean = cleanDisplayName(name as string | null);
    if ([...clean].length > DISPLAY_NAME_MAX) {
      throw new Error(tr.tr(S_("The name can't be longer than {n} characters"), { n: DISPLAY_NAME_MAX }));
    }
    this.customName = clean === this.modelName ? '' : clean;
  }

  reset(): void {
    this.buttons.clear();
    this.dpi = 1000;
    this.dpiPresets = null;
    this.hires = false;
    this.inverted = false;
    this.ssRatchet = true;
    this.ssThreshold = 30;
    this.stored = {};
  }
}

/** Removes control/format characters, collapses white space and trims (common::clean_display_name). */
export function cleanDisplayName(name: string | null | undefined): string {
  if (!name) return '';
  return name.replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu, ' ').replace(/\s+/gu, ' ').trim();
}

const KEY_NAMES = /^[A-Za-z0-9]+$/;

function validate(action: Action, tr: Translator): void {
  switch (action.type) {
    case 'keys': {
      const parts = (action.combo ?? '').replace(/\s+/g, '').split('+').filter(Boolean);
      if (!parts.length) throw new Error(tr.tr(S_('Empty shortcut')));
      const bad = parts.find((p) => !KEY_NAMES.test(p));
      if (bad) throw new Error(tr.tr(S_('Unknown key: {key}'), { key: bad }));
      return;
    }
    case 'mouse':
      if (!['left', 'right', 'middle', 'back', 'forward'].includes(action.button)) {
        throw new Error(tr.tr(S_('Unknown mouse button')));
      }
      return;
    case 'command':
      if (!(action.cmd ?? '').trim()) throw new Error(tr.tr(S_('Empty command')));
      return;
    case 'gesture':
      for (const k of ['tap', 'up', 'down', 'left', 'right'] as const) {
        const a = action[k] as SimpleAction | undefined;
        if (a) validate(a, tr);
      }
      return;
    case 'default': case 'disabled': case 'scroll': case 'dpi_cycle':
      return;
    default:
      throw new Error(tr.tr(S_('Unknown action type: {type}'), { type: (action as { type: string }).type }));
  }
}
