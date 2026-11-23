<script lang="ts">
  import Icon from '../../../components/Icon.svelte';
  import Button from '../../../components/Button.svelte';
  import { controlIcon, describe, isDefault } from '../../../actions';
  import { t } from '../../../i18n/index.svelte';
  import type { Control, PresetCategory } from '../../../types';

  interface Props {
    controls: Control[];
    presets: PresetCategory[];
    online: boolean;
    hovered: number | null;
    onopen: (cid: number) => void;
    onhover: (cid: number | null) => void;
    onresetall: () => void;
  }
  let { controls, presets, online, hovered, onopen, onhover, onresetall }: Props = $props();
  const anyCustom = $derived(controls.some((c) => !isDefault(c.action)));
</script>

<div class="overview">
  <div>
    <h2>{t('Buttons')}</h2>
    <p class="muted small">{t('Choose a button to change what it does')}</p>
  </div>
  <ul class="rows scroll">
    {#each controls as c (c.cid)}
      {@const custom = !isDefault(c.action)}
      <li>
        <button
          type="button"
          class="row"
          class:hot={hovered === c.cid}
          disabled={!online}
          data-cid={c.cid}
          onclick={() => onopen(c.cid)}
          onmouseenter={() => onhover(c.cid)}
          onmouseleave={() => onhover(null)}
          onfocus={() => onhover(c.cid)}
          onblur={() => onhover(null)}
        >
          <span class="icon"><Icon name={controlIcon(c.cid)} size={18} /></span>
          <span class="text">
            <span class="name ellipsis">{c.name || t('Button')}</span>
            <span class="action ellipsis" class:custom>{describe(c.action, presets)}</span>
          </span>
          <Icon name="chevron-right" size={16} class="chev" />
        </button>
      </li>
    {/each}
  </ul>
  {#if anyCustom && online}
    <div class="foot">
      <Button variant="link" icon="rotate-ccw" title={t('Return every button to its original function')} onclick={onresetall}>{t('Reset all')}</Button>
    </div>
  {/if}
</div>

<style>
  .overview {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    min-height: 0;
    flex: 1;
  }
  .rows {
    list-style: none;
    margin: 0 calc(-1 * var(--space-2));
    padding: 0 var(--space-2);
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-height: 0;
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    width: 100%;
    padding: var(--space-2) var(--space-3);
    border: 0;
    border-radius: var(--radius-m);
    background: transparent;
    text-align: left;
    transition: background-color var(--dur-fast) var(--ease-out);
  }
  .row:hover:not(:disabled),
  .row.hot:not(:disabled) {
    background: var(--surface-hover);
  }
  .row:disabled {
    opacity: 0.55;
  }
  .icon {
    display: grid;
    place-items: center;
    flex: none;
    width: 36px;
    height: 36px;
    border-radius: var(--radius-s);
    background: var(--surface-2);
    color: var(--text-muted);
  }
  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .name {
    font-weight: 560;
  }
  .action {
    font-size: var(--text-sm);
    color: var(--text-muted);
  }
  .action.custom {
    color: var(--accent);
    font-weight: 560;
  }
  .row :global(.chev) {
    color: var(--text-subtle);
  }
  .foot {
    border-top: 1px solid var(--border);
    padding-top: var(--space-2);
  }
</style>
