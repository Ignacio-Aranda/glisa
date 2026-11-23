<script lang="ts">
  interface Props {
    checked: boolean;
    label: string;
    disabled?: boolean;
    onchange?: (checked: boolean) => void;
    id?: string;
    describedby?: string;
  }
  let { checked, label, disabled = false, onchange, id, describedby }: Props = $props();

  function toggle() {
    if (!disabled) onchange?.(!checked);
  }
</script>

<button
  type="button"
  role="switch"
  class="toggle"
  {id}
  aria-checked={checked}
  aria-label={label}
  aria-describedby={describedby}
  {disabled}
  onclick={toggle}
>
  <span class="knob"></span>
</button>

<style>
  .toggle {
    position: relative;
    flex: none;
    width: 36px;
    height: 20px;
    padding: 0;
    border-radius: 999px;
    border: 0;
    background: var(--border-strong);
    transition: background-color var(--dur-med) var(--ease-out), border-color var(--dur-med) var(--ease-out);
  }
  .toggle:hover:not(:disabled) {
    background: var(--text-subtle);
  }
  .knob {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: var(--surface);
    box-shadow: var(--shadow-1);
    transition: transform var(--dur-med) var(--ease-spring);
  }
  .toggle[aria-checked='true'] {
    background: var(--accent);
  }
  .toggle[aria-checked='true']:hover:not(:disabled) {
    background: var(--accent-hover);
  }
  .toggle[aria-checked='true'] .knob {
    transform: translateX(16px);
    background: var(--text-on-accent);
  }
  .toggle:disabled {
    opacity: 0.45;
  }
  .toggle:focus-visible {
    outline: none;
    box-shadow: 0 0 0 3px var(--focus-ring);
  }
</style>
