<script lang="ts">
  import Dialog from '../../components/Dialog.svelte';
  import Button from '../../components/Button.svelte';
  import Icon from '../../components/Icon.svelte';
  import Spinner from '../../components/Spinner.svelte';
  import { api, errorText } from '../../backend';
  import { app, refreshFlow } from '../../state/app.svelte';
  import { checkAddress, MAX_INPUT } from '../../address';
  import { clock, formatCode } from '../../format';
  import { sentence, t, tc } from '../../i18n/index.svelte';
  import type { Pairing } from '../../types';

  interface Props { open: boolean; onclose: () => void }
  let { open, onclose }: Props = $props();

  /** Set when the dialog itself decides how it ended (errors, cancellation). */
  let result = $state<{ ok: boolean; text: string } | null>(null);
  let started = $state(false);
  let decided = $state(false);
  let byAddress = $state(false);
  let address = $state('');
  let now = $state(Date.now());
  let deadline = $state(0);

  const pairing = $derived<Pairing | null>(app.flow?.pairing ?? null);
  const discovered = $derived(app.flow?.discovered ?? []);
  const me = $derived(app.flow?.name ?? '');
  const peer = $derived(pairing?.peer?.name || t('the other computer'));
  const check = $derived(checkAddress(address));

  async function start() {
    result = null;
    decided = false;
    started = false;
    try {
      const p = await api<Pairing | null>('flow_pair_start');
      if (p?.expires_in) deadline = Date.now() + p.expires_in * 1000;
      void api('flow_discover', { wait: 1 }).catch(() => {});
      await refreshFlow();
      started = true;
    } catch (e) {
      result = { ok: false, text: sentence(errorText(e)) };
    }
  }

  $effect(() => {
    if (!open) return;
    byAddress = false;
    address = '';
    void start();
    const tick = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(tick);
  });

  // keep the local clock in step with the service's answer
  $effect(() => {
    if (pairing && ['waiting', 'connecting', 'confirm'].includes(pairing.state)) {
      const d = Date.now() + pairing.expires_in * 1000;
      if (Math.abs(d - deadline) > 2000) deadline = d;
    }
  });

  // How the dialog ends: from the service's pairing state.
  $effect(() => {
    if (!open || result || !started) return;
    const p = pairing;
    if (!p) {
      result = { ok: false, text: t('Pairing was cancelled') };
    } else if (p.state === 'done') {
      result = { ok: true, text: t('Paired with {name}. Place it in the layout and check which channel your mouse uses for it', { name: peer }) };
    } else if (p.state === 'failed' || p.state === 'expired') {
      result = { ok: false, text: p.message || t("Pairing didn't complete") };
    }
  });

  const phase = $derived(result ? 'result' : !pairing || pairing.state === 'waiting' ? 'pick' : pairing.state === 'connecting' ? 'connecting' : 'confirm');
  const left = $derived(Math.max(0, Math.round((deadline - now) / 1000)));

  async function request(params: Record<string, unknown>) {
    try {
      await api('flow_pair_request', params);
    } catch (e) {
      result = { ok: false, text: sentence(errorText(e)) };
    }
    await refreshFlow();
  }

  async function decide(accept: boolean) {
    decided = true;
    try {
      await api('flow_pair_confirm', { accept });
    } catch (e) {
      result = { ok: false, text: sentence(errorText(e)) };
    }
    await refreshFlow();
  }

  function close() {
    if (!result?.ok) void api('flow_pair_cancel').catch(() => {}).then(() => refreshFlow());
    onclose();
  }

  function connectAddress(e?: Event) {
    e?.preventDefault();
    if (check.address) void request({ address: check.address });
  }

  const title = $derived(
    phase === 'result' ? (result?.ok ? t('Computer paired') : t("Couldn't pair")) : phase === 'confirm' ? t('Check the number') : t('Pair a computer'),
  );
</script>

<Dialog {open} {title} icon={phase === 'result' ? (result?.ok ? 'circle-check' : 'triangle-exclamation-point') : 'arrow-left-right'}
  iconTone={phase === 'result' ? (result?.ok ? 'ok' : 'warn') : 'accent'} width={540} onclose={close}>
  {#if phase === 'pick'}
    <p class="muted">{t('Open “Pair a computer” on the other computer too. Then choose it here, or choose “{name}” over there', { name: me || t('this computer') })}</p>
    {#if discovered.length}
      <ul class="found">
        {#each discovered as d (d.id)}
          <li>
            <span class="icon"><Icon name="laptop" size={20} /></span>
            <span class="text">
              <strong class="ellipsis">{d.name}</strong>
              <span class="small muted">{d.compatible ? d.address : t('Different version of {app}: update it')}</span>
            </span>
            <Button variant="primary" size="sm" disabled={!d.compatible} label={t('Pair with {name}', { name: d.name })} onclick={() => request({ peer: d.id })}>{t('Pair')}</Button>
          </li>
        {/each}
      </ul>
    {/if}
    <div class="searching small muted"><Spinner size={14} /> {discovered.length ? t('Looking for more computers…') : t('Looking for computers on the network…')}</div>
    {#if !byAddress}
      <div><Button variant="link" onclick={() => (byAddress = true)}>{t('Pair by address…')}</Button></div>
    {:else}
      <form class="address" onsubmit={connectAddress}>
        <p class="small muted">{t('Is it on a different network? Type its IP address or name, and the port if it isn\'t 27461. Open “Pair a computer” there first')}</p>
        <div class="row">
          <input class="text-input" type="text" bind:value={address} maxlength={MAX_INPUT} placeholder={t('For example 192.168.1.20')}
            aria-label={t('Address of the other computer')} aria-invalid={check.error} aria-describedby="pair-address-note" data-autofocus spellcheck="false" />
          <Button variant="primary" type="submit" disabled={!check.address}>{t('Connect')}</Button>
        </div>
        {#if check.message}<p id="pair-address-note" class="small" class:tone-danger={check.error} class:subtle={!check.error}>{check.message}</p>{/if}
      </form>
    {/if}
  {:else if phase === 'connecting'}
    <div class="connecting"><Spinner size={22} /><span>{t('Connecting to {name}…', { name: peer })}</span></div>
  {:else if phase === 'confirm' && pairing}
    <p class="muted">
      {pairing.local
        ? t('You confirmed this number. Confirm it on {name} too', { name: peer })
        : t("{name} must show exactly the same numbers. Compare them and confirm on both computers. If they don't match, someone could be intercepting the connection", { name: peer })}
    </p>
    <div class="code" aria-label={t('Pairing code')}>
      {#each formatCode(pairing.sas).split('') as ch, i (i)}
        {#if ch === ' '}<span class="gap"></span>{:else}<span class="digit">{ch}</span>{/if}
      {/each}
    </div>
    {#if pairing.local || pairing.remote}
      <div class="waiting small muted">
        {#if pairing.local}<Spinner size={14} /> {t('Waiting for {name}…', { name: peer })}{:else}<Icon name="check" size={14} /> {t('{name} has already confirmed', { name: peer })}{/if}
      </div>
    {/if}
  {:else if result}
    <p>{result.text}</p>
  {/if}

  {#snippet footerStart()}
    {#if phase !== 'result' && left}<span class="small subtle">{t('Closes in {time}', { time: clock(left) })}</span>{/if}
  {/snippet}
  {#snippet footer()}
    {#if phase === 'confirm' && pairing}
      <Button variant="danger" disabled={decided || !!pairing.local} onclick={() => decide(false)}>{t("They don't match")}</Button>
      <Button variant="primary" disabled={decided || !!pairing.local} onclick={() => decide(true)} data-autofocus>{t('They match')}</Button>
    {:else if phase === 'result'}
      {#if !result?.ok}<Button variant="primary" onclick={start}>{t('Try again')}</Button>{/if}
      <Button variant={result?.ok ? 'primary' : 'secondary'} onclick={onclose}>{result?.ok ? tc('pairing', 'Done') : t('Close')}</Button>
    {:else}
      <Button onclick={close}>{t('Cancel')}</Button>
    {/if}
  {/snippet}
</Dialog>

<style>
  .found {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .found li {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    border-radius: var(--radius-m);
    border: 1px solid var(--border);
    background: var(--surface-2);
  }
  .icon {
    color: var(--text-muted);
    display: grid;
  }
  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .searching,
  .waiting {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .address {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .row {
    display: flex;
    gap: var(--space-2);
  }
  .connecting {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) 0;
  }
  .code {
    display: flex;
    justify-content: center;
    gap: var(--space-2);
    padding: var(--space-3) 0;
  }
  .digit {
    display: grid;
    place-items: center;
    width: 48px;
    height: 60px;
    border-radius: var(--radius-m);
    background: var(--surface-2);
    border: 1px solid var(--border-strong);
    font-size: 30px;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
  }
  .gap {
    width: var(--space-3);
  }
</style>
