#!/usr/bin/env node
// Translation checker for Glisa (the Rust + Tauri version of tools/i18n.py).
//
//   node scripts/i18n.mjs check [ui|rs]     fails on missing or untranslated entries,
//                                           placeholder mismatches, conflicting entries,
//                                           template literals inside t(), and Spanish text
//                                           left outside t() in the window's code
//   node scripts/i18n.mjs missing [es] [--domain ui|buttons|service] [--path FILE]
//                                           missing entries, as .po text to paste
//
// Domains and where their strings come from:
//   ui       app/src/**/*.{ts,svelte}: t(), tn(), tc(), N_()            → locale/<lang>/ui.po
//   buttons  app/src/**/*.{ts,svelte}: bt(), btc(), NB_()               → locale/<lang>/buttons.po
//   service  crates/**/*.rs and app/src-tauri/src/**/*.rs: tr!, trn!, trc!, n_!, and the
//            mock's S_() → core.po + flow.po + buttons.po (merged, as the Rust side loads them)
// Arguments must be string literals. A line containing "i18n: ignore" is skipped by the
// Spanish-text detector.

import { readFileSync, readdirSync, existsSync, statSync } from 'node:fs';
import { join, relative, dirname } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const APP = dirname(dirname(fileURLToPath(import.meta.url)));
const ROOT = dirname(APP);
const LANGS = ['es'];

// ------------------------------------------------------------------ .po parsing

function unescapePo(s) {
  return s.replace(/\\(.)/g, (_m, c) => ({ n: '\n', t: '\t', r: '\r', '"': '"', '\\': '\\' })[c] ?? c);
}

/** [{ctx, msgid, plural, value (string | string[]), line}] — header and fuzzy entries left out. */
export function parsePo(text) {
  const out = [];
  let cur = null;
  let field = null;
  let fuzzy = false;
  let line = 0;
  const flush = () => {
    if (cur && cur.msgid && !fuzzy) {
      out.push({
        ctx: cur.msgctxt ?? '', msgid: cur.msgid, plural: cur.msgid_plural ?? null, line: cur.line,
        value: cur.msgid_plural !== undefined ? cur.forms : cur.msgstr ?? '',
      });
    }
    cur = null;
    field = null;
    fuzzy = false;
  };
  for (const raw of text.split(/\r?\n/)) {
    line += 1;
    const l = raw.trim();
    if (!l) { if (cur && (cur.msgstr !== undefined || cur.forms.length)) flush(); continue; }
    if (l.startsWith('#')) {
      if (cur && (cur.msgstr !== undefined || cur.forms.length)) flush();
      if (l.startsWith('#,') && /\bfuzzy\b/.test(l)) fuzzy = true;
      continue;
    }
    const m = /^(msgctxt|msgid_plural|msgid|msgstr(?:\[(\d+)\])?)\s+"(.*)"$/.exec(l);
    if (m) {
      if ((m[1] === 'msgctxt' || m[1] === 'msgid') && cur && (cur.msgstr !== undefined || cur.forms.length)) {
        const f = fuzzy;
        flush();
        fuzzy = f;
      }
      cur ??= { forms: [], line };
      field = m[1];
      if (m[2] !== undefined) cur.forms[Number(m[2])] = unescapePo(m[3]);
      else cur[field] = unescapePo(m[3]);
      continue;
    }
    const c = /^"(.*)"$/.exec(l);
    if (c && cur && field) {
      const fm = /^msgstr\[(\d+)\]$/.exec(field);
      if (fm) cur.forms[Number(fm[1])] += unescapePo(c[1]);
      else cur[field] += unescapePo(c[1]);
    }
  }
  flush();
  return out;
}

function loadCatalog(files) {
  const entries = new Map();
  const conflicts = [];
  for (const f of files) {
    const path = join(ROOT, 'locale', f);
    if (!existsSync(path)) continue;
    for (const e of parsePo(readFileSync(path, 'utf8'))) {
      const k = e.ctx + '\u0004' + e.msgid;
      const prev = entries.get(k);
      const has = Array.isArray(e.value) ? e.value.some(Boolean) : !!e.value;
      if (prev && has && prev.hasValue && JSON.stringify(prev.value) !== JSON.stringify(e.value)) {
        conflicts.push(`locale/${f}:${e.line}: ${JSON.stringify(e.msgid.slice(0, 60))} conflicts with ${prev.file}`);
      }
      if (has || !prev) entries.set(k, { ...e, file: `locale/${f}`, hasValue: has });
    }
  }
  return { entries, conflicts };
}

// ------------------------------------------------------------------ extraction

function walk(dir, exts, skip = () => false) {
  const out = [];
  if (!existsSync(dir)) return out;
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (skip(p, name)) continue;
    const st = statSync(p);
    if (st.isDirectory()) out.push(...walk(p, exts, skip));
    else if (exts.some((e) => name.endsWith(e))) out.push(p);
  }
  return out;
}

/** Reads a JS string literal starting at `i` ('…', "…" or `…` without ${}). */
function jsString(src, i) {
  const q = src[i];
  if (q !== "'" && q !== '"' && q !== '`') return null;
  let s = '';
  let j = i + 1;
  while (j < src.length) {
    const c = src[j];
    if (c === '\\') {
      const n = src[j + 1];
      if (n === 'u' && src[j + 2] === '{') {
        const end = src.indexOf('}', j);
        s += String.fromCodePoint(parseInt(src.slice(j + 3, end), 16));
        j = end + 1;
        continue;
      }
      if (n === 'u') { s += String.fromCharCode(parseInt(src.slice(j + 2, j + 6), 16)); j += 6; continue; }
      s += { n: '\n', t: '\t', r: '\r', '0': '\0' }[n] ?? n;
      j += 2;
      continue;
    }
    if (q === '`' && c === '$' && src[j + 1] === '{') return { error: 'template literal with ${…} inside a translation call', end: j };
    if (c === q) return { value: s, end: j + 1 };
    s += c;
    j += 1;
  }
  return null;
}

/** Reads a Rust string literal ("…", r"…", r#"…"#) starting at `i`. */
function rsString(src, i) {
  let j = i;
  if (src[j] === 'r') {
    let hashes = 0;
    j += 1;
    while (src[j] === '#') { hashes += 1; j += 1; }
    if (src[j] !== '"') return null;
    const close = '"' + '#'.repeat(hashes);
    const end = src.indexOf(close, j + 1);
    return { value: src.slice(j + 1, end), end: end + close.length };
  }
  if (src[j] !== '"') return null;
  let s = '';
  j += 1;
  while (j < src.length) {
    const c = src[j];
    if (c === '\\') {
      const n = src[j + 1];
      if (n === '\n' || (n === '\r' && src[j + 2] === '\n')) {     // line continuation
        j += n === '\n' ? 2 : 3;
        while (/\s/.test(src[j])) j += 1;
        continue;
      }
      if (n === 'u' && src[j + 2] === '{') {
        const end = src.indexOf('}', j);
        s += String.fromCodePoint(parseInt(src.slice(j + 3, end), 16));
        j = end + 1;
        continue;
      }
      s += { n: '\n', t: '\t', r: '\r', '0': '\0' }[n] ?? n;
      j += 2;
      continue;
    }
    if (c === '"') return { value: s, end: j + 1 };
    s += c;
    j += 1;
  }
  return null;
}

function skipSpace(src, i) {
  while (i < src.length && /[\s,]/.test(src[i])) i += 1;
  return i;
}

function lineOf(src, i) {
  let n = 1;
  for (let k = 0; k < i; k++) if (src.charCodeAt(k) === 10) n += 1;
  return n;
}

const UI_FUNCS = {
  t: ['ui', 'single'], tn: ['ui', 'plural'], tc: ['ui', 'context'], N_: ['ui', 'single'],
  bt: ['buttons', 'single'], btc: ['buttons', 'context'], NB_: ['buttons', 'single'],
  S_: ['service', 'single'],
};
const RS_MACROS = { tr: 'single', n_: 'single', trc: 'context', trn: 'plural' };

/** messages: Map(domain → Map(key → {ctx, msgid, plural, where[]})) */
function newMessages() {
  return { ui: new Map(), buttons: new Map(), service: new Map() };
}

function add(messages, domain, ctx, msgid, plural, where) {
  const k = ctx + '\u0004' + msgid;
  const m = messages[domain].get(k) ?? { ctx, msgid, plural, where: [] };
  m.where.push(where);
  messages[domain].set(k, m);
}

function extractArgs(src, i, kind, read) {
  const args = [];
  const want = kind === 'single' ? 1 : 2;
  let j = skipSpace(src, i);
  for (let n = 0; n < want; n++) {
    const s = read(src, j);
    if (!s) return { args: null };
    if (s.error) return { error: s.error };
    args.push(s.value);
    j = skipSpace(src, s.end);
  }
  return { args };
}

export function extractUi(files, messages, errors) {
  const re = /(?<![\w$.])(tn|tc|btc|bt|t|N_|NB_|S_)\(/g;
  for (const file of files) {
    const src = readFileSync(file, 'utf8');
    const rel = relative(ROOT, file);
    let m;
    while ((m = re.exec(src))) {
      const lineStart = src.lastIndexOf('\n', m.index) + 1;
      const before = src.slice(lineStart, m.index);
      if (/^\s*(\/\/|\*|\/\*)/.test(before)) continue;                         // a comment line
      if (before.replace(/(['"`])(?:\\.|(?!\1).)*\1/g, '').includes('//')) continue;   // after //
      const [domain, kind] = UI_FUNCS[m[1]];
      const r = extractArgs(src, m.index + m[0].length, kind, jsString);
      const where = `${rel}:${lineOf(src, m.index)}`;
      if (r.error) { errors.push(`${where}: ${r.error}`); continue; }
      if (!r.args) continue;   // a runtime value: tr(LABEL) of an N_() constant
      if (kind === 'context') add(messages, domain, r.args[0], r.args[1], null, where);
      else if (kind === 'plural') add(messages, domain, '', r.args[0], r.args[1], where);
      else add(messages, domain, '', r.args[0], null, where);
    }
  }
}

export function extractRust(files, messages) {
  const re = /(?<![\w$])(trn|trc|tr|n_)!\(/g;
  for (const file of files) {
    const src = readFileSync(file, 'utf8');
    const rel = relative(ROOT, file);
    let m;
    while ((m = re.exec(src))) {
      const kind = RS_MACROS[m[1]];
      const r = extractArgs(src, m.index + m[0].length, kind, rsString);
      if (!r.args) continue;
      const where = `${rel}:${lineOf(src, m.index)}`;
      if (kind === 'context') add(messages, 'service', r.args[0], r.args[1], null, where);
      else if (kind === 'plural') add(messages, 'service', '', r.args[0], r.args[1], where);
      else add(messages, 'service', '', r.args[0], null, where);
    }
  }
}

// ------------------------------------------------------------------ Spanish detector

const SPANISH_WORDS = new Set(`de del la las los el para con una uno por que está este esta esto sin más cuando puede
pueden dispositivo dispositivos equipo equipos botón botones ratón teclado ajustes servicio conectado desconectado
activado desactivado predeterminado copiar pegar cortar deshacer rehacer cambiar elige pulsa mueve también aquí ahora
todavía vuelve ninguna ninguno nada todos todas sí canal batería rueda puntero velocidad desplazamiento restablecer
permisos instalar vincular emparejar abrir cerrar salir cancelar aceptar guardar añadir quitar buscar atrás adelante
clic acción acciones atajo comando ejecutar gestos tecla teclas pantalla escritorio ventana pestaña volumen pista
siguiente anterior silenciar desconocido desconocida válido válida inválido encontrado conexión red código versión`.split(/\s+/));
const SPANISH_CHARS = /[áéíóúñ¿¡ÁÉÍÓÚÑ]/;

function looksSpanish(text) {
  if (SPANISH_CHARS.test(text)) return true;
  const words = text.toLowerCase().match(/[a-záéíóúñ]+/g) ?? [];
  const hits = words.filter((w) => SPANISH_WORDS.has(w)).length;
  return hits >= 2 || (hits === 1 && words.length <= 2 && text.length > 3);
}

function suspects(files) {
  const out = [];
  const call = /(?<![\w$.])(?:tn|tc|btc|bt|t|N_|NB_|S_)\(\s*$/;
  for (const file of files) {
    const src = readFileSync(file, 'utf8');
    const lines = src.split('\n');
    const rel = relative(ROOT, file);
    let inStyle = false;
    lines.forEach((line, n) => {
      if (/<style/.test(line)) inStyle = true;
      if (/<\/style>/.test(line)) { inStyle = false; return; }
      if (inStyle || line.includes('i18n: ignore') || /^\s*(\/\/|\*|\/\*)/.test(line)) return;
      const re = /(['"`])((?:\\.|(?!\1).)*)\1/g;
      let m;
      while ((m = re.exec(line))) {
        const before = line.slice(0, m.index);
        if (call.test(before) || /(?:tn|tc|btc|t)\([^)]*,\s*$/.test(before)) continue;
        if (m[2].trim() && looksSpanish(m[2])) out.push(`${rel}:${n + 1}: ${JSON.stringify(m[2].slice(0, 70))}`);
      }
      // text between tags in .svelte markup
      if (file.endsWith('.svelte') && !/^\s*</.test(line) === false) {
        const txt = line.replace(/<[^>]*>/g, ' ').replace(/\{[^}]*\}/g, ' ').trim();
        if (txt && !/[=;(){}]/.test(txt) && looksSpanish(txt)) out.push(`${rel}:${n + 1}: ${JSON.stringify(txt.slice(0, 70))}`);
      }
    });
  }
  return out;
}

// ------------------------------------------------------------------ checks

const PLACEHOLDER = /(?<!\{)\{([A-Za-z_][A-Za-z0-9_]*)\}(?!\})/g;
const placeholders = (s) => new Set([...(s ?? '').matchAll(PLACEHOLDER)].map((m) => m[1]));
const sameSet = (a, b) => a.size === b.size && [...a].every((x) => b.has(x));

function uiFiles() {
  return walk(join(APP, 'src'), ['.ts', '.svelte'], (_p, name) => name === 'node_modules' || name === 'icons.ts');
}

function rustFiles() {
  return [
    ...walk(join(ROOT, 'crates'), ['.rs'], (_p, name) => name === 'target' || name === 'tests'),
    ...walk(join(APP, 'src-tauri', 'src'), ['.rs']),
  ];
}

const CATALOGS = {
  ui: (lang) => [`${lang}/ui.po`],
  buttons: (lang) => [`${lang}/buttons.po`],
  service: (lang) => [`${lang}/core.po`, `${lang}/flow.po`, `${lang}/buttons.po`, `${lang}/app.po`],
};

function collect(scope) {
  const messages = newMessages();
  const errors = [];
  if (scope !== 'rs') extractUi(uiFiles(), messages, errors);
  if (scope !== 'ui') extractRust(rustFiles(), messages);
  return { messages, errors };
}

function checkDomain(lang, domain, msgs, problems, warnings) {
  const { entries, conflicts } = loadCatalog(CATALOGS[domain](lang));
  problems.push(...conflicts.map((c) => `[${lang}] conflict: ${c}`));
  for (const m of msgs.values()) {
    const k = m.ctx + '\u0004' + m.msgid;
    const e = entries.get(k);
    const value = e?.value;
    const empty = !e || (Array.isArray(value) ? !value.length || value.some((v) => !v) : !value);
    if (empty) {
      problems.push(`[${lang}] missing (${domain}): ${m.where[0]}: ${JSON.stringify(m.msgid.slice(0, 70))}`);
      continue;
    }
    if (Boolean(m.plural) !== Array.isArray(value)) {
      problems.push(`[${lang}] ${m.plural ? 'needs plural forms' : 'is a plural entry'} (${domain}): ${m.where[0]}: ${JSON.stringify(m.msgid.slice(0, 60))}`);
      continue;
    }
    const want = new Set([...placeholders(m.msgid), ...placeholders(m.plural)]);
    for (const form of Array.isArray(value) ? value : [value]) {
      const got = placeholders(form);
      const extra = [...got].some((p) => !want.has(p));
      if (extra || (!m.plural && !sameSet(got, want))) {
        problems.push(`[${lang}] placeholders differ (${domain}): ${m.where[0]}: ${JSON.stringify(m.msgid.slice(0, 60))}`);
        break;
      }
    }
  }
  if (domain === 'ui') {
    for (const [k, e] of entries) {
      if (!msgs.has(k)) warnings.push(`[${lang}] unused entry in ${e.file}:${e.line}: ${JSON.stringify(e.msgid.slice(0, 70))}`);
    }
  }
}

function cmdCheck(scope) {
  const { messages, errors } = collect(scope);
  const problems = [...errors];
  const warnings = [];
  for (const lang of LANGS) {
    if (scope !== 'rs') {
      checkDomain(lang, 'ui', messages.ui, problems, warnings);
      checkDomain(lang, 'buttons', messages.buttons, problems, warnings);
    }
    checkDomain(lang, 'service', messages.service, problems, warnings);
  }
  if (scope !== 'rs') problems.push(...suspects(uiFiles().filter((f) => !f.includes(`${'/'}mock${'/'}`))).map((s) => `Spanish text outside t(): ${s}`));
  for (const w of warnings.slice(0, 30)) console.log(`warning: ${w}`);
  if (warnings.length > 30) console.log(`warning: … and ${warnings.length - 30} more unused entries`);
  for (const p of problems) console.log(p);
  const count = Object.values(messages).reduce((n, m) => n + m.size, 0);
  console.log(`${messages.ui.size} ui, ${messages.buttons.size} buttons, ${messages.service.size} service messages (${count}); ${problems.length} problem(s)`);
  return problems.length ? 1 : 0;
}

function poQuote(s) {
  return '"' + s.replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\n/g, '\\n').replace(/\t/g, '\\t') + '"';
}

function cmdMissing(args) {
  const lang = args.find((a) => !a.startsWith('--') && LANGS.includes(a)) ?? 'es';
  const di = args.indexOf('--domain');
  const domain = di >= 0 ? args[di + 1] : 'ui';
  const pi = args.indexOf('--path');
  const only = pi >= 0 ? join(process.cwd(), args[pi + 1]) : null;
  const messages = newMessages();
  const errors = [];
  if (domain === 'service') extractRust(only ? [only] : rustFiles(), messages);
  extractUi(only && !only.endsWith('.rs') ? [only] : uiFiles(), messages, errors);
  const { entries } = loadCatalog(CATALOGS[domain](lang));
  for (const m of messages[domain].values()) {
    const e = entries.get(m.ctx + '\u0004' + m.msgid);
    const empty = !e || (Array.isArray(e.value) ? e.value.some((v) => !v) : !e.value);
    if (!empty) continue;
    const out = [`#: ${m.where.slice(0, 3).join(' ')}`];
    if (m.ctx) out.push(`msgctxt ${poQuote(m.ctx)}`);
    out.push(`msgid ${poQuote(m.msgid)}`);
    if (m.plural) out.push(`msgid_plural ${poQuote(m.plural)}`, 'msgstr[0] ""', 'msgstr[1] ""');
    else out.push('msgstr ""');
    console.log(out.join('\n') + '\n');
  }
  return 0;
}

export function uiMessages() {
  const { messages, errors } = collect('ui');
  return { messages, errors };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [cmd, ...rest] = process.argv.slice(2);
  if (cmd === 'check') process.exit(cmdCheck(rest[0] ?? 'all'));
  else if (cmd === 'missing') process.exit(cmdMissing(rest));
  else {
    console.log('usage: node scripts/i18n.mjs check [ui|rs] | missing [es] [--domain ui|buttons|service] [--path FILE]');
    process.exit(2);
  }
}
