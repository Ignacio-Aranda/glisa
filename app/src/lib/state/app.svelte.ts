// Window state: what the service reported last, app settings, the current route and
// toasts. Everything is re-fetched when the service sends an event (no polling).

import { api, errorText, initBackend, onEvent, shell } from '../backend';
import { PRODUCT_FALLBACK, setLanguage, setProductName, t } from '../i18n/index.svelte';
import type {
  AppInfo, AppSettings, Device, DeviceSummary, FlowEvents, FlowStatus, GestureAction, LoEvent, PictureLayout,
  Platform, PresetCategory, PresetsAnswer, Status, Transfer,
} from '../types';

export type ToastKind = 'ok' | 'info' | 'warn' | 'error';
export interface Toast { id: number; text: string; kind: ToastKind }

export type Route =
  | { page: 'devices' }
  | { page: 'device'; id: string; tab: string | null }
  | { page: 'flow' }
  | { page: 'settings' };

export const app = $state({
  ready: false,
  info: { name: PRODUCT_FALLBACK, version: '', platform: 'linux', simulate: false } as AppInfo,
  settings: {
    theme: 'system', language: 'system', autostart: false, notifications: true, official_photos: false,
  } as AppSettings,
  status: null as Status | null,
  statusError: '',
  /** null while the first answer is on its way. */
  devices: null as DeviceSummary[] | null,
  devicesError: '',
  details: {} as Record<string, Device>,
  detailErrors: {} as Record<string, string>,
  presets: [] as PresetCategory[],
  /** The gesture set a button gets when it becomes a gesture button (from `presets`). */
  gestureDefaults: null as GestureAction | null,
  pictures: {} as Record<string, string | null>,
  photos: {} as Record<string, string | null>,
  /** Where the controls are on the user's own picture, and whether device pages use it. */
  layouts: {} as Record<string, PictureLayout>,
  route: { page: 'devices' } as Route,
  toasts: [] as Toast[],
  flow: null as FlowStatus | null,
  flowError: '',
  /** Transfers of copied files and images, as last reported. */
  transfers: [] as Transfer[],
  /** Dialog requested from anywhere (permissions onboarding, add device). */
  dialog: null as null | 'permissions' | 'add-device',
  resolvedTheme: 'light' as 'light' | 'dark',
});

export function platform(): Platform {
  return app.status?.platform ?? app.info.platform;
}

// ------------------------------------------------------------------ toasts

let toastSeq = 0;

export function toast(text: string, kind: ToastKind = 'ok', ms = 3500): void {
  if (!text) return;
  const id = ++toastSeq;
  app.toasts = [...app.toasts.filter((x) => x.text !== text), { id, text, kind }].slice(-4);
  if (kind !== 'error') setTimeout(() => dismissToast(id), ms);   // errors stay until closed
}

export function dismissToast(id: number): void {
  app.toasts = app.toasts.filter((x) => x.id !== id);
}

// ------------------------------------------------------------------ routing

export function parseHash(hash: string): Route {
  const parts = hash.replace(/^#\/?/, '').split('/').filter(Boolean).map(decodeURIComponent);
  if (parts[0] === 'device' && parts[1]) return { page: 'device', id: parts[1], tab: parts[2] ?? null };
  if (parts[0] === 'flow') return { page: 'flow' };
  if (parts[0] === 'settings') return { page: 'settings' };
  return { page: 'devices' };
}

export function routeHash(r: Route): string {
  switch (r.page) {
    case 'device': return `#/device/${encodeURIComponent(r.id)}${r.tab ? '/' + r.tab : ''}`;
    case 'flow': return '#/flow';
    case 'settings': return '#/settings';
    default: return '#/';
  }
}

export function navigate(r: Route, replace = false): void {
  const hash = routeHash(r);
  app.route = r;
  if (typeof location !== 'undefined' && location.hash !== hash) {
    if (replace) history.replaceState(null, '', hash);
    else location.hash = hash;
  }
}

export function goBack(): void {
  if (app.route.page !== 'devices') navigate({ page: 'devices' });
}

// ------------------------------------------------------------------ theme

let media: MediaQueryList | null = null;

export function applyTheme(choice: string): void {
  if (typeof window === 'undefined') return;
  media ??= window.matchMedia?.('(prefers-color-scheme: dark)') ?? null;
  const dark = choice === 'dark' || (choice !== 'light' && !!media?.matches);
  app.resolvedTheme = dark ? 'dark' : 'light';
  document.documentElement.dataset.theme = app.resolvedTheme;
  document.documentElement.style.colorScheme = app.resolvedTheme;
}

// ------------------------------------------------------------------ fetching

export async function refreshStatus(): Promise<void> {
  try {
    app.status = await api<Status>('status');
    app.statusError = '';
  } catch (e) {
    app.statusError = errorText(e);
  }
}

export async function refreshDevices(): Promise<void> {
  try {
    const list = await api<DeviceSummary[]>('devices');
    const before = app.devices;
    app.devices = list;
    app.devicesError = '';
    const r = app.route;
    if (r.page === 'device' && before?.some((d) => d.id === r.id) && !list.some((d) => d.id === r.id)) {
      const gone = before.find((d) => d.id === r.id);
      toast(t('{name} is no longer available', { name: gone?.name ?? t('The device') }), 'info');
      navigate({ page: 'devices' }, true);
    }
    for (const id of Object.keys(app.details)) if (!list.some((d) => d.id === id)) delete app.details[id];
    void refreshPictures(list);
  } catch (e) {
    app.devicesError = errorText(e);
  }
}

export async function refreshDevice(id: string): Promise<Device | null> {
  try {
    const d = await api<Device>('device', { id });
    app.details[id] = d;
    delete app.detailErrors[id];
    // keep the list's short snapshot in step (name, battery, online)
    if (app.devices) {
      const i = app.devices.findIndex((x) => x.id === id);
      if (i >= 0) {
        const s = app.devices[i];
        if (s.name !== d.name || s.online !== d.online || JSON.stringify(s.battery) !== JSON.stringify(d.battery)) {
          app.devices[i] = { ...s, name: d.name, online: d.online, battery: d.battery };
        }
      }
    }
    return d;
  } catch (e) {
    app.detailErrors[id] = errorText(e);
    return null;
  }
}

/** The service's presets in the window's shape (English ids as labels, its translations as titles). */
export function normalizePresets(answer: PresetsAnswer | PresetCategory[] | null): { presets: PresetCategory[]; gestures: GestureAction | null } {
  if (!answer) return { presets: [], gestures: null };
  if (Array.isArray(answer)) return { presets: answer, gestures: null };
  return {
    presets: (answer.categories ?? []).map((c) => ({
      category: c.id, title: c.label,
      items: (c.items ?? []).map((i) => ({ label: i.id, title: i.label, action: i.action })),
    })),
    gestures: answer.default_gestures ?? null,
  };
}

export async function refreshPresets(): Promise<void> {
  try {
    const { presets, gestures } = normalizePresets(await api<PresetsAnswer>('presets'));
    app.presets = presets;
    app.gestureDefaults = gestures;
  } catch {
    app.presets = [];
  }
}

export async function refreshPictures(list: DeviceSummary[] = app.devices ?? []): Promise<void> {
  await Promise.all(list.map(async (d) => {
    await refreshPicture(d.id);
    if (app.settings.official_photos && d.photo !== null) {
      try {
        app.photos[d.id] = await shell.photoGet(d.id);
      } catch {
        app.photos[d.id] = null;
      }
    } else {
      app.photos[d.id] = null;
    }
  }));
}

/** The user's picture of one device and its button layout. */
export async function refreshPicture(id: string): Promise<void> {
  try {
    app.pictures[id] = await shell.pictureGet(id);
  } catch {
    app.pictures[id] = null;
  }
  try {
    // Also without a picture of their own: the button positions may be for the official photo.
    app.layouts[id] = await shell.pictureLayoutGet(id);
  } catch {
    app.layouts[id] = { use_on_pages: false, anchors: {} };
  }
}

/** The picture to show on device pages, else null (the illustration):
 *  - the user's own picture, when they chose to use it on the pages;
 *  - otherwise the official product photo, when that setting is on and the user has no
 *    picture of their own. Buttons sit where the user placed them on it. */
export function pagePicture(id: string): { src: string; layout: PictureLayout; official: boolean } | null {
  const layout = app.layouts[id] ?? { use_on_pages: false, anchors: {} };
  const own = app.pictures[id];
  if (own) return layout.use_on_pages ? { src: own, layout, official: false } : null;
  const photo = app.settings.official_photos ? app.photos[id] : null;
  return photo ? { src: photo, layout, official: true } : null;
}

/** The picture whose buttons the editor places: the user's own, else the official photo. */
export function editablePicture(id: string): { src: string; official: boolean } | null {
  const own = app.pictures[id];
  if (own) return { src: own, official: false };
  const photo = app.settings.official_photos ? app.photos[id] : null;
  return photo ? { src: photo, official: true } : null;
}

let flowBusy = false;
let flowAgain = false;

export async function refreshFlow(): Promise<void> {
  if (flowBusy) {
    flowAgain = true;
    return;
  }
  flowBusy = true;
  try {
    app.flow = await api<FlowStatus>('flow_status');
    app.flowError = '';
    app.transfers = app.flow.transfers ?? [];
  } catch (e) {
    app.flowError = errorText(e);
  } finally {
    flowBusy = false;
    if (flowAgain) {
      flowAgain = false;
      void refreshFlow();
    }
  }
}

let noticeSeq: number | null = null;
let flowWatchers = 0;

/** Pages and dialogs that show Flow keep its status fresh while they are open. */
export function watchFlow(): () => void {
  flowWatchers += 1;
  void refreshFlow();
  return () => {
    flowWatchers = Math.max(0, flowWatchers - 1);
  };
}

/** Flow notices since the last look become toasts; transfers update the card. */
export async function refreshFlowEvents(): Promise<void> {
  try {
    const r = await api<FlowEvents>('flow_events', { since: noticeSeq ?? 0 });
    const first = noticeSeq === null;
    noticeSeq = r.seq;
    app.transfers = r.transfers ?? [];
    if (!first) {
      for (const e of r.events ?? []) {
        toast(e.text, e.kind === 'error' ? 'error' : e.kind === 'warn' ? 'warn' : e.kind === 'ok' ? 'ok' : 'info');
      }
    }
  } catch {
    // Flow notices are optional: the page shows errors where they matter
  }
}

function isTransfer(x: unknown): x is Transfer {
  return !!x && typeof x === 'object' && 'id' in x && 'state' in x && 'direction' in x;
}

function handleEvent(e: LoEvent): void {
  const data = (e.data ?? {}) as { id?: string };
  const r = app.route;
  switch (e.topic) {
    case 'devices':
      void refreshDevices();
      if (r.page === 'device') void refreshDevice(r.id);
      void refreshStatus();
      break;
    case 'device':
      if (!data.id || (r.page === 'device' && r.id === data.id)) {
        if (r.page === 'device') void refreshDevice(r.id);
      } else if (app.details[data.id]) {
        delete app.details[data.id];      // stale: fetched again when opened
      }
      void refreshDevices();
      break;
    case 'status':
      void refreshStatus();
      break;
    case 'flow':
      void refreshFlow();
      void refreshFlowEvents();
      break;
    case 'transfer':
      if (isTransfer(e.data)) {
        const tr = e.data;
        const i = app.transfers.findIndex((x) => x.id === tr.id);
        if (i >= 0) app.transfers[i] = tr;
        else app.transfers = [...app.transfers, tr].slice(-8);
      } else {
        void refreshFlowEvents();
      }
      break;
  }
}

// ------------------------------------------------------------------ settings

export async function updateSettings(patch: Partial<AppSettings>): Promise<boolean> {
  const before = { ...app.settings };
  Object.assign(app.settings, patch);
  if (patch.theme) applyTheme(patch.theme);
  if (patch.language) await setLanguage(patch.language);
  try {
    const saved = await shell.settingsSet(patch);
    Object.assign(app.settings, saved);
    if ('official_photos' in patch) void refreshPictures();
    if (patch.language) {
      // Texts that came from the service are in the old language: fetch them again.
      void refreshDevices();
      void refreshPresets();
      const route = app.route;
      if (route.page === 'device') void refreshDevice(route.id);
      if (route.page === 'flow') void refreshFlow();
    }
    return true;
  } catch (e) {
    Object.assign(app.settings, before);
    applyTheme(before.theme);
    await setLanguage(before.language);
    toast(errorText(e), 'error');
    return false;
  }
}

// ------------------------------------------------------------------ start

let unlisten: (() => void) | null = null;

export async function start(): Promise<void> {
  await initBackend();
  try {
    app.info = await shell.appInfo();
  } catch {
    // keep the defaults
  }
  setProductName(app.info.name);
  if (typeof document !== 'undefined') document.title = app.info.name || PRODUCT_FALLBACK;
  try {
    Object.assign(app.settings, await shell.settingsGet());
  } catch {
    // defaults
  }
  applyTheme(app.settings.theme);
  media?.addEventListener?.('change', () => applyTheme(app.settings.theme));
  await setLanguage(app.settings.language);
  if (typeof location !== 'undefined') {
    app.route = parseHash(location.hash);
    window.addEventListener('hashchange', () => {
      app.route = parseHash(location.hash);
    });
  }
  unlisten?.();
  unlisten = onEvent(handleEvent);
  app.ready = true;
  await Promise.all([refreshStatus(), refreshDevices(), refreshPresets(), refreshFlowEvents(), refreshFlow()]);
}

/** Tests: forget everything between runs. */
export function resetState(): void {
  unlisten?.();
  unlisten = null;
  noticeSeq = null;
  flowWatchers = 0;
  flowBusy = false;
  flowAgain = false;
  Object.assign(app, {
    ready: false, status: null, statusError: '', devices: null, devicesError: '', details: {},
    detailErrors: {}, presets: [], gestureDefaults: null, pictures: {}, photos: {}, layouts: {}, route: { page: 'devices' }, toasts: [],
    flow: null, flowError: '', transfers: [], dialog: null,
  });
}
