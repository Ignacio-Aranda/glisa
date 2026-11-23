// The only way the window talks to the outside world.
//
// Inside Tauri: `api(method, params)` → invoke('api', {method, params, lang}), the shell
// commands of app/src-tauri/src/commands.rs, and the event "lo://event" ({topic, data}).
// In a plain browser (development, tests, screenshots) the same calls go to the mock
// backend of src/lib/mock, which reproduces the service's JSON shapes.

import { currentLang } from './i18n/index.svelte';
import type {
  AppInfo, AppSettings, LoEvent, PictureLayout, PictureResult, SetupResult,
} from './types';

export interface Backend {
  /** The service's JSON API. Rejects with the service's (translated) message. */
  api(method: string, params: Record<string, unknown>, lang: string): Promise<unknown>;
  /** A shell command (settings, pictures, links…). */
  command(name: string, args?: Record<string, unknown>): Promise<unknown>;
  /** Subscribes to service events; returns the unsubscribe function. */
  onEvent(cb: (e: LoEvent) => void): () => void;
  readonly kind: 'tauri' | 'mock';
}

/** An error answered by the service or the shell (message already for the user). */
export class ApiError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ApiError';
  }
}

export function errorText(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === 'string') return e;
  if (e && typeof e === 'object' && 'message' in e) return String((e as { message: unknown }).message);
  return String(e);
}

let current: Backend | null = null;

export function inTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

async function tauriBackend(): Promise<Backend> {
  const [{ invoke }, { listen }] = await Promise.all([
    import('@tauri-apps/api/core'),
    import('@tauri-apps/api/event'),
  ]);
  return {
    kind: 'tauri',
    api: (method, params, lang) => invoke('api', { method, params, lang }),
    command: (name, args) => invoke(name, args ?? {}),
    onEvent(cb) {
      let stop: (() => void) | null = null;
      let cancelled = false;
      listen<LoEvent>('lo://event', (e) => cb(e.payload)).then((un) => {
        if (cancelled) un();
        else stop = un;
      });
      return () => {
        cancelled = true;
        stop?.();
      };
    },
  };
}

/** Picks the backend once: Tauri when available, else the mock. */
export async function initBackend(): Promise<Backend> {
  if (current) return current;
  if (inTauri()) {
    current = await tauriBackend();
  } else {
    const { createMockBackend } = await import('./mock');
    current = createMockBackend(new URLSearchParams(typeof location !== 'undefined' ? location.search : ''));
  }
  return current;
}

/** Tests and the mock use this to install a backend directly. */
export function setBackend(b: Backend | null): void {
  current = b;
}

/** A query-string switch of the mock backend (development and screenshots only; null in the app). */
export function mockParam(name: string): string | null {
  if (current?.kind !== 'mock' || typeof location === 'undefined') return null;
  return new URLSearchParams(location.search).get(name);
}

export function backend(): Backend {
  if (!current) throw new Error('backend not initialised');
  return current;
}

/** Calls the service's API, in the window's language. */
export async function api<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
  try {
    return (await backend().api(method, params, currentLang())) as T;
  } catch (e) {
    throw new ApiError(errorText(e));
  }
}

async function command<T>(name: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return (await backend().command(name, args)) as T;
  } catch (e) {
    throw new ApiError(errorText(e));
  }
}

export function onEvent(cb: (e: LoEvent) => void): () => void {
  return backend().onEvent(cb);
}

// ----------------------------------------------------------------- shell commands

export const shell = {
  settingsGet: () => command<AppSettings>('settings_get'),
  settingsSet: (patch: Partial<AppSettings>) => command<AppSettings>('settings_set', { patch }),
  appInfo: () => command<AppInfo>('app_info'),
  pictureChoose: (deviceId: string) => command<PictureResult>('picture_choose', { deviceId }),
  pictureGet: (deviceId: string) => command<string | null>('picture_get', { deviceId }),
  pictureFromUrl: (deviceId: string, url: string) => command<PictureResult>('picture_from_url', { deviceId, url }),
  pictureRemove: (deviceId: string) => command<void>('picture_remove', { deviceId }),
  pictureLayoutGet: (deviceId: string) => command<PictureLayout>('picture_layout_get', { deviceId }),
  pictureLayoutSet: (deviceId: string, layout: PictureLayout) => command<PictureLayout>('picture_layout_set', { deviceId, layout }),
  photoGet: (deviceId: string) => command<string | null>('photo_get', { deviceId }),
  openUrl: (url: string) => command<void>('open_url', { url }),
  openPath: (path: string) => command<void>('open_path', { path }),
  setupPermission: (id: string) => command<SetupResult>('setup_permission', { id }),
  /** Opens the system's Bluetooth settings. */
  openBluetooth: () => command<void>('open_bluetooth'),
  quit: () => command<void>('quit'),
};
