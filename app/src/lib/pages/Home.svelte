<script lang="ts">
  import DeviceCard from './home/DeviceCard.svelte';
  import Banner from '../components/Banner.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import Icon from '../components/Icon.svelte';
  import IconButton from '../components/IconButton.svelte';
  import Button from '../components/Button.svelte';
  import Menu, { type MenuItem } from '../components/Menu.svelte';
  import SettingRow from '../components/SettingRow.svelte';
  import Divider from '../components/Divider.svelte';
  import { app, navigate, refreshDevices, refreshStatus, toast } from '../state/app.svelte';
  import { choosePicture, removeDevice, removePicture, rename } from '../state/identity.svelte';
  import { permissionSummary } from '../permissions';
  import { shell } from '../backend';
  import { t, tc } from '../i18n/index.svelte';
  import type { DeviceSummary } from '../types';

  const devices = $derived(app.devices);
  const view = $derived(
    devices === null ? (app.devicesError ? 'error' : 'loading') : devices.length ? 'devices' : 'empty',
  );
  const summary = $derived(permissionSummary(app.status));
  const demo = $derived(!!app.status?.simulate || app.info.simulate);

  let menu = $state<{ dev: DeviceSummary; x: number; y: number } | null>(null);
  const menuItems = $derived.by((): MenuItem[] => {
    if (!menu) return [];
    const has = !!app.pictures[menu.dev.id];
    const items: MenuItem[] = [
      { id: 'open', label: t('Configure {name}', { name: menu.dev.name }), icon: 'settings' },
      { id: 'rename', label: t('Rename…'), icon: 'pencil' },
      { id: 'picture', label: has ? t('Change picture…') : t('Choose picture…'), icon: 'image' },
    ];
    if (has) items.push({ id: 'unpicture', label: t('Remove picture'), icon: 'image-off' });
    items.push({ id: 'remove', label: t('Remove device…'), icon: 'trash', danger: true });
    return items;
  });

  function onmenu(id: string) {
    const dev = menu?.dev;
    if (!dev) return;
    if (id === 'open') navigate({ page: 'device', id: dev.id, tab: null });
    else if (id === 'rename') rename(dev);
    else if (id === 'picture') void choosePicture(dev);
    else if (id === 'unpicture') void removePicture(dev);
    else if (id === 'remove') void removeDevice(dev);
  }

  async function retry() {
    await Promise.all([refreshDevices(), refreshStatus()]);
  }

  async function refresh() {
    await retry();
    toast(t('Refreshed'), 'ok', 1600);
  }

  async function openBluetooth() {
    try {
      await shell.openBluetooth();
    } catch {
      toast(t("Couldn't open the Bluetooth settings"), 'error');
    }
  }
</script>

<div class="page">
  <header class="head">
    <div class="titles">
      <h1>{tc('page title', 'Devices')}</h1>
      {#if view === 'devices'}
        <p class="muted">
          {(devices?.length ?? 0) > 1 ? t('Pick one to set up its buttons, pointer and more') : t('Click it to set up its buttons, pointer and more')}
        </p>
      {/if}
    </div>
    <IconButton icon="refresh-cw" label={t('Refresh')} tooltip={t('Refresh (F5)')} onclick={refresh} />
  </header>

  <div class="banners">
    {#if summary}
      <Banner kind="warn" title={t('Permissions needed')} text={summary} action={t('Fix')} onaction={() => (app.dialog = 'permissions')} />
    {/if}
    {#if app.devicesError && devices !== null}
      <Banner kind="error" title={t('The service isn\'t responding')} text={app.devicesError} action={t('Retry')} onaction={retry} />
    {/if}
    {#if demo}
      <Banner kind="quiet" title={t('Demo mode')} text={t('These devices are simulated: nothing you change affects a real mouse')} />
    {/if}
  </div>

  {#if view === 'loading'}
    <div class="grid" aria-busy="true" aria-label={t('Looking for devices…')}>
      {#each [0, 1, 2] as i (i)}
        <div class="skeleton"><div class="sk-art"></div><div class="sk-line"></div><div class="sk-line short"></div></div>
      {/each}
    </div>
  {:else if view === 'error'}
    <EmptyState icon="power" title={t('No connection to the service')} text={app.devicesError} action={t('Retry')} actionIcon="refresh-cw" onaction={retry} />
  {:else if view === 'empty'}
    <EmptyState
      icon="mouse"
      title={t('No devices found')}
      text={t('{app} looks for Logitech mice and keyboards connected to this computer via Bluetooth or a USB receiver.')}
      action={t('How to add a device')}
      actionIcon="plus"
      onaction={() => (app.dialog = 'add-device')}
    />
    <section class="tips" aria-labelledby="tips-title">
      <h3 id="tips-title">{t('Already connected? Try this')}</h3>
      <SettingRow icon="power" title={t('Wake the device up')} subtitle={t('Turn it on and move it or press a button: it may be asleep or out of battery')} />
      <Divider />
      <SettingRow icon="bluetooth" title={t('Check Bluetooth')} subtitle={t('It has to be on, and the device paired and connected to this computer')}>
        {#snippet control()}<Button size="sm" onclick={openBluetooth}>{t('Open Bluetooth')}</Button>{/snippet}
      </SettingRow>
      <Divider />
      <SettingRow icon="shield" title={t('Check the permissions')} subtitle={t('Without them, {app} can\'t see your devices')}>
        {#snippet control()}<Button size="sm" onclick={() => (app.dialog = 'permissions')}>{t('Review permissions')}</Button>{/snippet}
      </SettingRow>
    </section>
  {:else}
    <div class="grid">
      {#each devices ?? [] as dev (dev.id)}
        <DeviceCard
          {dev}
          picture={app.pictures[dev.id]}
          photo={app.photos[dev.id]}
          onopen={() => navigate({ page: 'device', id: dev.id, tab: null })}
          onmenu={(x, y) => (menu = { dev, x, y })}
        />
      {/each}
      <button type="button" class="add" onclick={() => (app.dialog = 'add-device')}>
        <span class="plus"><Icon name="plus" size={22} /></span>
        <strong>{t('Add device')}</strong>
        <span class="muted small">{t('Via Bluetooth or a USB receiver')}</span>
      </button>
    </div>
  {/if}
</div>

<Menu
  open={!!menu}
  label={t('Device options')}
  items={menuItems}
  x={menu?.x ?? 0}
  y={menu?.y ?? 0}
  onselect={onmenu}
  onclose={() => (menu = null)}
/>

<style>
  .page {
    max-width: 1120px;
    padding: var(--space-6) var(--space-7) var(--space-8);
    display: flex;
    flex-direction: column;
    gap: var(--space-5);
  }
  .head {
    display: flex;
    align-items: flex-start;
    gap: var(--space-4);
  }
  .titles {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .banners {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .banners:empty {
    display: none;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
    gap: var(--space-5);
  }
  .add {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    min-height: 268px;
    border-radius: var(--radius-l);
    border: 1.5px dashed var(--border-strong);
    background: transparent;
    color: var(--text);
    transition: background-color var(--dur-fast) var(--ease-out), border-color var(--dur-fast) var(--ease-out);
  }
  .add:hover {
    background: var(--surface);
    border-color: var(--accent);
  }
  .plus {
    display: grid;
    place-items: center;
    width: 48px;
    height: 48px;
    border-radius: 50%;
    background: var(--accent-soft);
    color: var(--accent);
    margin-bottom: var(--space-1);
  }
  .tips {
    max-width: 680px;
    width: 100%;
    margin: 0 auto;
    padding: var(--space-5) var(--space-6);
    border-radius: var(--radius-l);
    background: var(--surface);
    border: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .skeleton {
    min-height: 268px;
    border-radius: var(--radius-l);
    background: var(--surface);
    border: 1px solid var(--border);
    padding: var(--space-5);
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .sk-art,
  .sk-line {
    border-radius: var(--radius-s);
    background: linear-gradient(90deg, var(--surface-2), var(--surface-hover), var(--surface-2));
    background-size: 200% 100%;
    animation: shimmer 1.4s ease-in-out infinite;
  }
  .sk-art {
    flex: 1;
  }
  .sk-line {
    height: 14px;
  }
  .sk-line.short {
    width: 60%;
  }
  @keyframes shimmer {
    from { background-position: 100% 0; }
    to { background-position: -100% 0; }
  }
  @media (max-width: 1000px) {
    .page { padding: var(--space-6) var(--space-5) var(--space-7); }
  }
</style>
