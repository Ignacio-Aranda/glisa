<script lang="ts">
  interface Props { size?: number; label?: string }
  let { size = 18, label }: Props = $props();
</script>

<span class="spinner" style:width="{size}px" style:height="{size}px" role={label ? 'status' : undefined} aria-label={label}></span>

<style>
  .spinner {
    display: inline-block;
    flex: none;
    border-radius: 50%;
    border: 2px solid var(--border-strong);
    border-top-color: var(--accent);
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to { transform: rotate(360deg); }
  }
  @media (prefers-reduced-motion: reduce) {
    .spinner { animation-duration: 2.4s !important; animation-iteration-count: infinite !important; }
  }
</style>
