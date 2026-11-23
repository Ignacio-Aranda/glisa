<script lang="ts">
  import { tick } from 'svelte';
  import Dialog from '../../components/Dialog.svelte';
  import Button from '../../components/Button.svelte';
  import Icon from '../../components/Icon.svelte';
  import { shell, errorText } from '../../backend';
  import { app, toast } from '../../state/app.svelte';
  import { controlIcon } from '../../actions';
  import { t } from '../../i18n/index.svelte';
  import type { Device, PictureLayout } from '../../types';

  interface Props {
    open: boolean;
    dev: Device;
    src: string | null;
    onclose: () => void;
  }
  let { open, dev, src, onclose }: Props = $props();

  type Anchors = PictureLayout['anchors'];
  let anchors = $state<Anchors>({});
  let selected = $state<number | null>(null);
  let saving = $state(false);
  let frame: HTMLDivElement | null = $state(null);
  let tray: HTMLDivElement | null = $state(null);
  let dragging = $state<number | null>(null);

  const controls = $derived(dev.controls ?? []);
  const placed = $derived(controls.filter((c) => anchors[String(c.cid)]));
  const unplaced = $derived(controls.filter((c) => !anchors[String(c.cid)]));

  $effect(() => {
    if (!open) return;
    const saved = app.layouts[dev.id]?.anchors ?? {};
    anchors = JSON.parse(JSON.stringify(saved));
    selected = null;
  });

  const pct = (v: number) => Math.round(v * 100);
  const clamp = (v: number) => Math.max(0, Math.min(1, v));

  function fractionAt(x: number, y: number): { x: number; y: number } | null {
    const r = frame?.getBoundingClientRect();
    if (!r || !r.width || !r.height) return null;
    return { x: clamp((x - r.left) / r.width), y: clamp((y - r.top) / r.height) };
  }

  function inside(el: HTMLElement | null, x: number, y: number): boolean {
    const r = el?.getBoundingClientRect();
    return !!r && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
  }

  async function focusMarker(cid: number) {
    await tick();
    frame?.querySelector<HTMLElement>(`[data-marker="${cid}"]`)?.focus();
  }

  async function focusTray(cid: number) {
    await tick();
    tray?.querySelector<HTMLElement>(`[data-chip="${cid}"]`)?.focus();
  }

  function startDrag(e: PointerEvent, cid: number) {
    if (e.button !== 0) return;
    e.preventDefault();
    selected = cid;
    const from = { x: e.clientX, y: e.clientY };
    let moved = false;
    const move = (ev: PointerEvent) => {
      if (!moved && Math.hypot(ev.clientX - from.x, ev.clientY - from.y) < 4) return;
      moved = true;
      dragging = cid;
      if (inside(frame, ev.clientX, ev.clientY) || anchors[String(cid)]) {
        const f = fractionAt(ev.clientX, ev.clientY);
        if (f && !inside(tray, ev.clientX, ev.clientY)) anchors[String(cid)] = f;
      }
    };
    const up = (ev: PointerEvent) => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      dragging = null;
      if (moved && inside(tray, ev.clientX, ev.clientY)) {
        delete anchors[String(cid)];
        void focusTray(cid);
      } else if (anchors[String(cid)]) {
        void focusMarker(cid);
      }
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }

  /** A click on the picture places the selected control there. */
  function clickPicture(e: MouseEvent) {
    if (selected === null || e.target !== e.currentTarget && !(e.target as HTMLElement).matches('img')) return;
    const f = fractionAt(e.clientX, e.clientY);
    if (!f) return;
    anchors[String(selected)] = f;
    void focusMarker(selected);
  }

  function markerKey(e: KeyboardEvent, cid: number) {
    const a = anchors[String(cid)];
    if (!a) return;
    const step = e.shiftKey ? 0.05 : 0.01;
    const moves: Record<string, [number, number]> = {
      ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step],
    };
    if (moves[e.key]) {
      e.preventDefault();
      anchors[String(cid)] = { x: clamp(a.x + moves[e.key][0]), y: clamp(a.y + moves[e.key][1]) };
    } else if (e.key === 'Delete' || e.key === 'Backspace') {
      e.preventDefault();
      delete anchors[String(cid)];
      void focusTray(cid);
    }
  }

  function chipKey(e: KeyboardEvent, cid: number) {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      place(cid);
    }
  }

  function place(cid: number) {
    selected = cid;
    if (!anchors[String(cid)]) anchors[String(cid)] = { x: 0.5, y: 0.5 };
    void focusMarker(cid);
  }

  async function save() {
    saving = true;
    try {
      const layout: PictureLayout = { use_on_pages: app.layouts[dev.id]?.use_on_pages ?? false, anchors: $state.snapshot(anchors) };
      app.layouts[dev.id] = await shell.pictureLayoutSet(dev.id, layout);
      toast(t('Button positions saved'));
      onclose();
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      saving = false;
    }
  }
</script>

<Dialog {open} {onclose} title={t('Button positions')} icon="move" width={980}
  description={t('Drag each button onto the picture, where it is on your device. Or select one and move it with the arrow keys.')}>
  <div class="editor">
    <div class="stage">
      {#if src}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="frame" bind:this={frame} onclick={clickPicture}>
          <img {src} alt={t('Your picture of {name}', { name: dev.name })} draggable="false" />
          {#each placed as c (c.cid)}
            {@const a = anchors[String(c.cid)]}
            <button
              type="button"
              class="marker"
              class:selected={selected === c.cid}
              class:dragging={dragging === c.cid}
              data-marker={c.cid}
              style:left="{a.x * 100}%"
              style:top="{a.y * 100}%"
              aria-label={t('{name}: {x}% from the left, {y}% from the top. Arrow keys move it, Delete takes it off', { name: c.name || t('Button'), x: pct(a.x), y: pct(a.y) })}
              onpointerdown={(e) => startDrag(e, c.cid)}
              onkeydown={(e) => markerKey(e, c.cid)}
              onfocus={() => (selected = c.cid)}
            >
              <span class="dot"></span>
              <span class="tag">{c.name || t('Button')}</span>
            </button>
          {/each}
        </div>
      {:else}
        <p class="muted">{t('Choose a picture first')}</p>
      {/if}
    </div>

    <div class="tray" bind:this={tray} aria-label={t('Buttons')} role="group">
      {#if !controls.length}
        <p class="small muted">{t('This device has no buttons that can be customized.')}</p>
      {:else}
        <span class="overline">{t('Not placed')}</span>
        {#if unplaced.length}
          {#each unplaced as c (c.cid)}
            <button
              type="button"
              class="chip"
              class:selected={selected === c.cid}
              data-chip={c.cid}
              aria-label={t('{name}: not placed. Press Enter to place it, then move it with the arrow keys', { name: c.name || t('Button') })}
              onpointerdown={(e) => startDrag(e, c.cid)}
              onkeydown={(e) => chipKey(e, c.cid)}
              onclick={() => (selected = c.cid)}
            >
              <Icon name={controlIcon(c.cid)} size={16} />
              <span class="ellipsis">{c.name || t('Button')}</span>
            </button>
          {/each}
        {:else}
          <p class="small subtle">{t('Every button is on the picture')}</p>
        {/if}
        {#if placed.length}
          <span class="overline">{t('On the picture')}</span>
          {#each placed as c (c.cid)}
            <button type="button" class="chip placed" class:selected={selected === c.cid} onclick={() => { selected = c.cid; void focusMarker(c.cid); }}>
              <Icon name="check" size={15} />
              <span class="ellipsis">{c.name || t('Button')}</span>
            </button>
          {/each}
        {/if}
        <p class="small subtle help">{t('Buttons you leave off the picture are shown at free spots.')}</p>
      {/if}
    </div>
  </div>
  {#snippet footerStart()}
    <Button variant="ghost" icon="rotate-ccw" disabled={!placed.length} onclick={() => { anchors = {}; selected = null; }}>{t('Reset')}</Button>
  {/snippet}
  {#snippet footer()}
    <Button onclick={onclose}>{t('Cancel')}</Button>
    <Button variant="primary" busy={saving} onclick={save}>{t('Save')}</Button>
  {/snippet}
</Dialog>

<style>
  .editor {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 230px;
    gap: var(--space-5);
    min-height: 0;
  }
  .stage {
    display: grid;
    place-items: center;
    min-height: 300px;
    padding: var(--space-4);
    border-radius: var(--radius-l);
    background: var(--surface-2);
    border: 1px solid var(--border);
  }
  .frame {
    position: relative;
    display: inline-block;
    line-height: 0;
    cursor: crosshair;
    touch-action: none;
  }
  .frame img {
    max-width: 100%;
    max-height: min(56vh, 520px);
    user-select: none;
    -webkit-user-drag: none;
    filter: drop-shadow(0 4px 12px color-mix(in srgb, var(--text) 22%, transparent));
  }
  .marker {
    position: absolute;
    transform: translate(-50%, -50%);
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0;
    border: 0;
    background: transparent;
    cursor: grab;
    line-height: 1.2;
    touch-action: none;
  }
  .dot {
    flex: none;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--accent);
    border: 3px solid var(--bg-elevated);
    box-shadow: 0 0 0 2px var(--accent), var(--shadow-2);
  }
  .tag {
    position: absolute;
    left: 24px;
    white-space: nowrap;
    padding: 2px 8px;
    border-radius: var(--radius-xs);
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    box-shadow: var(--shadow-1);
    font-size: var(--text-xs);
    font-weight: 600;
    color: var(--text);
  }
  .marker.selected .dot,
  .marker:focus-visible .dot {
    box-shadow: 0 0 0 2px var(--accent), 0 0 0 7px var(--focus-ring), var(--shadow-2);
  }
  .marker.selected .tag {
    border-color: var(--accent);
    color: var(--accent);
  }
  .marker:focus-visible {
    outline: none;
  }
  .marker.dragging {
    cursor: grabbing;
  }
  .tray {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }
  .overline {
    margin-top: var(--space-2);
  }
  .chip {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    height: 34px;
    padding: 0 var(--space-3);
    border-radius: var(--radius-s);
    border: 1.5px dashed var(--border-strong);
    background: var(--surface);
    text-align: left;
    cursor: grab;
    touch-action: none;
  }
  .chip.placed {
    border-style: solid;
    border-color: var(--border);
    cursor: pointer;
  }
  .chip.placed :global(.icon) {
    color: var(--success);
  }
  .chip.selected {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .help {
    margin-top: auto;
  }
</style>
