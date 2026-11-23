import { describe, expect, it } from 'vitest';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { extractRust, extractUi, parsePo } from '../scripts/i18n.mjs';

describe('scripts/i18n.mjs', () => {
  it('extracts every call form from TypeScript, Svelte and Rust', () => {
    const dir = mkdtempSync(join(tmpdir(), 'lo-i18n-'));
    const ui = join(dir, 'a.svelte');
    writeFileSync(ui, [
      "<p>{t('Hello {name}', { name })}</p>",
      "<p>{tn('{n} file', '{n} files', n, { n })}</p>",
      "<p>{tc('mouse button', 'Back')} {bt('Copy')} {btc('gesture', 'Up')}</p>",
      "<script>const L = N_('Later'); const x = obj.t('not this'); // t('nor this')</script>",
    ].join('\n'));
    const rs = join(dir, 'b.rs');
    writeFileSync(rs, [
      'let a = tr!("Paired with {name}", name = n);',
      'let b = trn!("{count} file", "{count} files", n, count = n);',
      'let c = trc!("screen side", "Left");',
      'let d = n_!("Flow is not available: \\',
      '    no session");',
    ].join('\n'));
    const messages = { ui: new Map(), buttons: new Map(), service: new Map() };
    const errors: string[] = [];
    extractUi([ui], messages, errors);
    extractRust([rs], messages);
    expect([...messages.ui.values()].map((m) => m.msgid).sort()).toEqual(['Back', 'Hello {name}', 'Later', '{n} file']);
    expect([...messages.buttons.values()].map((m) => m.msgid).sort()).toEqual(['Copy', 'Up']);
    expect([...messages.service.values()].map((m) => m.msgid).sort())
      .toEqual(['Flow is not available: no session', 'Left', 'Paired with {name}', '{count} file']);
    expect(errors).toEqual([]);
  });

  it('parses .po files', () => {
    const e = parsePo('msgid ""\nmsgstr ""\n\nmsgctxt "c"\nmsgid "A"\nmsgstr "B"\n');
    expect(e).toEqual([{ ctx: 'c', msgid: 'A', plural: null, line: 4, value: 'B' }]);
  });

  it('the window strings are all translated', () => {
    const out = execFileSync(process.execPath, [join(__dirname, '..', 'scripts', 'i18n.mjs'), 'check', 'ui'], { encoding: 'utf8' });
    expect(out).toMatch(/0 problem\(s\)/);
  });
});
