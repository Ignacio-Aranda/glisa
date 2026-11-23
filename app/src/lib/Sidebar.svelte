<script lang="ts">
  import Icon from './components/Icon.svelte';
  import Logo from './components/Logo.svelte';
  import DeviceIcon from './components/DeviceIcon.svelte';
  import { app, navigate } from './state/app.svelte';
  import { productName, t, tc } from './i18n/index.svelte';
  import { batteryTone, percentText } from './format';

  const route = $derived(app.route);
  const devices = $derived(app.devices ?? []);
  const flowOn = $derived(app.flow ? app.flow.running : null);
  const demo = $derived(!!app.status?.simulate || app.info.simulate);

  /** A small picture for the list: the user's own, else the official photo (if on). */
  function thumbOf(id: string): string | null {
    return app.pictures[id] || (app.settings.official_photos ? app.photos[id] : null) || null;
  }

  function go(e: MouseEvent, r: Parameters<typeof navigate>[0]) {
    e.preventDefault();
    navigate(r);
  }
</script>

<nav class="sidebar" aria-label={t('Main')}>
  <div class="brand">
    <Logo size={24} />
    <span class="name">{productName()}</span>
  </div>

  <a href="#/" class="item" class:current={route.page === 'devices'} aria-current={route.page === 'devices' ? 'page' : undefined}
    onclick={(e) => go(e, { page: 'devices' })}>
    <Icon name="layout-grid" size={20} />
    <span class="label">{t('All devices')}</span>
  </a>

  {#if devices.length}
    <div class="overline section" id="nav-devices">{tc('page title', 'Devices')}</div>
    <ul class="devices" aria-labelledby="nav-devices">
      {#each devices as d (d.id)}
        {@const current = route.page === 'device' && route.id === d.id}
        <li>
          <a href="#/device/{encodeURIComponent(d.id)}" class="item device" class:current aria-current={current ? 'page' : undefined}
            onclick={(e) => go(e, { page: 'device', id: d.id, tab: null })}>
            <span class="thumb" class:offline={!d.online} aria-hidden="true">
              {#if thumbOf(d.id)}<img src={thumbOf(d.id)} alt="" draggable="false" />{:else}<DeviceIcon kind={d.kind} family={d.family} size={28} dimmed={!d.online} />{/if}
            </span>
            <span class="label ellipsis">{d.name}</span>
            {#if d.online && d.battery?.percent}
              <span class="meta tone-{batteryTone(d.battery)}">{percentText(d.battery.percent)}</span>
            {/if}
            <span class="dot" class:online={d.online} title={d.online ? t('Connected') : t('Disconnected')}></span>
            <span class="sr-only">{d.online ? t('Connected') : t('Disconnected')}</span>
          </a>
        </li>
      {/each}
    </ul>
  {/if}

  <div class="spacer"></div>

  {#if demo}
    <div class="demo" title={t('These devices are simulated: nothing you change affects a real mouse')}>
      <Icon name="wand-sparkles" size={16} />
      <span>{t('Demo mode')}</span>
    </div>
  {/if}

  <a href="#/flow" class="item" class:current={route.page === 'flow'} aria-current={route.page === 'flow' ? 'page' : undefined}
    onclick={(e) => go(e, { page: 'flow' })}>
    <Icon name="waypoints" size={20} />
    <span class="label">Flow</span>
    {#if flowOn !== null}
      <span class="badge" class:on={flowOn}>{flowOn ? tc('flow state', 'On') : tc('flow state', 'Off')}</span>
    {/if}
  </a>
  <a href="#/settings" class="item" class:current={route.page === 'settings'} aria-current={route.page === 'settings' ? 'page' : undefined}
    title={t('App settings (Ctrl+,)')} onclick={(e) => go(e, { page: 'settings' })}>
    <Icon name="settings-2" size={20} />
    <span class="label">{t('App settings')}</span>
  </a>
</nav>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    gap: 2px;
    height: 100%;
    padding: var(--space-4) var(--space-3);
    background: var(--bg);
    border-right: 1px solid var(--border);
    overflow-y: auto;
    scrollbar-width: none;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-1) var(--space-3) var(--space-5);
  }
  .name {
    font-weight: 650;
    font-size: var(--text-md);
  }
  .section {
    padding: var(--space-4) var(--space-3) var(--space-2);
  }
  .item {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    min-height: 36px;
    padding: 0 var(--space-3);
    border-radius: var(--radius-m);
    color: var(--text-muted);
    text-decoration: none;
    font-weight: 500;
    transition: background-color var(--dur-fast) var(--ease-out), color var(--dur-fast) var(--ease-out);
  }
  .item :global(.icon) {
    color: var(--text-muted);
  }
  .item:hover {
    background: var(--surface-hover);
    color: var(--text);
  }
  .item.current {
    background: var(--accent-soft);
    color: var(--text);
  }
  .item.current :global(.icon) {
    color: var(--accent);
  }
  .item:focus-visible {
    outline: none;
    box-shadow: 0 0 0 3px var(--focus-ring);
  }
  .label {
    flex: 1;
    min-width: 0;
  }
  .devices {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .device {
    min-height: 40px;
    gap: var(--space-2);
    padding-left: var(--space-2);
  }
  .thumb {
    width: 28px;
    height: 28px;
    flex: none;
    display: grid;
    place-items: center;
  }
  .thumb img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  .thumb.offline img {
    opacity: 0.5;
    filter: grayscale(0.7);
  }
  .meta {
    flex: none;
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
  }
  .meta.tone-ok {
    color: var(--text-subtle);
  }
  .dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-subtle);
  }
  .dot.online {
    background: var(--success);
  }
  .badge {
    font-size: var(--text-xs);
    font-weight: 600;
    padding: 1px 7px;
    border-radius: var(--radius-s);
    background: var(--surface-2);
    color: var(--text-muted);
  }
  .badge.on {
    background: var(--success-soft);
    color: var(--success);
  }
  .spacer {
    flex: 1;
    min-height: var(--space-4);
  }
  .demo {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin: 0 0 var(--space-2);
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-m);
    border: 1px solid var(--border);
    background: var(--surface-2);
    color: var(--text-muted);
    font-size: var(--text-xs);
    font-weight: 600;
  }
</style>
