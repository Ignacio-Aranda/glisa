<script lang="ts">
  import IconButton from '../components/IconButton.svelte';
  import StatusPill from '../components/StatusPill.svelte';
  import BatteryBadge from '../components/BatteryBadge.svelte';
  import Tabs from '../components/Tabs.svelte';
  import Spinner from '../components/Spinner.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import Menu, { type MenuItem } from '../components/Menu.svelte';
  import { app, navigate, refreshDevice } from '../state/app.svelte';
  import { choosePicture, removeDevice, removePicture, rename } from '../state/identity.svelte';
  import { connectionName } from '../format';
  import { t } from '../i18n/index.svelte';
  import type { Device } from '../types';

  interface Props { id: string; tab: string | null }
  let { id, tab }: Props = $props();

  const sections = {
    buttons: () => import('./device/ButtonsSection.svelte'),
    pointer: () => import('./device/PointerSection.svelte'),
    'easy-switch': () => import('./device/EasySwitchSection.svelte'),
    settings: () => import('./device/SettingsSection.svelte'),
  } as const;
  type SectionId = keyof typeof sections;

  const dev = $derived<Device | undefined>(app.details[id]);
  const summary = $derived(app.devices?.find((d) => d.id === id));
  const error = $derived(app.detailErrors[id] ?? '');

  $effect(() => {
    void refreshDevice(id);
  });

  const tabs = $derived.by(() => {
    const d = dev;
    const out: { id: SectionId; label: string; icon: string }[] = [{ id: 'buttons', label: t('Buttons'), icon: 'mouse-pointer-click' }];
    if (d && (d.dpi || d.wheel || d.smartshift)) out.push({ id: 'pointer', label: t('Pointer & scrolling'), icon: 'gauge' });
    if (d && (d.hosts?.count ?? 0) > 0) out.push({ id: 'easy-switch', label: t('Easy-Switch'), icon: 'monitor-smartphone' });
    out.push({ id: 'settings', label: t('Settings'), icon: 'settings-2' });
    return out;
  });
  const current = $derived<SectionId>(
    tab && tabs.some((x) => x.id === tab) ? (tab as SectionId) : 'buttons',
  );

  function select(next: string) {
    navigate({ page: 'device', id, tab: next }, true);
  }

  let more: HTMLButtonElement | null = $state(null);
  let menu = $state<{ x: number; y: number } | null>(null);
  const menuItems = $derived.by((): MenuItem[] => {
    const has = !!app.pictures[id];
    const items: MenuItem[] = [
      { id: 'rename', label: t('Rename…'), icon: 'pencil' },
      { id: 'picture', label: has ? t('Change picture…') : t('Choose picture…'), icon: 'image' },
    ];
    if (has) items.push({ id: 'unpicture', label: t('Remove picture'), icon: 'image-off' });
    items.push({ id: 'remove', label: t('Remove device…'), icon: 'trash', danger: true });
    return items;
  });

  function openMenu() {
    const r = more?.getBoundingClientRect();
    menu = { x: r ? r.right - 230 : 0, y: r ? r.bottom + 4 : 0 };
  }

  function onmenu(item: string) {
    const d = dev ?? summary;
    if (!d) return;
    if (item === 'rename') rename(d);
    else if (item === 'picture') void choosePicture(d);
    else if (item === 'unpicture') void removePicture(d);
    else if (item === 'remove') void removeDevice(d);
  }
</script>

<div class="device">
  <header class="head">
    <div class="row">
      <IconButton icon="chevron-left" label={t('Back to your devices')} tooltip={t('Back (Esc)')} onclick={() => navigate({ page: 'devices' })} />
      <div class="title">
        <h1 class="ellipsis">{dev?.name ?? summary?.name ?? t('Device')}</h1>
        {#if dev ?? summary}
          {@const d = (dev ?? summary)!}
          <span class="conn">{connectionName(d.connection)}</span>
        {/if}
      </div>
      {#if dev ?? summary}
        {@const d = (dev ?? summary)!}
        <div class="status">
          {#if d.online}<BatteryBadge battery={d.battery} />{/if}
          <StatusPill text={d.online ? t('Connected') : t('Disconnected')} tone={d.online ? 'ok' : 'muted'} />
          <IconButton bind:el={more} icon="ellipsis" label={t('More options')} onclick={openMenu} aria-haspopup="menu" />
        </div>
      {/if}
    </div>
    {#if dev}
      <div class="tabs"><Tabs {tabs} {current} label={t('Section')} idPrefix="device" onselect={select} /></div>
    {/if}
  </header>

  <div class="body" role="tabpanel" id="device-panel" aria-labelledby="device-{current}">
    {#if !dev}
      {#if error}
        <EmptyState icon="circle-exclamation-point" title={t('Couldn\'t load the device')} text={error} action={t('Retry')} actionIcon="refresh-cw" onaction={() => refreshDevice(id)} />
      {:else if app.devices && !summary}
        <EmptyState icon="mouse" title={t('Device not found')} text={t('It may have been removed. Go back to see your devices.')} action={t('Back to your devices')} onaction={() => navigate({ page: 'devices' })} />
      {:else}
        <div class="loading"><Spinner size={22} /><span class="muted">{t('Loading the device…')}</span></div>
      {/if}
    {:else}
      {#key current}
        {#await sections[current]()}
          <div class="loading"><Spinner size={22} /></div>
        {:then m}
          <m.default {dev} />
        {:catch}
          <EmptyState icon="circle-exclamation-point" title={t("Couldn't load this section")} />
        {/await}
      {/key}
    {/if}
  </div>
</div>

<Menu open={!!menu} label={t('Device options')} items={menuItems} x={menu?.x ?? 0} y={menu?.y ?? 0} onselect={onmenu} onclose={() => (menu = null)} />

<style>
  .device {
    display: flex;
    flex-direction: column;
    min-height: 100%;
  }
  .head {
    position: sticky;
    top: 0;
    z-index: 5;
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    padding: var(--space-6) var(--space-7) var(--space-4);
    background: var(--bg);
    border-bottom: 1px solid var(--border);
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    min-width: 0;
  }
  .title {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: baseline;
    gap: var(--space-3);
  }
  .conn {
    flex: none;
    font-size: var(--text-xs);
    font-weight: 650;
    padding: 2px 9px;
    border-radius: 999px;
    border: 1px solid var(--border-strong);
    color: var(--text-muted);
    transform: translateY(-3px);
  }
  .status {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }
  .tabs {
    display: flex;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .loading {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-3);
    padding: var(--space-8);
  }
  @media (max-width: 1000px) {
    .head { padding-inline: var(--space-5); }
  }
</style>
