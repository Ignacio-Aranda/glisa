// The mock backend must answer like the service.
import { describe, expect, it, beforeEach, afterEach } from 'vitest';
import { createMockBackend } from '../src/lib/mock';
import type { Device, DeviceSummary, FlowStatus, Status } from '../src/lib/types';

const LIFT = 'b031-lift';
const MX = 'b034-wireless_mouse_mx-sim';
const KEYS = 'b35b-wireless_keyboard_k-sim';

function make(query = '') {
  return createMockBackend(new URLSearchParams(query), { latency: 0 });
}

let b: ReturnType<typeof make>;
const api = <T>(method: string, params: Record<string, unknown> = {}, lang = 'en') => b.api(method, params, lang) as Promise<T>;
beforeEach(() => {
  b = make();
});
afterEach(() => b.service.stop());

describe('devices', () => {
  it('lists the three simulated devices with the short snapshot keys', async () => {
    const list = await api<DeviceSummary[]>('devices');
    expect(list.map((d) => d.id)).toEqual([LIFT, MX, KEYS]);
    expect(Object.keys(list[0]).sort()).toEqual(
      ['battery', 'connection', 'family', 'id', 'kind', 'model', 'name', 'online', 'photo', 'product_id', 'source'].sort());
    expect(list[0]).toMatchObject({ name: 'LIFT', model: 'LIFT', kind: 'mouse', product_id: 'B031', source: 'sim://lift',
      connection: 'bluetooth', battery: { percent: 78, level: 'good', charging: false, status: 'discharging' } });
  });

  it('full snapshots match the service', async () => {
    const lift = await api<Device>('device', { id: LIFT });
    expect(lift.controls.map((c) => c.cid)).toEqual([0x52, 0x53, 0x56, 0xfd]);
    expect(lift.controls[0]).toEqual({ cid: 82, name: 'Middle button', divertable: true, reprogrammable: true, gestures: true, action: { type: 'default' } });
    expect(lift.dpi).toMatchObject({ current: 1000, default: 1000, min: 400, max: 4000, step: 100, presets: [800, 1600, 2400] });
    expect(lift.hosts.count).toBe(3);
    expect(lift.hosts.entries[2]).toEqual({ index: 2, paired: false, name: '' });
    expect(lift.smartshift).toBeNull();
    expect(lift.firmware).toEqual(['FW RBM16.01.B0017']);
    const mx = await api<Device>('device', { id: MX });
    expect(mx.smartshift).toEqual({ ratchet: true, threshold: 30, enhanced: true });
    expect(mx.dpi?.choices.length).toBe(157);
    const keys = await api<Device>('device', { id: KEYS });
    expect(keys).toMatchObject({ kind: 'keyboard', dpi: null, wheel: null, smartshift: null, controls: [] });
  });

  it('answers in the client language (control names, errors)', async () => {
    const lift = await api<Device>('device', { id: LIFT }, 'es');
    expect(lift.controls[0].name).toBe('Clic central');
    await expect(api('device', { id: 'nope' }, 'es')).rejects.toThrow('Dispositivo no encontrado');
  });

  it('applies settings like the daemon (clamped DPI, sorted levels, SmartShift)', async () => {
    expect(await api('set', { id: LIFT, key: 'dpi', value: 1049 })).toBe(1000);
    expect(await api('set', { id: LIFT, key: 'dpi_presets', value: [3000, 800, 800, 1210] })).toEqual([800, 1200, 3000]);
    await api('set', { id: MX, key: 'ss_threshold', value: 255 });
    expect((await api<Device>('device', { id: MX })).smartshift?.threshold).toBe(255);
    expect((await api<Device>('device', { id: LIFT })).settings).toMatchObject({ dpi: 1000, dpi_presets: [800, 1200, 3000] });
    await expect(api('set', { id: LIFT, key: 'nonsense', value: 1 })).rejects.toThrow('Unknown setting: nonsense');
  });

  it('sets, validates and resets buttons', async () => {
    await api('set_button', { id: LIFT, cid: 0x53, action: { type: 'keys', combo: 'Ctrl+C' } });
    expect((await api<Device>('device', { id: LIFT })).controls[1].action).toEqual({ type: 'keys', combo: 'Ctrl+C' });
    await expect(api('set_button', { id: LIFT, cid: 0x53, action: { type: 'command', cmd: ' ' } })).rejects.toThrow('Empty command');
    await expect(api('set_button', { id: LIFT, cid: 0x999, action: null })).rejects.toThrow('Unknown control');
    await api('reset', { id: LIFT });
    expect((await api<Device>('device', { id: LIFT })).controls[1].action).toEqual({ type: 'default' });
  });

  it('renames (40 characters at most; the model name or "" goes back)', async () => {
    const r = await api<DeviceSummary>('rename', { id: LIFT, name: '  Mi \n ratón ' });
    expect(r).toMatchObject({ name: 'Mi ratón', model: 'LIFT' });
    await expect(api('rename', { id: LIFT, name: 'x'.repeat(41) }, 'es')).rejects.toThrow('El nombre no puede tener más de 40 caracteres');
    expect((await api<DeviceSummary>('rename', { id: LIFT, name: 'LIFT' })).name).toBe('LIFT');
  });

  it('forgets a device and refuses writes while disconnected', async () => {
    await api('forget', { id: KEYS });
    expect((await api<DeviceSummary[]>('devices')).length).toBe(2);
    b.service.devices[0].online = false;
    await expect(api('set', { id: LIFT, key: 'dpi', value: 800 })).rejects.toThrow('The device is disconnected');
    expect((await api<DeviceSummary>('rename', { id: LIFT, name: 'Away' })).name).toBe('Away');
  });

  it('status carries platform and permissions', async () => {
    const st = await api<Status>('status');
    expect(st).toMatchObject({ simulate: true, platform: 'linux', devices: 3 });
    expect(st.permissions?.map((p) => p.id)).toEqual(['hid', 'uinput']);
    b.service.stop();
    b = make('platform=macos&mock=no-permissions');
    const mac = await api<Status>('status');
    expect(mac.permissions?.find((p) => p.id === 'accessibility')?.state).toBe('missing');
    expect(await api('devices')).toEqual([]);
  });

  it('emits events after changes', async () => {
    const seen: string[] = [];
    b.onEvent((e) => seen.push(e.topic));
    await api('set', { id: LIFT, key: 'dpi', value: 800 });
    await new Promise((r) => setTimeout(r, 5));
    expect(seen).toContain('device');
  });
});

describe('presets', () => {
  it('answer like crates/service presets_json, translated, and normalise for the window', async () => {
    const { normalizePresets } = await import('../src/lib/state/app.svelte');
    const es = await api<import('../src/lib/types').PresetsAnswer>('presets', {}, 'es');
    expect(es.platform).toBe('linux');
    expect(es.categories[1]).toMatchObject({ id: 'Editing', label: 'Edición' });
    expect(es.categories[0].items[3]).toMatchObject({ id: 'Back', label: 'Atrás', action: { type: 'mouse', button: 'back' } });
    expect(es.default_gestures?.down).toEqual({ type: 'keys', combo: 'Meta+D' });
    expect(es.gesture_slots?.[1]).toEqual({ id: 'up', label: 'Arriba' });
    const { presets, gestures } = normalizePresets(es);
    expect(presets[1].items[0]).toEqual({ label: 'Copy', title: 'Copiar', action: { type: 'keys', combo: 'Ctrl+C' } });
    expect(gestures?.type).toBe('gesture');
  });
});

describe('shell commands', () => {
  it('settings, pictures and the picture layout', async () => {
    expect(await b.command('settings_get')).toMatchObject({ theme: 'system', language: 'system', official_photos: false });
    expect(await b.command('settings_set', { patch: { theme: 'dark' } })).toMatchObject({ theme: 'dark' });
    expect(await b.command('picture_get', { deviceId: LIFT })).toBeNull();
    expect(await b.command('picture_choose', { deviceId: LIFT })).toEqual({ ok: true });
    expect(String(await b.command('picture_get', { deviceId: LIFT }))).toMatch(/^data:image\//);
    expect(await b.command('picture_from_url', { deviceId: LIFT, url: 'http://example.org/a.png' })).toMatchObject({ ok: false });
    const layout = await b.command('picture_layout_set', { deviceId: LIFT,
      layout: { use_on_pages: true, anchors: { '82': { x: 1.4, y: -1 }, bad: { x: 0, y: 0 } } } });
    expect(layout).toEqual({ use_on_pages: true, anchors: { '82': { x: 1, y: 0 } } });
    await b.command('picture_remove', { deviceId: LIFT });
    expect(await b.command('picture_layout_get', { deviceId: LIFT })).toEqual({ use_on_pages: false, anchors: {} });
  });
});

describe('flow', () => {
  it('status has every key of flow_status', async () => {
    const st = await api<FlowStatus>('flow_status');
    for (const k of ['version', 'enabled', 'running', 'demo', 'error', 'backend', 'backend_state', 'backend_message',
      'available', 'ctrl_available', 'can_place', 'clipboard_backend', 'files_backend', 'transfers', 'staging', 'id',
      'name', 'fingerprint', 'ports', 'firewall', 'options', 'peers', 'discovered', 'pairing', 'devices', 'screens',
      'last_switch', 'last_arrival', 'events_seq', 'clipboard_seq']) {
      expect(st).toHaveProperty(k);
    }
    expect(st.ports.pairing).toBe(st.ports.tcp + 1);
    expect(Object.keys(st.peers[0]).sort()).toEqual(['address', 'address_source', 'fingerprint', 'files', 'id',
      'last_error', 'learned_address', 'manual_address', 'name', 'offline_for', 'online', 'side', 'state'].sort());
    expect(st.devices[0].channels[st.peers[0].id]).toEqual({ channel: null, suggested: null });
  });

  it('pairs with a six-digit code confirmed on both sides', async () => {
    b.service.stop();
    b = make('flow=new');
    await api('flow_pair_start');
    expect((await api<FlowStatus>('flow_status')).pairing?.state).toBe('waiting');
    await api('flow_pair_request', { peer: 'c0ffee00d15c0ve2' });
    await new Promise((r) => setTimeout(r, 1000));
    const p = (await api<FlowStatus>('flow_status')).pairing!;
    expect(p.state).toBe('confirm');
    expect(p.sas).toMatch(/^\d{6}$/);
    await api('flow_pair_confirm', { accept: true });
    await new Promise((r) => setTimeout(r, 3100));
    const st = await api<FlowStatus>('flow_status');
    expect(st.pairing?.state).toBe('done');
    expect(st.peers.map((x) => x.name)).toEqual(['office-desktop']);
    const ev = await api<{ events: { text: string }[] }>('flow_events', { since: 0 }, 'es');
    expect(ev.events.at(-1)?.text).toBe('Vinculado con office-desktop');
  }, 8000);

  it('rejecting the numbers saves nothing', async () => {
    b.service.stop();
    b = make('flow=new');
    await api('flow_pair_start');
    await api('flow_pair_request', { address: '10.0.0.5:27461' });
    await new Promise((r) => setTimeout(r, 1000));
    await api('flow_pair_confirm', { accept: false });
    const st = await api<FlowStatus>('flow_status');
    expect(st.pairing).toMatchObject({ state: 'failed', reason: 'rejected', message: "You said the numbers don't match. Nothing was saved" });
    expect(st.peers).toEqual([]);
  }, 4000);

  it('layout, options, channels, addresses and validation', async () => {
    const st = await api<FlowStatus>('flow_status');
    const peer = st.peers[0].id;
    expect(await api('flow_set_layout', { peer, side: 'top' })).toEqual({ [peer]: 'top' });
    await expect(api('flow_set_layout', { peer, side: 'diagonal' })).rejects.toThrow('Invalid position');
    expect((await api<{ cooldown_ms: number }>('flow_set_options', { cooldown_ms: 10 })).cooldown_ms).toBe(200);
    await expect(api('flow_set_options', { require_ctrl: 'sí' })).rejects.toThrow('require_ctrl must be true or false');
    expect(await api('flow_set_channel', { device: LIFT, peer, channel: 1 })).toBe(true);
    await expect(api('flow_set_channel', { device: LIFT, peer, channel: 7 })).rejects.toThrow('Invalid channel');
    const p = await api<{ address: string; address_source: string }>('flow_set_peer_address', { peer, address: '10.1.2.3' });
    expect(p).toMatchObject({ address: '10.1.2.3:27461', address_source: 'manual' });
    expect((await api<FlowStatus>('flow_enable', { enabled: false })).running).toBe(false);
  });

  it('transfers report progress and can be cancelled', async () => {
    b.service.flow.startTransfer('receive');
    await new Promise((r) => setTimeout(r, 1100));
    const ev = await api<{ transfers: { id: string; active: boolean; percent: number }[] }>('flow_events', { since: 0 });
    expect(ev.transfers[0].active).toBe(true);
    expect(ev.transfers[0].percent).toBeGreaterThan(0);
    expect(await api('flow_transfer_cancel', { transfer: ev.transfers[0].id })).toBe(true);
    const after = await api<{ transfers: { state: string }[] }>('flow_events', { since: 0 });
    expect(after.transfers[0].state).toBe('cancelled');
  });
});
