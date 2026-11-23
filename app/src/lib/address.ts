// Flow peer addresses typed by the user: `host[:port]` (IPv4, [IPv6] or a host name).
// Checked while typing; the service checks again and has the last word.

import { t } from './i18n/index.svelte';

export const FLOW_PORT = 27461;
export const MAX_INPUT = 300;
const LABEL = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?$/;

export class AddressError extends Error {}

export function isAuto(text: string | null | undefined): boolean {
  return text == null || ['', 'auto'].includes(String(text).trim().toLowerCase());
}

function port(text: string): number {
  if (!/^\d{1,5}$/.test(text) || Number(text) < 1 || Number(text) > 65535) {
    throw new AddressError(t('The port must be a number from 1 to 65535'));
  }
  return Number(text);
}

function isHostname(host: string): boolean {
  const name = host.endsWith('.') ? host.slice(0, -1) : host;
  if (!name || name.length > 253) return false;
  const labels = name.split('.');
  if (!labels.every((l) => LABEL.test(l))) return false;
  return !/^\d+$/.test(labels[labels.length - 1]);
}

export function isIPv4(s: string): boolean {
  const parts = s.split('.');
  return parts.length === 4 && parts.every((p) => /^\d{1,3}$/.test(p) && Number(p) <= 255 && !(p.length > 1 && p[0] === '0'));
}

export function isIPv6(s: string): boolean {
  if (!s || s.length > 45 || !/^[0-9A-Fa-f:.%a-zA-Z0-9]+$/.test(s)) return false;
  let addr = s;
  const zone = addr.indexOf('%');
  if (zone >= 0) {
    if (zone === addr.length - 1) return false;
    addr = addr.slice(0, zone);
  }
  let groupsNeeded = 8;
  const lastColon = addr.lastIndexOf(':');
  if (addr.includes('.')) {
    if (!isIPv4(addr.slice(lastColon + 1))) return false;
    addr = addr.slice(0, lastColon + 1) + '0:0';
  }
  const halves = addr.split('::');
  if (halves.length > 2) return false;
  const groups = (h: string) => (h === '' ? [] : h.split(':'));
  const left = groups(halves[0]);
  const right = halves.length === 2 ? groups(halves[1]) : [];
  const all = [...left, ...right];
  if (!all.every((g) => /^[0-9A-Fa-f]{1,4}$/.test(g))) return false;
  if (halves.length === 2) return all.length < groupsNeeded;
  return all.length === groupsNeeded;
}

/** `host[:port]` → [host, port]; null for empty / `auto`. Throws AddressError (translated). */
export function parseAddress(text: string | null | undefined, defaultPort = FLOW_PORT): [string, number] | null {
  if (text == null) return null;
  let s = String(text).trim();
  if (s.length > MAX_INPUT) throw new AddressError(t('That address is too long'));
  if (isAuto(s)) return null;
  let p = defaultPort;
  if (s.startsWith('[')) {
    const end = s.indexOf(']');
    if (end < 0) throw new AddressError(t('An IPv6 address needs its closing bracket: [address]:port'));
    const host = s.slice(1, end);
    const rest = s.slice(end + 1);
    if (rest) {
      if (!rest.startsWith(':')) throw new AddressError(t('Invalid address'));
      p = port(rest.slice(1));
    }
    if (!isIPv6(host)) throw new AddressError(t("“{host}” isn't a valid IPv6 address", { host: host.slice(0, 64) }));
    return [host, p];
  }
  if ((s.match(/:/g) ?? []).length > 1) {
    if (!isIPv6(s)) throw new AddressError(t('Put an IPv6 address in brackets, like [fe80::1]:27461'));
    return [s, p];
  }
  const colon = s.indexOf(':');
  let host = s;
  if (colon >= 0) {
    host = s.slice(0, colon);
    p = port(s.slice(colon + 1));
  }
  if (!host) throw new AddressError(t("Type the computer's IP address or name"));
  if (host.split('.').every((part) => /^\d+$/.test(part))) {
    if (!isIPv4(host)) throw new AddressError(t("“{host}” isn't a valid IP address", { host: host.slice(0, 64) }));
    return [host, p];
  }
  if (!isHostname(host)) {
    throw new AddressError(t("“{host}” isn't a valid computer name or IP address", { host: host.slice(0, 64) }));
  }
  s = host.toLowerCase();
  return [s, p];
}

/** [host, port] → 'host:port' ('[v6]:port'). */
export function formatAddress(addr: [string, number] | null): string {
  if (!addr) return '';
  const [host, p] = addr;
  return host.includes(':') ? `[${host}]:${p}` : `${host}:${p}`;
}

/** For a field being typed: the address ('' = automatic, null = not valid) and a note. */
export function checkAddress(text: string): { address: string | null; message: string; error: boolean } {
  if (isAuto(text)) return { address: '', message: '', error: false };
  try {
    const addr = formatAddress(parseAddress(text));
    return { address: addr, message: t('{app} will look for it at {address}', { address: addr }), error: false };
  } catch (e) {
    return { address: null, message: (e as Error).message, error: true };
  }
}
