<script lang="ts">
  import Icon from './Icon.svelte';
  import { batteryText, batteryTone, levelName, percentText } from '../format';
  import { t } from '../i18n/index.svelte';
  import type { Battery } from '../types';

  interface Props { battery: Battery | null | undefined; long?: boolean }
  let { battery, long = false }: Props = $props();

  const icon = $derived.by(() => {
    const b = battery;
    if (!b) return 'battery';
    if (b.charging) return 'battery-charging';
    const pct = b.percent ?? (b.level === 'full' ? 100 : b.level === 'good' ? 60 : b.level === 'low' ? 20 : 5);
    if (pct <= 10) return 'battery-exclamation-point';
    if (pct <= 25) return 'battery-low';
    if (pct <= 70) return 'battery-medium';
    return 'battery-full';
  });
  const short = $derived(battery ? (battery.percent ? percentText(battery.percent) : levelName(battery.level)) : '');
  const full = $derived(battery ? t('Battery: {state}', { state: batteryText(battery) }) : '');
</script>

{#if battery}
  <span class="battery tone-{batteryTone(battery)}" title={full}>
    <Icon name={icon} size={18} />
    <span class="value" aria-hidden="true">{long ? batteryText(battery) : short}</span>
    <span class="sr-only">{full}</span>
  </span>
{/if}

<style>
  .battery {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: var(--text-sm);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .value {
    color: var(--text-muted);
  }
  .tone-danger .value,
  .tone-warn .value {
    color: inherit;
    font-weight: 600;
  }
</style>
