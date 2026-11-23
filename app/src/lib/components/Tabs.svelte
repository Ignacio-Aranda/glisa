<script lang="ts">
  import Icon from './Icon.svelte';

  interface Tab { id: string; label: string; icon?: string }
  interface Props {
    tabs: Tab[];
    current: string;
    label: string;
    idPrefix?: string;
    onselect: (id: string) => void;
  }
  let { tabs, current, label, idPrefix = 'tab', onselect }: Props = $props();
  let els: HTMLButtonElement[] = $state([]);

  function onkeydown(e: KeyboardEvent, i: number) {
    let n = -1;
    if (e.key === 'ArrowRight') n = (i + 1) % tabs.length;
    else if (e.key === 'ArrowLeft') n = (i - 1 + tabs.length) % tabs.length;
    else if (e.key === 'Home') n = 0;
    else if (e.key === 'End') n = tabs.length - 1;
    if (n < 0) return;
    e.preventDefault();
    els[n]?.focus();
    onselect(tabs[n].id);
  }
</script>

<div class="tabs" role="tablist" aria-label={label}>
  {#each tabs as tab, i (tab.id)}
    <button
      bind:this={els[i]}
      type="button"
      role="tab"
      id="{idPrefix}-{tab.id}"
      aria-selected={tab.id === current}
      aria-controls="{idPrefix}-panel"
      tabindex={tab.id === current ? 0 : -1}
      onclick={() => onselect(tab.id)}
      onkeydown={(e) => onkeydown(e, i)}
    >
      {#if tab.icon}<Icon name={tab.icon} size={16} />{/if}
      <span>{tab.label}</span>
    </button>
  {/each}
</div>

<style>
  .tabs {
    display: flex;
    gap: var(--space-1);
    padding: 4px;
    border-radius: var(--radius-m);
    background: var(--surface-2);
    overflow-x: auto;
    scrollbar-width: none;
    max-width: 100%;
  }
  button {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    height: 32px;
    padding: 0 var(--space-4);
    border: 0;
    border-radius: var(--radius-s);
    background: transparent;
    color: var(--text-muted);
    font-weight: 560;
    white-space: nowrap;
    transition: background-color var(--dur-fast) var(--ease-out), color var(--dur-fast) var(--ease-out);
  }
  button:hover {
    color: var(--text);
  }
  button[aria-selected='true'] {
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-1);
  }
  button[aria-selected='true'] :global(.icon) {
    color: var(--accent);
  }
</style>
