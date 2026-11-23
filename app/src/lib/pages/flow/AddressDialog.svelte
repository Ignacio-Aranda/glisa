<script lang="ts">
  import Dialog from '../../components/Dialog.svelte';
  import Button from '../../components/Button.svelte';
  import { checkAddress, MAX_INPUT } from '../../address';
  import { t } from '../../i18n/index.svelte';
  import type { FlowPeer } from '../../types';

  interface Props {
    peer: FlowPeer | null;
    onclose: () => void;
    /** 'host:port', or '' to find it automatically. */
    onsave: (address: string) => void;
  }
  let { peer, onclose, onsave }: Props = $props();
  let text = $state('');
  let lastId = '';

  $effect(() => {
    if (peer && peer.id !== lastId) {
      lastId = peer.id;
      text = peer.manual_address ?? '';
    }
    if (!peer) lastId = '';
  });

  const name = $derived(peer?.name || t('the other computer'));
  const current = $derived(peer?.manual_address ?? '');
  const check = $derived(checkAddress(text));
  const note = $derived(check.address === '' ? t('Empty: it will be found automatically') : check.message);
  const canSave = $derived(check.address !== null && check.address !== current);

  function save(e?: Event) {
    e?.preventDefault();
    if (!canSave) return;
    onsave(check.address ?? '');
  }
</script>

<Dialog open={!!peer} title={t('Address of {name}', { name })} icon="network" width={500} {onclose}>
  <form id="address-form" class="form" onsubmit={save}>
    <p class="muted">{t("Computers on the same network find each other by themselves. If {name} is on a different one (for example, one on Wi-Fi and the other wired), type its IP address or name, and the port if it isn't 27461. Once one of the two connects, both learn each other's address", { name })}</p>
    <input class="text-input" type="text" bind:value={text} maxlength={MAX_INPUT} placeholder={t('For example 192.168.1.20')}
      aria-label={t('Address')} aria-invalid={check.error} aria-describedby="address-note" data-autofocus spellcheck="false" />
    <p id="address-note" class="small" class:tone-danger={check.error} class:subtle={!check.error}>{note}</p>
  </form>
  {#snippet footerStart()}
    <Button variant="ghost" disabled={!current} title={t('Forget the address you typed and find {name} automatically', { name })} onclick={() => onsave('')}>{t('Automatic')}</Button>
  {/snippet}
  {#snippet footer()}
    <Button onclick={onclose}>{t('Cancel')}</Button>
    <Button variant="primary" type="submit" form="address-form" disabled={!canSave}>{t('Save')}</Button>
  {/snippet}
</Dialog>

<style>
  .form {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  p.small {
    min-height: 1.3em;
  }
</style>
