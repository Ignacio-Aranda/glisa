<script lang="ts">
  import { tick, type Snippet } from 'svelte';
  import { fade, scale } from 'svelte/transition';
  import IconButton from './IconButton.svelte';
  import Icon from './Icon.svelte';
  import { t } from '../i18n/index.svelte';

  interface Props {
    open: boolean;
    title: string;
    description?: string;
    icon?: string;
    iconTone?: 'accent' | 'danger' | 'warn' | 'ok';
    width?: number;
    /** Esc, the close button and a click outside close it. */
    dismissable?: boolean;
    onclose: () => void;
    children?: Snippet;
    footer?: Snippet;
    footerStart?: Snippet;
  }
  let {
    open, title, description, icon, iconTone = 'accent', width = 440, dismissable = true, onclose, children,
    footer, footerStart,
  }: Props = $props();

  let box: HTMLDivElement | null = $state(null);
  const uid = Math.random().toString(36).slice(2, 8);
  const reduce = typeof window !== 'undefined' && window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;

  function focusables(): HTMLElement[] {
    if (!box) return [];
    return Array.from(box.querySelectorAll<HTMLElement>(
      'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
    )).filter((el) => el.offsetParent !== null || el === document.activeElement || el.getClientRects().length > 0 || true);
  }

  $effect(() => {
    if (!open) return;
    const previous = document.activeElement as HTMLElement | null;
    void tick().then(() => {
      const auto = box?.querySelector<HTMLElement>('[data-autofocus]');
      (auto ?? focusables().find((el) => !el.classList.contains('dialog-close')) ?? box)?.focus();
    });
    return () => {
      if (previous && document.contains(previous)) previous.focus();
    };
  });

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.stopPropagation();
      e.preventDefault();
      if (dismissable) onclose();
      return;
    }
    if (e.key !== 'Tab') return;
    const items = focusables();
    if (!items.length) return;
    const first = items[0];
    const last = items[items.length - 1];
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  }

  function backdrop(e: MouseEvent) {
    if (e.target === e.currentTarget && dismissable) onclose();
  }
</script>

{#if open}
  <div class="overlay" role="presentation" onmousedown={backdrop} transition:fade={{ duration: reduce ? 0 : 140 }}>
    <div
      bind:this={box}
      class="dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="dlg-{uid}-title"
      aria-describedby={description ? `dlg-${uid}-desc` : undefined}
      tabindex="-1"
      style:max-width="{width}px"
      {onkeydown}
      transition:scale={{ duration: reduce ? 0 : 180, start: 0.96 }}
    >
      <header>
        {#if icon}<span class="icon {iconTone}"><Icon name={icon} size={20} /></span>{/if}
        <div class="titles">
          <h2 id="dlg-{uid}-title">{title}</h2>
          {#if description}<p id="dlg-{uid}-desc" class="muted">{description}</p>{/if}
        </div>
        {#if dismissable}
          <IconButton icon="x" label={t('Close')} size={30} iconSize={17} onclick={onclose} class="dialog-close" />
        {/if}
      </header>
      {#if children}<div class="content scroll">{@render children()}</div>{/if}
      {#if footer || footerStart}
        <footer>
          <div class="start">{#if footerStart}{@render footerStart()}{/if}</div>
          <div class="end">{#if footer}{@render footer()}{/if}</div>
        </footer>
      {/if}
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 100;
    display: grid;
    place-items: center;
    padding: var(--space-6);
    background: rgb(10 9 20 / 45%);
  }
  .dialog {
    width: 100%;
    max-height: calc(100vh - 2 * var(--space-6));
    display: flex;
    flex-direction: column;
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius-xl);
    box-shadow: var(--shadow-3);
    outline: none;
  }
  header {
    display: flex;
    align-items: flex-start;
    gap: var(--space-3);
    padding: var(--space-6) var(--space-6) var(--space-3);
  }
  .icon {
    display: grid;
    place-items: center;
    flex: none;
    width: 38px;
    height: 38px;
    border-radius: var(--radius-m);
    background: var(--accent-soft);
    color: var(--accent);
  }
  .icon.danger { background: var(--danger-soft); color: var(--danger); }
  .icon.warn { background: var(--warning-soft); color: var(--warning); }
  .icon.ok { background: var(--success-soft); color: var(--success); }
  .titles {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    padding-top: 3px;
  }
  .content {
    padding: var(--space-2) var(--space-6) var(--space-4);
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }
  footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    padding: var(--space-4) var(--space-6) var(--space-5);
    border-top: 1px solid var(--border);
  }
  .start, .end {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .end {
    margin-left: auto;
  }
</style>
