<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon from './Icon.svelte';

  interface Props {
    title: string;
    subtitle?: string;
    icon?: string;
    control?: Snippet;
    children?: Snippet;
    disabled?: boolean;
    subtitleId?: string;
  }
  let { title, subtitle, icon, control, children, disabled = false, subtitleId }: Props = $props();
</script>

<div class="row" class:disabled>
  {#if icon}<span class="icon"><Icon name={icon} size={18} /></span>{/if}
  <div class="text">
    <div class="title">{title}</div>
    {#if subtitle}<div class="sub muted small" id={subtitleId}>{subtitle}</div>{/if}
    {#if children}{@render children()}{/if}
  </div>
  {#if control}<div class="control">{@render control()}</div>{/if}
</div>

<style>
  .row {
    display: flex;
    align-items: center;
    gap: var(--space-4);
    min-height: 44px;
  }
  .icon {
    flex: none;
    color: var(--text-muted);
    display: grid;
    place-items: center;
    width: 24px;
  }
  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .title {
    font-weight: 540;
  }
  .control {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    max-width: 60%;
  }
  .disabled .title,
  .disabled .sub {
    opacity: 0.6;
  }
</style>
