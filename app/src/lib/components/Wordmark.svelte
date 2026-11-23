<script lang="ts">
  // The mark and the product name (brand/logo/wordmark.svg); the name is drawn in currentColor.
  import wordmark from '../brand/logo-wordmark.svg?raw';
  import { productName } from '../i18n/index.svelte';

  interface Props { height?: number }
  let { height = 32 }: Props = $props();
</script>

<span class="wordmark" style:height="{height}px" role="img" aria-label={productName()}>
  <!-- Our own SVG (app/src/lib/brand): safe to inline. -->
  {@html wordmark}
</span>

<style>
  .wordmark {
    display: inline-flex;
    color: var(--text);
  }
  .wordmark :global(svg) {
    height: 100%;
    width: auto;
  }
</style>
