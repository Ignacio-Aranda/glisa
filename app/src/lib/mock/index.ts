// Mock backend for development, tests and screenshots in a plain browser: the service's
// JSON API with the simulated devices, the shell commands and the events.
//
// URL switches (query string):
//   ?mock=empty | no-permissions | denied | disconnected | loading | error | slow
//   ?platform=linux | windows | macos
//   ?flow=on | off | new | unavailable | pairing | incoming | transfer | offline | error
//   ?lang=en | es        ?theme=light | dark      (initial app settings)
//   ?photos=1            (official photos on)
//   ?picture=1           (LIFT has a picture of its own, used on its pages with two buttons placed)
//   ?dialog=permissions | add-device | rename | pair | layout   (opens it, see App.svelte)

import type { Backend } from '../backend';
import type {
  AppSettings, LoEvent, Permission, PictureLayout, Platform, SetupResult, Side, Status,
} from '../types';
import { loadServiceCatalog, S_, Translator } from './catalog';
import { SimDevice } from './devices';
import { FlowMock, type FlowScenario } from './flow';
import { presetsAnswer } from './presets';

export const MOCK_VERSION = '0.2.0';
const HOSTNAME = 'my-laptop';

export interface MockOptions {
  scenario: string;
  platform: Platform;
  flow: FlowScenario;
  /** Milliseconds every answer takes (0 in tests). */
  latency: number;
  settings: Partial<AppSettings>;
  picture: boolean;
}

export function optionsFromQuery(q: URLSearchParams): MockOptions {
  const platform = (['linux', 'windows', 'macos'].includes(q.get('platform') ?? '') ? q.get('platform') : 'linux') as Platform;
  const settings: Partial<AppSettings> = {};
  const lang = q.get('lang');
  if (lang === 'en' || lang === 'es') settings.language = lang;
  const theme = q.get('theme');
  if (theme === 'light' || theme === 'dark' || theme === 'system') settings.theme = theme;
  if (q.get('photos') === '1') settings.official_photos = true;
  const scenario = q.get('mock') ?? 'demo';
  return {
    scenario,
    platform,
    flow: (q.get('flow') ?? 'on') as FlowScenario,
    latency: scenario === 'slow' ? 900 : Number(q.get('latency') ?? 60),
    settings,
    picture: q.get('picture') === '1',
  };
}

function permissionsFor(platform: Platform, missing: boolean): Permission[] {
  const state = missing ? 'missing' : 'ok';
  if (platform === 'linux') {
    return [
      { id: 'hid', state, fix: missing ? 'install_rules' : null },
      { id: 'uinput', state, fix: missing ? 'install_rules' : null },
    ];
  }
  if (platform === 'macos') {
    return [
      { id: 'accessibility', state, fix: missing ? 'open_settings' : null },
      { id: 'input_monitoring', state, fix: missing ? 'open_settings' : null },
      { id: 'local_network', state: 'unknown', fix: 'open_settings' },
    ];
  }
  return [];
}

export class MockService {
  devices: SimDevice[] = [];
  forgotten: SimDevice[] = [];
  flow: FlowMock;
  settings: AppSettings = {
    theme: 'system', language: 'system', autostart: true, notifications: true, official_photos: false,
  };
  pictures = new Map<string, string>();
  layouts = new Map<string, PictureLayout>();
  permissionsMissing: boolean;
  denied: string[] = [];
  listeners = new Set<(e: LoEvent) => void>();
  calls: { method: string; params: Record<string, unknown> }[] = [];
  private timers = new Set<ReturnType<typeof setTimeout>>();

  constructor(public opts: MockOptions) {
    const s = opts.scenario;
    if (s !== 'empty' && s !== 'no-permissions') {
      this.devices = ['lift', 'mx', 'keys'].map((k) => new SimDevice(k, HOSTNAME));
    }
    if (s === 'disconnected') for (const d of this.devices) d.online = false;
    this.permissionsMissing = s === 'no-permissions' || s === 'denied';
    if (s === 'denied') this.denied = ['/dev/hidraw4'];
    Object.assign(this.settings, opts.settings);
    const lift = this.devices.find((d) => d.profile.key === 'lift');
    if (opts.picture && lift) {
      this.pictures.set(lift.id, samplePicture(lift.id));
      this.layouts.set(lift.id, { use_on_pages: true, anchors: { '82': { x: 0.5, y: 0.2 }, '83': { x: 0.14, y: 0.55 } } });
    }
    this.flow = new FlowMock(HOSTNAME, () => this.devices, (topic, data) => this.emit(topic, data), opts.flow);
  }

  stop(): void {
    this.flow.stop();
    for (const t of this.timers) clearTimeout(t);
    this.timers.clear();
  }

  emit(topic: string, data?: unknown): void {
    const ev: LoEvent = { topic, data: data ?? null };
    setTimeout(() => this.listeners.forEach((cb) => cb(ev)), 0);
  }

  later(ms: number, fn: () => void): void {
    const t = setTimeout(() => {
      this.timers.delete(t);
      fn();
    }, ms);
    this.timers.add(t);
  }

  status(): Status {
    return {
      version: MOCK_VERSION, denied: this.denied, input_error: '', simulate: true,
      rules_installed: !this.permissionsMissing, devices: this.devices.length,
      platform: this.opts.platform, permissions: permissionsFor(this.opts.platform, this.permissionsMissing),
    };
  }

  private device(id: unknown, tr: Translator): SimDevice {
    const d = this.devices.find((x) => x.id === id);
    if (!d) throw new Error(tr.tr(S_('Device not found')));
    return d;
  }

  /** The service's API. */
  handle(method: string, p: Record<string, unknown>, tr: Translator): unknown {
    if (this.opts.scenario === 'error' && method !== 'status') {
      throw new Error(tr.tr(S_("The device didn't respond. Check that it's on and connected")));
    }
    if (method.startsWith('flow_')) return this.handleFlow(method, p, tr);
    switch (method) {
      case 'ping': return { version: MOCK_VERSION };
      case 'status': return this.status();
      case 'devices': return this.devices.map((d) => d.summary());
      case 'presets': return presetsAnswer(this.opts.platform, tr);
    }
    const d = this.device(p.id, tr);
    switch (method) {
      case 'device': return d.snapshot(tr);
      case 'rename':
        d.rename(p.name, tr);
        this.emit('devices', { id: d.id });
        return d.summary();
      case 'forget':
        this.devices = this.devices.filter((x) => x !== d);
        this.forgotten.push(d);
        this.pictures.delete(d.id);
        this.emit('devices', { id: d.id });
        return true;
    }
    if (!d.online) throw new Error(tr.tr(S_('The device is disconnected')));
    switch (method) {
      case 'set': {
        const v = d.set(String(p.key), p.value, tr);
        this.emit('device', { id: d.id });
        return v;
      }
      case 'set_button':
        d.setButton(Number(p.cid), (p.action ?? null) as never, tr);
        this.emit('device', { id: d.id });
        return true;
      case 'set_host': {
        const host = Number(p.host);
        if (!(host >= 0 && host < 3)) throw new Error(tr.tr(S_('Invalid channel')));
        const back = d.host;
        if (host === back) return true;
        // The device leaves for the other computer, then (demo) comes back as if the
        // user pressed its Easy-Switch button again.
        this.later(1200, () => {
          d.online = false;
          this.emit('devices', { id: d.id });
          this.emit('device', { id: d.id });
        });
        this.later(12000, () => {
          d.online = true;
          d.host = back;
          this.emit('devices', { id: d.id });
          this.emit('device', { id: d.id });
        });
        return true;
      }
      case 'reset':
        d.reset();
        this.emit('device', { id: d.id });
        return true;
      case 'sim':
        if (p.op === 'battery') {
          d.battery = Number(p.percent);
          d.charging = Boolean(p.charging);
          this.emit('device', { id: d.id });
        }
        return { events: [] };
    }
    throw new Error(tr.tr(S_('Unknown method: {method}'), { method }));
  }

  private handleFlow(m: string, p: Record<string, unknown>, tr: Translator): unknown {
    const f = this.flow;
    const str = (k: string): string => {
      const v = p[k];
      if (typeof v !== 'string' || !v || v.length > 128) throw new Error(tr.tr(S_('Missing {key}'), { key: k }));
      return v;
    };
    const flag = (k: string): boolean => {
      if (typeof p[k] !== 'boolean') throw new Error(tr.tr(S_('Missing {key} (true or false)'), { key: k }));
      return p[k] as boolean;
    };
    switch (m) {
      case 'flow_status': return f.status(tr);
      case 'flow_enable': f.setEnabled(flag('enabled'), tr); return f.status(tr);
      case 'flow_peers': {
        const st = f.status(tr);
        return { peers: st.peers, discovered: f.discovered.map((d) => ({ ...d })) };
      }
      case 'flow_discover': return { discovered: f.status(tr).discovered };
      case 'flow_pair_start':
        if (!f.enabled) throw new Error(tr.tr(S_('Turn on Flow to pair computers')));
        f.pairStart();
        return f.pairingStatus(tr);
      case 'flow_pair_request':
        f.pairRequest(typeof p.peer === 'string' ? p.peer : null, typeof p.address === 'string' ? p.address : null, tr);
        return f.pairingStatus(tr);
      case 'flow_pair_confirm':
        if (!f.pairing || f.pairing.state !== 'confirm') throw new Error(tr.tr(S_('No pairing is waiting for confirmation')));
        f.pairConfirm(flag('accept'));
        return f.pairingStatus(tr);
      case 'flow_pair_cancel': f.pairCancel(); return true;
      case 'flow_remove_peer': return f.removePeer(str('peer'), tr);
      case 'flow_set_peer_address': {
        const address = p.address;
        const text = typeof address === 'string' && address.trim().toLowerCase() !== 'auto' ? address.trim() : '';
        if (text && !/^(\[[0-9a-fA-F:]+\]|[A-Za-z0-9.-]+)(:\d{1,5})?$/.test(text)) throw new Error(tr.tr(S_('Invalid address')));
        const full = text && !/:\d+$/.test(text) ? `${text}:27461` : text;
        return f.setPeerAddress(str('peer'), full, tr);
      }
      case 'flow_set_layout': return f.setLayout(str('peer'), (p.side ?? null) as Side | null, tr);
      case 'flow_set_options': return f.setOptions(p, tr);
      case 'flow_set_channel': return f.setChannel(str('device'), str('peer'), (p.channel ?? null) as number | null, tr);
      case 'flow_events': return f.events(typeof p.since === 'number' ? p.since : 0, tr);
      case 'flow_transfer_cancel': return f.cancelTransfer(str('transfer'));
      case 'flow_reset_identity': f.resetIdentity(); return f.status(tr);
      case 'flow_simulate': return f.simulate(p, tr);
      case 'flow_clipboard_take': return { seq: 0, text: '', kind: 'text', uris: [], image: null };
      case 'flow_clipboard_offer': return true;
    }
    throw new Error(tr.tr(S_('Unknown method: {method}'), { method: m }));
  }

  /** Shell commands (app/src-tauri/src/commands.rs). */
  command(name: string, args: Record<string, unknown>): unknown {
    switch (name) {
      case 'settings_get': return { ...this.settings };
      case 'settings_set': {
        const patch = (args.patch ?? {}) as Partial<AppSettings>;
        Object.assign(this.settings, patch);
        return { ...this.settings };
      }
      case 'app_info': return { name: 'Glisa', version: MOCK_VERSION, platform: this.opts.platform, simulate: true };
      case 'picture_choose': {
        // No file dialog in a browser: a generated picture stands in for the chosen file.
        const id = String(args.deviceId);
        this.pictures.set(id, samplePicture(id));
        return { ok: true };
      }
      case 'picture_from_url': {
        const id = String(args.deviceId);
        const url = String(args.url ?? '').trim();
        if (!/^https:\/\/[^\s/]+\.[^\s]+$/i.test(url)) return { ok: false, error: 'Only https:// links to a picture can be used' };
        if (/\.(gif|svg|bmp|tiff?)(\?|$)/i.test(url)) return { ok: false, error: "Glisa can't use this kind of file. Choose a PNG, JPEG or WebP picture" };
        this.pictures.set(id, samplePicture(url));
        return { ok: true };
      }
      case 'picture_get': return this.pictures.get(String(args.deviceId)) ?? null;
      case 'picture_remove':
        this.pictures.delete(String(args.deviceId));
        this.layouts.delete(String(args.deviceId));
        return null;
      case 'picture_layout_get':
        return this.layouts.get(String(args.deviceId)) ?? { use_on_pages: false, anchors: {} };
      case 'picture_layout_set': {
        const raw = (args.layout ?? {}) as Partial<PictureLayout>;
        const anchors: PictureLayout['anchors'] = {};
        const clamp = (v: unknown) => Math.max(0, Math.min(1, Number(v) || 0));
        for (const [cid, p] of Object.entries(raw.anchors ?? {}).slice(0, 64)) {
          if (/^\d+$/.test(cid) && p && typeof p === 'object') anchors[cid] = { x: clamp(p.x), y: clamp(p.y) };
        }
        const layout = { use_on_pages: !!raw.use_on_pages, anchors };
        this.layouts.set(String(args.deviceId), layout);
        return layout;
      }
      case 'photo_get': return null;
      case 'open_url': case 'open_path': case 'open_bluetooth': return null;
      case 'setup_permission': {
        const result: SetupResult = { ok: true };
        this.later(900, () => {
          this.permissionsMissing = false;
          this.denied = [];
          if (this.opts.scenario === 'no-permissions' && !this.devices.length) {
            this.devices = ['lift', 'mx', 'keys'].map((k) => new SimDevice(k, HOSTNAME));
            this.emit('devices');
          }
          this.emit('status');
        });
        return result;
      }
      case 'quit': return null;
    }
    throw new Error(`command ${name} not found`);
  }
}

/** A generated stand-in for a photo the user chose: a mouse on a transparent background
 *  (no files in the mock; colours vary with `seed`). */
export function samplePicture(seed: string): string {
  let h = 0;
  for (const c of seed) h = (h * 31 + c.charCodeAt(0)) % 360;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="600" height="720" viewBox="0 0 600 720"><defs>`
    + `<linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="hsl(${h},18%,46%)"/><stop offset="1" stop-color="hsl(${h},22%,20%)"/></linearGradient>`
    + `<linearGradient id="t" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="hsl(${h},16%,62%)"/><stop offset="1" stop-color="hsl(${h},18%,34%)"/></linearGradient></defs>`
    + `<path d="M300 40C430 40 500 150 500 300V470C500 610 410 690 300 690C190 690 100 610 100 470V300C100 150 170 40 300 40Z" fill="url(#g)"/>`
    + `<path d="M300 40C430 40 500 150 500 300V330H100V300C100 150 170 40 300 40Z" fill="url(#t)"/>`
    + `<path d="M300 44V330" stroke="hsl(${h},20%,18%)" stroke-width="6"/>`
    + `<rect x="282" y="110" width="36" height="92" rx="18" fill="hsl(${h},12%,14%)"/>`
    + `<rect x="92" y="360" width="26" height="70" rx="12" fill="hsl(${h},16%,30%)"/></svg>`;
  return 'data:image/svg+xml;base64,' + btoa(svg);
}

/** The mock as a Backend, with latency and translation per request. */
export function createMockBackend(q: URLSearchParams, overrides: Partial<MockOptions> = {}): Backend & { service: MockService } {
  const opts = { ...optionsFromQuery(q), ...overrides };
  const service = new MockService(opts);
  const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));
  const backend = {
    kind: 'mock' as const,
    service,
    async api(method: string, params: Record<string, unknown>, lang: string) {
      if (lang === 'es') await loadServiceCatalog();
      if (opts.latency) await wait(opts.latency);
      if (opts.scenario === 'loading') return new Promise(() => {});
      service.calls.push({ method, params });
      const result = service.handle(method, params ?? {}, new Translator(lang));
      return JSON.parse(JSON.stringify(result ?? null));
    },
    async command(name: string, args?: Record<string, unknown>) {
      if (opts.latency) await wait(Math.min(opts.latency, 120));
      return JSON.parse(JSON.stringify(service.command(name, args ?? {}) ?? null));
    },
    onEvent(cb: (e: LoEvent) => void) {
      service.listeners.add(cb);
      return () => service.listeners.delete(cb);
    },
  };
  if (typeof window !== 'undefined') (window as unknown as { __mock: unknown }).__mock = service;
  return backend;
}
