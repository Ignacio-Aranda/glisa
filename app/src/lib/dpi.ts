// Pointer speed (DPI) and SmartShift helpers.

import type { DpiInfo } from './types';

export const MAX_LEVELS = 5;

// SmartShift: the device stores a speed threshold (quarter turns per second) above which the
// wheel lets go; 255 means "never". The slider covers 1…50, one position per threshold.
export const SS_NEVER = 255;
export const SS_THRESHOLD_MAX = 50;
export const SS_DEFAULT_THRESHOLD = 16;

/** Device threshold → sensitivity shown (2…100 %, steps of 2; a lower threshold is more sensitive). */
export function thresholdToPercent(threshold: number): number {
  const t = Math.max(1, Math.min(SS_THRESHOLD_MAX, Math.trunc(threshold)));
  return (SS_THRESHOLD_MAX + 1 - t) * 2;
}

export function percentToThreshold(percent: number): number {
  const t = SS_THRESHOLD_MAX + 1 - Math.floor((Math.trunc(percent) + 1) / 2);
  return Math.max(1, Math.min(SS_THRESHOLD_MAX, t));
}

/** Every DPI the sensor accepts, or [] when the snapshot doesn't carry the whole list. */
export function dpiChoices(info: DpiInfo): number[] {
  const c = (info.choices ?? []).filter((v) => Number.isInteger(v));
  if (c.length && c[0] <= info.min && c[c.length - 1] >= info.max) return c;
  return [];
}

/** Nearest DPI the sensor accepts (a tie goes down, like the service). */
export function snapDpi(info: DpiInfo, value: number): number {
  const lo = info.min;
  const hi = info.max;
  const step = Math.max(1, info.step || 1);
  const v = Math.max(lo, Math.min(hi, Math.trunc(value)));
  const choices = dpiChoices(info);
  if (choices.length) {
    return choices.reduce((best, c) => (Math.abs(c - v) < Math.abs(best - v) ? c : best), choices[0]);
  }
  return Math.min(hi, lo + Math.floor((v - lo + Math.floor((step - 1) / 2)) / step) * step);
}

function gcd(a: number, b: number): number {
  a = Math.abs(a);
  b = Math.abs(b);
  while (b) [a, b] = [b, a % b];
  return a;
}

/** Step of the slider: the largest one that lands on every accepted value. */
export function gridStep(info: DpiInfo): number {
  let step = Math.max(1, info.step || 1);
  const choices = dpiChoices(info);
  if (choices.length > 1) {
    step = 0;
    for (let i = 1; i < choices.length; i++) step = gcd(step, choices[i] - choices[i - 1]);
  }
  return Math.max(1, gcd(step, info.min));
}

/** `steps` accepted values above (or below) `value`. */
export function stepDpi(info: DpiInfo, value: number, steps: number): number {
  const v = snapDpi(info, value);
  const choices = dpiChoices(info);
  if (choices.length) {
    const i = choices.indexOf(v) + steps;
    return choices[Math.max(0, Math.min(choices.length - 1, i))];
  }
  return snapDpi(info, v + steps * Math.max(1, info.step || 1));
}

/** Stored levels as the window shows them: valid for the sensor, unique, sorted, 1…5. */
export function cleanLevels(info: DpiInfo, levels: unknown): number[] {
  const list = Array.isArray(levels) ? levels.filter((v): v is number => typeof v === 'number') : [];
  const out = [...new Set(list.map((v) => snapDpi(info, v)))].sort((a, b) => a - b).slice(0, MAX_LEVELS);
  return out.length ? out : [snapDpi(info, info.default || info.min)];
}

/** A sensible value for a new level: one more step up if there is room, else the middle of the widest gap. */
export function suggestLevel(info: DpiInfo, levels: number[]): number | null {
  if (!levels.length) return null;
  const lo = info.min;
  const hi = info.max;
  const gap = levels.length > 1 ? levels[levels.length - 1] - levels[levels.length - 2] : 800;
  const up = snapDpi(info, levels[levels.length - 1] + gap);
  if (up > levels[levels.length - 1]) return up;
  const marks = [lo, ...levels, hi];
  let best: [number, number] = [marks[0], marks[1]];
  for (let i = 1; i < marks.length; i++) {
    if (marks[i] - marks[i - 1] > best[1] - best[0]) best = [marks[i - 1], marks[i]];
  }
  const mid = snapDpi(info, Math.floor((best[0] + best[1]) / 2));
  if (!levels.includes(mid)) return mid;
  const all = dpiChoices(info);
  const candidates = all.length ? all : Array.from({ length: Math.floor((hi - lo) / gridStep(info)) + 1 }, (_, i) => lo + i * gridStep(info));
  return candidates.find((v) => !levels.includes(v)) ?? null;
}
