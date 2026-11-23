<script lang="ts" module>
  export interface Spot {
    cid: number;
    name: string;
    /** Second line of the callout (the action). */
    detail?: string;
    /** The action is not the default one. */
    custom?: boolean;
  }

  export interface Box { x: number; y: number; w: number; h: number }

  /** Spreads label centres so that they keep `gap` apart inside [lo, hi], staying close to where they want to be. */
  export function spread(desired: number[], gap: number, lo: number, hi: number): number[] {
    const order = desired.map((v, i) => [v, i] as const).sort((a, b) => a[0] - b[0]);
    const pos = order.map(([v]) => Math.max(lo, Math.min(hi, v)));
    for (let i = 1; i < pos.length; i++) pos[i] = Math.max(pos[i], pos[i - 1] + gap);
    const over = pos.length ? pos[pos.length - 1] - hi : 0;
    if (over > 0) {
      pos[pos.length - 1] -= over;
      for (let i = pos.length - 2; i >= 0; i--) pos[i] = Math.min(pos[i], pos[i + 1] - gap);
    }
    const out = new Array<number>(desired.length);
    order.forEach(([, i], k) => (out[i] = pos[k]));
    return out;
  }
</script>

<script lang="ts">
  import { t } from '../i18n/index.svelte';
  import type { PictureLayout } from '../types';

  interface Props {
    /** The device's picture (the official product photo or the user's own) and where the
     *  user placed each control on it. Controls without a position get no hotspot. */
    picture: { src: string; layout: PictureLayout };
    spots?: Spot[];
    selected?: number | null;
    hovered?: number | null;
    /** Labels with leader lines beside the picture (when there is room). */
    callouts?: boolean;
    disabled?: boolean;
    label?: string;
    onselect?: (cid: number) => void;
    onhover?: (cid: number | null) => void;
  }
  let {
    picture, spots = [], selected = null, hovered = null, callouts = false, disabled = false, label,
    onselect, onhover,
  }: Props = $props();

  let natural = $state({ w: 0, h: 0 });
  /** The picture's pixels: the coordinate box of the saved positions (fractions of it). */
  const vb = $derived({ x: 0, y: 0, w: natural.w || 400, h: natural.h || 300 });

  let width = $state(0);
  let height = $state(0);
  let focusIndex = $state(0);
  let buttons: HTMLButtonElement[] = $state([]);

  const LABEL_W = 150;
  const LABEL_H = 46;
  const GAP = 18;

  /** Where each placed control sits on the picture. */
  const points = $derived.by(() => {
    const out = new Map<number, { x: number; y: number }>();
    for (const s of spots) {
      const own = picture.layout.anchors[String(s.cid)];
      if (own) out.set(s.cid, { x: own.x * vb.w, y: own.y * vb.h });
    }
    return out;
  });

  const withLabels = $derived(callouts && points.size > 0 && width >= 560);

  /** The picture's box inside the canvas (px): as large as the height allows while the
   *  callouts still fit beside it. */
  const box = $derived.by((): Box => {
    const ah = Math.max(16, height);
    let scale = Math.min(Math.max(16, width) / vb.w, ah / vb.h);
    if (withLabels) {
      const room = width / 2 - (LABEL_W + GAP);
      scale = Math.max(Math.min(scale, room / (vb.w / 2)), 0.2);
    }
    const w = vb.w * scale;
    const h = vb.h * scale;
    return { x: (width - w) / 2, y: (height - h) / 2, w, h };
  });

  function toPx(p: { x: number; y: number }): { x: number; y: number } {
    return { x: box.x + ((p.x - vb.x) / vb.w) * box.w, y: box.y + ((p.y - vb.y) / vb.h) * box.h };
  }

  /** Where the callout columns end (px): beside what is drawn, not the whole frame. */
  const columns = $derived.by(() => {
    const x0 = vb.x;
    const x1 = vb.x + vb.w;
    const left = toPx({ x: x0, y: vb.y }).x - GAP;
    const right = toPx({ x: x1, y: vb.y }).x + GAP;
    return { left: Math.max(LABEL_W, left), right: Math.min(width - LABEL_W, right) };
  });

  const placed = $derived.by(() => {
    const items = spots.filter((s) => points.has(s.cid)).map((s) => {
      const p = toPx(points.get(s.cid) ?? { x: vb.w / 2, y: vb.h / 2 });
      return { spot: s, p, side: p.x < box.x + box.w / 2 ? 'left' : 'right' as 'left' | 'right', ly: p.y };
    });
    if (withLabels) {
      for (const side of ['left', 'right'] as const) {
        const group = items.filter((i) => i.side === side);
        const ys = spread(group.map((i) => i.p.y), LABEL_H + 8, LABEL_H / 2 + 4, height - LABEL_H / 2 - 4);
        group.forEach((i, k) => (i.ly = ys[k]));
      }
    }
    return items;
  });

  function onkeydown(e: KeyboardEvent, i: number) {
    const n = placed.length;
    if (!n) return;
    let next = -1;
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') next = (i + 1) % n;
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') next = (i - 1 + n) % n;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = n - 1;
    if (next < 0) return;
    e.preventDefault();
    focusIndex = next;
    buttons[next]?.focus();
    onhover?.(placed[next].spot.cid);
  }
</script>

<div
  class="canvas"
  class:disabled
  bind:clientWidth={width}
  bind:clientHeight={height}
  role={spots.length ? 'group' : 'img'}
  aria-label={label ?? (spots.length ? t('Device buttons') : undefined)}
>
  <div class="art" style:left="{box.x}px" style:top="{box.y}px" style:width="{box.w}px" style:height="{box.h}px" aria-hidden="true">
    <img src={picture.src} alt="" draggable="false" onload={(e) => {
      const img = e.currentTarget as HTMLImageElement;
      natural = { w: img.naturalWidth, h: img.naturalHeight };
    }} />
  </div>

  {#if withLabels}
    <svg class="leaders" width={width} height={height} aria-hidden="true">
      {#each placed as item (item.spot.cid)}
        {@const lx = item.side === 'left' ? columns.left : columns.right}
        {@const active = item.spot.cid === selected || item.spot.cid === hovered}
        <path
          class:active
          d="M {item.p.x} {item.p.y} L {(item.p.x + lx) / 2} {item.ly} L {lx} {item.ly}"
          fill="none"
        />
      {/each}
    </svg>
  {/if}

  {#each placed as item, i (item.spot.cid)}
    {@const active = item.spot.cid === selected}
    {@const hot = item.spot.cid === hovered}
    <button
      bind:this={buttons[i]}
      type="button"
      class="spot"
      class:active
      class:hot
      class:custom={item.spot.custom}
      style:left="{item.p.x}px"
      style:top="{item.p.y}px"
      tabindex={i === focusIndex ? 0 : -1}
      aria-label={item.spot.detail ? `${item.spot.name}: ${item.spot.detail}` : item.spot.name}
      aria-pressed={active}
      title={withLabels ? undefined : item.spot.name}
      disabled={disabled || !onselect}
      onclick={() => onselect?.(item.spot.cid)}
      onmouseenter={() => onhover?.(item.spot.cid)}
      onmouseleave={() => onhover?.(null)}
      onfocus={() => { focusIndex = i; onhover?.(item.spot.cid); }}
      onblur={() => onhover?.(null)}
      onkeydown={(e) => onkeydown(e, i)}
    >
      <span class="ring"></span>
    </button>
    {#if withLabels}
      <button
        type="button"
        class="callout {item.side}"
        class:active
        class:hot
        tabindex="-1"
        aria-hidden="true"
        style:top="{item.ly}px"
        style:left={item.side === 'left' ? `${columns.left - LABEL_W}px` : `${columns.right}px`}
        style:width="{LABEL_W}px"
        disabled={disabled || !onselect}
        onclick={() => onselect?.(item.spot.cid)}
        onmouseenter={() => onhover?.(item.spot.cid)}
        onmouseleave={() => onhover?.(null)}
      >
        <span class="name ellipsis">{item.spot.name}</span>
        {#if item.spot.detail}<span class="detail ellipsis" class:custom={item.spot.custom}>{item.spot.detail}</span>{/if}
      </button>
    {/if}
  {/each}
</div>

<style>
  .canvas {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 120px;
  }
  .art {
    position: absolute;
    color: var(--text-muted);
    transition: opacity var(--dur-med) var(--ease-out);
  }
  .art img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: contain;
    /* transparent pictures keep their outline on light and dark backgrounds */
    filter: drop-shadow(0 4px 12px color-mix(in srgb, var(--text) 22%, transparent));
  }
  .disabled .art {
    opacity: 0.45;
  }
  .leaders {
    position: absolute;
    inset: 0;
    pointer-events: none;
    overflow: visible;
  }
  .leaders path {
    stroke: var(--border-strong);
    stroke-width: 1;
    transition: stroke var(--dur-fast) var(--ease-out);
  }
  .leaders path.active {
    stroke: var(--accent);
    stroke-width: 1.75;
  }
  .spot {
    position: absolute;
    width: 30px;
    height: 30px;
    margin: -15px 0 0 -15px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    display: grid;
    place-items: center;
  }
  .ring {
    position: relative;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: var(--surface);
    border: 2px solid var(--accent);
    box-shadow: var(--shadow-1);
    transition: width var(--dur-fast) var(--ease-spring), height var(--dur-fast) var(--ease-spring),
      box-shadow var(--dur-fast) var(--ease-out), background-color var(--dur-fast) var(--ease-out);
  }
  .spot:hover:not(:disabled) .ring,
  .spot.hot .ring,
  .spot:focus-visible .ring {
    width: 16px;
    height: 16px;
    box-shadow: 0 0 0 6px var(--accent-soft), var(--shadow-1);
  }
  .spot:focus-visible {
    outline: none;
  }
  .spot:focus-visible .ring {
    box-shadow: 0 0 0 6px var(--accent-soft), 0 0 0 9px var(--focus-ring);
  }
  .spot.active .ring {
    width: 16px;
    height: 16px;
    background: var(--accent);
    box-shadow: 0 0 0 6px var(--accent-soft), var(--shadow-1);
    animation: pulse var(--dur-slow) var(--ease-out) 1;
  }
  .spot.active .ring::after {
    content: '';
    position: absolute;
    inset: 0;
    margin: auto;
    width: 4px;
    height: 4px;
    border-radius: 50%;
    background: var(--text-on-accent);
  }
  @keyframes pulse {
    from { box-shadow: 0 0 0 0 var(--accent-soft), var(--shadow-1); }
    60% { box-shadow: 0 0 0 10px var(--accent-soft), var(--shadow-1); }
    to { box-shadow: 0 0 0 6px var(--accent-soft), var(--shadow-1); }
  }
  .spot:disabled {
    cursor: default;
  }
  .spot:disabled .ring {
    border-color: var(--text-subtle);
    box-shadow: none;
  }
  .callout {
    position: absolute;
    transform: translateY(-50%);
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 1px;
    padding: 6px 10px;
    border-radius: var(--radius-m);
    border: 1px solid var(--border);
    background: var(--bg-elevated);
    box-shadow: var(--shadow-1);
    text-align: left;
    transition: border-color var(--dur-fast) var(--ease-out), background-color var(--dur-fast) var(--ease-out);
  }
  .callout.right {
    align-items: flex-start;
  }
  .callout.left {
    align-items: flex-end;
    text-align: right;
  }
  .callout:hover:not(:disabled),
  .callout.hot {
    border-color: var(--border-strong);
    background: var(--surface-hover);
  }
  .callout.active {
    border-color: var(--accent);
  }
  .callout.active .name {
    color: var(--accent);
  }
  .name {
    font-weight: 600;
    font-size: var(--text-sm);
    max-width: 100%;
  }
  .detail {
    font-size: var(--text-xs);
    color: var(--text-muted);
    max-width: 100%;
  }
  .detail.custom {
    color: var(--accent);
  }
  .callout:disabled {
    cursor: default;
  }
</style>
