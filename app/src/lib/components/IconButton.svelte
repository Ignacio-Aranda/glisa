<script lang="ts">
  import Icon from './Icon.svelte';

  interface Props {
    icon: string;
    label: string;
    tooltip?: string;
    size?: number;
    iconSize?: number;
    disabled?: boolean;
    active?: boolean;
    tone?: 'default' | 'danger';
    onclick?: (e: MouseEvent) => void;
    el?: HTMLButtonElement | null;
    class?: string;
    [rest: string]: unknown;
  }
  let {
    icon, label, tooltip, size = 34, iconSize = 18, disabled = false, active = false, tone = 'default',
    onclick, el = $bindable(null), class: cls = '', ...rest
  }: Props = $props();
</script>

<button
  bind:this={el}
  type="button"
  class="ib {tone} {cls}"
  class:active
  style:width="{size}px"
  style:height="{size}px"
  aria-label={label}
  title={tooltip ?? label}
  {disabled}
  {onclick}
  {...rest}
>
  <Icon name={icon} size={iconSize} />
</button>

<style>
  .ib {
    display: inline-grid;
    place-items: center;
    flex: none;
    border: 0;
    border-radius: var(--radius-s);
    background: transparent;
    color: var(--text-muted);
    transition: background-color var(--dur-fast) var(--ease-out), color var(--dur-fast) var(--ease-out);
  }
  .ib:hover:not(:disabled) {
    background: var(--surface-hover);
    color: var(--text);
  }
  .ib.active {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .ib.danger:hover:not(:disabled) {
    background: var(--danger-soft);
    color: var(--danger);
  }
  .ib:disabled {
    opacity: 0.4;
  }
</style>
