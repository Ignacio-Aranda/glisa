import { describe, expect, it } from 'vitest';
import {
  cleanLevels, gridStep, percentToThreshold, snapDpi, stepDpi, suggestLevel, thresholdToPercent,
} from '../src/lib/dpi';
import { comboFromEvent, isValidCombo, keyLabel, normalizeCombo, ComboError } from '../src/lib/keys';
import { checkAddress, parseAddress, AddressError } from '../src/lib/address';
import { defaultGestures, describe as describeAction, findPreset, iconFor, sameAction, slotLabel } from '../src/lib/actions';
import { presetsFor } from '../src/lib/mock/presets';
import type { DpiInfo } from '../src/lib/types';

const lift: DpiInfo = { current: 1000, default: 1000, min: 400, max: 4000, step: 100, choices: [], presets: [800, 1600, 2400] };
for (let v = 400; v <= 4000; v += 100) lift.choices.push(v);

describe('dpi', () => {
  it('snaps, steps and cleans levels like the Python GUI', () => {
    expect(snapDpi(lift, 1049)).toBe(1000);
    expect(snapDpi(lift, 99999)).toBe(4000);
    expect(gridStep(lift)).toBe(100);
    expect(stepDpi(lift, 1000, 2)).toBe(1200);
    expect(cleanLevels(lift, [2400, 800, 800, 1610])).toEqual([800, 1600, 2400]);
    expect(cleanLevels(lift, [])).toEqual([1000]);
    expect(suggestLevel(lift, [800, 1600, 2400])).toBe(3200);
    expect(suggestLevel(lift, [3200, 4000])).toBe(1800);
  });

  it('SmartShift sensitivity maps both ways for every slider position', () => {
    for (let pct = 2; pct <= 100; pct += 2) expect(thresholdToPercent(percentToThreshold(pct))).toBe(pct);
    expect(thresholdToPercent(1)).toBe(100);
    expect(thresholdToPercent(255)).toBe(2);
  });
});

describe('keys', () => {
  it('normalises and validates combinations', () => {
    expect(normalizeCombo('meta + tab')).toBe('Meta+Tab');
    expect(normalizeCombo('ctrl+shift+t')).toBe('Ctrl+Shift+T');
    expect(normalizeCombo('cmd+c')).toBe('Meta+C');
    expect(isValidCombo('Ctrl+Plus')).toBe(true);
    expect(() => normalizeCombo('Ctrl+Bogus')).toThrow(ComboError);
    expect(() => normalizeCombo('+')).toThrow(/Empty shortcut/);
  });

  it('records from keyboard events', () => {
    const base = { ctrlKey: false, shiftKey: false, altKey: false, metaKey: false };
    expect(comboFromEvent({ ...base, ctrlKey: true, code: 'ControlLeft' })).toEqual({ combo: 'Ctrl', done: false });
    expect(comboFromEvent({ ...base, ctrlKey: true, shiftKey: true, code: 'KeyT' })).toEqual({ combo: 'Ctrl+Shift+T', done: true });
    expect(comboFromEvent({ ...base, metaKey: true, code: 'ArrowLeft' }).combo).toBe('Meta+Left');
    expect(comboFromEvent({ ...base, code: 'F5' }).combo).toBe('F5');
  });

  it('labels keys per platform', () => {
    expect(keyLabel('Meta', 'windows')).toBe('Win');
    expect(keyLabel('Meta', 'macos')).toContain('Cmd');
    expect(keyLabel('Plus', 'linux')).toBe('+');
  });
});

describe('addresses (flow/address.py)', () => {
  it('accepts IPv4, [IPv6]:port and host names', () => {
    expect(parseAddress('192.168.1.20')).toEqual(['192.168.1.20', 27461]);
    expect(parseAddress('[fe80::1]:2000')).toEqual(['fe80::1', 2000]);
    expect(parseAddress('Office-PC.local:27461')).toEqual(['office-pc.local', 27461]);
    expect(parseAddress('auto')).toBeNull();
    expect(checkAddress('')).toEqual({ address: '', message: '', error: false });
  });

  it('refuses the rest with a message', () => {
    expect(() => parseAddress('300.1.1.1')).toThrow(AddressError);
    expect(() => parseAddress('host:70000')).toThrow(/port/);
    expect(() => parseAddress('fe80::1')).not.toThrow();
    expect(() => parseAddress('[fe80::1')).toThrow(/bracket/);
    expect(checkAddress('bad host').error).toBe(true);
  });
});

describe('actions', () => {
  const presets = presetsFor('linux');
  it('describes actions with the presets', () => {
    expect(describeAction({ type: 'default' }, presets)).toBe('Default');
    expect(describeAction({ type: 'keys', combo: 'Ctrl+C' }, presets)).toBe('Copy');
    expect(describeAction({ type: 'keys', combo: 'Ctrl+Alt+Q' }, presets)).toBe('Shortcut: Ctrl+Alt+Q');
    expect(describeAction({ type: 'command', cmd: 'konsole' }, presets)).toBe('Run: konsole');
    expect(describeAction({ type: 'mouse', button: 'back' }, presets)).toBe('Back');
    expect(describeAction(defaultGestures('linux'), presets)).toBe('Gesture button');
    expect(slotLabel({ type: 'disabled' }, presets)).toBe('None');
    expect(findPreset({ type: 'scroll', dir: 'left' }, presets)?.label).toBe('Scroll left');
    expect(iconFor({ type: 'keys', combo: 'Ctrl+C' }, presets)).toBe('copy');
  });

  it('presets differ per OS', () => {
    const win = presetsFor('windows');
    const mac = presetsFor('macos');
    expect(describeAction({ type: 'keys', combo: 'Meta+C' }, mac)).toBe('Copy');
    expect(describeAction({ type: 'keys', combo: 'Meta+Tab' }, win)).toBe('Overview');
    expect(defaultGestures('macos').up).toEqual({ type: 'keys', combo: 'Ctrl+Up' });
  });

  it('compares actions structurally', () => {
    expect(sameAction({ type: 'keys', combo: 'A' }, { combo: 'A', type: 'keys' })).toBe(true);
    expect(sameAction({ type: 'keys', combo: 'A' }, { type: 'keys', combo: 'B' })).toBe(false);
  });
});
