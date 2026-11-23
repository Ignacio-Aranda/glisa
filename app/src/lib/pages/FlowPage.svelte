<script lang="ts">
  import Panel from '../components/Panel.svelte';
  import SettingRow from '../components/SettingRow.svelte';
  import Toggle from '../components/Toggle.svelte';
  import Button from '../components/Button.svelte';
  import IconButton from '../components/IconButton.svelte';
  import Banner from '../components/Banner.svelte';
  import Divider from '../components/Divider.svelte';
  import StatusPill from '../components/StatusPill.svelte';
  import Select from '../components/Select.svelte';
  import Icon from '../components/Icon.svelte';
  import Spinner from '../components/Spinner.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import LayoutEditor from './flow/LayoutEditor.svelte';
  import PairDialog from './flow/PairDialog.svelte';
  import AddressDialog from './flow/AddressDialog.svelte';
  import { api, errorText, mockParam } from '../backend';
  import { app, navigate, platform, refreshFlow, toast, watchFlow } from '../state/app.svelte';
  import { confirm } from '../state/confirm.svelte';
  import { kindNoun, sizeText } from '../format';
  import { sentence, t, tc } from '../i18n/index.svelte';
  import type { FlowDevice, FlowPeer, FlowStatus, Side } from '../types';

  const SIZE_CHOICES = [100, 500, 1024, 2048, 5120, 10240, 20480];
  const STUCK_OFFLINE_S = 15;

  $effect(() => watchFlow());

  const st = $derived<FlowStatus | null>(app.flow);
  const running = $derived(!!st?.running);
  const peers = $derived(st?.peers ?? []);

  let pairing = $state(mockParam('dialog') === 'pair');
  let addressFor = $state<FlowPeer | null>(null);
  let toggling = $state(false);

  async function call(method: string, params: Record<string, unknown> = {}, ok = '') {
    try {
      await api(method, params);
      if (ok) toast(ok);
    } catch (e) {
      toast(sentence(errorText(e)), 'error');
    }
    await refreshFlow();
  }

  async function setEnabled(on: boolean) {
    toggling = true;
    try {
      await api('flow_enable', { enabled: on });
      toast(on ? t('Flow turned on') : t('Flow turned off'));
    } catch (e) {
      toast(sentence(errorText(e)), 'error');
    } finally {
      toggling = false;
      await refreshFlow();
    }
  }

  const header = $derived.by((): { text: string; tone: 'ok' | 'warn' | 'muted' | 'danger' } => {
    if (!st) return { text: t('No service'), tone: 'danger' };
    if (st.running && (st.backend_state === 'error' || st.backend_state === 'unavailable')) return { text: t('Needs attention'), tone: 'warn' };
    return st.running ? { text: tc('flow state', 'On'), tone: 'ok' } : { text: tc('flow state', 'Off'), tone: 'muted' };
  });

  interface BannerSpec { kind: 'info' | 'warn' | 'error' | 'quiet'; title: string; text: string }
  const banners = $derived.by((): BannerSpec[] => {
    if (!st) return [];
    const out: BannerSpec[] = [];
    if (!st.available) {
      const title = t('Flow is not available on this desktop');
      let why = st.backend_message || '';
      if (why.startsWith(title)) why = sentence(why.slice(title.length).replace(/^[:\s]+/, ''));
      out.push({ kind: 'warn', title, text: why || t("{app} can't tell where the pointer is on this desktop") });
    } else if (st.running && st.backend_state === 'error') {
      out.push({ kind: 'error', title: t("The screen edges aren't being detected"), text: st.backend_message || '' });
    }
    if (st.error) out.push({ kind: 'error', title: t("Flow couldn't fully start"), text: st.error });
    if (st.running && st.options?.require_ctrl && !st.ctrl_available) {
      out.push({ kind: 'warn', title: t("This desktop doesn't report whether Ctrl is held"),
        text: t('Turn off “Switch only while holding Ctrl” or Flow won\'t switch computers') });
    }
    if (st.running && !st.can_place) {
      out.push({ kind: 'info', title: t("The pointer won't be placed when it arrives"),
        text: t('No access to the virtual device: check the permissions in App settings') });
    }
    if (st.firewall?.ufw) {
      const p = st.ports ?? { udp: 27461, tcp: 27461, pairing: 27462 };
      out.push({ kind: 'info', title: t('Your firewall is on'),
        text: t('Allow UDP port {udp} and TCP ports {tcp} and {pairing} so the computers can find each other (for example, with sudo ufw allow {udp}/udp and sudo ufw allow {tcp}:{pairing}/tcp)', { udp: p.udp, tcp: p.tcp, pairing: p.pairing }) });
    }
    if (st.demo) out.push({ kind: 'quiet', title: t('Demo mode'), text: t("Flow doesn't connect to the network or move the pointer") });
    return out;
  });

  /** Dragging files across the edge works on KDE Plasma (Wayland) only. */
  const dragHere = $derived(app.flow?.drag_available !== false);
  const dragSubtitle = $derived(
    dragHere
      ? t('Keep holding the button and the drag goes on there, or paste them with Ctrl+V')
      : platform() === 'linux'
        ? t('Needs KDE Plasma on Wayland. Files dragged to you from another computer arrive like copied ones: paste them with Ctrl+V')
        : t('Available on KDE Plasma (Linux). Files dragged to you from another computer arrive like copied ones: paste them with Ctrl+V'),
  );

  const statusLine = $derived.by((): { text: string; tone: string } => {
    if (!st) return { text: '', tone: 'muted' };
    if (!st.available) return { text: t('Not available on this desktop'), tone: 'danger' };
    if (!st.running) return { text: t('Flow is off. Turn it on at the top right'), tone: 'muted' };
    if (st.backend_state === 'error' || st.backend_state === 'unavailable') return { text: t("The screen edges aren't being detected"), tone: 'danger' };
    if (st.backend_state === 'starting') return { text: t('Getting edge detection ready…'), tone: 'warn' };
    if (!peers.length) return { text: t('Ready. Now pair the other computer'), tone: 'info' };
    if (!peers.some((p) => p.side)) return { text: t('Place the other computer in the layout'), tone: 'info' };
    if (!peers.some((p) => p.online)) return { text: t('Waiting for the other computer to come online'), tone: 'warn' };
    return { text: t('Ready: move the pointer to the edge where the other computer is'), tone: 'ok' };
  });

  function sideText(side: Side): string {
    return { left: t('on the left'), right: t('on the right'), top: t('above'), bottom: t('below') }[side];
  }

  function peerSubtitle(p: FlowPeer): string {
    if (p.state === 'identity') return t("It isn't the computer you paired (or it was reinstalled). Remove it and pair it again");
    let placement = p.side ? t('Placed {where}', { where: sideText(p.side) }) : t('Not placed in the layout yet');
    if (p.state === 'offline' && (p.offline_for ?? 0) >= STUCK_OFFLINE_S) {
      placement = t("{placement}. Can't reach it: computers on different networks may need its address set manually", { placement });
    }
    return placement;
  }

  function addressText(p: FlowPeer): string {
    if (!p.address) return t('Address not known yet');
    return p.address_source === 'manual' ? t('{address} · set manually', { address: p.address }) : t('{address} · found automatically', { address: p.address });
  }

  async function removePeer(p: FlowPeer) {
    const ok = await confirm({
      title: t('Remove {name}?', { name: p.name }),
      text: t("You won't be able to move the mouse to {name} any more, and its fingerprint will be deleted. To use it again, you'll both have to pair again", { name: p.name }),
      confirm: t('Remove'), danger: true, icon: 'trash',
    });
    if (ok) await call('flow_remove_peer', { peer: p.id }, t('{name} removed', { name: p.name }));
  }

  async function saveAddress(p: FlowPeer, text: string) {
    addressFor = null;
    await call('flow_set_peer_address', { peer: p.id, address: text },
      text ? t('{name} will be looked for at {address}', { name: p.name, address: text }) : t('The address of {name} will be found automatically', { name: p.name }));
  }

  async function resetIdentity() {
    const ok = await confirm({
      title: t('Create a new identity for this computer?'),
      text: t("All paired computers will stop recognizing it and you'll have to pair them again. Do it if you think someone has copied this computer's settings"),
      confirm: t('Create new identity'), danger: true, icon: 'fingerprint-pattern',
    });
    if (ok) await call('flow_reset_identity', {}, t('New identity created'));
  }

  // ---- channels
  function channelOptions(dev: FlowDevice, peerId: string): { value: number | null; label: string }[] {
    const hosts = dev.hosts ?? { count: 0, current: 0, entries: [] };
    const entries = new Map((hosts.entries ?? []).map((e) => [e.index, e]));
    const nameOf = (i: number) => {
      const e = entries.get(i);
      if (e?.name) return e.name;
      return e?.paired === false ? tc('channel', 'free') : tc('channel', 'unnamed');
    };
    const suggested = dev.channels?.[peerId]?.suggested;
    const auto = suggested != null
      ? t('Automatic: channel {channel} · {name}', { channel: suggested + 1, name: nameOf(suggested) })
      : t('Choose the channel');
    const out: { value: number | null; label: string }[] = [{ value: null, label: auto }];
    for (let i = 0; i < (hosts.count ?? 0); i++) {
      if (i !== hosts.current) out.push({ value: i, label: t('Channel {channel} · {name}', { channel: i + 1, name: nameOf(i) }) });
    }
    return out;
  }

  const channelRows = $derived((st?.devices ?? []).flatMap((d) => peers.map((p) => ({ dev: d, peer: p }))));
  const channelsEmpty = $derived(!peers.length ? t('Pair a computer to choose which channel your mouse uses for it')
    : !(st?.devices ?? []).length ? t('Connect your mouse to this computer to see its channels') : '');

  // ---- options
  const o = $derived(st?.options);
  const sizeOptions = $derived.by(() => {
    const mb = o?.max_transfer_mb ?? 2048;
    const list = SIZE_CHOICES.includes(mb) ? SIZE_CHOICES : [...SIZE_CHOICES, mb].sort((a, b) => a - b);
    return list.map((v) => ({ value: v, label: sizeText(v) }));
  });
  const filesNote = $derived.by(() => {
    if (!st) return '';
    let note = t('Files, folders and images you copy (screenshots too) go to the other computer when you switch. Paste them there with Ctrl+V');
    if (st.running && st.files_backend === 'gui') {
      note += '. ' + t("This desktop doesn't let background apps use the clipboard, so it only works while this window is open");
    }
    const old = peers.filter((p) => p.online && !p.files).map((p) => p.name);
    if (old.length) note += '. ' + t('Update {app} on {name} to send files there: until then only text goes', { name: old.join(', ') });
    return note;
  });
  const clipboardNote = $derived(st?.clipboard_backend === 'gui' && st.running
    ? t('Text you copy goes to the other computer when you switch (up to 1 MB). Without the Plasma clipboard, it only works while this window is open')
    : t('Text you copy goes to the other computer when you switch (up to 1 MB)'));

  function detection(backend: string): string {
    switch (backend) {
      case 'kwin': return 'Plasma (KWin)';
      case 'x11': return 'X11';
      case 'windows': return 'Windows';
      case 'macos': return 'macOS';
      case 'fake': return tc('edge detection', 'Simulated');
      case 'none': case '': return tc('edge detection', 'Not available');
      default: return backend;
    }
  }
</script>

<div class="page">
  <header class="head">
    <IconButton icon="chevron-left" label={tc('navigation', 'Back')} tooltip={t('Back (Esc)')} onclick={() => navigate({ page: 'devices' })} />
    <h1>Flow</h1>
    <span class="spacer"></span>
    <StatusPill text={header.text} tone={header.tone} />
    {#if toggling}<Spinner size={16} />{/if}
    <Toggle checked={running} label={t('Turn on Flow')} disabled={!st || toggling || (!running && !st.available)} onchange={setEnabled} />
  </header>

  {#if !st}
    {#if app.flowError}
      <Banner kind="error" title={t('No connection to the service')} text={app.flowError} action={t('Retry')} onaction={refreshFlow} />
    {:else}
      <div class="loading"><Spinner size={22} /></div>
    {/if}
  {:else}
    {#each banners as b (b.title)}
      <Banner kind={b.kind} title={b.title} text={b.text} />
    {/each}

    <Panel title={t('Use the same mouse on two computers')} subtitle={t("Move the pointer to the edge of the screen and your mouse moves to the other computer, with the keyboard if you like. Text you've copied goes with it")} icon="waypoints">
      <div class="needs">
        <span class="overline">{t('You need')}</span>
        <div class="need"><Icon name="mouse" /><span>{t('Your mouse paired with both computers, each on its own channel')}</span></div>
        <div class="need"><Icon name="laptop" /><span>{t('{app} open on both')}</span></div>
        <div class="need"><Icon name="wifi" /><span>{t('Both on the same network, or on networks that can reach each other')}</span></div>
      </div>
      <Divider />
      <div class="status-line" role="status">
        <span class="dot tone-{statusLine.tone}" aria-hidden="true"></span>
        <span>{statusLine.text}</span>
      </div>
    </Panel>

    <Panel title={t('Layout')} subtitle={t('Drag each computer to the side where it sits on your desk. The mouse switches when it reaches that edge of the screen')} icon="layout-grid">
      <LayoutEditor me={st.name} {peers} disabled={false} onside={(peer, side) => call('flow_set_layout', { peer, side })} />
      {#if !peers.length}
        <p class="center small muted">{t('Once you pair another computer, it will appear here so you can place it')}</p>
      {/if}
    </Panel>

    <Panel title={t('Paired computers')} subtitle={t('Only computers you pair here can receive your mouse')} icon="laptop">
      {#snippet trailing()}
        <Button variant="primary" icon="plus" disabled={!running} title={running ? undefined : t('Turn on Flow to pair computers')} onclick={() => (pairing = true)}>{t('Pair a computer')}</Button>
      {/snippet}
      {#if peers.length}
        <ul class="peers">
          {#each peers as p, i (p.id)}
            {#if i}<li aria-hidden="true"><Divider /></li>{/if}
            <li class="peer">
              <span class="peer-icon"><Icon name="laptop" size={20} /></span>
              <div class="peer-text">
                <strong class="ellipsis">{p.name}</strong>
                <span class="small muted">{peerSubtitle(p)}</span>
                <span class="addr small">
                  <span class="mono subtle ellipsis">{addressText(p)}</span>
                  <Button variant="link" size="sm" label={t('Set the address of {name}', { name: p.name })} onclick={() => (addressFor = p)}>{t('Set address…')}</Button>
                </span>
              </div>
              {#if p.state === 'identity'}
                <StatusPill text={t('Identity changed')} tone="danger" />
              {:else}
                <StatusPill text={p.state === 'online' ? tc('computer', 'Online') : tc('computer', 'Offline')} tone={p.state === 'online' ? 'ok' : 'muted'} />
              {/if}
              <IconButton icon="trash" tone="danger" label={t('Remove {name}', { name: p.name })} onclick={() => removePeer(p)} />
            </li>
          {/each}
        </ul>
      {:else}
        <EmptyState compact icon="laptop" title={t('No other computers yet')} text={t('You haven\'t paired a computer yet. Click “Pair a computer” here and on the other computer')} />
      {/if}
    </Panel>

    <Panel title={t('Channels')} subtitle={t('Which Easy-Switch channel each device uses for the other computer. {app} suggests one from its name')} icon="radio-receiver">
      {#if channelsEmpty}
        <p class="muted">{channelsEmpty}</p>
      {:else}
        {#each channelRows as { dev, peer }, i (dev.id + peer.id)}
          {#if i}<Divider />{/if}
          <SettingRow
            icon={dev.kind === 'keyboard' ? 'keyboard' : 'mouse'}
            title={t('{device} on {name}', { device: dev.name, name: peer.name })}
            subtitle={dev.online
              ? t('Your {device} is using channel {channel} for this computer', { device: kindNoun(dev.kind), channel: (dev.hosts?.current ?? 0) + 1 })
              : t("Your {device} isn't connected to this computer right now", { device: kindNoun(dev.kind) })}
          >
            {#snippet control()}
              <Select
                label={t('Channel of {device} for {name}', { device: dev.name, name: peer.name })}
                options={channelOptions(dev, peer.id)}
                value={dev.channels?.[peer.id]?.channel ?? null}
                width="250px"
                onchange={(channel) => call('flow_set_channel', { device: dev.id, peer: peer.id, channel })}
              />
            {/snippet}
          </SettingRow>
        {/each}
      {/if}
    </Panel>

    {#if o}
      <Panel title={t('Options')} icon="settings">
        <SettingRow title={t('Switch only while holding Ctrl')} subtitle={st.ctrl_available ? t('Hold Ctrl when you reach the edge, so you never switch by accident') : t("Not available on this desktop: it doesn't report whether Ctrl is held")}>
          {#snippet control()}
            <Toggle checked={o.require_ctrl} label={t('Switch only while holding Ctrl')} disabled={!st.ctrl_available && !o.require_ctrl} onchange={(v) => call('flow_set_options', { require_ctrl: v })} />
          {/snippet}
        </SettingRow>
        <Divider />
        <SettingRow title={t('Bring the clipboard along')} subtitle={clipboardNote}>
          {#snippet control()}
            <Toggle checked={o.clipboard} label={t('Bring the clipboard along')} onchange={(v) => call('flow_set_options', { clipboard: v })} />
          {/snippet}
        </SettingRow>
        <Divider />
        <SettingRow title={t('Bring copied files and images along')} subtitle={filesNote}>
          {#snippet control()}
            <Toggle checked={o.files} label={t('Bring copied files and images along')} onchange={(v) => call('flow_set_options', { files: v })} />
          {/snippet}
        </SettingRow>
        <SettingRow title={t('Size limit')} subtitle={t("Bigger copies aren't sent or received. Images: up to {limit}", { limit: sizeText(o.max_image_mb || 50) })}>
          {#snippet control()}
            <Select label={t('Size limit for copied files')} options={sizeOptions} value={o.max_transfer_mb || 2048} disabled={!o.files} width="130px"
              onchange={(v) => call('flow_set_options', { max_transfer_mb: v })} />
          {/snippet}
        </SettingRow>
        <SettingRow title={t('Drag files across the edge')} subtitle={dragSubtitle} disabled={!dragHere}>
          {#snippet control()}
            <Toggle checked={dragHere && o.drag_files} label={t('Drag files across the edge')} disabled={!o.files || !dragHere} onchange={(v) => call('flow_set_options', { drag_files: v })} />
          {/snippet}
        </SettingRow>
        <Divider />
        <SettingRow title={t('Keyboard switches too')} subtitle={t('Your Logitech keyboards with Easy-Switch follow the mouse')}>
          {#snippet control()}
            <Toggle checked={o.keyboard} label={t('Keyboard switches too')} onchange={(v) => call('flow_set_options', { keyboard: v })} />
          {/snippet}
        </SettingRow>
      </Panel>
    {/if}

    <Panel title={t('Security and network')} subtitle={t('Paired computers recognize each other by their fingerprint, and everything travels encrypted')} icon="shield-check">
      <SettingRow title={t("This computer's fingerprint")} subtitle={t('Shown on the other computer next to its name')}>
        {#snippet control()}
          {#if st.fingerprint}<span class="mono selectable fp">{st.fingerprint}</span>{:else}<span class="muted small">{t('Created when you turn on Flow')}</span>{/if}
        {/snippet}
      </SettingRow>
      <Divider />
      <SettingRow title={t('Network')} subtitle={t('Uses UDP port {udp} to find computers, and TCP ports {tcp} (paired computers) and {pairing} (only while pairing)', { udp: st.ports?.udp, tcp: st.ports?.tcp, pairing: st.ports?.pairing })} />
      <Divider />
      <SettingRow title={t('Edge detection')}>
        {#snippet control()}<span class="muted">{detection(st.backend)}</span>{/snippet}
      </SettingRow>
      <div>
        <Button variant="danger" icon="refresh-cw" disabled={!st.fingerprint} onclick={resetIdentity}>{t('Create a new identity')}</Button>
      </div>
    </Panel>
  {/if}
</div>

<PairDialog open={pairing} onclose={() => { pairing = false; void refreshFlow(); }} />
<AddressDialog peer={addressFor} onclose={() => (addressFor = null)} onsave={(text) => addressFor && saveAddress(addressFor, text)} />

<style>
  .page {
    max-width: 960px;
    padding: var(--space-6) var(--space-7) var(--space-8);
    display: flex;
    flex-direction: column;
    gap: var(--space-5);
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    margin-left: calc(-1 * var(--space-2));
  }
  .spacer {
    flex: 1;
  }
  .loading {
    display: grid;
    place-items: center;
    padding: var(--space-8);
  }
  .needs {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .need {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }
  .need :global(.icon) {
    color: var(--text-muted);
    flex: none;
  }
  .status-line {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-weight: 540;
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: currentColor;
    flex: none;
  }
  .center {
    text-align: center;
  }
  .peers {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .peer {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }
  .peer-icon {
    display: grid;
    place-items: center;
    flex: none;
    width: 40px;
    height: 40px;
    border-radius: var(--radius-m);
    background: var(--surface-2);
    color: var(--text-muted);
  }
  .peer-text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .addr {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }
  .fp {
    letter-spacing: 0.04em;
  }
  @media (max-width: 1000px) {
    .page { padding-inline: var(--space-5); }
  }
</style>
