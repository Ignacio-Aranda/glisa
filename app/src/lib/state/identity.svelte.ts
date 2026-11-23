// Naming, picturing and removing a device: one place used by the Devices cards, the device
// header menu and the device's Settings tab.

import { api, errorText, shell } from '../backend';
import { t } from '../i18n/index.svelte';
import type { DeviceSummary } from '../types';
import { app, navigate, refreshDevice, refreshDevices, refreshPicture, toast } from './app.svelte';
import { confirm } from './confirm.svelte';

export const identity = $state({
  renaming: null as DeviceSummary | null,
});

export function rename(dev: DeviceSummary): void {
  identity.renaming = dev;
}

/** Saves a name ('' goes back to the model's). Returns an error message, or ''. */
export async function saveName(dev: DeviceSummary, name: string): Promise<string> {
  try {
    const shown = await api<DeviceSummary>('rename', { id: dev.id, name });
    if (!name.trim() || shown.name === shown.model) toast(t('Shown as {model} again', { model: shown.model || dev.model }));
    else toast(t('Renamed to {name}', { name: shown.name }));
    await refreshDevices();
    if (app.route.page === 'device' && app.route.id === dev.id) await refreshDevice(dev.id);
    return '';
  } catch (e) {
    return errorText(e);
  }
}

export async function choosePicture(dev: DeviceSummary): Promise<void> {
  try {
    const r = await shell.pictureChoose(dev.id);
    if (r.ok) {
      await refreshPicture(dev.id);
      toast(t('Picture updated'));
    } else if (r.error) {
      toast(r.error, 'error');
    }
  } catch (e) {
    toast(t("Couldn't use the picture ({error})", { error: errorText(e) }), 'error');
  }
}

/** Downloads a picture from an https link. Returns an error message, or ''. */
export async function pictureFromUrl(dev: DeviceSummary, url: string): Promise<string> {
  try {
    const r = await shell.pictureFromUrl(dev.id, url);
    if (!r.ok) return r.error || t("Couldn't use the picture ({error})", { error: url });
    await refreshPicture(dev.id);
    toast(t('Picture updated'));
    return '';
  } catch (e) {
    return errorText(e);
  }
}

export async function setUseOnPages(dev: DeviceSummary, on: boolean): Promise<void> {
  const layout = app.layouts[dev.id] ?? { use_on_pages: false, anchors: {} };
  try {
    app.layouts[dev.id] = await shell.pictureLayoutSet(dev.id, { ...layout, use_on_pages: on });
  } catch (e) {
    toast(errorText(e), 'error');
  }
}

export async function removePicture(dev: DeviceSummary): Promise<void> {
  try {
    await shell.pictureRemove(dev.id);
    app.pictures[dev.id] = null;
    app.layouts[dev.id] = { use_on_pages: false, anchors: {} };
    toast(t('Picture removed'));
  } catch (e) {
    toast(errorText(e), 'error');
  }
}

export function removeText(name: string): string {
  return t("{app} forgets {name} and everything it stored for it: button actions, pointer settings, its name and its picture. If it's connected, it goes back to its factory settings. It shows up again, with default settings, the next time it connects.", { name });
}

export async function removeDevice(dev: DeviceSummary): Promise<void> {
  const name = dev.name || t('Device');
  const ok = await confirm({
    title: t('Remove {name}?', { name }), text: removeText(name), confirm: t('Remove'), danger: true, icon: 'trash',
  });
  if (!ok) return;
  try {
    await api('forget', { id: dev.id });
    try {
      await shell.pictureRemove(dev.id);
    } catch {
      // the picture may not exist
    }
    toast(t('{name} removed', { name }));
    if (app.route.page === 'device' && app.route.id === dev.id) navigate({ page: 'devices' }, true);
    await refreshDevices();
  } catch (e) {
    toast(errorText(e), 'error');
  }
}
