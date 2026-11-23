<script lang="ts" generics="T">
  interface Option { value: T; label: string; disabled?: boolean }
  interface Props {
    options: Option[];
    value: T;
    label: string;
    disabled?: boolean;
    onchange?: (value: T) => void;
    width?: string;
  }
  let { options, value, label, disabled = false, onchange, width }: Props = $props();
  const index = $derived(options.findIndex((o) => o.value === value));
</script>

<span class="select" style:width>
  <select
    aria-label={label}
    {disabled}
    value={String(index)}
    onchange={(e) => {
      const o = options[Number((e.currentTarget as HTMLSelectElement).value)];
      if (o) onchange?.(o.value);
    }}
  >
    {#each options as o, i (i)}
      <option value={String(i)} disabled={o.disabled}>{o.label}</option>
    {/each}
  </select>
</span>

<style>
  .select {
    position: relative;
    display: inline-flex;
    min-width: 0;
    max-width: 100%;
  }
  .select::after {
    content: '';
    position: absolute;
    right: 12px;
    top: 50%;
    width: 7px;
    height: 7px;
    border-right: 1.6px solid var(--text-muted);
    border-bottom: 1.6px solid var(--text-muted);
    transform: translateY(-70%) rotate(45deg);
    pointer-events: none;
  }
  select {
    -webkit-appearance: none;
    appearance: none;
    width: 100%;
    height: 36px;
    padding: 0 36px 0 var(--space-3);
    border-radius: var(--radius-s);
    border: 1px solid var(--border-strong);
    background: var(--surface);
    color: var(--text);
    text-overflow: ellipsis;
    cursor: pointer;
  }
  select:hover:not(:disabled) {
    background: var(--surface-hover);
  }
  select:focus-visible {
    outline: 2px solid var(--focus-ring);
    outline-offset: 1px;
  }
  select:disabled {
    opacity: 0.5;
    cursor: default;
  }
  option {
    background: var(--bg-elevated);
    color: var(--text);
  }
</style>
