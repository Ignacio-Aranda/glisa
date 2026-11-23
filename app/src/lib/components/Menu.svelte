<script lang="ts" module>
  export interface MenuItem { id: string; label: string; icon?: string; danger?: boolean; disabled?: boolean }
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import Icon from './Icon.svelte';
  interface Props {
    items: MenuItem[];
    open: boolean;
    label: string;
    /** Viewport position of the menu's top-left corner (it is kept on screen). */
    x: number;
    y: number;
    onselect: (id: string) => void;
    onclose: () => void;
  }
  let { items, open, label, x, y, onselect, onclose }: Props = $props();
  let menu: HTMLDivElement | null = $state(null);
  let pos = $state({ left: 0, top: 0 });

  $effect(() => {
    if (!open) return;
    const previous = document.activeElement as HTMLElement | null;
    pos = { left: x, top: y };
    void tick().then(() => {
      if (!menu) return;
      const r = menu.getBoundingClientRect();
      pos = {
        left: Math.max(8, Math.min(x, window.innerWidth - r.width - 8)),
        top: Math.max(8, Math.min(y, window.innerHeight - r.height - 8)),
      };
      menu.querySelector<HTMLElement>('[role="menuitem"]:not([disabled])')?.focus();
    });
    const away = (e: MouseEvent) => {
      if (menu && !menu.contains(e.target as Node)) onclose();
    };
    window.addEventListener('mousedown', away, true);
    return () => {
      window.removeEventListener('mousedown', away, true);
      if (previous && document.contains(previous)) previous.focus();
    };
  });

  function onkeydown(e: KeyboardEvent) {
    const els = Array.from(menu?.querySelectorAll<HTMLElement>('[role="menuitem"]:not([disabled])') ?? []);
    const i = els.indexOf(document.activeElement as HTMLElement);
    if (e.key === 'ArrowDown') { e.preventDefault(); els[(i + 1) % els.length]?.focus(); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); els[(i - 1 + els.length) % els.length]?.focus(); }
    else if (e.key === 'Home') { e.preventDefault(); els[0]?.focus(); }
    else if (e.key === 'End') { e.preventDefault(); els[els.length - 1]?.focus(); }
    else if (e.key === 'Escape' || e.key === 'Tab') { e.preventDefault(); e.stopPropagation(); onclose(); }
  }
</script>

{#if open}
  <div bind:this={menu} class="menu" role="menu" aria-label={label} tabindex="-1" style:left="{pos.left}px" style:top="{pos.top}px" {onkeydown}>
    {#each items as item (item.id)}
      <button
        type="button"
        role="menuitem"
        class:danger={item.danger}
        disabled={item.disabled}
        onclick={() => { onselect(item.id); onclose(); }}
      >
        {#if item.icon}<Icon name={item.icon} size={16} />{/if}
        <span>{item.label}</span>
      </button>
    {/each}
  </div>
{/if}

<style>
  .menu {
    position: fixed;
    z-index: 150;
    min-width: 220px;
    padding: var(--space-1);
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius-m);
    box-shadow: var(--shadow-3);
  }
  button {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    width: 100%;
    height: 36px;
    padding: 0 var(--space-3);
    border: 0;
    border-radius: var(--radius-s);
    background: transparent;
    color: var(--text);
    text-align: left;
  }
  button:hover:not(:disabled),
  button:focus-visible {
    background: var(--surface-hover);
    outline: none;
  }
  button :global(.icon) {
    color: var(--text-muted);
  }
  .danger,
  .danger :global(.icon) {
    color: var(--danger);
  }
  button:disabled {
    opacity: 0.5;
  }
</style>
