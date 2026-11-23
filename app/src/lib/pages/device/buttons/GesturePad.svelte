<script lang="ts">
  import Icon from '../../../components/Icon.svelte';
  import Button from '../../../components/Button.svelte';
  import PanelHead from './PanelHead.svelte';
  import { slotAction, slotLabel, slotName, sameAction } from '../../../actions';
  import { t } from '../../../i18n/index.svelte';
  import type { Action, GestureAction, GestureSlot, PresetCategory } from '../../../types';

  interface Props {
    title: string;
    action: Action;
    defaults: GestureAction;
    presets: PresetCategory[];
    online: boolean;
    onback: () => void;
    onslot: (slot: GestureSlot) => void;
    onchange: () => void;
    onreset: () => void;
  }
  let { title, action, defaults, presets, online, onback, onslot, onchange, onreset }: Props = $props();

  const gesture = $derived(action.type === 'gesture' ? action : defaults);
  const pristine = $derived((['tap', 'up', 'down', 'left', 'right'] as const)
    .every((s) => sameAction(slotAction(gesture, s), slotAction(defaults, s))));
  const ICONS: Record<GestureSlot, string> = { tap: 'circle-dot', up: 'arrow-up', down: 'arrow-down', left: 'arrow-left', right: 'arrow-right' };
  const order: GestureSlot[] = ['up', 'left', 'tap', 'right', 'down'];
  let first: HTMLButtonElement | null = $state(null);

  export function focusFirst() {
    first?.focus();
  }
</script>

<div class="gestures">
  <PanelHead back={t('All buttons')} {onback} {title} icon="move" iconTone="accent">
    <p class="small muted">{t('Hold the button down and move the mouse. Choose what each direction does')}</p>
  </PanelHead>

  <div class="pad">
    {#each order as slot (slot)}
      {@const a = slotAction(gesture, slot)}
      {@const none = a.type === 'disabled' || a.type === 'default'}
      {#if slot === 'tap'}
        <button bind:this={first} type="button" class="slot {slot}" class:none disabled={!online} onclick={() => onslot(slot)}
          aria-label="{slotName(slot)}: {slotLabel(a, presets)}">
          <span class="dir"><Icon name={ICONS[slot]} size={14} /> {slotName(slot)}</span>
          <span class="what">{slotLabel(a, presets)}</span>
        </button>
      {:else}
        <button type="button" class="slot {slot}" class:none disabled={!online} onclick={() => onslot(slot)}
          aria-label="{slotName(slot)}: {slotLabel(a, presets)}">
          <span class="dir"><Icon name={ICONS[slot]} size={14} /> {slotName(slot)}</span>
          <span class="what">{slotLabel(a, presets)}</span>
        </button>
      {/if}
    {/each}
  </div>

  <div class="foot">
    {#if online}
      <Button variant="link" icon="pencil" title={t('Stop using gestures on this button and choose another action')} onclick={onchange}>{t('Use another action')}</Button>
      {#if !pristine}
        <Button variant="link" icon="rotate-ccw" onclick={onreset}>{t('Reset gestures')}</Button>
      {/if}
    {/if}
  </div>
</div>

<style>
  .gestures {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    flex: 1;
    min-height: 0;
  }
  .pad {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    grid-template-areas: '. up .' 'left tap right' '. down .';
    gap: var(--space-2);
  }
  .up { grid-area: up; }
  .left { grid-area: left; }
  .tap { grid-area: tap; }
  .right { grid-area: right; }
  .down { grid-area: down; }
  .slot {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-1);
    min-height: 74px;
    padding: var(--space-2);
    border-radius: var(--radius-m);
    border: 1px solid var(--border);
    background: var(--surface-2);
    text-align: center;
    transition: border-color var(--dur-fast) var(--ease-out), background-color var(--dur-fast) var(--ease-out);
  }
  .slot:hover:not(:disabled) {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .tap {
    border-color: color-mix(in srgb, var(--accent) 45%, transparent);
  }
  .dir {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: var(--text-xs);
    font-weight: 650;
    color: var(--text-subtle);
  }
  .what {
    font-size: var(--text-sm);
    font-weight: 560;
    line-height: 1.25;
    overflow-wrap: anywhere;
  }
  .none .what {
    color: var(--text-subtle);
    font-weight: 450;
  }
  .slot:disabled {
    opacity: 0.55;
  }
  .foot {
    margin-top: auto;
    display: flex;
    justify-content: space-between;
    gap: var(--space-3);
    border-top: 1px solid var(--border);
    padding-top: var(--space-2);
  }
</style>
