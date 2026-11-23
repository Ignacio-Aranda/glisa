<script lang="ts">
  import Button from '../../components/Button.svelte';
  import StatusPill from '../../components/StatusPill.svelte';
  import Banner from '../../components/Banner.svelte';
  import EmptyState from '../../components/EmptyState.svelte';
  import { api, errorText } from '../../backend';
  import { refreshDevice, toast } from '../../state/app.svelte';
  import { confirm } from '../../state/confirm.svelte';
  import { kindNoun } from '../../format';
  import { t, tn } from '../../i18n/index.svelte';
  import type { Device } from '../../types';

  interface Props { dev: Device }
  let { dev }: Props = $props();

  /** The channel button; the hyphen must not break across lines. */
  const EASY_SWITCH = 'Easy‑Switch';
  const GRACE_MS = 10000;

  let target = $state<number | null>(null);   // channel we just sent the device to
  let left = $state(false);                    // …and it has dropped off this computer since
  let grace: ReturnType<typeof setTimeout> | null = null;
  let shownId = '';

  const hosts = $derived(dev.hosts ?? { count: 0, current: null, entries: [] });
  const count = $derived(Number(hosts.count || 0));
  const current = $derived(hosts.current);
  const online = $derived(dev.online);
  const noun = $derived(kindNoun(dev.kind));

  function forget() {
    target = null;
    left = false;
    if (grace) clearTimeout(grace);
    grace = null;
  }

  $effect(() => {
    if (dev.id !== shownId) {
      shownId = dev.id;
      forget();
    }
  });

  $effect(() => {
    if (target === null) return;
    if (!online) {
      left = true;               // as expected: it went to the other computer
      if (grace) clearTimeout(grace);
      grace = null;
    } else if (left) {
      forget();                  // and now it is back with us
    }
  });

  $effect(() => () => {
    if (grace) clearTimeout(grace);
  });

  const intro = $derived(count === 1
    ? t('This {device} can only be paired with one computer.', { device: noun })
    : tn('This {device} can be paired with up to {count} computer at once. Choose here which one to use it with, or press the {button} button on the {device} itself.',
      'This {device} can be paired with up to {count} computers at once. Choose here which one to use it with, or press the {button} button on the {device} itself.',
      count, { device: noun, count, button: EASY_SWITCH }));

  interface CardState { title: string; detail: string; pill: { text: string; tone: 'ok' | 'info' } | null; here: boolean; button: string }

  function card(i: number): CardState {
    const entry = hosts.entries[i] ?? { index: i, paired: null, name: '' };
    const name = String(entry.name ?? '').trim();
    const paired = entry.paired;
    const isCurrent = i === current;
    const here = isCurrent && online && !left;
    let title: string;
    let detail: string;
    if (isCurrent) [title, detail] = name ? [name, t('This computer')] : [t('This computer'), ''];
    else if (paired === false) [title, detail] = [t('Not paired'), t('Free channel for another computer')];
    else if (name) [title, detail] = [name, t('Paired')];
    else if (paired) [title, detail] = [t('Computer {n}', { n: i + 1 }), t('Paired')];
    else [title, detail] = [t('Channel {n}', { n: i + 1 }), ''];
    const pill = here ? { text: t('Connected here'), tone: 'ok' as const }
      : target === i && !left ? { text: t('Switching…'), tone: 'info' as const } : null;
    const button = isCurrent ? '' : paired === false ? t('Switch and pair') : t('Switch');
    return { title, detail, pill, here, button };
  }

  const note = $derived.by((): { kind: 'info' | 'warn'; title: string; text: string } | null => {
    const known = typeof current === 'number';
    const channel = known ? (current as number) + 1 : 0;
    if (target !== null && left) {
      const back = known
        ? t('To use it here again, press its {button} button until channel {n} is selected, or switch it from the other computer.', { button: EASY_SWITCH, n: channel })
        : t('To use it here again, press its {button} button or switch it from the other computer.', { button: EASY_SWITCH });
      return { kind: 'info', title: t('The {device} is now on channel {n}', { device: noun, n: target + 1 }), text: back };
    }
    if (target !== null) {
      return { kind: 'info', title: t('Switching to channel {n}…', { n: target + 1 }), text: t('The {device} will disconnect from this computer in a few seconds.', { device: noun }) };
    }
    if (!online) {
      const text = known
        ? t('It may be off, asleep or in use with another computer. If you switched its channel, press its {button} button until channel {n} is selected to come back.', { button: EASY_SWITCH, n: channel })
        : t('It may be off, asleep or in use with another computer. If you switched its channel, press its {button} button to come back.', { button: EASY_SWITCH });
      return { kind: 'warn', title: t('Device disconnected'), text };
    }
    return null;
  });

  async function request(index: number) {
    if (!online || target !== null || index === current || index < 0 || index >= count) return;
    const entry = hosts.entries[index] ?? { index, paired: null, name: '' };
    const name = String(entry.name ?? '').trim();
    const n = index + 1;
    const back = typeof current === 'number'
      ? t('To come back, press the {button} button on the {device} until channel {current} is selected, or switch it from the other computer.', { button: EASY_SWITCH, device: noun, current: current + 1 })
      : t('To come back, press the {button} button on the {device} or switch it from the other computer.', { button: EASY_SWITCH, device: noun });
    let title: string;
    let text: string;
    let accept: string;
    if (entry.paired === false) {
      title = t('Switch to channel {n} and pair?', { n });
      text = t('The {device} will disconnect from this computer and wait for another computer to pair with it on channel {n}. {back}', { device: noun, n, back });
      accept = t('Switch and pair');
    } else {
      title = t('Switch to channel {n}?', { n });
      text = name
        ? t('The {device} will disconnect from this computer and move to “{name}”. {back}', { device: noun, name, back })
        : t('The {device} will disconnect from this computer and move to the computer on channel {n}. {back}', { device: noun, n, back });
      accept = t('Switch');
    }
    const id = dev.id;
    if (!(await confirm({ title, text, confirm: accept, icon: 'arrow-left-right' }))) return;
    if (dev.id !== id || !dev.online) return;          // things changed while the question was open
    target = index;
    left = false;
    try {
      await api('set_host', { id, host: index });
      toast(t('Switching to channel {n}…', { n }), 'info');
      if (target !== null && !left) {
        grace = setTimeout(() => {
          if (target !== null && !left) forget();     // it never dropped off (the other computer may be off)
        }, GRACE_MS);
      }
    } catch (e) {
      forget();
      toast(errorText(e), 'error');
    }
    await refreshDevice(id);
  }
</script>

<div class="page">
  {#if count === 0}
    <EmptyState icon="laptop" title={t('No computer switching')} text={t('This device can only be paired with one computer.')} />
  {:else}
    <p class="muted">{intro}</p>
    {#if note}<Banner kind={note.kind} title={note.title} text={note.text} />{/if}
    <div class="grid">
      {#each Array.from({ length: count }, (_, i) => i) as i (i)}
        {@const c = card(i)}
        <article class="card" class:here={c.here} aria-label={t('Channel {n}', { n: i + 1 })}>
          <div class="top">
            <span class="badge" class:active={c.here} aria-hidden="true">{i + 1}</span>
            {#if c.pill}<StatusPill text={c.pill.text} tone={c.pill.tone} />{/if}
          </div>
          <div class="text">
            <h3 class="ellipsis">{c.title}</h3>
            {#if c.detail}<p class="muted small">{c.detail}</p>{/if}
          </div>
          {#if c.button}
            <Button full disabled={!online || target !== null} onclick={() => request(i)}
              label={t('{action} to channel {n}', { action: c.button, n: i + 1 })}>{c.button}</Button>
          {/if}
        </article>
      {/each}
    </div>
  {/if}
</div>

<style>
  .page {
    padding: var(--space-5) var(--space-7) var(--space-8);
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    max-width: 1120px;
    width: 100%;
    margin: 0 auto;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: var(--space-4);
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-5);
    border-radius: var(--radius-l);
    background: var(--surface);
    border: 1px solid var(--border);
    box-shadow: var(--shadow-1);
    min-height: 188px;
  }
  .card.here {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft), var(--shadow-1);
  }
  .top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
  }
  .badge {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    border-radius: 50%;
    border: 2px solid var(--border-strong);
    font-size: var(--text-xl);
    font-weight: 700;
    color: var(--text);
  }
  .badge.active {
    border-color: var(--accent);
    background: var(--accent-soft);
    color: var(--accent);
  }
  .text {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  @media (max-width: 1000px) {
    .page { padding-inline: var(--space-5); }
  }
</style>
