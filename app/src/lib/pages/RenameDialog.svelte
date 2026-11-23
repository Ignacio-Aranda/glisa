<script lang="ts">
  import Dialog from '../components/Dialog.svelte';
  import Button from '../components/Button.svelte';
  import { identity, saveName } from '../state/identity.svelte';
  import { t } from '../i18n/index.svelte';

  const MAX = 40;
  const dev = $derived(identity.renaming);
  let name = $state('');
  let error = $state('');
  let busy = $state(false);
  let lastId = '';

  $effect(() => {
    const d = identity.renaming;
    if (d && d.id !== lastId) {
      lastId = d.id;
      name = d.name === d.model ? '' : d.name;
      error = '';
    }
    if (!d) lastId = '';
  });

  const clean = $derived(name.replace(/\s+/g, ' ').trim());
  const length = $derived([...clean].length);
  const tooLong = $derived(length > MAX);
  const note = $derived(
    error ? error
      : tooLong ? t('Too long: {n} characters, {max} at most', { n: length, max: MAX })
      : !clean ? t('Empty: the model name, {model}, will be used', { model: dev?.model ?? '' })
      : t('{n} of {max} characters', { n: length, max: MAX }),
  );

  function close() {
    identity.renaming = null;
  }

  async function save(e?: Event) {
    e?.preventDefault();
    if (!dev || tooLong || busy) return;
    busy = true;
    error = await saveName(dev, clean);
    busy = false;
    if (!error) close();
  }

  async function useModel() {
    name = '';
    await save();
  }
</script>

<Dialog open={!!dev} title={t('Rename device')} icon="pen-line" onclose={close} width={480}>
  <form id="rename-form" onsubmit={save} class="form">
    <p class="muted">{t('Choose the name {app} shows for your {model}, for example “Office mouse”. The device itself doesn\'t change.', { model: dev?.model ?? '' })}</p>
    <label class="field">
      <span class="small">{t('Name')}</span>
      <input
        class="text-input"
        type="text"
        bind:value={name}
        placeholder={dev?.model ?? ''}
        maxlength={120}
        aria-invalid={tooLong || !!error}
        aria-describedby="rename-note"
        data-autofocus
      />
    </label>
    <p id="rename-note" class="small" class:tone-danger={tooLong || !!error} class:subtle={!tooLong && !error}>{note}</p>
  </form>
  {#snippet footerStart()}
    {#if dev && dev.name !== dev.model}
      <Button variant="ghost" onclick={useModel} title={t('Show it as {model} again', { model: dev.model })}>{t('Use model name')}</Button>
    {/if}
  {/snippet}
  {#snippet footer()}
    <Button onclick={close}>{t('Cancel')}</Button>
    <Button variant="primary" type="submit" form="rename-form" disabled={tooLong} {busy}>{t('Save')}</Button>
  {/snippet}
</Dialog>

<style>
  .form {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
</style>
