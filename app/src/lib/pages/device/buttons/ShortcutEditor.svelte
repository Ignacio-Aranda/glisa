<script lang="ts">
  import Button from '../../../components/Button.svelte';
  import KeyRecorder from '../../../components/KeyRecorder.svelte';
  import PanelHead from './PanelHead.svelte';
  import { normalizeCombo, ComboError } from '../../../keys';
  import { findPreset, presetLabel } from '../../../actions';
  import { t } from '../../../i18n/index.svelte';
  import type { Platform, PresetCategory } from '../../../types';

  interface Props {
    context: string;
    initial: string;
    online: boolean;
    platform: Platform;
    presets: PresetCategory[];
    onback: () => void;
    onassign: (combo: string) => void;
  }
  let { context, initial, online, platform, presets, onback, onassign }: Props = $props();

  // svelte-ignore state_referenced_locally
  let text = $state(initial);
  let recorder: HTMLButtonElement | null = $state(null);

  export function focus() {
    recorder?.focus();
  }

  const parsed = $derived.by((): { combo: string | null; error: string } => {
    const s = text.trim();
    if (!s) return { combo: null, error: '' };
    try {
      return { combo: normalizeCombo(s), error: '' };
    } catch (e) {
      return { combo: null, error: e instanceof ComboError ? e.message : String(e) };
    }
  });

  const note = $derived.by(() => {
    if (parsed.error) return parsed.error;
    if (parsed.combo) {
      const found = findPreset({ type: 'keys', combo: parsed.combo }, presets);
      return found ? t('Same as “{action}”', { action: presetLabel(found.label, found.title) }) : '';
    }
    return t('Separate the keys with +, for example {shortcut}', { shortcut: platform === 'macos' ? 'Meta+Shift+T' : 'Ctrl+Shift+T' });
  });

  function assign(e?: Event) {
    e?.preventDefault();
    if (parsed.combo && online) onassign(parsed.combo);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onback();
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="editor" {onkeydown}>
  <PanelHead back={t('Back to actions')} {onback} title={t('Keyboard shortcut')} icon="keyboard">
    <p class="small muted ellipsis">{context}</p>
  </PanelHead>
  <form class="form" onsubmit={assign}>
    <KeyRecorder bind:el={recorder} combo={parsed.combo ?? ''} {platform} label={t('Record keyboard shortcut')} disabled={!online} onrecord={(c) => (text = c)} />
    <p class="small muted">{t('If the system intercepts the shortcut, type it instead')}</p>
    <input
      class="text-input mono"
      type="text"
      bind:value={text}
      placeholder={t('e.g. {shortcut}', { shortcut: platform === 'windows' ? 'Meta+Tab' : platform === 'macos' ? 'Meta+Space' : 'Meta+Tab' })}
      aria-label={t('Type a keyboard shortcut')}
      aria-invalid={!!parsed.error}
      aria-describedby="shortcut-note"
      spellcheck="false"
      autocomplete="off"
    />
    <p id="shortcut-note" class="small" class:tone-danger={!!parsed.error} class:subtle={!parsed.error} aria-live="polite">{note}</p>
    <div class="actions">
      <Button onclick={onback}>{t('Cancel')}</Button>
      <Button variant="primary" type="submit" disabled={!parsed.combo || !online}>{t('Assign')}</Button>
    </div>
  </form>
</div>

<style>
  .editor {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    flex: 1;
  }
  .form {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    flex: 1;
  }
  .actions {
    margin-top: auto;
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
    padding-top: var(--space-3);
  }
  p {
    min-height: 1.2em;
  }
</style>
