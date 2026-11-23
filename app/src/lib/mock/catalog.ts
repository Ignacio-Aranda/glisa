// The mock answers in the client's language, like the service: its messages are the
// service's own msgids, translated with locale/es/{core,flow,buttons}.po.

import { parsePo, key, type Catalog } from '../i18n/po';
import { format, type Args } from '../i18n/index.svelte';

let es: Catalog | null = null;
let loading: Promise<void> | null = null;

export function loadServiceCatalog(): Promise<void> {
  if (es) return Promise.resolve();
  if (!loading) {
    loading = Promise.all([
      import('../../../../locale/es/core.po?raw'),
      import('../../../../locale/es/flow.po?raw'),
      import('../../../../locale/es/buttons.po?raw'),
    ]).then((mods) => {
      const merged: Catalog = new Map();
      for (const m of mods) for (const [k, v] of parsePo(m.default)) merged.set(k, v);
      es = merged;
    });
  }
  return loading;
}

/** Marks a service msgid (core.po / flow.po) for the i18n checker. */
export function S_(msgid: string): string {
  return msgid;
}

export class Translator {
  constructor(public lang: string) {}

  private get(context: string, msgid: string): string {
    if (this.lang !== 'es' || !es) return msgid;
    const e = es.get(key(context, msgid));
    return typeof e === 'string' && e ? e : msgid;
  }

  /** `msgid` (a service message) in this language. */
  tr(msgid: string, args?: Args): string {
    return format(this.get('', msgid), args);
  }

  trc(context: string, msgid: string, args?: Args): string {
    return format(this.get(context, msgid), args);
  }

  trn(singular: string, plural: string, n: number, args?: Args): string {
    let text = n === 1 ? singular : plural;
    if (this.lang === 'es' && es) {
      const e = es.get(key('', singular));
      if (Array.isArray(e) && e[n === 1 ? 0 : 1]) text = e[n === 1 ? 0 : 1];
    }
    return format(text, args);
  }
}
