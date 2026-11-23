<script lang="ts">
  import Dialog from '../components/Dialog.svelte';
  import Button from '../components/Button.svelte';
  import Icon from '../components/Icon.svelte';
  import StatusPill from '../components/StatusPill.svelte';
  import Banner from '../components/Banner.svelte';
  import { shell, errorText } from '../backend';
  import { app, platform, refreshStatus, refreshDevices, toast } from '../state/app.svelte';
  import { describePermission, permissionsIntro } from '../permissions';
  import { t } from '../i18n/index.svelte';
  import type { Permission } from '../types';

  interface Props { open: boolean; onclose: () => void }
  let { open, onclose }: Props = $props();

  const perms = $derived(app.status?.permissions ?? []);
  const missing = $derived(perms.filter((p) => p.state !== 'ok'));
  const os = $derived(platform());
  let busy = $state<string | null>(null);
  let result = $state<{ kind: 'ok' | 'error' | 'info'; title: string; text: string } | null>(null);
  let manual = $state('');

  $effect(() => {
    if (open) {
      result = null;
      manual = '';
    }
  });

  function stateText(p: Permission): { text: string; tone: 'ok' | 'warn' | 'muted' } {
    if (p.state === 'ok') return { text: t('Allowed'), tone: 'ok' };
    if (p.state === 'missing') return { text: t('Missing'), tone: 'warn' };
    return { text: t('Unknown'), tone: 'muted' };
  }

  async function fix(p: Permission) {
    busy = p.id;
    result = null;
    try {
      const r = await shell.setupPermission(p.id);
      if (r.opened || (!r.ok && os === 'macos')) {
        result = { kind: 'info', title: t('Opened System Settings'), text: t('Turn on {app} there, then come back: this window updates by itself.') };
      } else if (r.ok) {
        result = { kind: 'ok', title: t('All set'), text: t('Turn your mouse off and on again, or unplug it and plug it back in') };
        toast(t('All set'));
      } else if (r.cancelled) {
        result = { kind: 'info', title: t('Installation cancelled'), text: t('Nothing was changed. You can try again whenever you like.') };
      } else {
        result = { kind: 'error', title: t("Couldn't install the permissions"), text: r.error || t('Try again, or use the manual command in a terminal.') };
      }
      manual = r.manual ?? '';
    } catch (e) {
      result = { kind: 'error', title: t("Couldn't install the permissions"), text: errorText(e) };
    } finally {
      busy = null;
      await Promise.all([refreshStatus(), refreshDevices()]);
    }
  }

  async function copyManual() {
    try {
      await navigator.clipboard.writeText(manual);
      toast(t('Command copied'));
    } catch {
      toast(t("Couldn't copy to the clipboard"), 'error');
    }
  }
</script>

<Dialog {open} {onclose} title={t('Permission to use your devices')} icon="shield" width={580} description={permissionsIntro(os)}>
  {#if perms.length}
    <ul class="list">
      {#each perms as p (p.id)}
        {@const d = describePermission(p.id)}
        {@const s = stateText(p)}
        <li>
          <span class="icon"><Icon name={d.icon} size={18} /></span>
          <div class="text">
            <strong>{d.title}</strong>
            {#if d.text}<span class="muted small">{d.text}</span>{/if}
          </div>
          {#if p.state !== 'ok' && p.fix}
            <Button size="sm" variant="primary" busy={busy === p.id} disabled={!!busy && busy !== p.id} onclick={() => fix(p)}>
              {os === 'macos' ? t('Open System Settings') : t('Install permissions')}
            </Button>
          {:else}
            <StatusPill text={s.text} tone={s.tone} />
          {/if}
        </li>
      {/each}
    </ul>
    {#if os === 'linux' && missing.length}
      <p class="small subtle">{t('Your system will ask for your administrator password')} · {t('No new software is installed, and {app} keeps running as your user')}</p>
    {/if}
  {:else}
    <Banner kind="ok" title={t('Nothing to do')} text={t('Windows lets {app} use your devices without extra permissions.')} />
  {/if}
  {#if result}
    <Banner kind={result.kind === 'error' ? 'error' : result.kind === 'ok' ? 'ok' : 'info'} title={result.title} text={result.text} />
  {/if}
  {#if manual}
    <div class="manual">
      <p class="small muted">{t('If you prefer, copy this command, paste it into a terminal, then turn your mouse off and on again.')}</p>
      <pre class="mono selectable">{manual}</pre>
      <div><Button size="sm" icon="copy" onclick={copyManual}>{t('Copy command')}</Button></div>
    </div>
  {/if}
  {#snippet footer()}
    {#if missing.length}
      <Button onclick={onclose}>{t('Not now')}</Button>
    {:else}
      <Button variant="primary" onclick={onclose}>{t('Done')}</Button>
    {/if}
  {/snippet}
</Dialog>

<style>
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  li {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    border-radius: var(--radius-m);
    background: var(--surface-2);
    border: 1px solid var(--border);
  }
  .icon {
    display: grid;
    place-items: center;
    flex: none;
    width: 34px;
    height: 34px;
    border-radius: var(--radius-s);
    background: var(--accent-soft);
    color: var(--accent);
  }
  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .manual {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  pre {
    margin: 0;
    padding: var(--space-3);
    border-radius: var(--radius-s);
    background: var(--surface-2);
    border: 1px solid var(--border);
    white-space: pre-wrap;
    word-break: break-all;
  }
</style>
