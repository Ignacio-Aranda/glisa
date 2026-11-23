<script lang="ts">
  import Icon from './Icon.svelte';
  import Button from './Button.svelte';
  import IconButton from './IconButton.svelte';
  import { t } from '../i18n/index.svelte';

  interface Props {
    kind?: 'info' | 'warn' | 'error' | 'ok' | 'quiet';
    title: string;
    text?: string;
    action?: string;
    onaction?: () => void;
    onclose?: () => void;
    busy?: boolean;
  }
  let { kind = 'info', title, text, action, onaction, onclose, busy = false }: Props = $props();
  const icon = $derived(
    kind === 'warn' ? 'triangle-exclamation-point' : kind === 'error' ? 'circle-exclamation-point'
      : kind === 'ok' ? 'circle-check' : 'info',
  );
</script>

<div class="banner {kind}" role={kind === 'error' || kind === 'warn' ? 'alert' : 'status'}>
  <span class="icon"><Icon name={icon} size={18} /></span>
  <div class="text">
    <strong>{title}</strong>
    {#if text}<span class="body">{text}</span>{/if}
  </div>
  {#if action && onaction}
    <Button size="sm" variant={kind === 'quiet' ? 'ghost' : 'secondary'} onclick={onaction} {busy}>{action}</Button>
  {/if}
  {#if onclose}
    <IconButton icon="x" label={t('Close')} size={28} iconSize={15} onclick={onclose} />
  {/if}
</div>

<style>
  .banner {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    border-radius: var(--radius-m);
    border: 1px solid transparent;
  }
  .icon {
    flex: none;
    display: grid;
    align-self: flex-start;
    padding-top: 1px;
  }
  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  strong {
    font-weight: 620;
  }
  .body {
    color: var(--text-muted);
    font-size: var(--text-sm);
  }
  .info {
    background: var(--info-soft);
    border-color: color-mix(in srgb, var(--info) 30%, transparent);
  }
  .info .icon { color: var(--info); }
  .warn {
    background: var(--warning-soft);
    border-color: color-mix(in srgb, var(--warning) 35%, transparent);
  }
  .warn .icon { color: var(--warning); }
  .error {
    background: var(--danger-soft);
    border-color: color-mix(in srgb, var(--danger) 35%, transparent);
  }
  .error .icon { color: var(--danger); }
  .ok {
    background: var(--success-soft);
    border-color: color-mix(in srgb, var(--success) 30%, transparent);
  }
  .ok .icon { color: var(--success); }
  .quiet {
    background: var(--surface-2);
    border-color: var(--border);
    padding-block: var(--space-2);
  }
  .quiet .icon { color: var(--text-muted); }
  .quiet .text { flex-direction: row; flex-wrap: wrap; gap: var(--space-2); align-items: baseline; }
</style>
