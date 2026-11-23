<script lang="ts">
  interface Props {
    text: string;
    tone?: 'ok' | 'warn' | 'danger' | 'info' | 'muted' | 'accent';
    dot?: boolean;
  }
  let { text, tone = 'muted', dot = true }: Props = $props();
</script>

<span class="pill {tone}">
  {#if dot}<span class="dot" aria-hidden="true"></span>{/if}
  {text}
</span>

<style>
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 20px;
    padding: 0 var(--space-2);
    border-radius: var(--radius-s);
    font-size: var(--text-xs);
    font-weight: 620;
    white-space: nowrap;
    background: var(--surface-2);
    color: var(--text-muted);
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: currentColor;
  }
  .ok { background: var(--success-soft); color: var(--success); }
  .warn { background: var(--warning-soft); color: var(--warning); }
  .danger { background: var(--danger-soft); color: var(--danger); }
  .info { background: var(--info-soft); color: var(--info); }
  .accent { background: var(--accent-soft); color: var(--accent); }
</style>
