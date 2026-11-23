<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon from './Icon.svelte';
  import Spinner from './Spinner.svelte';

  interface Props {
    variant?: 'primary' | 'secondary' | 'ghost' | 'danger' | 'danger-solid' | 'link' | 'danger-ghost';
    size?: 'sm' | 'md';
    icon?: string;
    iconEnd?: string;
    disabled?: boolean;
    busy?: boolean;
    type?: 'button' | 'submit';
    title?: string;
    label?: string;
    pressed?: boolean;
    full?: boolean;
    onclick?: (e: MouseEvent) => void;
    children?: Snippet;
    el?: HTMLButtonElement | null;
    class?: string;
    [rest: string]: unknown;
  }
  let {
    variant = 'secondary', size = 'md', icon, iconEnd, disabled = false, busy = false, type = 'button',
    title, label, pressed, full = false, onclick, children, el = $bindable(null), class: cls = '', ...rest
  }: Props = $props();
</script>

<button
  bind:this={el}
  {type}
  class="btn {variant} {size} {cls}"
  class:full
  disabled={disabled || busy}
  aria-busy={busy || undefined}
  aria-pressed={pressed}
  aria-label={label}
  {title}
  {onclick}
  {...rest}
>
  {#if busy}
    <Spinner size={14} />
  {:else if icon}
    <Icon name={icon} size={size === 'sm' ? 15 : 16} />
  {/if}
  {#if children}<span class="text">{@render children()}</span>{/if}
  {#if iconEnd}<Icon name={iconEnd} size={15} />{/if}
</button>

<style>
  .btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    height: 36px;
    padding: 0 var(--space-4);
    border-radius: var(--radius-m);
    border: 1px solid transparent;
    font-weight: 500;
    font-size: var(--text-md);
    white-space: nowrap;
    transition: background-color var(--dur-fast) var(--ease-out), border-color var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out), box-shadow var(--dur-fast) var(--ease-out), transform var(--dur-fast) var(--ease-out);
  }
  .btn:active:not(:disabled) {
    transform: scale(0.98);
  }
  .sm {
    height: 30px;
    padding: 0 var(--space-3);
    font-size: var(--text-sm);
  }
  .full {
    width: 100%;
  }
  .text {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .primary {
    background: var(--accent);
    color: var(--text-on-accent);
    box-shadow: var(--shadow-1);
  }
  .primary:hover:not(:disabled) {
    background: var(--accent-hover);
  }
  .secondary {
    background: var(--surface);
    border-color: var(--border-strong);
    color: var(--text);
    box-shadow: var(--shadow-1);
  }
  .secondary:hover:not(:disabled) {
    background: var(--surface-hover);
  }
  .secondary[aria-pressed='true'] {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--accent);
  }
  .ghost {
    background: transparent;
    color: var(--text-muted);
  }
  .ghost:hover:not(:disabled) {
    background: var(--surface-hover);
    color: var(--text);
  }
  .danger-solid {
    background: var(--danger);
    color: var(--text-on-accent);
    box-shadow: var(--shadow-1);
  }
  .danger-solid:hover:not(:disabled) {
    background: color-mix(in srgb, var(--danger) 88%, var(--text));
  }
  .danger {
    background: var(--surface);
    border-color: var(--border-strong);
    color: var(--danger);
  }
  .danger:hover:not(:disabled) {
    background: var(--danger-soft);
  }
  .danger-ghost {
    background: transparent;
    color: var(--danger);
  }
  .danger-ghost:hover:not(:disabled) {
    background: var(--danger-soft);
  }
  .link {
    background: transparent;
    color: var(--accent);
    height: auto;
    padding: var(--space-1) 0;
    font-weight: 560;
  }
  .link:hover:not(:disabled) .text {
    text-decoration: underline;
    text-underline-offset: 3px;
  }
  .btn:disabled {
    opacity: 0.45;
    box-shadow: none;
  }
</style>
