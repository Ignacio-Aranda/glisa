<script lang="ts">
  import { tick } from 'svelte';
  import { fade } from 'svelte/transition';
  import DeviceArt, { type Spot } from '../../components/DeviceArt.svelte';
  import DeviceIcon from '../../components/DeviceIcon.svelte';
  import Banner from '../../components/Banner.svelte';
  import Button from '../../components/Button.svelte';
  import Overview from './buttons/Overview.svelte';
  import Picker from './buttons/Picker.svelte';
  import GesturePad from './buttons/GesturePad.svelte';
  import ShortcutEditor from './buttons/ShortcutEditor.svelte';
  import CommandEditor from './buttons/CommandEditor.svelte';
  import {
    DEFAULT_ACTION, DISABLED_ACTION, SLOTS, clone, controlIcon, defaultGestures, describe, isDefault, sameAction,
    short, slotAction, slotLabel, slotName,
  } from '../../actions';
  import { api, errorText } from '../../backend';
  import { app, navigate, pagePicture, platform, refreshDevice, toast } from '../../state/app.svelte';
  import { confirm } from '../../state/confirm.svelte';
  import { bt, t } from '../../i18n/index.svelte';
  import type { Action, Control, Device, GestureAction, GestureSlot, SimpleAction } from '../../types';

  interface Props { dev: Device }
  let { dev }: Props = $props();

  type View = 'overview' | 'picker' | 'gesture' | 'shortcut' | 'command';
  const PENDING_MS = 6000;

  let view = $state<View>('overview');
  let cid = $state<number | null>(null);
  let slot = $state<GestureSlot | null>(null);
  let hovered = $state<number | null>(null);
  let pending = $state<Record<number, { action: Action; until: number }>>({});
  let editorInitial = $state('');
  let editorKey = $state(0);
  let picker: Picker | null = $state(null);
  let pad: GesturePad | null = $state(null);
  let shortcut: ShortcutEditor | null = $state(null);
  let command: CommandEditor | null = $state(null);
  let panel: HTMLElement | null = $state(null);
  let shownId = '';

  const os = $derived(platform());
  const presets = $derived(app.presets);
  const online = $derived(dev.online);
  const gestureDefaults = $derived(app.gestureDefaults ?? defaultGestures(os));

  /** The snapshot's controls with not-yet-confirmed changes on top (no flicker). */
  const controls = $derived.by((): Control[] => {
    const now = Date.now();
    return (dev.controls ?? []).map((c) => {
      const p = pending[c.cid];
      if (p && now < p.until && !sameAction(p.action, c.action ?? DEFAULT_ACTION)) return { ...c, action: p.action };
      return { ...c, action: c.action ?? DEFAULT_ACTION };
    });
  });

  const control = $derived(cid === null ? null : controls.find((c) => c.cid === cid) ?? null);

  // A different device, or the control went away: back to the list.
  $effect(() => {
    if (dev.id !== shownId) {
      shownId = dev.id;
      pending = {};
      view = 'overview';
      cid = null;
      slot = null;
    } else if (cid !== null && !control) {
      view = 'overview';
      cid = null;
      slot = null;
    }
  });

  // Stopped being a gesture button elsewhere (another window, the CLI): leave the pad.
  $effect(() => {
    if (control && control.action.type !== 'gesture' && (view === 'gesture' || slot !== null) && view !== 'picker') {
      slot = null;
      view = 'picker';
    }
  });

  const spots = $derived<Spot[]>(controls.map((c) => ({
    cid: c.cid, name: c.name || t('Button'), detail: describe(c.action, presets), custom: !isDefault(c.action),
  })));

  async function show(next: View) {
    view = next;
    await tick();
    if (next === 'picker') picker?.focusSearch();
    else if (next === 'gesture') pad?.focusFirst();
    else if (next === 'shortcut') shortcut?.focus();
    else if (next === 'command') command?.focus();
  }

  function open(c: number) {
    const target = controls.find((x) => x.cid === c);
    if (!target) return;
    cid = c;
    slot = null;
    picker?.reset();
    void show(target.action.type === 'gesture' ? 'gesture' : 'picker').then(() => picker?.reveal());
  }

  async function overview() {
    const came = cid;
    cid = null;
    slot = null;
    view = 'overview';
    await tick();
    panel?.querySelector<HTMLElement>(`[data-cid="${came}"]`)?.focus();
  }

  async function apply(c: Control, action: Action | null, ok: string) {
    if (!online) return;
    pending = { ...pending, [c.cid]: { action: action ? clone(action) : DEFAULT_ACTION, until: Date.now() + PENDING_MS } };
    try {
      await api('set_button', { id: dev.id, cid: c.cid, action });
      if (ok) toast(ok);
    } catch (e) {
      pending = {};
      toast(errorText(e), 'error');
    }
    await refreshDevice(dev.id);
  }

  function gestureOf(c: Control): GestureAction {
    const src = c.action.type === 'gesture' ? c.action : gestureDefaults;
    const out: GestureAction = { type: 'gesture' };
    for (const s of SLOTS) out[s] = clone(slotAction(src, s));
    return out;
  }

  function choose(action: SimpleAction | null) {
    const c = control;
    if (!c || !online) return;
    if (slot !== null) {
      const s = slot;
      const g = gestureOf(c);
      const next = action ? clone(action) : DISABLED_ACTION;
      const changed = !sameAction(g[s], next) || c.action.type !== 'gesture';
      g[s] = next;
      slot = null;
      if (changed) void apply(c, g, `${c.name} · ${slotName(s)} → ${short(slotLabel(next, presets), 30)}`);
      void show('gesture');
      return;
    }
    if (!sameAction(action ?? DEFAULT_ACTION, c.action)) {
      void apply(c, action, `${c.name} → ${short(describe(action, presets))}`);
    }
  }

  function chooseGesture() {
    const c = control;
    if (!c || !online) return;
    if (c.action.type !== 'gesture') void apply(c, clone(gestureDefaults), `${c.name} → ${bt('Gesture button')}`);
    void show('gesture');
  }

  function resetGestures() {
    const c = control;
    if (c && online) void apply(c, clone(gestureDefaults), `${c.name} → ${t('Default gestures')}`);
  }

  async function resetAll() {
    const custom = controls.filter((c) => !isDefault(c.action));
    if (!custom.length || !online) return;
    const ok = await confirm({
      title: t('Reset all buttons?'),
      text: dev.name ? t('The buttons on {name} will go back to their original function.', { name: dev.name })
        : t('The buttons on this device will go back to their original function.'),
      confirm: t('Reset'), danger: true, icon: 'rotate-ccw',
    });
    if (!ok) return;
    for (const [i, c] of custom.entries()) await apply(c, null, i === custom.length - 1 ? t('Buttons reset') : '');
  }

  function pickerBack() {
    if (slot !== null) {
      slot = null;
      void show('gesture');
    } else {
      void overview();
    }
  }

  function openSlot(s: GestureSlot) {
    if (!control || !online) return;
    slot = s;
    picker?.reset();
    void show('picker').then(() => picker?.reveal());
  }

  function changeFromGestures() {
    slot = null;
    picker?.reset();
    void show('picker').then(() => picker?.reveal());
  }

  function slotTitle(s: GestureSlot): string {
    switch (s) {
      case 'tap': return t('Press without moving');
      case 'up': return t('Move up');
      case 'down': return t('Move down');
      case 'left': return t('Move left');
      case 'right': return t('Move right');
    }
  }

  function contextText(c: Control): string {
    const name = c.name || t('Button');
    return slot !== null ? `${name} · ${slotTitle(slot).toLocaleLowerCase()}` : t('For “{name}”', { name });
  }

  function currentAction(c: Control): Action {
    return slot !== null ? slotAction(c.action, slot) : c.action;
  }

  function openShortcut() {
    const c = control;
    if (!c) return;
    const cur = currentAction(c);
    editorInitial = cur.type === 'keys' ? cur.combo : '';
    editorKey += 1;
    void show('shortcut');
  }

  function openCommand() {
    const c = control;
    if (!c) return;
    const cur = currentAction(c);
    editorInitial = cur.type === 'command' ? cur.cmd : '';
    editorKey += 1;
    void show('command');
  }

  function assignCustom(action: SimpleAction) {
    const inSlot = slot !== null;
    choose(action);
    if (!inSlot) void show('picker').then(() => picker?.reveal());
  }

  function onpanelkey(e: KeyboardEvent) {
    if (e.key !== 'Escape' || e.defaultPrevented || view === 'overview') return;
    e.preventDefault();
    if (view === 'picker') pickerBack();
    else if (view === 'gesture') void overview();
    else void show('picker');
  }

  const pickerProps = $derived.by(() => {
    const c = control;
    if (!c) return null;
    if (slot !== null) {
      const cur = slotAction(c.action, slot);
      const none = cur.type === 'disabled' || cur.type === 'default';
      return {
        back: t('Back to gestures'), title: slotTitle(slot), icon: { tap: 'circle-dot', up: 'arrow-up', down: 'arrow-down', left: 'arrow-left', right: 'arrow-right' }[slot],
        current: cur as Action, currentText: slotLabel(cur, presets), customised: !none, slotMode: true, gestures: false, mouseOnly: false,
      };
    }
    return {
      back: t('All buttons'), title: c.name || t('Button'), icon: controlIcon(c.cid), current: c.action,
      currentText: describe(c.action, presets), customised: !isDefault(c.action), slotMode: false,
      gestures: !!c.gestures, mouseOnly: !c.divertable,
    };
  });

  const keyboard = $derived(dev.kind === 'keyboard');
  const picture = $derived(pagePicture(dev.id));
  /** Buttons not placed on the picture yet (they get no hotspot on it). */
  const unplaced = $derived(picture ? controls.filter((c) => !picture.layout.anchors[String(c.cid)]).length : 0);
</script>

{#if !controls.length}
  <div class="showcase">
    {#if picture}
      <div class="stage"><DeviceArt {picture} disabled={!online} label={dev.name} /></div>
    {:else}
      <DeviceIcon kind={dev.kind} family={dev.family} size={96} dimmed={!online} />
    {/if}
    <div class="note">
      <h3>{keyboard ? t("Keys can't be configured yet") : t('No configurable buttons')}</h3>
      <p class="muted">{keyboard ? t("The keys of this keyboard can't be customized from {app} yet.") : t('This device has no buttons that can be customized.')}</p>
    </div>
  </div>
{:else}
  <div class="layout" class:list-only={!picture}>
    {#if !picture && !online}
      <Banner kind="warn" title={t('Device disconnected')} text={t('Turn it on or move it to wake it up. You can change its buttons once it reconnects.')} />
    {/if}
    {#if picture}
    <div class="left">
      {#if !online}
        <Banner kind="warn" title={t('Device disconnected')} text={t('Turn it on or move it to wake it up. You can change its buttons once it reconnects.')} />
      {/if}
      <div class="stage">
        <DeviceArt
          {picture}
          {spots}
          selected={cid}
          {hovered}
          callouts
          disabled={!online}
          onselect={open}
          onhover={(c) => (hovered = c)}
        />
      </div>
      {#if picture && unplaced}
        <p class="hint small place">
          <span class="muted">{picture.official
            ? t('Place the buttons on the product photo to see them on it.')
            : t('Place the buttons on your picture to see them on it.')}</span>
          <Button size="sm" variant="ghost" icon="move" onclick={() => navigate({ page: 'device', id: dev.id, tab: 'settings' })}>{t('Place buttons…')}</Button>
        </p>
      {:else}
        <p class="hint subtle small">{t('Use the arrow keys to move between the buttons and Enter to choose one')}</p>
      {/if}
    </div>
    {/if}

    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <aside class="panel" bind:this={panel} onkeydown={onpanelkey} aria-label={t('Buttons')}>
      {#key view}
        <div class="view" in:fade={{ duration: 120 }}>
          {#if view === 'overview' || !control}
            <Overview {controls} {presets} {online} {hovered} onopen={open} onhover={(c) => (hovered = c)} onresetall={resetAll} />
          {:else if view === 'picker' && pickerProps}
            <Picker
              bind:this={picker}
              {...pickerProps}
              {online}
              {presets}
              platform={os}
              onback={pickerBack}
              onchoose={choose}
              ongesture={chooseGesture}
              onshortcut={openShortcut}
              oncommand={openCommand}
            />
          {:else if view === 'gesture'}
            <GesturePad
              bind:this={pad}
              title={control.name || t('Button')}
              action={control.action}
              defaults={gestureDefaults}
              {presets}
              {online}
              onback={overview}
              onslot={openSlot}
              onchange={changeFromGestures}
              onreset={resetGestures}
            />
          {:else if view === 'shortcut'}
            {#key editorKey}
              <ShortcutEditor bind:this={shortcut} context={contextText(control)} initial={editorInitial} {online} platform={os} {presets}
                onback={() => show('picker')} onassign={(combo) => assignCustom({ type: 'keys', combo })} />
            {/key}
          {:else if view === 'command'}
            {#key editorKey}
              <CommandEditor bind:this={command} context={contextText(control)} initial={editorInitial} {online} platform={os}
                onback={() => show('picker')} onassign={(cmd) => assignCustom({ type: 'command', cmd })} />
            {/key}
          {/if}
        </div>
      {/key}
    </aside>
  </div>
{/if}

<style>
  .layout {
    flex: 1;
    display: grid;
    grid-template-columns: minmax(0, 1fr) 340px;
    gap: var(--space-6);
    padding: var(--space-5) var(--space-7) var(--space-6);
    min-height: 0;
    height: calc(100vh - 150px);
    min-height: 440px;
  }
  /* No picture: the list of buttons on its own. */
  .layout.list-only {
    display: flex;
    flex-direction: column;
    max-width: 820px;
    gap: var(--space-4);
  }
  .list-only .panel {
    flex: 1;
  }
  .left {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    min-width: 0;
    min-height: 0;
  }
  .stage {
    flex: 1;
    min-height: 260px;
    position: relative;
  }
  /* The drawing fills its stage whatever the flex context resolves 100 % to. */
  .stage > :global(.canvas) {
    position: absolute;
    inset: 0;
    width: auto;
    height: auto;
  }
  .hint {
    text-align: center;
  }
  .panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    padding: var(--space-5);
    border-radius: var(--radius-l);
    background: var(--surface);
    border: 1px solid var(--border);
    box-shadow: var(--shadow-1);
  }
  .view {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .showcase {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-4);
    padding: var(--space-6) var(--space-7);
    height: calc(100vh - 150px);
    min-height: 400px;
  }
  .showcase .stage {
    width: min(760px, 100%);
    flex: 1;
  }
  .note {
    text-align: center;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  @media (max-width: 1000px) {
    .layout {
      grid-template-columns: minmax(0, 1fr) 330px;
      padding-inline: var(--space-5);
      gap: var(--space-4);
    }
  }
  .hint.place {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    flex-wrap: wrap;
  }
</style>
