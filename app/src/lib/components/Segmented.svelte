<script lang="ts" generics="T extends string | number">
  import Icon from './Icon.svelte';

  interface Option { value: T; label: string; icon?: string; title?: string }
  interface Props {
    options: Option[];
    value: T;
    label: string;
    disabled?: boolean;
    size?: 'sm' | 'md';
    onchange?: (value: T) => void;
  }
  let { options, value, label, disabled = false, size = 'md', onchange }: Props = $props();
  let buttons: HTMLButtonElement[] = $state([]);

  function pick(v: T) {
    if (!disabled && v !== value) onchange?.(v);
  }

  function onkeydown(e: KeyboardEvent, i: number) {
    const dir = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0;
    if (!dir) return;
    e.preventDefault();
    const n = (i + dir + options.length) % options.length;
    buttons[n]?.focus();
    pick(options[n].value);
  }
</script>

<div class="seg {size}" role="radiogroup" aria-label={label} aria-disabled={disabled || undefined}>
  {#each options as o, i (o.value)}
    <button
      bind:this={buttons[i]}
      type="button"
      role="radio"
      aria-checked={o.value === value}
      tabindex={o.value === value ? 0 : -1}
      title={o.title}
      {disabled}
      onclick={() => pick(o.value)}
      onkeydown={(e) => onkeydown(e, i)}
    >
      {#if o.icon}<Icon name={o.icon} size={15} />{/if}
      <span>{o.label}</span>
    </button>
  {/each}
</div>

<style>
  .seg {
    display: inline-flex;
    flex-wrap: wrap;
    padding: 3px;
    gap: 2px;
    border-radius: var(--radius-m);
    background: var(--surface-2);
  }
  button {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    height: 30px;
    padding: 0 var(--space-3);
    border: 0;
    border-radius: var(--radius-s);
    background: transparent;
    color: var(--text-muted);
    font-size: var(--text-sm);
    font-weight: 500;
    white-space: nowrap;
    transition: background-color var(--dur-fast) var(--ease-out), color var(--dur-fast) var(--ease-out),
      box-shadow var(--dur-fast) var(--ease-out);
  }
  .sm button {
    height: 26px;
    font-size: var(--text-sm);
  }
  button:hover:not(:disabled) {
    color: var(--text);
  }
  button[aria-checked='true'] {
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-1);
  }
  button:disabled {
    opacity: 0.5;
  }
</style>
