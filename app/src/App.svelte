<script lang="ts">
  import Sidebar from './lib/Sidebar.svelte';
  import Toasts from './lib/components/Toasts.svelte';
  import ConfirmDialog from './lib/components/ConfirmDialog.svelte';
  import Spinner from './lib/components/Spinner.svelte';
  import Home from './lib/pages/Home.svelte';
  import DevicePage from './lib/pages/DevicePage.svelte';
  import TransferCard from './lib/pages/flow/TransferCard.svelte';
  import PermissionsDialog from './lib/pages/PermissionsDialog.svelte';
  import AddDeviceDialog from './lib/pages/AddDeviceDialog.svelte';
  import RenameDialog from './lib/pages/RenameDialog.svelte';
  import { app, goBack, navigate, refreshDevice, refreshDevices, refreshFlow, refreshStatus, toast } from './lib/state/app.svelte';
  import { confirmState } from './lib/state/confirm.svelte';
  import { shell, inTauri, mockParam } from './lib/backend';
  import { identity } from './lib/state/identity.svelte';
  import { t } from './lib/i18n/index.svelte';

  const loadFlow = () => import('./lib/pages/FlowPage.svelte');
  const loadSettings = () => import('./lib/pages/SettingsPage.svelte');

  let main: HTMLElement | null = $state(null);

  // Missing permissions: offer the onboarding once for each new set of missing ones.
  const missing = $derived(
    (app.status?.permissions ?? []).filter((p) => p.state === 'missing' && p.fix).map((p) => p.id).sort().join(','),
  );
  $effect(() => {
    if (!missing || app.dialog) return;
    let seen = '';
    try {
      seen = localStorage.getItem('lo.permissions.offered') ?? '';
    } catch {
      // private mode: offer every time
    }
    if (seen === missing) return;
    try {
      localStorage.setItem('lo.permissions.offered', missing);
    } catch {
      // ignore
    }
    app.dialog = 'permissions';
  });

  // Screenshots and development: ?dialog=permissions|add-device|rename opens it (mock only).
  let opened = false;
  $effect(() => {
    if (!app.ready || opened || !app.devices) return;
    opened = true;
    const d = mockParam('dialog');
    if (d === 'permissions' || d === 'add-device') app.dialog = d;
    else if (d === 'rename' && app.devices[0]) identity.renaming = app.devices[0];
  });

  // A new page starts at the top, and the focus moves to its heading for screen readers.
  let lastPage = '';
  $effect(() => {
    const key = app.route.page + (app.route.page === 'device' ? app.route.id : '');
    if (key === lastPage) return;
    lastPage = key;
    main?.scrollTo?.({ top: 0 });
  });

  function typing(target: EventTarget | null): boolean {
    const el = target as HTMLElement | null;
    if (!el) return false;
    return el.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(el.tagName);
  }

  async function refreshAll() {
    await Promise.all([refreshDevices(), refreshStatus()]);
    const r = app.route;
    if (r.page === 'device') await refreshDevice(r.id);
    if (r.page === 'flow') await refreshFlow();
    toast(t('Refreshed'), 'ok', 1600);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.defaultPrevented || app.dialog || confirmState.request) return;
    const ctrl = e.ctrlKey || e.metaKey;
    if (e.key === 'Escape' && !ctrl && !e.altKey && !typing(e.target)) {
      e.preventDefault();
      goBack();
    } else if (e.key === 'ArrowLeft' && e.altKey && !ctrl) {
      e.preventDefault();
      goBack();
    } else if (e.key === ',' && ctrl) {
      e.preventDefault();
      navigate({ page: 'settings' });
    } else if (e.key === 'F5' || (ctrl && e.key.toLowerCase() === 'r')) {
      e.preventDefault();
      void refreshAll();
    } else if (ctrl && e.key.toLowerCase() === 'q' && inTauri()) {
      e.preventDefault();
      void shell.quit();
    }
  }
</script>

<svelte:window {onkeydown} />

<a class="skip" href="#main" onclick={(e) => { e.preventDefault(); main?.focus(); }}>{t('Skip to content')}</a>

<div class="shell">
  <Sidebar />
  <main id="main" bind:this={main} class="scroll" tabindex="-1">
    {#if !app.ready}
      <div class="boot"><Spinner size={22} label={t('Loading…')} /></div>
    {:else if app.route.page === 'devices'}
      <Home />
    {:else if app.route.page === 'device'}
      <DevicePage id={app.route.id} tab={app.route.tab} />
    {:else if app.route.page === 'flow'}
      {#await loadFlow()}
        <div class="boot"><Spinner size={22} label={t('Loading…')} /></div>
      {:then m}
        <m.default />
      {/await}
    {:else if app.route.page === 'settings'}
      {#await loadSettings()}
        <div class="boot"><Spinner size={22} label={t('Loading…')} /></div>
      {:then m}
        <m.default />
      {/await}
    {/if}
  </main>
</div>

<TransferCard />
<Toasts />
<ConfirmDialog />
<PermissionsDialog open={app.dialog === 'permissions'} onclose={() => (app.dialog = null)} />
<AddDeviceDialog open={app.dialog === 'add-device'} onclose={() => (app.dialog = null)} />
<RenameDialog />

<style>
  .shell {
    display: grid;
    grid-template-columns: 232px minmax(0, 1fr);
    height: 100vh;
    min-width: 0;
  }
  main {
    position: relative;
    min-width: 0;
    height: 100vh;
    background: var(--bg);
  }
  main:focus {
    outline: none;
  }
  .boot {
    display: grid;
    place-items: center;
    height: 60vh;
  }
  .skip {
    position: absolute;
    left: var(--space-3);
    top: -60px;
    z-index: 300;
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-s);
    background: var(--accent);
    color: var(--text-on-accent);
  }
  .skip:focus {
    top: var(--space-3);
  }
  @media (max-width: 980px) {
    .shell {
      grid-template-columns: 208px minmax(0, 1fr);
    }
  }
</style>
