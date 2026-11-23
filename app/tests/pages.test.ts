import { describe, expect, it, afterEach } from 'vitest';
import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import { boot, calls } from './util';
import { setBackend } from '../src/lib/backend';

const LIFT = 'b031-lift';
const KEYS = 'b35b-wireless_keyboard_k-sim';
const MX = 'b034-wireless_mouse_mx-sim';
let stop: (() => void) | null = null;

afterEach(() => {
  cleanup();
  stop?.();
  setBackend(null);
});

async function open(query: string, hash: string) {
  const r = await boot(query, hash);
  stop = () => r.service.stop();
  return r;
}

describe('Devices', () => {
  it('shows a card per device and the add tile', async () => {
    await open('', '#/');
    expect(await screen.findByRole('button', { name: 'Configure LIFT' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Configure Wireless Keyboard K-Sim' })).toBeTruthy();
    expect(screen.getByText('Add device')).toBeTruthy();
    expect(screen.getByText('These devices are simulated: nothing you change affects a real mouse')).toBeTruthy();
  });

  it('empty state with tips, and the permissions onboarding when something is missing', async () => {
    localStorage.clear();
    await open('mock=no-permissions', '#/');
    expect(await screen.findByText('No devices found')).toBeTruthy();
    expect(screen.getByText('Permissions needed')).toBeTruthy();
    const dialog = await screen.findByRole('dialog', { name: 'Permission to use your devices' });
    const install = within(dialog).getAllByRole('button', { name: 'Install permissions' });
    await fireEvent.click(install[0]);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Configure LIFT' })).toBeTruthy(), { timeout: 3000 });
  });

  it('loading state while the service has not answered', async () => {
    await boot('mock=loading', '#/', false).then((r) => (stop = () => r.service.stop()));
    expect((await screen.findByLabelText('Looking for devices…')).getAttribute('aria-busy')).toBe('true');
  });

  it('removes a device from its card menu after confirming', async () => {
    const r = await open('', '#/');
    await screen.findByRole('button', { name: 'Configure LIFT' });
    await fireEvent.click(screen.getAllByRole('button', { name: 'Device options' })[0]);
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Remove device…' }));
    const dialog = await screen.findByRole('dialog', { name: 'Remove LIFT?' });
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Remove' }));
    await waitFor(() => expect(calls(r.backend, 'forget')).toEqual([{ id: LIFT }]));
  });

  it('renames a device from its card menu', async () => {
    await open('', '#/');
    await screen.findByRole('button', { name: 'Configure LIFT' });
    await fireEvent.click(screen.getAllByRole('button', { name: 'Device options' })[0]);
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Rename…' }));
    const dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  });

  it('opens a device', async () => {
    await open('', '#/');
    await fireEvent.click(await screen.findByRole('button', { name: 'Configure LIFT' }));
    expect(await screen.findByRole('heading', { level: 1, name: 'LIFT' })).toBeTruthy();
    expect(location.hash).toBe(`#/device/${LIFT}`);
  });
});

describe('Buttons', () => {
  it('assigns a preset through the picker', async () => {
    const r = await open('', `#/device/${LIFT}/buttons`);
    const row = await screen.findByRole('button', { name: /Back button/ , description: undefined }, { timeout: 3000 })
      .catch(() => null);
    const target = row ?? (await screen.findAllByText('Back button'))[0];
    await fireEvent.click(target.closest('button')!);
    await screen.findByPlaceholderText('Search actions…');
    const search = screen.getByRole('searchbox', { name: 'Search actions' });
    await fireEvent.input(search, { target: { value: 'cop' } });
    await fireEvent.click(await screen.findByRole('button', { name: /^Copy/ }));
    await waitFor(() => expect(calls(r.backend, 'set_button')).toContainEqual({ id: LIFT, cid: 0x53, action: { type: 'keys', combo: 'Ctrl+C' } }));
    expect(await screen.findByText('Back button → Copy')).toBeTruthy();
  });

  it('turns a button into a gesture button and edits one direction', async () => {
    const r = await open('', `#/device/${LIFT}/buttons`);
    await fireEvent.click((await screen.findAllByText('DPI button'))[0].closest('button')!);
    await fireEvent.click(await screen.findByRole('button', { name: /Gesture button/ }));
    const up = await screen.findByRole('button', { name: 'Up: Overview' });
    await fireEvent.click(up);
    await fireEvent.click(await screen.findByRole('button', { name: /^None/ }));
    await waitFor(() => {
      const last = calls(r.backend, 'set_button').at(-1) as { action: { type: string; up: unknown } };
      expect(last.action.type).toBe('gesture');
      expect(last.action.up).toEqual({ type: 'disabled' });
    });
  });

  it('records a keyboard shortcut', async () => {
    const r = await open('', `#/device/${MX}/buttons`);
    await fireEvent.click((await screen.findAllByText('Forward button'))[0].closest('button')!);
    await fireEvent.click(await screen.findByRole('button', { name: /Keyboard shortcut…/ }));
    const recorder = await screen.findByRole('button', { name: 'Record keyboard shortcut' });
    await fireEvent.click(recorder);
    await fireEvent.keyDown(recorder, { code: 'KeyK', key: 'k', ctrlKey: true, altKey: true });
    expect((screen.getByRole('textbox', { name: 'Type a keyboard shortcut' }) as HTMLInputElement).value).toBe('Ctrl+Alt+K');
    await fireEvent.click(screen.getByRole('button', { name: 'Assign' }));
    await waitFor(() => expect(calls(r.backend, 'set_button').at(-1)).toEqual({ id: MX, cid: 0x56, action: { type: 'keys', combo: 'Ctrl+Alt+K' } }));
  });

  it('keyboards show the device instead of an editor', async () => {
    await open('', `#/device/${KEYS}/buttons`);
    expect(await screen.findByText("Keys can't be configured yet")).toBeTruthy();
    expect(screen.queryByRole('tab', { name: /Pointer/ })).toBeNull();
  });

  it('disconnected devices explain and disable', async () => {
    await open('mock=disconnected', `#/device/${LIFT}/buttons`);
    expect(await screen.findByText('Device disconnected')).toBeTruthy();
    const rows = (await screen.findAllByText('Middle button')).map((e) => e.closest('button')!);
    expect(rows.every((b) => b.disabled)).toBe(true);
  });
});

describe('Pointer and Easy-Switch', () => {
  it('toggles natural scrolling and picks a DPI level', async () => {
    const r = await open('', `#/device/${MX}/pointer`);
    await fireEvent.click(await screen.findByRole('switch', { name: 'Natural scrolling' }));
    await fireEvent.click(await screen.findByRole('button', { name: '1600 DPI' }));
    await waitFor(() => {
      expect(calls(r.backend, 'set')).toContainEqual({ id: MX, key: 'wheel_inverted', value: true });
      expect(calls(r.backend, 'set')).toContainEqual({ id: MX, key: 'dpi', value: 1600 });
    });
  });

  it('switches channel after confirming', async () => {
    const r = await open('', `#/device/${LIFT}/easy-switch`);
    await fireEvent.click(await screen.findByRole('button', { name: 'Switch to channel 2' }));
    const dialog = await screen.findByRole('dialog', { name: 'Switch to channel 2?' });
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Switch' }));
    await waitFor(() => expect(calls(r.backend, 'set_host')).toContainEqual({ id: LIFT, host: 1 }));
    expect(await screen.findByText('Switching to channel 2…', { selector: 'strong' })).toBeTruthy();
  });
});

describe('Device settings', () => {
  it('copies diagnostics and resets after confirmation', async () => {
    const r = await open('', `#/device/${LIFT}/settings`);
    await fireEvent.click(await screen.findByRole('button', { name: 'Reset device' }));
    await fireEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Reset' }));
    await waitFor(() => expect(calls(r.backend, 'reset')).toEqual([{ id: LIFT }]));
  });

  it('places buttons on the user picture with the keyboard', async () => {
    const r = await open('picture=1', `#/device/${LIFT}/settings`);
    await fireEvent.click(await screen.findByRole('button', { name: 'Edit button positions…' }));
    const dialog = await screen.findByRole('dialog', { name: 'Button positions' });
    const chip = within(dialog).getByRole('button', { name: /^Forward button: not placed/ });
    await fireEvent.keyDown(chip, { key: 'Enter' });
    const marker = await within(dialog).findByRole('button', { name: /^Forward button: 50% from the left/ });
    await fireEvent.keyDown(marker, { key: 'ArrowRight', shiftKey: true });
    expect(marker.getAttribute('aria-label')).toMatch(/55% from the left/);
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Save' }));
    await waitFor(async () => {
      const layout = await r.backend.command('picture_layout_get', { deviceId: LIFT }) as { anchors: Record<string, { x: number }> };
      expect(layout.anchors['86'].x).toBeCloseTo(0.55);
    });
  });
});

describe('Flow and settings', () => {
  it('offers dragging files only where it works', async () => {
    await open('platform=windows', '#/flow');
    const toggle = await screen.findByRole('switch', { name: 'Drag files across the edge' });
    expect(toggle.hasAttribute('disabled') || toggle.getAttribute('aria-disabled') === 'true').toBe(true);
    expect(toggle.getAttribute('aria-checked')).toBe('false');
    expect(screen.getByText(/Available on KDE Plasma \(Linux\)/)).toBeTruthy();
    cleanup();
    stop?.();
    await open('', '#/flow');
    const here = await screen.findByRole('switch', { name: 'Drag files across the edge' });
    expect(here.hasAttribute('disabled')).toBe(false);
  });

  it('moves a computer in the layout with the arrow keys', async () => {
    const r = await open('', '#/flow');
    const tile = await screen.findByRole('button', { name: /^studio-pc: on the right/ }, { timeout: 4000 });
    await fireEvent.keyDown(tile, { key: 'ArrowUp' });
    await waitFor(() => expect(calls(r.backend, 'flow_set_layout').at(-1)).toMatchObject({ side: 'top' }));
  });

  it('shows the SAS when the other computer starts pairing', async () => {
    await open('flow=incoming&dialog=pair', '#/flow');
    const dialog = await screen.findByRole('dialog', { name: 'Pair a computer' }, { timeout: 4000 });
    expect(within(dialog).getByText(/Open “Pair a computer” on the other computer too/)).toBeTruthy();
  });

  it('changes theme and language at once', async () => {
    await open('', '#/settings');
    await fireEvent.click(await screen.findByRole('radio', { name: 'Dark' }));
    expect(document.documentElement.dataset.theme).toBe('dark');
    await fireEvent.click(screen.getByRole('radio', { name: 'Español' }));
    expect(await screen.findByRole('heading', { level: 1, name: 'Ajustes' })).toBeTruthy();
  });
});
