<script lang="ts">
  import DevicePicture from '../../components/DevicePicture.svelte';
  import BatteryBadge from '../../components/BatteryBadge.svelte';
  import StatusPill from '../../components/StatusPill.svelte';
  import IconButton from '../../components/IconButton.svelte';
  import { connectionName, kindName } from '../../format';
  import { t } from '../../i18n/index.svelte';
  import type { DeviceSummary } from '../../types';

  interface Props {
    dev: DeviceSummary;
    picture?: string | null;
    photo?: string | null;
    onopen: () => void;
    onmenu: (x: number, y: number) => void;
  }
  let { dev, picture = null, photo = null, onopen, onmenu }: Props = $props();
  let more: HTMLButtonElement | null = $state(null);

  function menuFromButton() {
    const r = more?.getBoundingClientRect();
    onmenu(r ? r.left : 0, r ? r.bottom + 4 : 0);
  }

  function oncontextmenu(e: MouseEvent) {
    e.preventDefault();
    onmenu(e.clientX, e.clientY);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'ContextMenu' || (e.key === 'F10' && e.shiftKey)) {
      e.preventDefault();
      menuFromButton();
    }
  }
</script>

<article class="card" class:offline={!dev.online} {oncontextmenu}>
  <button type="button" class="open" onclick={onopen} {onkeydown} aria-label={t('Configure {name}', { name: dev.name })}>
    <div class="visual">
      <DevicePicture family={dev.family} kind={dev.kind} {picture} {photo} dimmed={!dev.online} alt={dev.name} />
    </div>
    <div class="info">
      <h3 class="ellipsis">{dev.name}</h3>
      <div class="meta">
        <span class="muted small ellipsis">{kindName(dev.kind)} · {connectionName(dev.connection)}</span>
        {#if dev.online}
          <BatteryBadge battery={dev.battery} />
        {:else}
          <StatusPill text={t('Disconnected')} tone="muted" />
        {/if}
      </div>
    </div>
  </button>
  <div class="more">
    <IconButton bind:el={more} icon="ellipsis" label={t('Device options')} tooltip={t('More options')} size={32} onclick={menuFromButton} aria-haspopup="menu" />
  </div>
</article>

<style>
  .card {
    position: relative;
    border-radius: var(--radius-l);
    background: var(--surface);
    border: 1px solid var(--border);
    box-shadow: var(--shadow-1);
    transition: box-shadow var(--dur-med) var(--ease-out), border-color var(--dur-med) var(--ease-out),
      transform var(--dur-med) var(--ease-out);
  }
  .card:hover {
    box-shadow: var(--shadow-2);
    border-color: var(--border-strong);
    transform: translateY(-1px);
  }
  .open {
    display: flex;
    flex-direction: column;
    width: 100%;
    padding: 0;
    border: 0;
    border-radius: inherit;
    background: transparent;
    text-align: left;
  }
  .open:focus-visible {
    outline: 2px solid var(--focus-ring);
    outline-offset: 2px;
  }
  .visual {
    height: 176px;
    padding: var(--space-5) var(--space-6) var(--space-2);
    border-radius: var(--radius-l) var(--radius-l) 0 0;
    background: var(--surface-2);
  }
  .info {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-3) var(--space-5) var(--space-5);
    min-width: 0;
  }
  h3 {
    font-size: var(--text-lg);
    font-weight: 650;
  }
  .meta {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    min-height: 24px;
  }
  .more {
    position: absolute;
    top: var(--space-2);
    right: var(--space-2);
    opacity: 0;
    transition: opacity var(--dur-fast) var(--ease-out);
  }
  .card:hover .more,
  .card:focus-within .more {
    opacity: 1;
  }
  @media (hover: none) {
    .more { opacity: 1; }
  }
</style>
