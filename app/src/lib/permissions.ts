// What the app needs from each OS (status.permissions) explained in plain words.

import { t, tn } from './i18n/index.svelte';
import type { Permission, Platform, Status } from './types';

export interface PermissionText {
  icon: string;
  title: string;
  text: string;
}

export function describePermission(id: string): PermissionText {
  switch (id) {
    case 'hid':
    case 'hidraw':
    case 'udev':
      return { icon: 'mouse', title: t('Access to your Logitech devices'),
        text: t('A system rule (udev) so your user can read and change their settings without being an administrator') };
    case 'uinput':
      return { icon: 'keyboard', title: t('Sending key presses'),
        text: t('So a mouse button can trigger shortcuts such as copy or switching desktops') };
    case 'accessibility':
      return { icon: 'accessibility', title: t('Accessibility'),
        text: t('So buttons can send keyboard shortcuts and Flow can move the pointer') };
    case 'input_monitoring':
      return { icon: 'mouse', title: t('Input Monitoring'),
        text: t('So {app} can talk to your mice and keyboards') };
    case 'local_network':
      return { icon: 'wifi', title: t('Local Network'),
        text: t('So Flow can find your other computers') };
    default:
      return { icon: 'shield', title: id, text: '' };
  }
}

export function missingPermissions(status: Status | null): Permission[] {
  return (status?.permissions ?? []).filter((p) => p.state === 'missing');
}

/** '' when everything is fine, else a plain-language explanation for the Devices banner. */
export function permissionSummary(status: Status | null): string {
  if (!status) return '';
  const missing = new Set(missingPermissions(status).map((p) => p.id));
  const denied = status.denied?.length ?? 0;
  const parts: string[] = [];
  if (denied) {
    parts.push(tn('A Logitech device is connected, but your user isn\'t allowed to use it.',
      '{n} Logitech devices are connected, but your user isn\'t allowed to use them.', denied, { n: denied }));
  } else if (missing.has('hid') || missing.has('hidraw') || missing.has('udev') || missing.has('input_monitoring')) {
    parts.push(t('Permissions have to be installed once so that {app} can see and configure your devices.'));
  }
  if (status.input_error || missing.has('uinput') || missing.has('accessibility')) {
    parts.push(t("Without access to the virtual keyboard, buttons can't send keyboard shortcuts."));
  }
  if (!parts.length && missing.size) parts.push(t('Some permissions are missing: {app} may not work fully.'));
  return parts.join(' ');
}

/** Intro of the permissions dialog, per OS. */
export function permissionsIntro(platform: Platform): string {
  if (platform === 'macos') {
    return t('macOS asks you to allow apps that read input devices or send key presses. Turn {app} on in each of these panes of System Settings.');
  }
  if (platform === 'windows') {
    return t('Windows lets {app} use your devices without extra permissions.');
  }
  return t("For security, Linux doesn't let a regular app talk to your mouse directly or send key presses. {app} needs to install a small system rule, just once.");
}
