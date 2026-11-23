<script lang="ts">
  import { fly } from 'svelte/transition';
  import Icon from '../../components/Icon.svelte';
  import Button from '../../components/Button.svelte';
  import IconButton from '../../components/IconButton.svelte';
  import { api, shell } from '../../backend';
  import { app, toast } from '../../state/app.svelte';
  import { transferText } from '../../format';
  import { t } from '../../i18n/index.svelte';
  import type { Transfer } from '../../types';

  const HIDE_MS = 9000;
  let current = $state<Transfer | null>(null);
  let hidden = $state(false);
  const seenActive = new Set<string>();
  const known = new Set<string>();
  let started = false;
  let timer: ReturnType<typeof setTimeout> | null = null;

  $effect(() => {
    const list = app.transfers;
    const first = !started;
    started = true;
    let pick: Transfer | null = null;
    for (const tr of list) {
      const wasActive = seenActive.has(tr.id);
      const seen = known.has(tr.id);
      known.add(tr.id);
      if (tr.active) {
        seenActive.add(tr.id);
        pick = tr;
        continue;
      }
      if (!wasActive && (first || !seen || (tr.ago ?? 0) > 15)) continue;   // finished before we were looking
      if (tr.state === 'failed' || tr.state === 'refused') {
        if (current?.id === tr.id) current = null;                          // the service's notice says why
        continue;
      }
      if (wasActive && (!pick || !pick.active)) pick = tr;
    }
    if (!pick) {
      if (current && !list.some((x) => x.id === current!.id)) current = null;
      return;
    }
    if (!current || current.id !== pick.id || current.state !== pick.state || current.percent !== pick.percent) {
      if (current?.id !== pick.id) hidden = false;
      current = pick;
      if (timer) clearTimeout(timer);
      timer = null;
      if (!pick.active) timer = setTimeout(() => (current = null), HIDE_MS);
    }
  });

  async function cancel() {
    if (!current) return;
    try {
      await api('flow_transfer_cancel', { transfer: current.id });
    } catch (e) {
      toast(String((e as Error).message ?? e), 'error');
    }
  }

  async function openFolder() {
    if (!current?.folder) return;
    try {
      await shell.openPath(current.folder);
      current = null;
    } catch {
      toast(t("Couldn't open the folder"), 'error');
    }
  }

  const tone = $derived(!current ? 'accent' : current.active ? 'accent' : current.state === 'done' ? 'ok' : 'warn');
  const icon = $derived(!current ? 'arrow-left-right' : current.active ? 'arrow-left-right' : current.state === 'done' ? 'circle-check' : 'triangle-exclamation-point');
</script>

{#if current && !hidden}
  <div class="card" role="status" aria-live="polite" transition:fly={{ y: 20, duration: 200 }}>
    <div class="top">
      <span class="icon tone-{tone}"><Icon name={icon} size={18} /></span>
      <p class="text">{transferText(current)}</p>
      <IconButton icon="x" label={t('Close')} size={28} iconSize={15} onclick={() => (hidden = true)} />
    </div>
    {#if current.active}
      <div class="bar" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={current.percent} aria-label={t('Progress')}>
        <span style:width="{Math.max(2, current.percent)}%"></span>
      </div>
    {/if}
    {#if current.active || (current.state === 'done' && current.direction === 'receive' && current.folder)}
      <div class="actions">
        {#if current.active}
          <Button size="sm" variant="ghost" onclick={cancel}>{t('Cancel')}</Button>
        {:else}
          <Button size="sm" icon="folder-open" onclick={openFolder}>{t('Open folder')}</Button>
        {/if}
      </div>
    {/if}
  </div>
{/if}

<style>
  .card {
    position: fixed;
    right: var(--space-6);
    bottom: var(--space-6);
    z-index: 180;
    width: min(420px, calc(100vw - 2 * var(--space-6)));
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-4);
    border-radius: var(--radius-l);
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    box-shadow: var(--shadow-3);
  }
  .top {
    display: flex;
    align-items: flex-start;
    gap: var(--space-3);
  }
  .icon {
    display: grid;
    flex: none;
    padding-top: 2px;
  }
  .text {
    flex: 1;
    min-width: 0;
    font-weight: 520;
  }
  .bar {
    height: 6px;
    border-radius: 999px;
    background: var(--surface-2);
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: var(--accent);
    transition: width var(--dur-med) var(--ease-out);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }
</style>
