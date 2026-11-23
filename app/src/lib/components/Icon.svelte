<script lang="ts">
  import { ICONS } from './icons';

  interface Props {
    name: string;
    size?: number;
    stroke?: number;
    class?: string;
    label?: string;
  }
  let { name, size = 18, stroke = 1.75, class: cls = '', label }: Props = $props();
  const node = $derived(ICONS[name] ?? ICONS['circle']);
</script>

<!-- Lucide icons (ISC), drawn from the data in icons.ts. -->
<svg
  xmlns="http://www.w3.org/2000/svg"
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width={stroke}
  stroke-linecap="round"
  stroke-linejoin="round"
  class={'icon ' + cls}
  aria-hidden={label ? undefined : 'true'}
  aria-label={label}
  role={label ? 'img' : undefined}
  focusable="false"
>
  {#each node as [tag, attrs], i (i)}
    <svelte:element this={tag} {...attrs} />
  {/each}
</svg>

<style>
  svg {
    flex: none;
    display: block;
  }
</style>
