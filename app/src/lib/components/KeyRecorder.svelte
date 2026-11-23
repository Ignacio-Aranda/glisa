<script lang="ts">
  import KeyCaps from './KeyCaps.svelte';
  import Icon from './Icon.svelte';
  import { comboFromEvent } from '../keys';
  import { t } from '../i18n/index.svelte';
  import type { Platform } from '../types';

  interface Props {
    combo: string;
    platform: Platform;
    label: string;
    disabled?: boolean;
    onrecord: (combo: string) => void;
    el?: HTMLButtonElement | null;
  }
  let { combo, platform, label, disabled = false, onrecord, el = $bindable(null) }: Props = $props();
  let recording = $state(false);
  let partial = $state('');

  function start() {
    if (disabled) return;
    recording = true;
    partial = '';
  }

  function stop() {
    recording = false;
    partial = '';
  }

  function onkeydown(e: KeyboardEvent) {
    if (!recording) {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        start();
      }
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    if (e.key === 'Escape' && !e.ctrlKey && !e.altKey && !e.shiftKey && !e.metaKey) {
      stop();
      return;
    }
    const r = comboFromEvent(e);
    partial = r.combo;
    if (r.done) {
      stop();
      onrecord(r.combo);
    }
  }

  function onkeyup(e: KeyboardEvent) {
    if (recording) {
      e.preventDefault();
      partial = comboFromEvent(e).combo;
    }
  }
</script>

<button
  bind:this={el}
  type="button"
  class="recorder"
  class:recording
  aria-label={label}
  aria-pressed={recording}
  {disabled}
  onclick={() => (recording ? stop() : start())}
  {onkeydown}
  {onkeyup}
  onblur={stop}
>
  <Icon name="keyboard" size={18} />
  {#if recording}
    {#if partial}
      <KeyCaps combo={partial} {platform} />
    {:else}
      <span class="hint">{t('Press the key combination…  (Esc to cancel)')}</span>
    {/if}
  {:else if combo}
    <KeyCaps {combo} {platform} />
  {:else}
    <span class="hint">{t('Click, then press a key combination')}</span>
  {/if}
</button>

<style>
  .recorder {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    width: 100%;
    min-height: 56px;
    padding: var(--space-2) var(--space-4);
    border-radius: var(--radius-m);
    border: 1.5px dashed var(--border-strong);
    background: var(--surface-2);
    color: var(--text-muted);
    text-align: left;
    transition: border-color var(--dur-fast) var(--ease-out), background-color var(--dur-fast) var(--ease-out);
  }
  .recorder:hover:not(:disabled) {
    border-color: var(--text-subtle);
  }
  .recording {
    border-style: solid;
    border-color: var(--accent);
    background: var(--accent-soft);
    color: var(--accent);
  }
  .hint {
    font-size: var(--text-sm);
  }
  .recorder:disabled {
    opacity: 0.5;
  }
</style>
