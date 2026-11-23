<script lang="ts">
  // A plain icon for a device without a picture (no product photo, none of the user's).
  import Icon from './Icon.svelte';

  interface Props {
    kind?: string | null;
    family?: string | null;
    /** Tile size in px. */
    size?: number;
    dimmed?: boolean;
  }
  let { kind = null, family = null, size = 28, dimmed = false }: Props = $props();

  const name = $derived(
    kind === 'keyboard' || family?.startsWith('keyboard')
      ? 'keyboard'
      : kind === 'receiver' || family === 'receiver'
        ? 'usb'
        : 'mouse',
  );
</script>

<span class="tile" class:dimmed style:--s="{size}px" aria-hidden="true">
  <Icon {name} size={Math.round(size * 0.5)} stroke={size >= 64 ? 1.5 : 1.75} />
</span>

<style>
  .tile {
    width: var(--s);
    height: var(--s);
    flex: none;
    display: grid;
    place-items: center;
    border-radius: calc(var(--s) * 0.28);
    background: var(--accent-soft);
    color: var(--accent);
    transition: opacity var(--dur-med) var(--ease-out);
  }
  .dimmed {
    opacity: 0.5;
    filter: grayscale(0.7);
  }
</style>
