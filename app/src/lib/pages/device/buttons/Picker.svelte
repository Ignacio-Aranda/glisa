<script lang="ts">
  import { tick } from 'svelte';
  import Icon from '../../../components/Icon.svelte';
  import SearchField from '../../../components/SearchField.svelte';
  import KeyCaps from '../../../components/KeyCaps.svelte';
  import Button from '../../../components/Button.svelte';
  import PanelHead from './PanelHead.svelte';
  import {
    DISABLED_ACTION, categoryIcon, findPreset, hint, iconFor, presetLabel, sameAction, typeIcon,
  } from '../../../actions';
  import { btc, fold, t, tc } from '../../../i18n/index.svelte';
  import type { Action, Platform, PresetCategory, SimpleAction } from '../../../types';

  type Key = 'default' | 'disabled' | 'none' | 'gesture' | 'preset' | 'keys' | 'command';
  interface Entry { key: Key; icon: string; title: string; action?: SimpleAction; trailing?: string; words: string; chevron?: boolean }
  interface Group { title: string; icon?: string; entries: Entry[] }

  interface Props {
    title: string;
    icon: string;
    back: string;
    current: Action;
    currentText: string;
    customised: boolean;
    /** Choosing for one gesture direction (no Default, Disabled or Gesture; "None" instead). */
    slotMode: boolean;
    /** The control can be a gesture button. */
    gestures: boolean;
    /** The control can only act as another mouse button. */
    mouseOnly: boolean;
    online: boolean;
    presets: PresetCategory[];
    platform: Platform;
    onback: () => void;
    onchoose: (action: SimpleAction | null) => void;
    ongesture: () => void;
    onshortcut: () => void;
    oncommand: () => void;
  }
  let {
    title, icon, back, current, currentText, customised, slotMode, gestures, mouseOnly, online, presets, platform,
    onback, onchoose, ongesture, onshortcut, oncommand,
  }: Props = $props();

  let query = $state('');
  let search: HTMLInputElement | null = $state(null);
  let list: HTMLDivElement | null = $state(null);

  export function focusSearch() {
    search?.focus();
  }

  export function reset() {
    query = '';
    list?.scrollTo?.({ top: 0 });
  }

  /** Scrolls the current choice into view. */
  export async function reveal() {
    await tick();
    const el = list?.querySelector<HTMLElement>('[aria-current="true"]');
    el?.scrollIntoView?.({ block: 'nearest' });
  }

  const groups = $derived.by((): Group[] => {
    const top: Entry[] = [
      { key: 'default', icon: typeIcon('default'), title: btc('button action', 'Default'),
        words: tc('search keywords', 'original factory normal reset restore') },
      { key: 'disabled', icon: typeIcon('disabled'), title: btc('button action', 'Disabled'), action: DISABLED_ACTION,
        words: tc('search keywords', 'none nothing no action off') },
      { key: 'none', icon: typeIcon('disabled'), title: btc('button action', 'None'), action: DISABLED_ACTION,
        words: tc('search keywords', 'nothing no action disabled off') },
      { key: 'gesture', icon: typeIcon('gesture'), title: t('Gesture button'), chevron: true,
        words: tc('search keywords', 'gestures hold move directions') },
    ];
    const out: Group[] = [{ title: '', entries: top }];
    for (const cat of presets) {
      const label = presetLabel(cat.category, cat.title);
      out.push({
        title: label,
        icon: categoryIcon(cat.category),
        entries: cat.items.map((item) => ({
          key: 'preset' as const, icon: iconFor(item.action, presets), title: presetLabel(item.label, item.title), action: item.action,
          trailing: hint(item.action), words: `${label} ${hint(item.action)}`,
        })),
      });
    }
    out.push({
      title: t('Custom'),
      entries: [
        { key: 'keys', icon: typeIcon('keys'), title: t('Keyboard shortcut…'), chevron: true,
          words: tc('search keywords', 'keys key combination hotkey custom') },
        { key: 'command', icon: typeIcon('command'), title: t('Run command…'), chevron: true,
          words: tc('search keywords', 'program application app open launch script terminal custom') },
      ],
    });
    return out;
  });

  function allowed(e: Entry): boolean {
    if (mouseOnly) return e.key === 'default' || (e.key === 'preset' && e.action?.type === 'mouse');
    if (e.key === 'default' || e.key === 'disabled') return !slotMode;
    if (e.key === 'none') return slotMode;
    if (e.key === 'gesture') return gestures && !slotMode;
    return true;
  }

  const visible = $derived.by(() => {
    const q = fold(query.trim());
    return groups
      .map((g) => ({ ...g, entries: g.entries.filter((e) => allowed(e) && (!q || fold(`${e.title} ${e.words}`).includes(q))) }))
      .filter((g) => g.entries.length);
  });

  const isPreset = $derived(findPreset(current, presets) !== null);

  function isCurrent(e: Entry): boolean {
    const kind = current.type ?? 'default';
    switch (e.key) {
      case 'default': return kind === 'default' && !slotMode;
      case 'disabled': case 'none': return kind === 'disabled' || (slotMode && kind === 'default');
      case 'gesture': return kind === 'gesture';
      case 'preset': return sameAction(e.action, current);
      case 'keys': return kind === 'keys' && !isPreset;
      case 'command': return kind === 'command' && !isPreset;
    }
  }

  function trailingOf(e: Entry): string {
    if (e.key === 'keys' && isCurrent(e) && current.type === 'keys') return current.combo;
    if (e.key === 'command' && isCurrent(e) && current.type === 'command') return current.cmd;
    return e.trailing ?? '';
  }

  function activate(e: Entry) {
    if (e.key === 'gesture') ongesture();
    else if (e.key === 'keys') onshortcut();
    else if (e.key === 'command') oncommand();
    else onchoose(e.action ? { ...e.action } : null);
  }

  function rows(): HTMLElement[] {
    return Array.from(list?.querySelectorAll<HTMLElement>('button.entry:not(:disabled)') ?? []);
  }

  function onsearchkey(e: KeyboardEvent) {
    if (e.key === 'ArrowDown' || e.key === 'Enter') {
      e.preventDefault();
      rows()[0]?.focus();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      if (query) query = '';
      else onback();
    }
  }

  function onrowkey(e: KeyboardEvent) {
    const all = rows();
    const i = all.indexOf(e.currentTarget as HTMLElement);
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      all[Math.min(all.length - 1, i + 1)]?.focus();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (i <= 0) search?.focus();
      else all[i - 1]?.focus();
    }
  }
</script>

<div class="picker">
  <PanelHead {back} {onback} {title} {icon}>
    <div class="current">
      <span class="small muted">{t('Current action')}</span>
      <strong class="ellipsis" class:tone-accent={customised}>{currentText}</strong>
    </div>
    {#if mouseOnly}<p class="small muted">{t('This button can only act as another mouse button')}</p>{/if}
  </PanelHead>

  <SearchField bind:value={query} bind:el={search} placeholder={t('Search actions…')} label={t('Search actions')} onkeydown={onsearchkey} />

  <div class="list scroll" bind:this={list}>
    {#each visible as g (g.title)}
      {#if g.title}<div class="group overline">{g.title}</div>{/if}
      {#each g.entries as e (e.key + e.title)}
        {@const on = isCurrent(e)}
        {@const trail = trailingOf(e)}
        <button
          type="button"
          class="entry"
          class:on
          aria-current={on ? 'true' : undefined}
          disabled={!online}
          onclick={() => activate(e)}
          onkeydown={onrowkey}
        >
          <span class="icon"><Icon name={e.icon} size={17} /></span>
          <span class="label ellipsis">{e.title}</span>
          {#if trail}
            {#if e.key === 'command' || e.action?.type === 'command'}
              <span class="trail mono ellipsis">{trail}</span>
            {:else}
              <span class="trail"><KeyCaps combo={trail} {platform} size="sm" /></span>
            {/if}
          {/if}
          {#if e.chevron}
            <Icon name="chevron-right" size={16} class="chev" />
          {:else if on}
            <Icon name="check" size={16} class="check" />
          {/if}
        </button>
      {/each}
    {/each}
    {#if !visible.length}
      <div class="empty">
        <p class="muted">{t('No action matches “{query}”', { query: query.trim() })}</p>
        {#if !mouseOnly}
          <Button variant="link" icon="keyboard" onclick={onshortcut}>{t('Use a keyboard shortcut')}</Button>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .picker {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    min-height: 0;
    flex: 1;
  }
  .current {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    min-width: 0;
  }
  .list {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
    margin: 0 calc(-1 * var(--space-2));
    padding: 0 var(--space-2) var(--space-2);
  }
  .group {
    padding: var(--space-3) var(--space-3) var(--space-1);
  }
  .entry {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    width: 100%;
    min-height: 38px;
    padding: 0 var(--space-3);
    border: 1px solid transparent;
    border-radius: var(--radius-s);
    background: transparent;
    text-align: left;
    flex: none;
  }
  .entry:hover:not(:disabled) {
    background: var(--surface-hover);
  }
  .entry.on {
    background: var(--accent-soft);
    border-color: color-mix(in srgb, var(--accent) 40%, transparent);
  }
  .entry:disabled {
    opacity: 0.5;
  }
  .icon {
    flex: none;
    display: grid;
    color: var(--text-muted);
  }
  .on .icon {
    color: var(--accent);
  }
  .label {
    flex: 1;
  }
  .trail {
    flex: none;
    max-width: 45%;
    color: var(--text-subtle);
    font-size: var(--text-xs);
  }
  .entry :global(.chev) {
    color: var(--text-subtle);
    flex: none;
  }
  .entry :global(.check) {
    color: var(--accent);
    flex: none;
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-2);
    padding: var(--space-4) var(--space-3);
  }
</style>
