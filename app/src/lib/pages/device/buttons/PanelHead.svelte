<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon from '../../../components/Icon.svelte';

  interface Props {
    back?: string;
    onback?: () => void;
    title: string;
    icon?: string;
    iconTone?: 'muted' | 'accent';
    children?: Snippet;
  }
  let { back, onback, title, icon, iconTone = 'muted', children }: Props = $props();
</script>

<div class="head">
  {#if back && onback}
    <button type="button" class="back" onclick={onback}>
      <Icon name="chevron-left" size={16} />
      <span>{back}</span>
    </button>
  {/if}
  <div class="title">
    {#if icon}<span class="icon tone-{iconTone}"><Icon name={icon} size={20} /></span>{/if}
    <h2 class="ellipsis">{title}</h2>
  </div>
  {#if children}{@render children()}{/if}
</div>

<style>
  .head {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .back {
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: 2px;
    padding: 2px 6px 2px 0;
    border: 0;
    background: transparent;
    color: var(--accent);
    font-weight: 560;
    font-size: var(--text-sm);
  }
  .back:hover span {
    text-decoration: underline;
    text-underline-offset: 3px;
  }
  .title {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    min-width: 0;
  }
  .icon {
    display: grid;
    flex: none;
  }
  h2 {
    font-size: var(--text-xl);
  }
</style>
