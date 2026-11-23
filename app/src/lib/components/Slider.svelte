<script lang="ts">
  interface Props {
    min: number;
    max: number;
    step?: number;
    value: number;
    label: string;
    valuetext?: string;
    disabled?: boolean;
    /** While dragging. */
    oninput?: (value: number) => void;
    /** Settled (released, or a key press). */
    onchange?: (value: number) => void;
    busy?: boolean;
  }
  let {
    min, max, step = 1, value, label, valuetext, disabled = false, oninput, onchange, busy = $bindable(false),
  }: Props = $props();

  const pct = $derived(max > min ? ((value - min) / (max - min)) * 100 : 0);
</script>

<input
  class="slider"
  type="range"
  {min}
  {max}
  {step}
  {value}
  {disabled}
  aria-label={label}
  aria-valuetext={valuetext}
  style:--pct="{pct}%"
  oninput={(e) => oninput?.(Number((e.currentTarget as HTMLInputElement).value))}
  onchange={(e) => onchange?.(Number((e.currentTarget as HTMLInputElement).value))}
  onpointerdown={() => (busy = true)}
  onpointerup={() => (busy = false)}
  onblur={() => (busy = false)}
/>

<style>
  .slider {
    -webkit-appearance: none;
    appearance: none;
    width: 100%;
    height: 22px;
    margin: 0;
    background: transparent;
    cursor: pointer;
  }
  .slider:disabled {
    cursor: default;
    opacity: 0.5;
  }
  .slider::-webkit-slider-runnable-track {
    height: 4px;
    border-radius: 999px;
    background: linear-gradient(to right, var(--accent) var(--pct), var(--surface-2) var(--pct));
    border: 1px solid var(--border);
  }
  .slider::-moz-range-track {
    height: 4px;
    border-radius: 999px;
    background: var(--surface-2);
    border: 1px solid var(--border);
  }
  .slider::-moz-range-progress {
    height: 4px;
    border-radius: 999px;
    background: var(--accent);
  }
  .slider::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 18px;
    height: 18px;
    margin-top: -8px;
    border-radius: 50%;
    background: var(--surface);
    border: 2px solid var(--accent);
    box-shadow: var(--shadow-1);
    transition: transform var(--dur-fast) var(--ease-out);
  }
  .slider::-moz-range-thumb {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--surface);
    border: 2px solid var(--accent);
    box-shadow: var(--shadow-1);
  }
  .slider:hover:not(:disabled)::-webkit-slider-thumb {
    transform: scale(1.1);
  }
  .slider:focus-visible {
    outline: none;
  }
  .slider:focus-visible::-webkit-slider-thumb {
    box-shadow: 0 0 0 4px var(--focus-ring);
  }
  .slider:focus-visible::-moz-range-thumb {
    box-shadow: 0 0 0 4px var(--focus-ring);
  }
</style>
