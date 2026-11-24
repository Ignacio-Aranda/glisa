// Flow as the service runs it in demo mode: status snapshots, discovery, SAS pairing in
// both directions, peers, layout, channels, options, notices and transfers of copied files
// and images with progress.

import type {
  Discovered, FlowNotice, FlowOptions, FlowPeer, FlowStatus, Pairing, Side, Transfer,
} from '../types';
import { S_, type Translator } from './catalog';
import type { SimDevice } from './devices';

/** As the service decides: only KDE Plasma (Wayland) can drag across the edge. The mock
 *  takes the platform from the URL (?platform=…, ?desktop=gnome) like the rest. */
function dragHere(): boolean {
  const q = new URLSearchParams(typeof location === 'undefined' ? '' : location.search);
  const platform = q.get('platform') ?? 'linux';
  return platform === 'linux' && q.get('desktop') !== 'gnome';
}

const SIDES: Side[] = ['left', 'right', 'top', 'bottom'];
const PAIRING_SECONDS = 120;
const PORT = 27461;

export type FlowScenario = 'on' | 'off' | 'new' | 'unavailable' | 'pairing' | 'incoming' | 'transfer' | 'offline' | 'error';

interface StoredPeer extends FlowPeer {}

/** A notice kept as msgid + arguments, rendered in the language of whoever reads it. */
interface Notice { seq: number; msgid: string; args: Record<string, string | number>; kind: string }

interface PairingState extends Pairing {
  expires: number;
  peerAddress: string;
}

function hex(n: number): string {
  let s = '';
  for (let i = 0; i < n; i++) s += Math.floor(Math.random() * 16).toString(16);
  return s;
}

function fingerprint(): string {
  return hex(16).toUpperCase().replace(/(.{4})(?=.)/g, '$1 ');
}

export class FlowMock {
  enabled = true;
  available = true;
  backendState: FlowStatus['backend_state'] = 'ok';
  error = '';
  id = hex(16);
  fp = fingerprint();
  options: FlowOptions = {
    require_ctrl: false, clipboard: true, files: true, drag_files: true, keyboard: true, cooldown_ms: 800,
    corner_px: 48, max_transfer_mb: 2048, max_image_mb: 50,
  };
  peers: StoredPeer[] = [];
  discovered: Discovered[] = [
    { id: 'c0ffee00d15c0ve2', name: 'office-desktop', address: '192.168.1.42:27461', compatible: true, paired: false },
    { id: '0ld0ld0ld0ld0ld1', name: 'old-laptop', address: '192.168.1.57:27461', compatible: false, paired: false },
  ];
  pairing: PairingState | null = null;
  channels = new Map<string, number | null>();   // `${device}/${peer}` → channel
  notices: Notice[] = [];
  seq = 0;
  transfers: Transfer[] = [];
  private timers = new Set<ReturnType<typeof setTimeout>>();

  constructor(
    public name: string,
    private devices: () => SimDevice[],
    private emit: (topic: string, data?: unknown) => void,
    scenario: FlowScenario,
  ) {
    const studio: StoredPeer = {
      id: '9e1f2a3b4c5d6e7f', name: 'studio-pc', online: true, state: 'online', side: 'right',
      fingerprint: '31E2 B687 56EB 3E1B', address: '192.168.1.30:27461', address_source: 'learned',
      manual_address: '', learned_address: '192.168.1.30:27461', offline_for: 0, last_error: '', files: true,
    };
    if (scenario === 'off') {
      this.enabled = false;
      this.backendState = 'off';
    } else if (scenario === 'unavailable') {
      this.available = false;
      this.backendState = 'unavailable';
    } else if (scenario === 'error') {
      this.backendState = 'error';
    }
    if (!['new', 'off', 'pairing', 'incoming', 'unavailable'].includes(scenario)) this.peers.push(studio);
    if (scenario === 'offline') {
      Object.assign(studio, { online: false, state: 'offline', offline_for: 40, last_error: 'timed out',
        address: '10.0.4.12:27461', address_source: 'manual', manual_address: '10.0.4.12:27461' });
    }
    if (scenario === 'pairing') this.pairStart();
    if (scenario === 'incoming') {
      this.pairStart();
      this.later(300, () => this.incoming());
    }
    if (scenario === 'transfer') this.later(600, () => this.startTransfer('receive'));
  }

  stop(): void {
    for (const t of this.timers) clearTimeout(t);
    this.timers.clear();
  }

  private later(ms: number, fn: () => void): void {
    const t = setTimeout(() => {
      this.timers.delete(t);
      fn();
    }, ms);
    this.timers.add(t);
  }

  private notice(msgid: string, args: Record<string, string | number>, kind: string): void {
    this.seq += 1;
    this.notices.push({ seq: this.seq, msgid, args, kind });
    if (this.notices.length > 40) this.notices.shift();
    this.emit('flow', { seq: this.seq });
  }

  private get running(): boolean {
    return this.enabled && this.available;
  }

  // ---------------------------------------------------------------- snapshots

  pairingStatus(tr: Translator): Pairing | null {
    const p = this.pairing;
    if (!p) return null;
    const left = Math.max(0, Math.round((p.expires - Date.now()) / 1000));
    if (left === 0 && (p.state === 'waiting' || p.state === 'failed')) {
      p.state = 'expired';
      p.reason = 'expired';
    }
    let message = '';
    const name = p.peer?.name || tr.tr(S_('The other computer'));
    if (p.reason === 'rejected' && p.local === false) message = tr.tr(S_("You said the numbers don't match. Nothing was saved"));
    else if (p.reason === 'rejected') message = tr.tr(S_('On {name}, the numbers were marked as not matching'), { name });
    else if (p.reason === 'expired') message = tr.tr(S_('Pairing has expired'));
    else if (p.reason === 'not_pairing') message = tr.tr(S_('{name} isn\'t ready to pair: open “Pair a computer” there too'), { name });
    else if (p.reason) message = tr.tr(S_("Pairing didn't complete"));
    return {
      state: p.state, role: p.role, expires_in: left, peer: p.peer,
      sas: p.state === 'confirm' ? p.sas : null, local: p.local, remote: p.remote,
      reason: p.reason, message, attempts_left: p.attempts_left,
    };
  }

  status(tr: Translator): FlowStatus {
    const running = this.running;
    return {
      version: 1, enabled: this.enabled, running, demo: true, error: this.error,
      backend: this.available ? 'fake' : 'none',
      backend_state: running ? this.backendState : this.available ? 'off' : 'unavailable',
      backend_message: !this.available
        ? tr.tr(S_('Flow is not available on this desktop: no graphical session found'))
        : this.backendState === 'error' ? 'KWin script stopped answering' : '',
      available: this.available, ctrl_available: true, can_place: true,
      drag_available: dragHere(),
      clipboard_backend: running ? 'fake' : 'none', files_backend: running ? 'fake' : 'none',
      transfers: this.transfers.map((t) => ({ ...t })),
      staging: '/home/user/.cache/glisa/flow/received',
      id: this.id, name: this.name, fingerprint: this.fp,
      ports: { udp: PORT, tcp: PORT, pairing: PORT + 1 }, firewall: { ufw: false },
      options: { ...this.options },
      peers: this.peers.map((p) => ({ ...p, online: running && p.online, state: !running ? 'offline' : p.state, files: running && p.files })),
      discovered: running ? this.discovered.filter((d) => !this.peers.some((p) => p.id === d.id)) : [],
      pairing: this.pairingStatus(tr),
      devices: this.devices().map((d) => {
        const snap = d.snapshot(tr);
        const channels: Record<string, { channel: number | null; suggested: number | null }> = {};
        for (const p of this.peers) {
          const suggested = snap.hosts.entries.findIndex((e) => e.name && e.name.toLowerCase() === p.name.toLowerCase());
          channels[p.id] = { channel: this.channels.get(`${d.id}/${p.id}`) ?? null, suggested: suggested >= 0 ? suggested : null };
        }
        return { id: d.id, name: d.displayName, kind: d.profile.kind, online: d.online, hosts: snap.hosts, channels };
      }),
      screens: [[0, 0, 1920, 1080]], last_switch: null, last_arrival: null,
      events_seq: this.seq, clipboard_seq: 0,
    };
  }

  // ---------------------------------------------------------------- pairing

  pairStart(): Pairing | null {
    this.pairing = {
      state: 'waiting', role: null, expires_in: PAIRING_SECONDS, peer: null, sas: null, local: null,
      remote: null, reason: '', message: '', attempts_left: 3, expires: Date.now() + PAIRING_SECONDS * 1000,
      peerAddress: '',
    };
    this.emit('flow');
    return null;
  }

  /** The other computer starts pairing with us (we are the responder). */
  incoming(): void {
    const p = this.pairing;
    if (!p || p.state !== 'waiting') return;
    const d = this.discovered[0];
    Object.assign(p, { state: 'confirm', role: 'responder', peer: { id: d.id, name: d.name },
      sas: String(Math.floor(100000 + Math.random() * 899999)), peerAddress: d.address, attempts_left: 2 });
    this.emit('flow');
    // The other side compares too, and confirms a moment later.
    this.later(2500, () => {
      if (this.pairing === p && p.state === 'confirm') {
        p.remote = true;
        this.emit('flow');
        this.finishIfBoth();
      }
    });
  }

  pairRequest(peer: string | null, address: string | null, tr: Translator): void {
    if (!this.running) throw new Error(tr.tr(S_('Turn on Flow to pair computers')));
    if (this.pairing && ['connecting', 'confirm'].includes(this.pairing.state)) {
      throw new Error(tr.tr(S_('Pairing is already in progress')));
    }
    let target: Discovered | undefined;
    if (peer) {
      target = this.discovered.find((d) => d.id === peer);
      if (!target) throw new Error(tr.tr(S_("That computer can't be found on the network")));
      if (!target.compatible) {
        throw new Error(tr.tr(S_('{name} uses a different version of Flow: update {app} on both computers'), { name: target.name, app: 'Glisa' }));
      }
    } else if (address) {
      target = { id: hex(16), name: address.split(':')[0], address, compatible: true, paired: false };
    } else {
      throw new Error(tr.tr(S_('Say which computer to pair with')));
    }
    const p: PairingState = {
      state: 'connecting', role: 'initiator', expires_in: PAIRING_SECONDS, peer: { id: '', name: target.name },
      sas: null, local: null, remote: null, reason: '', message: '', attempts_left: 2,
      expires: (this.pairing?.expires ?? Date.now() + PAIRING_SECONDS * 1000), peerAddress: target.address,
    };
    this.pairing = p;
    this.emit('flow');
    this.later(900, () => {
      if (this.pairing !== p) return;
      if (address && address.startsWith('10.99.')) {
        Object.assign(p, { state: 'failed', reason: 'not_pairing' });
        this.emit('flow');
        return;
      }
      Object.assign(p, { state: 'confirm', peer: { id: target!.id, name: target!.name },
        sas: String(Math.floor(100000 + Math.random() * 899999)) });
      this.emit('flow');
      this.later(3000, () => {
        if (this.pairing === p && p.state === 'confirm') {
          p.remote = true;
          this.emit('flow');
          this.finishIfBoth();
        }
      });
    });
  }

  pairConfirm(accept: boolean): Pairing | null {
    const p = this.pairing;
    if (!p || p.state !== 'confirm') return null;
    p.local = accept;
    if (!accept) {
      Object.assign(p, { state: 'failed', reason: 'rejected' });
    }
    this.emit('flow');
    this.finishIfBoth();
    return null;
  }

  private finishIfBoth(): void {
    const p = this.pairing;
    if (!p || p.state !== 'confirm' || !p.local || !p.remote || !p.peer) return;
    p.state = 'done';
    const found = this.discovered.find((d) => d.id === p.peer!.id);
    this.peers.push({
      id: p.peer.id, name: p.peer.name, online: true, state: 'online', side: null,
      fingerprint: fingerprint().slice(0, 19), address: p.peerAddress, address_source: found ? 'learned' : 'manual',
      manual_address: found ? '' : p.peerAddress, learned_address: p.peerAddress, offline_for: 0, last_error: '',
      files: true,
    });
    if (found) found.paired = true;
    this.notice(S_('Paired with {name}'), { name: p.peer.name }, 'ok');
  }

  pairCancel(): void {
    this.pairing = null;
    this.emit('flow');
  }

  // ---------------------------------------------------------------- the rest

  setEnabled(on: boolean, tr: Translator): void {
    if (on && !this.available) throw new Error(tr.tr(S_('Flow is not available on this desktop: no graphical session found')));
    this.enabled = on;
    if (!on) this.pairing = null;
    this.emit('flow');
  }

  removePeer(pid: string, tr: Translator): boolean {
    const peer = this.peers.find((p) => p.id === pid);
    if (!peer) throw new Error(tr.tr(S_("That computer isn't paired")));
    this.peers = this.peers.filter((p) => p.id !== pid);
    const d = this.discovered.find((x) => x.id === pid);
    if (d) d.paired = false;
    this.emit('flow');
    return true;
  }

  setPeerAddress(pid: string, address: string, tr: Translator): FlowPeer {
    const peer = this.peers.find((p) => p.id === pid);
    if (!peer) throw new Error(tr.tr(S_("That computer isn't paired")));
    if (address) {
      peer.manual_address = address;
      peer.address = address;
      peer.address_source = 'manual';
    } else {
      peer.manual_address = '';
      peer.address = peer.learned_address;
      peer.address_source = peer.learned_address ? 'learned' : '';
    }
    this.emit('flow');
    return { ...peer };
  }

  setLayout(pid: string, side: Side | null, tr: Translator): Record<string, Side> {
    if (side !== null && !SIDES.includes(side)) throw new Error(tr.tr(S_('Invalid position')));
    if (!this.peers.some((p) => p.id === pid)) throw new Error(tr.tr(S_("That computer isn't paired")));
    for (const p of this.peers) {
      if (p.id === pid) p.side = side;
      else if (side && p.side === side) p.side = null;     // one computer per side
    }
    this.emit('flow');
    return Object.fromEntries(this.peers.filter((p) => p.side).map((p) => [p.id, p.side!]));
  }

  setOptions(params: Record<string, unknown>, tr: Translator): FlowOptions {
    const bools = ['require_ctrl', 'clipboard', 'files', 'drag_files', 'keyboard'] as const;
    const ranges: Record<string, [number, number]> = {
      cooldown_ms: [200, 5000], corner_px: [0, 400], max_transfer_mb: [1, 102400], max_image_mb: [1, 200],
    };
    for (const [k, v] of Object.entries(params)) {
      if ((bools as readonly string[]).includes(k)) {
        if (typeof v !== 'boolean') throw new Error(tr.tr(S_('{option} must be true or false'), { option: k }));
        (this.options as unknown as Record<string, unknown>)[k] = v;
      } else if (k in ranges) {
        if (typeof v !== 'number' || !Number.isInteger(v)) throw new Error(tr.tr(S_('{option} must be a whole number'), { option: k }));
        const [lo, hi] = ranges[k];
        (this.options as unknown as Record<string, unknown>)[k] = Math.max(lo, Math.min(hi, Math.round(v)));
      } else {
        throw new Error(tr.tr(S_('Unknown option: {option}'), { option: k }));
      }
    }
    this.emit('flow');
    return { ...this.options };
  }

  setChannel(device: string, pid: string, channel: number | null, tr: Translator): boolean {
    const dev = this.devices().find((d) => d.id === device);
    if (!dev) throw new Error(tr.tr(S_('Device not found')));
    if (channel !== null && (!Number.isInteger(channel) || channel < 0 || channel >= 3)) {
      throw new Error(tr.tr(S_('Invalid channel')));
    }
    this.channels.set(`${device}/${pid}`, channel);
    this.emit('flow');
    return true;
  }

  events(since: number, tr: Translator) {
    return {
      seq: this.seq, enabled: this.enabled, clipboard_seq: 0,
      clipboard_backend: this.running ? 'fake' : 'none', files_backend: this.running ? 'fake' : 'none',
      transfers: this.transfers.map((t) => ({ ...t })),
      events: this.notices.filter((n) => n.seq > since)
        .map((n): FlowNotice => ({ seq: n.seq, text: tr.tr(n.msgid, n.args), kind: n.kind })),
    };
  }

  resetIdentity(): void {
    this.fp = fingerprint();
    this.id = hex(16);
    this.peers = [];
    for (const d of this.discovered) d.paired = false;
    this.emit('flow');
  }

  cancelTransfer(id: string): boolean {
    const t = this.transfers.find((x) => x.id === id);
    if (!t || !t.active) return false;
    Object.assign(t, { state: 'cancelled', active: false, ago: 0 });
    this.emit('transfer', { ...t });
    return true;
  }

  /** A copy of files arriving from (or going to) the first paired computer. */
  startTransfer(direction: 'send' | 'receive', kind: 'files' | 'image' = 'files'): void {
    const peer = this.peers[0];
    if (!peer || !this.running) return;
    const t: Transfer = {
      id: hex(8), direction, peer: peer.id, name: peer.name, kind, state: 'preparing', active: true,
      items: kind === 'image' ? 1 : 3, top_folders: kind === 'image' ? 0 : 1, files: kind === 'image' ? 1 : 24,
      folders: kind === 'image' ? 0 : 2, bytes: 48_500_000, done: 0, percent: 0, failed: 0, skipped: 0,
      first: kind === 'image' ? 'image.png' : 'Holiday photos', folder: '', placed: '', cut: false, ago: 0,
    };
    this.transfers.push(t);
    if (this.transfers.length > 8) this.transfers.shift();
    this.emit('transfer', { ...t });
    const step = () => {
      if (!t.active) return;
      t.state = direction === 'send' ? 'sending' : 'receiving';
      t.done = Math.min(t.bytes, t.done + 4_400_000);
      t.percent = Math.min(100, Math.floor((t.done * 100) / t.bytes));
      if (t.done >= t.bytes) {
        Object.assign(t, { state: 'done', active: false, percent: 100,
          folder: direction === 'receive' ? '/home/user/.cache/glisa/flow/received/1' : '',
          placed: direction === 'receive' ? 'clipboard' : '' });
        this.emit('transfer', { ...t });
        return;
      }
      this.emit('transfer', { ...t });
      this.later(450, step);
    };
    this.later(500, step);
  }

  simulate(params: Record<string, unknown>, tr: Translator): boolean {
    const op = params.op;
    if (op === 'transfer') this.startTransfer(params.direction === 'send' ? 'send' : 'receive', params.kind === 'image' ? 'image' : 'files');
    else if (op === 'edge' || op === 'away' || op === 'ctrl') return true;
    else throw new Error(tr.tr(S_('Unknown operation')));
    return true;
  }
}
