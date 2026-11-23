<script lang="ts">
  import { comboKeys, keyLabel } from '../keys';
  import type { Platform } from '../types';

  interface Props { combo: string; platform: Platform; size?: 'sm' | 'md' }
  let { combo, platform, size = 'md' }: Props = $props();
  const keys = $derived(comboKeys(combo));
</script>

<span class="caps {size}" aria-label={combo}>
  {#each keys as k, i (i)}
    {#if i}<span class="plus" aria-hidden="true">+</span>{/if}
    <kbd aria-hidden="true">{keyLabel(k, platform)}</kbd>
  {/each}
</span>

<style>
  .caps {
    display: inline-flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 4px;
  }
  kbd {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 28px;
    height: 28px;
    padding: 0 var(--space-2);
    border-radius: var(--radius-xs);
    border: 1px solid var(--border-strong);
    border-bottom-width: 2px;
    background: var(--surface);
    font-family: var(--font-sans);
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text);
  }
  .sm kbd {
    min-width: 22px;
    height: 22px;
    font-size: var(--text-xs);
    padding: 0 6px;
  }
  .plus {
    color: var(--text-subtle);
    font-size: var(--text-xs);
  }
</style>
