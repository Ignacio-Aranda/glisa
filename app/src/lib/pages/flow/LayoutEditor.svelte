<script lang="ts">
  import Icon from '../../components/Icon.svelte';
  import { t, tc } from '../../i18n/index.svelte';
  import type { FlowPeer, Side } from '../../types';

  interface Props {
    me: string;
    peers: FlowPeer[];
    disabled?: boolean;
    onside: (peer: string, side: Side | null) => void;
  }
  let { me, peers, disabled = false, onside }: Props = $props();

  const SIDES: Side[] = ['top', 'left', 'right', 'bottom'];
  let selected = $state<string | null>(null);
  let drag = $state<{ id: string; x: number; y: number; over: Side | 'tray' | null } | null>(null);
  let root: HTMLDivElement | null = $state(null);

  const at = $derived(Object.fromEntries(SIDES.map((s) => [s, peers.find((p) => p.side === s) ?? null])) as Record<Side, FlowPeer | null>);
  const unplaced = $derived(peers.filter((p) => !p.side));

  function slotText(side: Side): string {
    switch (side) {
      case 'left': return tc('screen side', 'Left');
      case 'right': return tc('screen side', 'Right');
      case 'top': return tc('screen side', 'Top');
      case 'bottom': return tc('screen side', 'Bottom');
    }
  }

  function whereText(side: Side | null): string {
    switch (side) {
      case 'left': return t('on the left');
      case 'right': return t('on the right');
      case 'top': return t('above');
      case 'bottom': return t('below');
      default: return t('not placed');
    }
  }

  function online(p: FlowPeer): string {
    return p.state === 'online' ? tc('computer', 'Online') : tc('computer', 'Offline');
  }

  function place(id: string, side: Side | null) {
    const p = peers.find((x) => x.id === id);
    if (!p || disabled || (p.side ?? null) === side) return;
    onside(id, side);
  }

  function onkeydown(e: KeyboardEvent, p: FlowPeer) {
    const map: Record<string, Side> = { ArrowLeft: 'left', ArrowRight: 'right', ArrowUp: 'top', ArrowDown: 'bottom' };
    if (map[e.key]) {
      e.preventDefault();
      selected = p.id;
      place(p.id, map[e.key]);
    } else if (e.key === 'Delete' || e.key === 'Backspace') {
      e.preventDefault();
      place(p.id, null);
    }
  }

  function targetAt(x: number, y: number): Side | 'tray' | null {
    const el = document.elementFromPoint?.(x, y) as HTMLElement | null;
    const host = el?.closest<HTMLElement>('[data-drop]');
    if (!host || !root?.contains(host)) return null;
    return host.dataset.drop as Side | 'tray';
  }

  function pointerdown(e: PointerEvent, p: FlowPeer) {
    if (disabled || e.button !== 0) return;
    selected = p.id;
    const startX = e.clientX;
    const startY = e.clientY;
    const el = e.currentTarget as HTMLElement;
    let moved = false;
    const move = (ev: PointerEvent) => {
      if (!moved && Math.hypot(ev.clientX - startX, ev.clientY - startY) < 5) return;
      moved = true;
      drag = { id: p.id, x: ev.clientX, y: ev.clientY, over: targetAt(ev.clientX, ev.clientY) };
    };
    const up = (ev: PointerEvent) => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      if (moved) {
        const over = targetAt(ev.clientX, ev.clientY);
        if (over === 'tray') place(p.id, null);
        else if (over) place(p.id, over);
      }
      drag = null;
      el.focus();
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }

  function clickSlot(side: Side) {
    if (selected && !at[side]) place(selected, side);
  }

  const dragged = $derived(drag ? peers.find((p) => p.id === drag!.id) ?? null : null);
</script>

{#snippet tile(p: FlowPeer)}
  <button
    type="button"
    class="monitor peer"
    class:offline={p.state !== 'online'}
    class:identity={p.state === 'identity'}
    class:selected={selected === p.id}
    class:dragging={drag?.id === p.id}
    {disabled}
    aria-label={t('{name}: {where}. Arrow keys to place it, Delete to take it off', { name: p.name, where: whereText(p.side) })}
    onpointerdown={(e) => pointerdown(e, p)}
    onkeydown={(e) => onkeydown(e, p)}
    onfocus={() => (selected = p.id)}
  >
    <span class="screen">
      <Icon name="laptop" size={18} />
      <strong class="ellipsis">{p.name}</strong>
      <span class="small">{online(p)}</span>
    </span>
    <span class="stand" aria-hidden="true"></span>
  </button>
{/snippet}

<div class="editor" bind:this={root}>
  <div class="grid" role="group" aria-label={t('Computer layout')}>
    {#each SIDES as side (side)}
      {@const p = at[side]}
      <div class="slot {side}" class:over={drag?.over === side} data-drop={side}>
        {#if p}
          {@render tile(p)}
        {:else}
          <button type="button" class="empty" tabindex="-1" aria-hidden="true" onclick={() => clickSlot(side)} disabled={disabled || !selected}>
            {drag ? t('Drop it on a side') : slotText(side)}
          </button>
        {/if}
      </div>
    {/each}
    <div class="slot me">
      <div class="monitor mine">
        <span class="screen">
          <Icon name="monitor" size={18} />
          <strong class="ellipsis">{me || t('This computer')}</strong>
          <span class="small">{t('This computer')}</span>
        </span>
        <span class="stand" aria-hidden="true"></span>
      </div>
    </div>
  </div>

  {#if unplaced.length || drag}
    <div class="tray" class:over={drag?.over === 'tray'} data-drop="tray">
      <span class="overline">{t('Not placed')}</span>
      <div class="tray-items">
        {#each unplaced as p (p.id)}
          {@render tile(p)}
        {/each}
      </div>
    </div>
  {/if}
</div>

{#if drag && dragged}
  <div class="ghost" style:left="{drag.x}px" style:top="{drag.y}px" aria-hidden="true">
    <Icon name="laptop" size={16} /> {dragged.name}
  </div>
{/if}

<style>
  .editor {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-4);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 150px));
    grid-template-rows: repeat(3, 96px);
    grid-template-areas: '. top .' 'left me right' '. bottom .';
    gap: var(--space-3);
    width: 100%;
    justify-content: center;
  }
  .top { grid-area: top; }
  .left { grid-area: left; }
  .right { grid-area: right; }
  .bottom { grid-area: bottom; }
  .me { grid-area: me; }
  .slot {
    display: grid;
    min-width: 0;
    border-radius: var(--radius-m);
  }
  .empty {
    border: 1.5px dashed var(--border-strong);
    border-radius: var(--radius-m);
    background: transparent;
    color: var(--text-subtle);
    font-size: var(--text-sm);
  }
  .empty:not(:disabled):hover {
    border-color: var(--accent);
    color: var(--accent);
  }
  .slot.over .empty,
  .slot.over {
    border-color: var(--accent);
    background: var(--accent-soft);
    color: var(--accent);
  }
  .monitor {
    display: flex;
    flex-direction: column;
    align-items: center;
    min-width: 0;
    padding: 0;
    border: 0;
    background: transparent;
  }
  .screen {
    flex: 1;
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 2px;
    padding: var(--space-2);
    border-radius: var(--radius-m);
    border: 2px solid var(--border-strong);
    background: var(--surface);
    min-width: 0;
    transition: border-color var(--dur-fast) var(--ease-out), transform var(--dur-fast) var(--ease-out);
  }
  .screen strong {
    max-width: 100%;
    font-size: var(--text-sm);
  }
  .screen .small {
    font-size: var(--text-xs);
    color: var(--text-muted);
  }
  .stand {
    width: 40%;
    height: 6px;
    margin-top: 3px;
    border-radius: 3px;
    background: var(--border-strong);
  }
  .mine .screen {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .mine .screen .small,
  .mine .screen :global(.icon) {
    color: var(--accent);
  }
  .mine .stand {
    background: var(--accent);
  }
  .peer {
    cursor: grab;
    touch-action: none;
  }
  .peer:hover:not(:disabled) .screen,
  .peer.selected .screen {
    border-color: var(--text-muted);
  }
  .peer:focus-visible {
    outline: none;
  }
  .peer:focus-visible .screen {
    box-shadow: 0 0 0 3px var(--focus-ring);
  }
  .peer.offline .screen {
    opacity: 0.7;
    border-style: dashed;
  }
  .peer.identity .screen {
    border-color: var(--danger);
  }
  .peer.dragging {
    opacity: 0.35;
  }
  .peer:disabled {
    cursor: default;
  }
  .tray {
    width: 100%;
    max-width: 480px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-3);
    border-radius: var(--radius-m);
    border: 1.5px dashed var(--border);
  }
  .tray.over {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .tray-items {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: var(--space-3);
    min-height: 40px;
  }
  .tray-items .peer {
    width: 150px;
    height: 90px;
  }
  .ghost {
    position: fixed;
    z-index: 400;
    transform: translate(-50%, -120%);
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-s);
    background: var(--accent);
    color: var(--text-on-accent);
    box-shadow: var(--shadow-3);
    font-weight: 600;
    pointer-events: none;
  }
</style>
