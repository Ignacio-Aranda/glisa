<script lang="ts">
  import DeviceIcon from './DeviceIcon.svelte';

  interface Props {
    family: string | null | undefined;
    kind?: string | null;
    /** The user's own picture (data URL), shown first. */
    picture?: string | null;
    /** The official product photo (data URL), when the setting is on. */
    photo?: string | null;
    dimmed?: boolean;
    alt?: string;
  }
  let { family, kind = null, picture = null, photo = null, dimmed = false, alt = '' }: Props = $props();
  const src = $derived(picture || photo || null);
</script>

<div class="pic" class:dimmed>
  {#if src}
    <img {src} {alt} class:own={!!picture} draggable="false" />
  {:else}
    <DeviceIcon {kind} {family} size={88} {dimmed} />
  {/if}
</div>

<style>
  .pic {
    width: 100%;
    height: 100%;
    display: grid;
    place-items: center;
    transition: opacity var(--dur-med) var(--ease-out);
  }
  img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  img.own {
    border-radius: var(--radius-m);
    box-shadow: var(--shadow-2);
  }
  .dimmed img {
    opacity: 0.5;
    filter: grayscale(0.6);
  }
</style>
