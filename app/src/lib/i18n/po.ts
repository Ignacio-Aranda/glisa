// Minimal gettext .po parser (no build step): the catalogues are imported as text
// (`?raw`) and parsed at runtime, like the Rust and Python sides do.
//
// Supports msgctxt, msgid_plural / msgstr[n], multi-line strings, the usual C
// escapes, and skips the header and entries flagged "fuzzy".

export type Entry = string | string[];
/** Key: `${context}\u0004${msgid}` (context "" when there is none). */
export type Catalog = Map<string, Entry>;

export const CTX_SEP = '\u0004';

export function key(context: string, msgid: string): string {
  return context + CTX_SEP + msgid;
}

export function unescape(s: string): string {
  return s.replace(/\\(.)/g, (_m, c: string) => {
    switch (c) {
      case 'n': return '\n';
      case 't': return '\t';
      case 'r': return '\r';
      case '"': return '"';
      case '\\': return '\\';
      default: return c;
    }
  });
}

interface Raw {
  msgctxt?: string;
  msgid?: string;
  msgid_plural?: string;
  msgstr?: string;
  forms: string[];
  fuzzy: boolean;
}

function fresh(): Raw {
  return { forms: [], fuzzy: false };
}

/** Parses a .po text into entries (header and fuzzy entries left out). */
export function parsePo(text: string): Catalog {
  const out: Catalog = new Map();
  let cur = fresh();
  let field: string | null = null;

  const flush = () => {
    if (cur.msgid !== undefined && cur.msgid !== '' && !cur.fuzzy) {
      const k = key(cur.msgctxt ?? '', cur.msgid);
      if (cur.msgid_plural !== undefined) {
        if (cur.forms.length && cur.forms.every((f) => f !== undefined)) out.set(k, cur.forms.slice());
      } else if (cur.msgstr !== undefined) {
        out.set(k, cur.msgstr);
      }
    }
    cur = fresh();
    field = null;
  };

  const append = (name: string, value: string) => {
    const m = /^msgstr\[(\d+)\]$/.exec(name);
    if (m) {
      const i = Number(m[1]);
      cur.forms[i] = (cur.forms[i] ?? '') + value;
      return;
    }
    const r = cur as unknown as Record<string, string | undefined>;
    r[name] = (r[name] ?? '') + value;
  };

  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line) {
      // a blank line ends an entry only once it has a msgstr
      if (cur.msgstr !== undefined || cur.forms.length) flush();
      continue;
    }
    if (line.startsWith('#')) {
      if (cur.msgstr !== undefined || cur.forms.length) flush();
      if (line.startsWith('#,') && /\bfuzzy\b/.test(line)) cur.fuzzy = true;
      continue;
    }
    const kw = /^(msgctxt|msgid_plural|msgid|msgstr(?:\[\d+\])?)\s+"(.*)"$/.exec(line);
    if (kw) {
      const name = kw[1];
      if ((name === 'msgctxt' || name === 'msgid') && (cur.msgstr !== undefined || cur.forms.length)) {
        flush(); // entries not separated by a blank line
      }
      field = name;
      // a keyword starts its field afresh
      if (/^msgstr\[(\d+)\]$/.test(name)) {
        const i = Number(/\[(\d+)\]/.exec(name)![1]);
        cur.forms[i] = '';
      } else {
        (cur as unknown as Record<string, string>)[name] = '';
      }
      append(name, unescape(kw[2]));
      continue;
    }
    const cont = /^"(.*)"$/.exec(line);
    if (cont && field) {
      append(field, unescape(cont[1]));
    }
  }
  flush();
  return out;
}

/** Placeholders `{name}` in a string (ignoring `{{` / `}}`). */
export function placeholders(s: string): Set<string> {
  const out = new Set<string>();
  const re = /(?<!\{)\{([A-Za-z_][A-Za-z0-9_]*)\}(?!\})/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(s))) out.add(m[1]);
  return out;
}
