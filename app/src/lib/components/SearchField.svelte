<script lang="ts">
  import Icon from './Icon.svelte';
  import { t } from '../i18n/index.svelte';

  interface Props {
    value: string;
    placeholder: string;
    label: string;
    disabled?: boolean;
    el?: HTMLInputElement | null;
    onkeydown?: (e: KeyboardEvent) => void;
  }
  let { value = $bindable(''), placeholder, label, disabled = false, el = $bindable(null), onkeydown }: Props = $props();
</script>

<div class="search">
  <span class="glass"><Icon name="search" size={16} /></span>
  <input bind:this={el} type="search" class="text-input" bind:value {placeholder} aria-label={label} {disabled} {onkeydown} />
  {#if value}
    <button type="button" class="clear" aria-label={t('Clear')} title={t('Clear')} onclick={() => { value = ''; el?.focus(); }}>
      <Icon name="x" size={14} />
    </button>
  {/if}
</div>

<style>
  .search {
    position: relative;
    display: flex;
    align-items: center;
  }
  .glass {
    position: absolute;
    left: 11px;
    color: var(--text-subtle);
    display: grid;
    pointer-events: none;
  }
  input {
    padding-left: 34px;
    padding-right: 32px;
  }
  input::-webkit-search-cancel-button {
    display: none;
  }
  .clear {
    position: absolute;
    right: 6px;
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border: 0;
    border-radius: 50%;
    background: var(--surface-2);
    color: var(--text-muted);
  }
  .clear:hover {
    color: var(--text);
  }
</style>
