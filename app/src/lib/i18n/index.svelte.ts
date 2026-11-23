// Translations for the window. English is the source language; Spanish comes from
// locale/es/ui.po (the window) and locale/es/buttons.po (control and action names,
// shared with the service), parsed at runtime.
//
//   t('Pointer speed')
//   t('Channel {n}', { n: 3 })
//   tn('{count} file', '{count} files', n, { count: n })
//   tc('mouse button', 'Back')
//   bt('Copy'), btc('gesture', 'Up')        // buttons.po domain
//   t('{app} needs…')                        // {app}: the product name (app_info().name)
//
// Placeholders are named (`{name}`); `{{` and `}}` are literal braces. Calls must use
// string literals so that scripts/i18n.mjs can find them.

import { parsePo, key, type Catalog, type Entry } from './po';

export type Lang = 'en' | 'es';
export type LangChoice = 'system' | Lang;
export type Args = Record<string, string | number>;

export const SOURCE_LANGUAGE: Lang = 'en';
/** Languages with a catalogue, each named in its own language. */
export const LANGUAGES: { code: Lang; name: string }[] = [
  { code: 'en', name: 'English' },
  { code: 'es', name: 'Español' }, // i18n: ignore
];

type Domain = 'ui' | 'buttons';
const catalogs: Partial<Record<Lang, Record<Domain, Catalog>>> = {};

const loaders: Record<Exclude<Lang, 'en'>, () => Promise<[string, string]>> = {
  es: async () => {
    const [ui, buttons] = await Promise.all([
      import('../../../../locale/es/ui.po?raw'),
      import('../../../../locale/es/buttons.po?raw'),
    ]);
    return [ui.default, buttons.default];
  },
};

/** Until the shell says otherwise (app_info().name). */
export const PRODUCT_FALLBACK = 'Glisa';

const state = $state({ lang: SOURCE_LANGUAGE as Lang, product: PRODUCT_FALLBACK });

/** The product's name, as shown everywhere and in `{app}` placeholders (reactive). */
export function productName(): string {
  return state.product;
}

export function setProductName(name: string | null | undefined): void {
  state.product = name?.trim() || PRODUCT_FALLBACK;
}

/** The language the window is shown in now (reactive). */
export function currentLang(): Lang {
  return state.lang;
}

/** 'es-ES' → 'es'; unsupported → null. */
export function normalize(tag: string | null | undefined): Lang | null {
  if (!tag) return null;
  const code = tag.trim().split(/[-_.@]/)[0].toLowerCase();
  return LANGUAGES.some((l) => l.code === code) ? (code as Lang) : null;
}

/** The desktop's language (first supported entry of navigator.languages). */
export function systemLanguage(): Lang {
  const tags = typeof navigator !== 'undefined' ? [...(navigator.languages ?? []), navigator.language] : [];
  for (const tag of tags) {
    const l = normalize(tag);
    if (l) return l;
  }
  return SOURCE_LANGUAGE;
}

export function resolveLanguage(choice: LangChoice | string | null | undefined): Lang {
  if (!choice || choice === 'system') return systemLanguage();
  return normalize(choice) ?? systemLanguage();
}

/** Loads a catalogue from text (tests, and the loaders above). */
export function installCatalog(lang: Lang, ui: string, buttons: string): void {
  catalogs[lang] = { ui: parsePo(ui), buttons: parsePo(buttons) };
}

/** Switches the language (loading its catalogue first). Returns the language in use. */
export async function setLanguage(choice: LangChoice | string | null | undefined): Promise<Lang> {
  const lang = resolveLanguage(choice);
  if (lang !== SOURCE_LANGUAGE && !catalogs[lang]) {
    try {
      const [ui, buttons] = await loaders[lang as Exclude<Lang, 'en'>]();
      installCatalog(lang, ui, buttons);
    } catch (e) {
      console.error('could not load the catalogue', lang, e);
    }
  }
  state.lang = lang;
  if (typeof document !== 'undefined') document.documentElement.lang = lang;
  return lang;
}

function lookup(domain: Domain, context: string, msgid: string): Entry | undefined {
  const lang = state.lang;
  if (lang === SOURCE_LANGUAGE) return undefined;
  return catalogs[lang]?.[domain].get(key(context, msgid));
}

/** Replaces `{name}` placeholders; unknown ones stay as they are. */
export function format(template: string, args?: Args): string {
  if (!args) return template.replace(/\{\{|\}\}/g, (m) => m[0]);
  return template.replace(/\{\{|\}\}|\{([A-Za-z_][A-Za-z0-9_]*)\}/g, (m, name?: string) => {
    if (m === '{{' || m === '}}') return m[0];
    if (name !== undefined && Object.prototype.hasOwnProperty.call(args, name)) return String(args[name]);
    return m;
  });
}

function single(domain: Domain, context: string, msgid: string, args?: Args): string {
  const e = lookup(domain, context, msgid);
  const text = typeof e === 'string' && e ? e : msgid;
  return format(text, { app: state.product, ...args });
}

function plural(domain: Domain, singular: string, pluralForm: string, n: number, args?: Args): string {
  const index = n === 1 ? 0 : 1; // English and Spanish: plural = n != 1
  const e = lookup(domain, '', singular);
  let text = index === 0 ? singular : pluralForm;
  if (Array.isArray(e) && e[index]) text = e[index];
  return format(text, { app: state.product, ...args });
}

/** Window text (ui.po). */
export function t(msgid: string, args?: Args): string {
  return single('ui', '', msgid, args);
}

/** Window text with a plural form chosen by `n`. */
export function tn(singular: string, pluralForm: string, n: number, args?: Args): string {
  return plural('ui', singular, pluralForm, n, args);
}

/** Window text in a context ("mouse button", "screen side"…). */
export function tc(context: string, msgid: string, args?: Args): string {
  return single('ui', context, msgid, args);
}

/** Control and action names (buttons.po). */
export function bt(msgid: string, args?: Args): string {
  return single('buttons', '', msgid, args);
}

/** Control and action names in a context (buttons.po). */
export function btc(context: string, msgid: string, args?: Args): string {
  return single('buttons', context, msgid, args);
}

/** Marks a string for extraction without translating it (translate later with t()). */
export function N_(msgid: string): string {
  return msgid;
}

/** Translates a runtime value that came from N_() (ui domain), or from the buttons domain. */
export function tr(msgid: string, domain: Domain = 'ui'): string {
  return single(domain, '', msgid);
}

/** Case- and accent-insensitive form used by searches: 'Atrás' → 'atras'. */
export function fold(text: string): string {
  return (text || '').normalize('NFD').replace(/\p{M}/gu, '').toLocaleLowerCase();
}

/** First letter upper case (messages from the service used as sentences). */
export function sentence(text: string): string {
  const s = (text || '').trim();
  return s ? s[0].toLocaleUpperCase() + s.slice(1) : s;
}
