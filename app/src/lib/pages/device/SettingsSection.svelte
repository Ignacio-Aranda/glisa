<script lang="ts">
  import Panel from '../../components/Panel.svelte';
  import Button from '../../components/Button.svelte';
  import SettingRow from '../../components/SettingRow.svelte';
  import Divider from '../../components/Divider.svelte';
  import DeviceArt from '../../components/DeviceArt.svelte';
  import DeviceIcon from '../../components/DeviceIcon.svelte';
  import StatusPill from '../../components/StatusPill.svelte';
  import BatteryBadge from '../../components/BatteryBadge.svelte';
  import Icon from '../../components/Icon.svelte';
  import { api, errorText, mockParam } from '../../backend';
  import { app, editablePicture, pagePicture, refreshDevice, toast } from '../../state/app.svelte';
  import { confirm } from '../../state/confirm.svelte';
  import { choosePicture, pictureFromUrl, removeDevice, removePicture, rename, setUseOnPages } from '../../state/identity.svelte';
  import Toggle from '../../components/Toggle.svelte';
  import PictureLayoutEditor from './PictureLayoutEditor.svelte';
  import { batteryText, connectionName, kindName } from '../../format';
  import { describe } from '../../actions';
  import { t, tc } from '../../i18n/index.svelte';
  import type { Device } from '../../types';

  interface Props { dev: Device }
  let { dev }: Props = $props();

  let techOpen = $state(false);
  let resetting = $state(false);
  let shownId = '';
  $effect(() => {
    if (dev.id !== shownId) {
      shownId = dev.id;
      techOpen = false;
    }
  });

  const renamed = $derived(!!dev.name && dev.name !== dev.model);
  const picture = $derived(app.pictures[dev.id] ?? null);
  const layout = $derived(app.layouts[dev.id]);
  /** The picture shown for the device on its pages, if any. */
  const shown = $derived(pagePicture(dev.id));
  /** What the button-position editor works on: the user's picture, else the official photo. */
  const editable = $derived(editablePicture(dev.id));
  let editing = $state(mockParam('dialog') === 'layout');
  let linkOpen = $state(false);
  let link = $state('');
  let linkError = $state('');
  let linkBusy = $state(false);

  async function useLink(e: Event) {
    e.preventDefault();
    const url = link.trim();
    if (!url.startsWith('https://')) {
      linkError = t('Only https:// links. The picture is downloaded once and kept on this computer.');
      return;
    }
    linkBusy = true;
    linkError = await pictureFromUrl(dev, url);
    linkBusy = false;
    if (!linkError) {
      link = '';
      linkOpen = false;
    }
  }

  /** What a reset does to this device, naming only what it has. */
  function resetExplanation(d: Device): string {
    if (d.controls?.length && d.dpi) {
      return t('The buttons and the pointer speed go back to their factory settings, and everything {app} has stored for this device is deleted.');
    }
    if (d.controls?.length) return t('The buttons go back to their factory settings, and everything {app} has stored for this device is deleted.');
    if (d.dpi) return t('The pointer speed goes back to its factory setting, and everything {app} has stored for this device is deleted.');
    return t('Everything {app} has stored for this device is deleted.');
  }

  async function reset() {
    if (!dev.online) return;
    const id = dev.id;
    const name = dev.name || t('the device');
    const ok = await confirm({
      title: t('Reset {name}?', { name }), text: resetExplanation(dev) + ' ' + t("This can't be undone."),
      confirm: t('Reset'), danger: true, icon: 'rotate-ccw',
    });
    if (!ok || dev.id !== id) return;
    resetting = true;
    try {
      await api('reset', { id });
      toast(t('Device reset'));
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      resetting = false;
      await refreshDevice(id);
    }
  }

  function yesNo(v: unknown): string {
    return v ? t('yes') : t('no');
  }

  /** Plain-text report of everything known about the device. */
  export function diagnosticText(d: Device = dev): string {
    const out: string[] = [
      t('{app} {version} · device diagnostics', { version: app.info.version || app.status?.version || '?' }),
      new Date().toISOString().slice(0, 16).replace('T', ' '),
      '',
      t('Name: {value}', { value: d.name ?? '?' }),
      t('Model: {value}', { value: d.model || d.name || '?' }),
      t('Type: {name} ({kind})', { name: kindName(d.kind), kind: d.kind ?? '?' }),
      t('Identifier: {value}', { value: d.id ?? '?' }),
      t('Product ID: {value}', { value: d.product_id ?? '?' }),
      t('Connection: {name} ({connection})', { name: connectionName(d.connection), connection: d.connection ?? '?' }),
      t('Source: {value}', { value: d.source ?? '?' }),
      d.online ? t('Status: connected') : t('Status: disconnected'),
    ];
    const b = d.battery;
    out.push(b ? t('Battery: {battery} (level={level}, status={status}, charging={charging})', {
      battery: batteryText(b) || '?', level: b.level, status: b.status, charging: yesNo(b.charging),
    }) : t('Battery: no information'));
    out.push('Firmware:');
    out.push(...(d.firmware?.length ? d.firmware.map((f) => `  ${f}`) : ['  ' + t('(not available)')]));
    const features = (d.features ?? []).map(String);
    out.push('', t('HID++ features ({n}):', { n: features.length }));
    out.push(...(features.length ? features.map((f) => `  ${f}`) : ['  ' + tc('features', '(none)')]));
    const controls = d.controls ?? [];
    out.push('', t('Controls ({n}):', { n: controls.length }));
    for (const c of controls) {
      const flags = ([[t('divertable'), c.divertable], [t('reprogrammable'), c.reprogrammable], [t('gestures'), c.gestures]] as const)
        .filter(([, on]) => on).map(([n]) => n);
      const action = c.action ?? { type: 'default' };
      out.push(`  0x${c.cid.toString(16).padStart(4, '0')}  ${c.name ?? '?'}  [${flags.join(', ') || '-'}]  -> ${describe(action, app.presets)}  ${JSON.stringify(action)}`);
    }
    if (!controls.length) out.push('  ' + tc('controls', '(none)'));
    out.push('');
    const dpi = d.dpi;
    out.push(dpi ? t('DPI: current {current}, default {default}, range {min}-{max}, step {step}, levels {levels}', {
      current: dpi.current, default: dpi.default, min: dpi.min, max: dpi.max, step: dpi.step, levels: JSON.stringify(dpi.presets),
    }) : t('DPI: not available'));
    const w = d.wheel;
    out.push(w ? t('Wheel: smooth={smooth}, inverted={inverted}, can invert={can_invert}', {
      smooth: yesNo(w.hires), inverted: yesNo(w.inverted), can_invert: yesNo(w.has_invert),
    }) : t('Wheel: not available'));
    const ss = d.smartshift;
    out.push(ss ? t('SmartShift: mode={mode}, threshold={threshold}, enhanced={enhanced}', {
      mode: ss.ratchet ? t('with clicks') : t('free spin'), threshold: ss.threshold, enhanced: yesNo(ss.enhanced),
    }) : t('SmartShift: not available'));
    const h = d.hosts;
    if (h?.count) {
      out.push(t('Channels: {count}, current {current}', { count: h.count, current: typeof h.current === 'number' ? h.current + 1 : '?' }));
      for (const e of h.entries ?? []) {
        const state = e.paired === true ? tc('channel', 'paired') : e.paired === false ? tc('channel', 'free') : tc('channel', 'unknown');
        out.push(`  ${Number(e.index) + 1}  ${state}  ${e.name ?? ''}`.trimEnd());
      }
    } else {
      out.push(t('Channels: not available'));
    }
    const settings = d.settings ?? {};
    out.push('', t('Stored settings:'));
    const keys = Object.keys(settings).sort();
    out.push(...(keys.length ? keys.map((k) => `  ${k} = ${JSON.stringify(settings[k])}`) : ['  ' + tc('stored settings', '(none)')]));
    return out.join('\n') + '\n';
  }

  async function copyDiagnostics() {
    try {
      await navigator.clipboard.writeText(diagnosticText());
      toast(t('Diagnostics copied to the clipboard'));
    } catch {
      toast(t("Couldn't copy to the clipboard"), 'error');
    }
  }
</script>

<div class="page">
  <section class="summary">
    <div class="art">{#if shown}<DeviceArt picture={shown} disabled={!dev.online} label={dev.name} />{:else}<DeviceIcon kind={dev.kind} family={dev.family} size={72} dimmed={!dev.online} />{/if}</div>
    <div class="info">
      <h2 class="ellipsis">{dev.name}</h2>
      <p class="muted">{kindName(dev.kind)} · {connectionName(dev.connection)}</p>
      {#if dev.battery}<BatteryBadge battery={dev.battery} long />{/if}
    </div>
    <StatusPill text={dev.online ? t('Connected') : t('Disconnected')} tone={dev.online ? 'ok' : 'muted'} />
  </section>

  <Panel title={t('Appearance')} subtitle={t('How {app} names and shows this device')} icon="pen-line">
    <SettingRow title={t('Name')} subtitle={renamed ? t('{name} (model: {model})', { name: dev.name, model: dev.model }) : t("{name} (the model's name)", { name: dev.name })}>
      {#snippet control()}<Button onclick={() => rename(dev)}>{t('Rename…')}</Button>{/snippet}
    </SettingRow>
    <Divider />
    <div class="picture-row">
      <button type="button" class="preview" onclick={() => choosePicture(dev)} title={t('Choose a file…')} aria-label={t('Choose a file…')}>
        {#if editable}<img src={editable.src} alt="" />{:else}<Icon name="image" size={26} />{/if}
      </button>
      <div class="pic-text">
        <div class="title">{t('Picture')}</div>
        <p class="small muted">{t('A photo of your device: one you took (with the background removed, if you like) or a product photo you choose. It stays on this computer.')}</p>
        {#if !picture && editable?.official}
          <p class="small subtle">{t('Showing the official product photo (App settings). It is used on the device pages too; your own picture would replace it.')}</p>
        {/if}
        <div class="pic-actions">
          <Button size="sm" icon="folder-open" onclick={() => choosePicture(dev)}>{t('Choose a file…')}</Button>
          <Button size="sm" icon="link" pressed={linkOpen} onclick={() => { linkOpen = !linkOpen; linkError = ''; }}>{t('From a link…')}</Button>
          {#if picture}<Button size="sm" variant="danger-ghost" icon="trash" onclick={() => removePicture(dev)}>{t('Remove')}</Button>{/if}
        </div>
        {#if linkOpen}
          <form class="link" onsubmit={useLink}>
            <input class="text-input" type="url" bind:value={link} placeholder="https://…" aria-label={t('Link to a picture')}
              aria-invalid={!!linkError} aria-describedby="link-note" spellcheck="false" autocomplete="off" />
            <Button variant="primary" type="submit" busy={linkBusy} disabled={!link.trim().startsWith('https://')}>{t('Use this picture')}</Button>
          </form>
          <p id="link-note" class="small" class:tone-danger={!!linkError} class:subtle={!linkError}>{linkError || t('Only https:// links. The picture is downloaded once and kept on this computer.')}</p>
        {/if}
      </div>
    </div>
    <Divider />
    <SettingRow title={t('Use this picture on the device pages too')} subtitle={t('Off: it only shows on its card in Devices, and the device pages show the illustration')} disabled={!picture}>
      {#snippet control()}
        <Toggle checked={!!picture && !!layout?.use_on_pages} disabled={!picture} label={t('Use this picture on the device pages too')} onchange={(v) => setUseOnPages(dev, v)} />
      {/snippet}
    </SettingRow>
    <SettingRow title={t('Button positions')} subtitle={t('Where each button is on the picture, for the Buttons page')} disabled={!editable}>
      {#snippet control()}
        <Button icon="move" disabled={!editable} onclick={() => (editing = true)}>{t('Edit button positions…')}</Button>
      {/snippet}
    </SettingRow>
  </Panel>

  <div class="pair">
    <Panel title={t('Details')} icon="info">
      <dl class="details">
        <dt>{t('Model')}</dt><dd class="selectable">{dev.model || dev.name || '—'}</dd>
        <dt>{t('Product ID')}</dt><dd class="selectable mono">{dev.product_id || '—'}</dd>
        <dt>{t('Connection')}</dt><dd>{connectionName(dev.connection)}</dd>
        <dt>{t('Firmware')}</dt><dd class="selectable">{#if dev.firmware?.length}{#each dev.firmware as f (f)}<div>{f}</div>{/each}{:else}{t('Not available')}{/if}</dd>
        <dt>{t('Battery')}</dt><dd>{batteryText(dev.battery) || t('No information')}</dd>
      </dl>
    </Panel>

    <Panel title={t('Reset and remove')} subtitle={resetExplanation(dev)} icon="rotate-ccw" tone="danger">
      <div class="danger-actions">
        <Button variant="danger" disabled={!dev.online} busy={resetting} onclick={reset}>{t('Reset device')}</Button>
        <Button variant="danger-ghost" icon="trash" title={t('{app} forgets this device and everything it stored for it')} onclick={() => removeDevice(dev)}>{t('Remove device…')}</Button>
      </div>
      {#if !dev.online}<p class="small subtle">{t('Connect the device to be able to reset it')}</p>{/if}
    </Panel>
  </div>

  <section class="tech">
    <button type="button" class="tech-head" aria-expanded={techOpen} aria-controls="tech-body" onclick={() => (techOpen = !techOpen)}
      title={techOpen ? t('Hide technical information') : t('Show technical information')}>
      <span class="titles">
        <strong>{t('Technical information')}</strong>
        <span class="small muted">{t('Details for diagnosing a problem or asking for help')}</span>
      </span>
      <Icon name={techOpen ? 'chevron-up' : 'chevron-down'} size={18} />
    </button>
    {#if techOpen}
      <div id="tech-body" class="tech-body">
        <dl class="details">
          <dt>{t('Identifier')}</dt><dd class="mono selectable">{dev.id || '—'}</dd>
          <dt>{t('Source')}</dt><dd class="mono selectable">{dev.source || '—'}</dd>
          <dt>{t('HID++ features')}</dt><dd class="mono selectable">{(dev.features ?? []).join(', ') || '—'}</dd>
        </dl>
        <div><Button icon="copy" title={t('Copies a plain-text report to paste wherever you ask for help')} onclick={copyDiagnostics}>{t('Copy diagnostics')}</Button></div>
      </div>
    {/if}
  </section>
</div>

<PictureLayoutEditor open={editing} {dev} src={editable?.src ?? null} onclose={() => (editing = false)} />

<style>
  .page {
    container-type: inline-size;
    padding: var(--space-5) var(--space-7) var(--space-8);
    display: flex;
    flex-direction: column;
    gap: var(--space-5);
    max-width: 1120px;
    width: 100%;
    margin: 0 auto;
  }
  .summary {
    display: flex;
    align-items: center;
    gap: var(--space-6);
    padding: var(--space-5) var(--space-6);
    border-radius: var(--radius-l);
    background: var(--surface);
    border: 1px solid var(--border);
    box-shadow: var(--shadow-1);
  }
  .art {
    width: 132px;
    height: 104px;
    flex: none;
  }
  .info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .summary :global(.pill) {
    align-self: flex-start;
  }
  .picture-row {
    display: flex;
    gap: var(--space-5);
    align-items: flex-start;
  }
  .preview {
    flex: none;
    display: grid;
    place-items: center;
    width: 112px;
    height: 84px;
    border-radius: var(--radius-m);
    border: 1.5px dashed var(--border-strong);
    background: var(--surface-2);
    color: var(--text-subtle);
    overflow: hidden;
    padding: 0;
  }
  .preview:hover {
    border-color: var(--accent);
    color: var(--accent);
  }
  .preview img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .pic-text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .pic-text .title {
    font-weight: 540;
  }
  .pic-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
  .link {
    display: flex;
    gap: var(--space-2);
    margin-top: var(--space-1);
  }
  .pair {
    display: grid;
    grid-template-columns: 3fr 2fr;
    gap: var(--space-5);
    align-items: stretch;
  }
  .details {
    display: grid;
    grid-template-columns: minmax(110px, auto) 1fr;
    gap: var(--space-2) var(--space-5);
    margin: 0;
  }
  dt {
    color: var(--text-muted);
  }
  dd {
    margin: 0;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .danger-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
  .tech {
    border-radius: var(--radius-l);
    background: var(--surface);
    border: 1px solid var(--border);
    box-shadow: var(--shadow-1);
  }
  .tech-head {
    display: flex;
    align-items: center;
    gap: var(--space-4);
    width: 100%;
    padding: var(--space-5) var(--space-6);
    border: 0;
    border-radius: inherit;
    background: transparent;
    text-align: left;
    color: var(--text);
  }
  .tech-head :global(.icon) {
    color: var(--text-muted);
  }
  .titles {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .tech-body {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    padding: 0 var(--space-6) var(--space-5);
    border-top: 1px solid var(--border);
    padding-top: var(--space-4);
  }
  @container (max-width: 720px) {
    .pair { grid-template-columns: minmax(0, 1fr); }
  }
  @media (max-width: 1000px) {
    .page { padding-inline: var(--space-5); }
  }
</style>
