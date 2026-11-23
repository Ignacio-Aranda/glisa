<script lang="ts">
  import { fly } from 'svelte/transition';
  import { backOut } from 'svelte/easing';
  import Icon from './Icon.svelte';
  import { app, dismissToast } from '../state/app.svelte';
  import { t } from '../i18n/index.svelte';

  const icons = { ok: 'circle-check', info: 'info', warn: 'triangle-exclamation-point', error: 'circle-exclamation-point' };
</script>

<div class="toasts" role="region" aria-label={t('Notifications')} aria-live="polite">
  {#each app.toasts as toast (toast.id)}
    <div class="toast {toast.kind}" role={toast.kind === 'error' ? 'alert' : 'status'} transition:fly={{ y: 8, duration: 200, easing: backOut }}>
      <span class="icon"><Icon name={icons[toast.kind]} size={17} /></span>
      <span class="text">{toast.text}</span>
      <button type="button" class="close" aria-label={t('Close')} onclick={() => dismissToast(toast.id)}>
        <Icon name="x" size={14} />
      </button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    left: 50%;
    bottom: var(--space-3);
    transform: translateX(-50%);
    z-index: 200;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-2);
    pointer-events: none;
    width: min(420px, calc(100vw - 2 * var(--space-6)));
  }
  .toast {
    pointer-events: auto;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    max-width: 100%;
    padding: var(--space-2) var(--space-2) var(--space-2) var(--space-4);
    border-radius: var(--radius-m);
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    color: var(--text);
    box-shadow: var(--shadow-3);
  }
  .icon {
    display: grid;
    flex: none;
  }
  .ok .icon { color: var(--success); }
  .warn .icon { color: var(--warning); }
  .error .icon { color: var(--danger); }
  .info .icon { color: var(--info); }
  .text {
    flex: 1;
    min-width: 0;
  }
  .close {
    flex: none;
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border: 0;
    border-radius: var(--radius-s);
    background: transparent;
    color: var(--text-muted);
  }
  .close:hover {
    background: var(--surface-hover);
  }
</style>
