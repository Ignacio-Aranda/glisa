import { describe, expect, it, beforeAll, afterEach } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { parsePo, key, placeholders } from '../src/lib/i18n/po';
import {
  bt, btc, format, fold, installCatalog, normalize, sentence, setLanguage, setProductName, t, tc, tn,
} from '../src/lib/i18n/index.svelte';

const LOCALE = join(__dirname, '..', '..', 'locale', 'es');

describe('po parser', () => {
  it('reads contexts, plurals, multi-line strings and escapes, and skips fuzzy entries', () => {
    const cat = parsePo([
      'msgid ""', 'msgstr ""', '"Language: es\\n"', '',
      '#: x', 'msgctxt "mouse button"', 'msgid "Back"', 'msgstr "Atrás"', '',
      'msgid "{count} file"', 'msgid_plural "{count} files"', 'msgstr[0] "{count} archivo"', 'msgstr[1] "{count} archivos"', '',
      'msgid ""', '"Long "', '"text"', 'msgstr "Texto "', '"largo \\"citado\\"\\n"', '',
      '#, fuzzy', 'msgid "Draft"', 'msgstr "Borrador"',
      'msgid "No blank line before"', 'msgstr "Sin línea en blanco"',
    ].join('\n'));
    expect(cat.get(key('mouse button', 'Back'))).toBe('Atrás');
    expect(cat.get(key('', '{count} file'))).toEqual(['{count} archivo', '{count} archivos']);
    expect(cat.get(key('', 'Long text'))).toBe('Texto largo "citado"\n');
    expect(cat.has(key('', 'Draft'))).toBe(false);
    expect(cat.get(key('', 'No blank line before'))).toBe('Sin línea en blanco');
    expect(cat.has(key('', ''))).toBe(false);
  });

  it('finds placeholders', () => {
    expect([...placeholders('{a} and {b} but not {{c}}')].sort()).toEqual(['a', 'b']);
  });

  it('parses the real catalogues', () => {
    const ui = parsePo(readFileSync(join(LOCALE, 'ui.po'), 'utf8'));
    const buttons = parsePo(readFileSync(join(LOCALE, 'buttons.po'), 'utf8'));
    expect(ui.size).toBeGreaterThan(400);
    expect(buttons.get(key('', 'Copy'))).toBe('Copiar');
  });
});

describe('translation', () => {
  beforeAll(() => {
    installCatalog('es', readFileSync(join(LOCALE, 'ui.po'), 'utf8'), readFileSync(join(LOCALE, 'buttons.po'), 'utf8'));
  });
  afterEach(async () => {
    await setLanguage('en');
    setProductName(null);
  });

  it('formats named placeholders and literal braces', () => {
    expect(format('Channel {n} of {total}', { n: 2, total: 3 })).toBe('Channel 2 of 3');
    expect(format('{{literal}} {missing}', {})).toBe('{literal} {missing}');
  });

  it('English is the source', async () => {
    await setLanguage('en');
    expect(t('Pointer speed')).toBe('Pointer speed');
    expect(tn('{count} file', '{count} files', 2, { count: 2 })).toBe('2 files');
  });

  it('Spanish comes from ui.po and buttons.po, with plurals and contexts', async () => {
    await setLanguage('es');
    expect(t('Pointer speed')).toBe('Velocidad del puntero');
    expect(t('Channel {n}', { n: 3 })).toBe('Canal 3');
    expect(tn('{count} file', '{count} files', 1, { count: 1 })).toBe('1 archivo');
    expect(tn('{count} file', '{count} files', 4, { count: 4 })).toBe('4 archivos');
    expect(tc('battery level', 'Low')).toBe('Baja');
    expect(btc('mouse button', 'Back')).toBe('Atrás');
    expect(bt('Copy')).toBe('Copiar');
    expect(t('A string nobody translated')).toBe('A string nobody translated');
    expect(document.documentElement.lang).toBe('es');
  });

  it('fills {app} with the product name', async () => {
    setProductName('Glisa');
    expect(t('Open {app}', {})).toBe('Open Glisa');
    await setLanguage('es');
    expect(t('{app} will start when you log in')).toBe('Glisa se iniciará con tu sesión');
  });

  it('normalises language tags and helpers', () => {
    expect(normalize('es-ES')).toBe('es');
    expect(normalize('es_AR.UTF-8')).toBe('es');
    expect(normalize('fr')).toBeNull();
    expect(fold('Atrás Ñu')).toBe('atras nu');
    expect(sentence('the device is off')).toBe('The device is off');
  });
});
