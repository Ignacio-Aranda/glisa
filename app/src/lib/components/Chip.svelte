<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon from './Icon.svelte';

  interface Props {
    selected?: boolean;
    disabled?: boolean;
    icon?: string;
    title?: string;
    label?: string;
    mono?: boolean;
    onclick?: (e: MouseEvent) => void;
    children: Snippet;
  }
  let { selected = false, disabled = false, icon, title, label, mono = false, onclick, children }: Props = $props();
</script>

<button type="button" class="chip" class:tabular={mono} aria-pressed={selected} aria-label={label} {title} {disabled} {onclick}>
  {#if icon}<Icon name={icon} size={14} />{/if}
  {@render children()}
</button>

<style>
  .chip {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    height: 30px;
    padding: 0 var(--space-3);
    border-radius: 999px;
    border: 1px solid var(--border-strong);
    background: var(--surface);
    color: var(--text);
    font-size: var(--text-sm);
    font-weight: 540;
    white-space: nowrap;
    transition: background-color var(--dur-fast) var(--ease-out), border-color var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .chip.tabular {
    font-variant-numeric: tabular-nums;
  }
  .chip:hover:not(:disabled) {
    background: var(--surface-hover);
  }
  .chip[aria-pressed='true'] {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--accent);
  }
  .chip:disabled {
    opacity: 0.5;
  }
</style>
