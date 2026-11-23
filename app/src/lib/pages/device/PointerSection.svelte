<script lang="ts">
  import Panel from '../../components/Panel.svelte';
  import Slider from '../../components/Slider.svelte';
  import Chip from '../../components/Chip.svelte';
  import Toggle from '../../components/Toggle.svelte';
  import SettingRow from '../../components/SettingRow.svelte';
  import Segmented from '../../components/Segmented.svelte';
  import Divider from '../../components/Divider.svelte';
  import Button from '../../components/Button.svelte';
  import IconButton from '../../components/IconButton.svelte';
  import Banner from '../../components/Banner.svelte';
  import EmptyState from '../../components/EmptyState.svelte';
  import {
    MAX_LEVELS, SS_DEFAULT_THRESHOLD, SS_NEVER, cleanLevels, gridStep, percentToThreshold, snapDpi, suggestLevel,
    thresholdToPercent,
  } from '../../dpi';
  import { api, errorText } from '../../backend';
  import { refreshDevice, toast } from '../../state/app.svelte';
  import { t, tn } from '../../i18n/index.svelte';
  import type { Device } from '../../types';

  interface Props { dev: Device }
  let { dev }: Props = $props();

  const online = $derived(dev.online);
  const dpi = $derived(dev.dpi);
  const wheel = $derived(dev.wheel);
  const ss = $derived(dev.smartshift);

  // ---- writes: the value just chosen stays on screen until the device's answer arrives
  let writing = $state(0);

  async function write(key: string, value: unknown, ok = '') {
    if (!online) return;
    writing += 1;
    try {
      await api('set', { id: dev.id, key, value });
      if (ok) toast(ok);
    } catch (e) {
      toast(errorText(e), 'error');
      draggingDpi = null;
      draftSens = null;
    } finally {
      writing -= 1;
      await refreshDevice(dev.id);
    }
  }

  // ---- pointer speed
  let draggingDpi = $state<number | null>(null);
  const levels = $derived(dpi ? cleanLevels(dpi, dpi.presets) : []);
  const step = $derived(dpi ? gridStep(dpi) : 1);
  const shownDpi = $derived(draggingDpi ?? dpi?.current ?? 0);

  function pickDpi(value: number) {
    if (!dpi) return;
    const v = snapDpi(dpi, value);
    draggingDpi = v;
    if (v !== dpi.current) void write('dpi', v).then(() => (draggingDpi = null));
    else draggingDpi = null;
  }

  // ---- levels
  let editing = $state<Record<number, string>>({});
  const suggestion = $derived(dpi ? suggestLevel(dpi, levels) : null);
  const canAdd = $derived(levels.length < MAX_LEVELS && suggestion !== null);

  function storeLevels(next: number[]) {
    void write('dpi_presets', next, t('DPI levels saved'));
  }

  function commitLevel(i: number) {
    if (!dpi) return;
    const raw = editing[i];
    delete editing[i];
    if (raw === undefined) return;
    const n = Number(raw.replace(/\D/g, ''));
    if (!raw.trim() || !Number.isFinite(n) || n <= 0) return;
    const value = snapDpi(dpi, n);
    if (value === levels[i]) return;
    if (levels.some((v, k) => k !== i && v === value)) {
      toast(t('You already have a {dpi} DPI level', { dpi: value }), 'warn');
      return;
    }
    const next = [...levels];
    next[i] = value;
    storeLevels([...new Set(next)].sort((a, b) => a - b));
  }

  function removeLevel(i: number) {
    if (levels.length <= 1) return;
    storeLevels(levels.filter((_, k) => k !== i));
  }

  function addLevel() {
    if (!canAdd || suggestion === null) return;
    storeLevels([...levels, suggestion].sort((a, b) => a - b));
  }

  function levelKey(e: KeyboardEvent, i: number) {
    if (e.key === 'Enter') {
      e.preventDefault();
      (e.currentTarget as HTMLInputElement).blur();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      delete editing[i];
      (e.currentTarget as HTMLInputElement).blur();
    }
  }

  // ---- SmartShift (the sensitivity to come back to is remembered on this computer)
  let draftSens = $state<number | null>(null);
  const ssKey = $derived(`lo.smartshift.${dev.id}`);
  function remembered(): number {
    try {
      const v = Number(localStorage.getItem(ssKey));
      return v >= 1 && v < SS_NEVER ? v : SS_DEFAULT_THRESHOLD;
    } catch {
      return SS_DEFAULT_THRESHOLD;
    }
  }
  function remember(threshold: number) {
    try {
      localStorage.setItem(ssKey, String(threshold));
    } catch {
      // not remembered: fine
    }
  }
  const auto = $derived(!!ss && ss.threshold >= 1 && ss.threshold < SS_NEVER);
  $effect(() => {
    if (ss && auto) remember(ss.threshold);
  });
  const shownSens = $derived(draftSens ?? thresholdToPercent(auto && ss ? ss.threshold : remembered()));

  function setAuto(on: boolean) {
    void write('ss_threshold', on ? remembered() : SS_NEVER);
  }

  function setSensitivity(percent: number) {
    draftSens = null;
    const threshold = percentToThreshold(percent);
    if (!ss || !auto || threshold === ss.threshold) return;
    remember(threshold);
    void write('ss_threshold', threshold);
  }

  const anything = $derived(!!(dpi || wheel || ss));
</script>

<div class="page">
  {#if !anything}
    <EmptyState icon="gauge" title={t('Nothing to adjust here')} text={t("This device doesn't let you change the pointer speed or how the wheel behaves.")} />
  {:else}
    {#if !online}
      <Banner kind="warn" title={t('Device disconnected')} text={t('Turn it on or bring it closer to change these settings.')} />
    {/if}
    <div class="columns">
      <div class="col">
        {#if dpi}
          <Panel title={t('Pointer speed')} subtitle={t('A higher value moves the pointer further for the same hand movement')} icon="mouse-pointer-2">
            {#snippet trailing()}
              {#if online && dpi.default && shownDpi !== snapDpi(dpi, dpi.default)}
                <Button variant="link" size="sm" title={t('Back to {dpi} DPI', { dpi: snapDpi(dpi, dpi.default) })} onclick={() => pickDpi(dpi.default)}>{t('Reset')}</Button>
              {/if}
            {/snippet}
            <div class="value" aria-hidden="true"><span class="big">{shownDpi}</span><span class="unit">DPI</span></div>
            <div class="scale">
              <Slider
                min={dpi.min}
                max={dpi.max}
                {step}
                value={shownDpi}
                label={t('Pointer speed')}
                valuetext="{shownDpi} DPI"
                disabled={!online}
                oninput={(v) => (draggingDpi = snapDpi(dpi, v))}
                onchange={pickDpi}
              />
              <div class="ends small subtle"><span>{dpi.min} DPI</span><span>{dpi.max} DPI</span></div>
            </div>
            <div class="chips">
              {#each levels as v (v)}
                <Chip selected={v === shownDpi} disabled={!online} mono title={t('Use {dpi} DPI', { dpi: v })} onclick={() => pickDpi(v)}>{v} DPI</Chip>
              {/each}
            </div>
          </Panel>

          <Panel title={t('DPI levels')} subtitle={t('The speeds that the “Change DPI” action cycles through')} icon="gauge">
            {#snippet trailing()}
              <Button
                size="sm"
                variant="ghost"
                icon="plus"
                disabled={!online || !canAdd}
                title={canAdd ? t('Add another level') : levels.length >= MAX_LEVELS
                  ? tn('You can save up to {n} level', 'You can save up to {n} levels', MAX_LEVELS, { n: MAX_LEVELS })
                  : t('You have used every value this device supports')}
                onclick={addLevel}
              >{t('Add')}</Button>
            {/snippet}
            <div class="levels">
              {#each levels as v, i (v)}
                <div class="level">
                  <input
                    class="level-input"
                    type="text"
                    inputmode="numeric"
                    value={editing[i] ?? String(v)}
                    aria-label={t('DPI level')}
                    disabled={!online}
                    oninput={(e) => (editing[i] = (e.currentTarget as HTMLInputElement).value)}
                    onblur={() => commitLevel(i)}
                    onkeydown={(e) => levelKey(e, i)}
                  />
                  <span class="unit small subtle">DPI</span>
                  <IconButton
                    icon="x"
                    size={24}
                    iconSize={14}
                    label={t('Remove this level')}
                    tooltip={levels.length > 1 ? t('Remove this level') : t('At least one level has to stay')}
                    disabled={!online || levels.length <= 1}
                    onclick={() => removeLevel(i)}
                  />
                </div>
              {/each}
            </div>
          </Panel>
        {/if}
      </div>

      <div class="col">
        {#if wheel}
          <Panel title={t('Scroll wheel')} icon="arrow-down-up">
            <SettingRow title={t('Natural scrolling')} subtitle={wheel.has_invert ? t('Content follows the movement of your fingers') : t("This device can't reverse the scrolling direction")}>
              {#snippet control()}
                <Toggle checked={!!wheel.inverted && wheel.has_invert} label={t('Natural scrolling')} disabled={!online || !wheel.has_invert} onchange={(v) => write('wheel_inverted', v)} />
              {/snippet}
            </SettingRow>
            <Divider />
            <SettingRow title={t('Smooth scrolling')} subtitle={t('Smaller steps for smoother movement')}>
              {#snippet control()}
                <Toggle checked={wheel.hires} label={t('Smooth scrolling')} disabled={!online} onchange={(v) => write('wheel_hires', v)} />
              {/snippet}
            </SettingRow>
          </Panel>
        {/if}

        {#if ss}
          <Panel title="SmartShift" subtitle={t('Give the wheel a hard flick and it lets go and spins freely')} icon="disc-3">
            <SettingRow title={t('Switch to free spin automatically')} subtitle={t('Once the flick is over, the wheel goes back to clicks')}>
              {#snippet control()}
                <Toggle checked={auto} label={t('Switch to free spin automatically')} disabled={!online} onchange={setAuto} />
              {/snippet}
            </SettingRow>
            <div class="sens" class:off={!auto}>
              <div class="sens-head"><span>{t('Sensitivity')}</span><span class="muted">{t('{percent}%', { percent: shownSens })}</span></div>
              <Slider
                min={2}
                max={100}
                step={2}
                value={shownSens}
                label={t('Sensitivity of the automatic switch')}
                valuetext={t('{percent}%', { percent: shownSens })}
                disabled={!online || !auto}
                oninput={(v) => (draftSens = v)}
                onchange={setSensitivity}
              />
              <div class="ends small subtle"><span>{t('Needs a harder flick')}</span><span>{t('A gentle flick is enough')}</span></div>
            </div>
            <Divider />
            <SettingRow title={t('Wheel mode')} subtitle={t('In free spin, the automatic switch does nothing')}>
              {#snippet control()}
                <Segmented
                  size="sm"
                  label={t('Wheel mode')}
                  value={ss.ratchet ? 'ratchet' : 'free'}
                  disabled={!online}
                  options={[{ value: 'ratchet', label: t('With clicks') }, { value: 'free', label: t('Free spin') }]}
                  onchange={(v) => write('ss_ratchet', v === 'ratchet')}
                />
              {/snippet}
            </SettingRow>
          </Panel>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .page {
    container-type: inline-size;
    padding: var(--space-5) var(--space-7) var(--space-8);
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    max-width: 1120px;
    width: 100%;
    margin: 0 auto;
  }
  .columns {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--space-5);
    align-items: start;
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: var(--space-5);
    min-width: 0;
  }
  .value {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
  }
  .big {
    font-size: 40px;
    font-weight: 700;
    letter-spacing: -0.03em;
    font-variant-numeric: tabular-nums;
    line-height: 1;
  }
  .unit {
    color: var(--text-muted);
    font-weight: 600;
  }
  .scale {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .ends {
    display: flex;
    justify-content: space-between;
    gap: var(--space-3);
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
  .levels {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
  .level {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    height: 36px;
    padding: 0 4px 0 var(--space-3);
    border-radius: var(--radius-s);
    border: 1px solid var(--border-strong);
    background: var(--surface);
  }
  .level:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--focus-ring);
  }
  .level-input {
    width: 3.4em;
    border: 0;
    background: transparent;
    font-variant-numeric: tabular-nums;
    font-weight: 600;
    text-align: right;
    padding: 0;
  }
  .level-input:focus {
    outline: none;
  }
  .level .unit {
    margin-right: var(--space-1);
  }
  .sens {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    transition: opacity var(--dur-med) var(--ease-out);
  }
  .sens.off {
    opacity: 0.5;
  }
  .sens-head {
    display: flex;
    justify-content: space-between;
    font-weight: 540;
  }
  @media (max-width: 1000px) {
    .page { padding-inline: var(--space-5); }
  }
  @container (max-width: 700px) {
    .columns { grid-template-columns: minmax(0, 1fr); }
  }
</style>
