<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon from './Icon.svelte';

  interface Props {
    title?: string;
    subtitle?: string;
    icon?: string;
    trailing?: Snippet;
    children?: Snippet;
    padded?: boolean;
    tone?: 'default' | 'danger';
    id?: string;
    class?: string;
  }
  let { title, subtitle, icon, trailing, children, padded = true, tone = 'default', id, class: cls = '' }: Props = $props();
  const headingId = $derived(id ? `${id}-title` : undefined);
</script>

<section class="panel {cls}" class:padded class:danger={tone === 'danger'} {id} aria-labelledby={title ? headingId : undefined}>
  {#if title || trailing}
    <header>
      {#if icon}<span class="icon-badge"><Icon name={icon} size={17} /></span>{/if}
      <div class="titles">
        {#if title}<h3 id={headingId}>{title}</h3>{/if}
        {#if subtitle}<p class="muted small">{subtitle}</p>{/if}
      </div>
      {#if trailing}<div class="trailing">{@render trailing()}</div>{/if}
    </header>
  {/if}
  {#if children}<div class="body">{@render children()}</div>{/if}
</section>

<style>
  .panel {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-l);
    box-shadow: var(--shadow-1);
    min-width: 0;
  }
  .padded {
    padding: var(--space-5) var(--space-6);
  }
  header {
    display: flex;
    align-items: flex-start;
    gap: var(--space-3);
    margin-bottom: var(--space-4);
  }
  .icon-badge {
    display: grid;
    place-items: center;
    flex: none;
    width: 32px;
    height: 32px;
    border-radius: var(--radius-s);
    background: var(--accent-soft);
    color: var(--accent);
  }
  .danger .icon-badge {
    background: var(--danger-soft);
    color: var(--danger);
  }
  .titles {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding-top: 4px;
  }
  .trailing {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
</style>
