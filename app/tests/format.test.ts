import { describe, expect, it } from 'vitest';
import { batteryText, batteryTone, clock, formatCode, sizeText, transferText, transferWhat } from '../src/lib/format';
import type { Transfer } from '../src/lib/types';

const transfer = (over: Partial<Transfer>): Transfer => ({
  id: 'x', direction: 'receive', peer: 'p', name: 'studio-pc', kind: 'files', state: 'receiving', active: true,
  items: 3, top_folders: 0, files: 3, folders: 0, bytes: 100, done: 50, percent: 50, failed: 0, skipped: 0,
  first: 'a', folder: '', placed: '', cut: false, ago: 0, ...over,
});

describe('format', () => {
  it('battery', () => {
    expect(batteryText({ percent: 78, level: 'good', charging: false, status: 'discharging' })).toBe('78%');
    expect(batteryText({ percent: 40, level: 'good', charging: true, status: 'recharging' })).toBe('40% · Charging');
    expect(batteryText({ percent: 9, level: 'critical', charging: false, status: 'discharging' })).toBe('9% · Very low battery');
    expect(batteryText({ percent: null, level: 'good', charging: false, status: '' })).toBe('Good');
    expect(batteryText(null)).toBe('');
    expect(batteryTone({ percent: 12, level: 'low', charging: false, status: '' })).toBe('warn');
    expect(batteryTone({ percent: 5, level: 'critical', charging: false, status: '' })).toBe('danger');
  });

  it('sizes, clocks and codes', () => {
    expect(sizeText(2048)).toBe('2 GB');
    expect(sizeText(500)).toBe('500 MB');
    expect(sizeText(1536)).toBe('1.5 GB');
    expect(clock(95)).toBe('1:35');
    expect(formatCode('372649')).toBe('372 649');
  });

  it('transfers (flow/text.py)', () => {
    expect(transferWhat(transfer({ kind: 'image' }))).toBe('an image');
    expect(transferWhat(transfer({ items: 2, top_folders: 2 }))).toBe('2 folders');
    expect(transferWhat(transfer({ items: 3, top_folders: 1 }))).toBe('3 items');
    expect(transferText(transfer({}))).toBe('Receiving 3 files from studio-pc… 50%');
    expect(transferText(transfer({ state: 'done', active: false, placed: 'clipboard' })))
      .toBe('Received 3 files from studio-pc — paste them with Ctrl+V');
    expect(transferText(transfer({ state: 'done', active: false, direction: 'send' }))).toBe('Sent 3 files to studio-pc');
    // dragged here and dropped where the user chose: nothing to paste
    expect(transferText(transfer({ state: 'done', active: false, placed: 'drop' }))).toBe('Received 3 files from studio-pc');
    expect(transferText(transfer({ state: 'cancelled', active: false, direction: 'send' }))).toBe('Cancelled: 3 files for studio-pc');
  });
});
