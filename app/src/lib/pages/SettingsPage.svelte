<script lang="ts">
  import Panel from '../components/Panel.svelte';
  import SettingRow from '../components/SettingRow.svelte';
  import Segmented from '../components/Segmented.svelte';
  import Toggle from '../components/Toggle.svelte';
  import Button from '../components/Button.svelte';
  import Divider from '../components/Divider.svelte';
  import StatusPill from '../components/StatusPill.svelte';
  import IconButton from '../components/IconButton.svelte';
  import Wordmark from '../components/Wordmark.svelte';
  import { app, navigate, platform, toast, updateSettings } from '../state/app.svelte';
  import { describePermission } from '../permissions';
  import { shell } from '../backend';
  import { LANGUAGES, t, tc } from '../i18n/index.svelte';
  import type { AppSettings, Theme } from '../types';

  const s = $derived(app.settings);
  const perms = $derived(app.status?.permissions ?? []);
  const missing = $derived(perms.filter((p) => p.state === 'missing'));
  const os = $derived(platform());
  const REPO = 'https://github.com/Ignacio-Aranda/glisa';

  async function setAutostart(on: boolean) {
    if (await updateSettings({ autostart: on })) {
      toast(on ? t('{app} will start when you log in') : t('{app} will no longer start when you log in'));
    }
  }

  async function setNotifications(on: boolean) {
    await updateSettings({ notifications: on });
  }

  async function setPhotos(on: boolean) {
    await updateSettings({ official_photos: on });
  }

  async function openLink(url: string) {
    try {
      await shell.openUrl(url);
    } catch {
      toast(t("Couldn't open the link"), 'error');
    }
  }
</script>

<div class="page">
  <header class="head">
    <IconButton icon="chevron-left" label={tc('navigation', 'Back')} tooltip={t('Back (Esc)')} onclick={() => navigate({ page: 'devices' })} />
    <h1>{t('App settings')}</h1>
  </header>

  <Panel title={t('Appearance')} icon="sun-moon">
    <SettingRow title={t('Theme')} subtitle={t('“System” follows the light or dark mode of your desktop')}>
      {#snippet control()}
        <Segmented
          label={t('Theme')}
          value={s.theme}
          options={[
            { value: 'system' as Theme, label: t('System'), icon: 'monitor', title: t('System theme') },
            { value: 'light' as Theme, label: tc('theme', 'Light'), icon: 'sun', title: t('Light theme') },
            { value: 'dark' as Theme, label: tc('theme', 'Dark'), icon: 'moon', title: t('Dark theme') },
          ]}
          onchange={(v) => updateSettings({ theme: v })}
        />
      {/snippet}
    </SettingRow>
    <Divider />
    <SettingRow title={t('App language')} subtitle={t('“System” follows the language of your desktop')}>
      {#snippet control()}
        <Segmented
          label={t('App language')}
          value={s.language}
          options={[
            { value: 'system' as AppSettings['language'], label: t('System') },
            ...LANGUAGES.map((l) => ({ value: l.code as AppSettings['language'], label: l.name })),
          ]}
          onchange={(v) => updateSettings({ language: v })}
        />
      {/snippet}
    </SettingRow>
  </Panel>

  <Panel title={t('Startup and notifications')} icon="rocket">
    <SettingRow title={t('Start when you log in')} subtitle={t('{app} starts in the background when you log in, so your buttons and settings work without opening this window')}>
      {#snippet control()}<Toggle checked={s.autostart} label={t('Start when you log in')} onchange={setAutostart} />{/snippet}
    </SettingRow>
    <Divider />
    <SettingRow title={t('Let me know when a battery is low')} subtitle={t("You'll get a notice when a device is down to 15% or less")}>
      {#snippet control()}<Toggle checked={s.notifications} label={t('Let me know when a battery is low')} onchange={setNotifications} />{/snippet}
    </SettingRow>
  </Panel>

  <Panel title={t('Device pictures')} icon="image">
    <SettingRow
      title={t('Show product photos from logitech.com')}
      subtitle={t('Downloads the official photo of each device from logitech.com and shows it on its card in Devices and on its pages, unless you gave the device a picture of your own. Off: nothing is downloaded.')}
    >
      {#snippet control()}<Toggle checked={s.official_photos} label={t('Show product photos from logitech.com')} onchange={setPhotos} />{/snippet}
    </SettingRow>
  </Panel>

  <Panel title="Flow" icon="arrow-left-right">
    <SettingRow title={t('Use the same mouse on two computers')} subtitle={t('Move the pointer to the edge of the screen to take the mouse to another computer running {app}')}>
      {#snippet control()}<Button icon="arrow-left-right" onclick={() => navigate({ page: 'flow' })}>{t('Open Flow')}</Button>{/snippet}
    </SettingRow>
  </Panel>

  <Panel title={t('Permissions')} subtitle={t('What {app} needs from the system to work')} icon="shield">
    {#if perms.length}
      {#each perms as p, i (p.id)}
        {@const d = describePermission(p.id)}
        {#if i}<Divider />{/if}
        <SettingRow title={d.title} subtitle={d.text} icon={d.icon}>
          {#snippet control()}
            {#if p.state === 'ok'}
              <StatusPill text={t('Allowed')} tone="ok" />
            {:else if p.state === 'missing'}
              <StatusPill text={t('Missing')} tone="warn" />
            {:else}
              <StatusPill text={t('Unknown')} tone="muted" />
            {/if}
          {/snippet}
        </SettingRow>
      {/each}
      {#if (app.status?.denied?.length ?? 0) > 0}
        <Divider />
        <SettingRow title={t('Devices without access')} subtitle={t('Turn them off and on (or unplug them and plug them back in) for the permission to take effect')}>
          {#snippet control()}<StatusPill text={String(app.status?.denied.length)} tone="warn" />{/snippet}
        </SettingRow>
      {/if}
      <div>
        <Button icon="shield" variant={missing.length ? 'primary' : 'secondary'} onclick={() => (app.dialog = 'permissions')}>
          {missing.length ? (os === 'macos' ? t('Review permissions') : t('Install permissions')) : t('Review permissions')}
        </Button>
      </div>
    {:else}
      <p class="muted">{os === 'windows' ? t('Windows lets {app} use your devices without extra permissions.') : t('No information')}</p>
    {/if}
  </Panel>

  <section class="about" aria-label={t('About')}>
    <div class="about-text">
      <Wordmark height={34} />
      <span class="muted">{t('Version {version}', { version: app.info.version || app.status?.version || '?' })}</span>
      <p class="small muted">{t('Set up the buttons, pointer and wheel of your Logitech mice and keyboards')}</p>
      <p class="small subtle">{t('License: {license}', { license: 'GPL-3.0-or-later' })} · {t('Independent project, not affiliated with Logitech. Trademarks belong to their owners.')}</p>
      <div><Button variant="link" iconEnd="external-link" onclick={() => openLink(REPO)}>{t('Website and source code')}</Button></div>
    </div>
  </section>
</div>

<style>
  .page {
    max-width: 960px;
    padding: var(--space-6) var(--space-7) var(--space-8);
    display: flex;
    flex-direction: column;
    gap: var(--space-5);
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    margin-left: calc(-1 * var(--space-2));
  }
  .about {
    display: flex;
    align-items: flex-start;
    gap: var(--space-4);
    padding: var(--space-5) var(--space-6);
    border-radius: var(--radius-l);
    border: 1px dashed var(--border-strong);
  }
  .about-text {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  @media (max-width: 1000px) {
    .page { padding-inline: var(--space-5); }
  }
</style>
