import { describe, expect, it, afterEach } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { createRawSnippet } from 'svelte';
import Dialog from '../src/lib/components/Dialog.svelte';
import Segmented from '../src/lib/components/Segmented.svelte';
import Toggle from '../src/lib/components/Toggle.svelte';
import DeviceArt, { spread } from '../src/lib/components/DeviceArt.svelte';

afterEach(cleanup);

const body = createRawSnippet(() => ({ render: () => '<div><input aria-label="first" /><button>Middle</button></div>' }));

describe('Dialog', () => {
  it('is a labelled modal that focuses inside, traps Tab and closes on Esc', async () => {
    let closed = 0;
    render(Dialog, { props: { open: true, title: 'Rename device', onclose: () => closed++, children: body } });
    const dialog = screen.getByRole('dialog', { name: 'Rename device' });
    expect(dialog.getAttribute('aria-modal')).toBe('true');
    await new Promise((r) => setTimeout(r, 0));
    expect(document.activeElement).toBe(screen.getByLabelText('first'));
    const close = screen.getByRole('button', { name: 'Close' });
    close.focus();
    await fireEvent.keyDown(dialog, { key: 'Tab', shiftKey: true });
    await fireEvent.keyDown(dialog, { key: 'Escape' });
    expect(closed).toBe(1);
  });
});

describe('controls', () => {
  it('segmented control is a radio group driven by the arrow keys', async () => {
    let value = 'light';
    render(Segmented, { props: { label: 'Theme', value, options: [
      { value: 'system', label: 'System' }, { value: 'light', label: 'Light' }, { value: 'dark', label: 'Dark' },
    ], onchange: (v: string | number) => (value = String(v)) } });
    const light = screen.getByRole('radio', { name: 'Light' });
    expect(light.getAttribute('aria-checked')).toBe('true');
    expect(light.tabIndex).toBe(0);
    await fireEvent.keyDown(light, { key: 'ArrowRight' });
    expect(value).toBe('dark');
  });

  it('toggle is a switch', async () => {
    let on = false;
    render(Toggle, { props: { checked: false, label: 'Smooth scrolling', onchange: (v: boolean) => (on = v) } });
    const sw = screen.getByRole('switch', { name: 'Smooth scrolling' });
    expect(sw.getAttribute('aria-checked')).toBe('false');
    await fireEvent.click(sw);
    expect(on).toBe(true);
  });
});

describe('DeviceArt', () => {
  const PNG = 'data:image/png;base64,iVBORw0KGgo=';

  it('draws a hotspot for each control placed on the picture, none for the others', () => {
    render(DeviceArt, { props: { picture: {
      src: PNG, layout: { use_on_pages: true, anchors: { '82': { x: 0.5, y: 0.25 } } },
    }, spots: [
      { cid: 82, name: 'Middle button', detail: 'Default' },
      { cid: 0x1234, name: 'Mystery', detail: 'Copy', custom: true },
    ], onselect: () => {} } });
    expect(document.querySelector('.art img')).toBeTruthy();
    const middle = screen.getByRole('button', { name: 'Middle button: Default' });
    expect(middle.tabIndex).toBe(0);
    expect(screen.queryByRole('button', { name: 'Mystery: Copy' })).toBeNull();
  });

  it('shows the picture alone when nothing is placed yet', () => {
    render(DeviceArt, { props: { picture: { src: PNG, layout: { use_on_pages: true, anchors: {} } },
      spots: [{ cid: 82, name: 'Middle button' }] } });
    expect(document.querySelector('.art img')).toBeTruthy();
    expect(document.querySelectorAll('.spot').length).toBe(0);
  });

  it('spreads callouts apart within the canvas', () => {
    const ys = spread([100, 105, 110], 50, 0, 1000);
    expect(ys[1] - ys[0]).toBeGreaterThanOrEqual(50);
    expect(ys[2] - ys[1]).toBeGreaterThanOrEqual(50);
    const clamped = spread([990, 995], 50, 0, 1000);
    expect(Math.max(...clamped)).toBeLessThanOrEqual(1000);
  });
});
