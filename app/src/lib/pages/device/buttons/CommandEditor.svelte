<script lang="ts">
  import Button from '../../../components/Button.svelte';
  import Chip from '../../../components/Chip.svelte';
  import PanelHead from './PanelHead.svelte';
  import { t } from '../../../i18n/index.svelte';
  import type { Platform } from '../../../types';

  interface Props {
    context: string;
    initial: string;
    online: boolean;
    platform: Platform;
    onback: () => void;
    onassign: (cmd: string) => void;
  }
  let { context, initial, online, platform, onback, onassign }: Props = $props();

  // svelte-ignore state_referenced_locally
  let cmd = $state(initial);
  let input: HTMLInputElement | null = $state(null);

  export function focus() {
    input?.focus();
  }

  /** Example commands for this OS (the Downloads folder has a different name in each language). */
  const examples = $derived(
    platform === 'windows'
      ? ['notepad', 'calc', 'explorer shell:Downloads', 'wt']
      : platform === 'macos'
        ? ['open -a Terminal', 'open ~/' + t('Downloads'), 'open -a Calculator', 'open https://example.org']
        : ['konsole', 'dolphin ' + t('~/Downloads'), 'spectacle --region', 'xdg-open https://kde.org'],
  );
  const placeholder = $derived(
    platform === 'windows' ? t('e.g. {command} or {other}', { command: 'notepad', other: 'explorer shell:Downloads' })
      : platform === 'macos' ? t('e.g. {command} or {other}', { command: 'open -a Terminal', other: 'open https://…' })
        : t('e.g. {command} or {other}', { command: 'konsole', other: 'xdg-open https://…' }),
  );

  function assign(e?: Event) {
    e?.preventDefault();
    if (cmd.trim() && online) onassign(cmd.trim());
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
  <PanelHead back={t('Back to actions')} {onback} title={t('Run command')} icon="terminal">
    <p class="small muted ellipsis">{context}</p>
  </PanelHead>
  <form class="form" onsubmit={assign}>
    <input
      bind:this={input}
      class="text-input mono"
      type="text"
      bind:value={cmd}
      {placeholder}
      aria-label={t('Command to run')}
      spellcheck="false"
      autocomplete="off"
    />
    <p class="small muted">{t('Runs as your user every time you press the button')}</p>
    <div class="overline">{t('Examples')}</div>
    <div class="chips">
      {#each examples as ex (ex)}
        <Chip mono onclick={() => { cmd = ex; input?.focus(); }}>{ex}</Chip>
      {/each}
    </div>
    <div class="actions">
      <Button onclick={onback}>{t('Cancel')}</Button>
      <Button variant="primary" type="submit" disabled={!cmd.trim() || !online}>{t('Assign')}</Button>
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
  .overline {
    margin-top: var(--space-2);
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
  .actions {
    margin-top: auto;
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
    padding-top: var(--space-3);
  }
</style>
