<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon from './Icon.svelte';
  import Button from './Button.svelte';

  interface Props {
    icon: string;
    title: string;
    text?: string;
    action?: string;
    actionIcon?: string;
    onaction?: () => void;
    children?: Snippet;
    compact?: boolean;
  }
  let { icon, title, text, action, actionIcon, onaction, children, compact = false }: Props = $props();
</script>

<div class="empty" class:compact>
  <span class="art"><Icon name={icon} size={compact ? 20 : 24} /></span>
  <h3>{title}</h3>
  {#if text}<p class="muted">{text}</p>{/if}
  {#if action && onaction}<span class="action"><Button variant="primary" icon={actionIcon} onclick={onaction}>{action}</Button></span>{/if}
  {#if children}{@render children()}{/if}
</div>

<style>
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: var(--space-3);
    padding: var(--space-8) var(--space-6);
    max-width: 520px;
    margin: 0 auto;
  }
  .compact {
    padding: var(--space-5) var(--space-4);
  }
  .art {
    display: grid;
    place-items: center;
    width: 48px;
    height: 48px;
    border-radius: 50%;
    background: var(--accent-soft);
    color: var(--accent);
    margin-bottom: var(--space-1);
  }
  .compact .art {
    width: 40px;
    height: 40px;
  }
  h3 {
    font-size: var(--text-xl);
  }
  p {
    max-width: 44ch;
  }
  .action {
    margin-top: var(--space-2);
  }
  p {
    max-width: 440px;
  }
</style>
