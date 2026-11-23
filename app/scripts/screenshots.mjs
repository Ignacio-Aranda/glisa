#!/usr/bin/env node
// Screenshots of every page in light/dark × en/es, against the mock backend:
// builds, serves `vite preview` and captures each URL with headless Firefox (a throwaway
// profile, so the user's own Firefox is never touched). Output: app/screenshots-out/.
//
//   npm run screenshots                     everything
//   npm run screenshots -- --only flow      shots whose name contains "flow"
//   npm run screenshots -- --no-build       reuse dist/
//   npm run screenshots -- --themes dark --langs en

import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const APP = dirname(dirname(fileURLToPath(import.meta.url)));
const OUT = join(APP, 'screenshots-out');
const PORT = Number(process.env.SHOT_PORT || 4719);
const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : fallback;
};
const only = opt('--only', '');
const themes = opt('--themes', 'light,dark').split(',');
const langs = opt('--langs', 'en,es').split(',');

const LIFT = 'b031-lift';
const MX = 'b034-wireless_mouse_mx-sim';
const KEYS = 'b35b-wireless_keyboard_k-sim';

/** name → [query, hash, window size] */
const SHOTS = [
  ['devices', '', '#/'],
  ['devices-photos-picture', 'picture=1', '#/'],
  ['devices-empty', 'mock=empty', '#/'],
  ['devices-loading', 'mock=loading', '#/'],
  ['devices-permissions', 'mock=no-permissions&dialog=permissions', '#/'],
  ['devices-permissions-macos', 'mock=no-permissions&platform=macos&dialog=permissions', '#/'],
  ['devices-add', 'dialog=add-device', '#/'],
  ['devices-rename', 'dialog=rename', '#/'],
  ['buttons-lift', '', `#/device/${LIFT}/buttons`],
  ['buttons-mx', '', `#/device/${MX}/buttons`],
  ['buttons-picture', 'picture=1', `#/device/${LIFT}/buttons`],
  ['buttons-keyboard', '', `#/device/${KEYS}/buttons`],
  ['buttons-offline', 'mock=disconnected', `#/device/${LIFT}/buttons`],
  ['pointer-mx', '', `#/device/${MX}/pointer`],
  ['easy-switch', '', `#/device/${LIFT}/easy-switch`],
  ['device-settings', '', `#/device/${LIFT}/settings`],
  ['device-settings-picture', 'picture=1', `#/device/${LIFT}/settings`],
  ['picture-layout-editor', 'picture=1&dialog=layout', `#/device/${LIFT}/settings`],
  ['flow', '', '#/flow'],
  ['flow-new', 'flow=new', '#/flow'],
  ['flow-off', 'flow=off', '#/flow'],
  ['flow-unavailable', 'flow=unavailable', '#/flow'],
  ['flow-offline-peer', 'flow=offline', '#/flow'],
  ['flow-pair', 'flow=new&dialog=pair', '#/flow'],
  ['flow-pair-sas', 'flow=incoming&dialog=pair&shot=2600', '#/flow'],
  ['flow-transfer', 'flow=transfer&shot=2600', '#/'],
  ['app-settings', '', '#/settings'],
  ['app-settings-windows', 'platform=windows', '#/settings'],
  ['small-devices', '', '#/', [900, 600]],
  ['small-buttons', '', `#/device/${LIFT}/buttons`, [900, 600]],
  ['small-pointer', '', `#/device/${MX}/pointer`, [900, 600]],
];

function run(cmd, argv) {
  const r = spawnSync(cmd, argv, { cwd: APP, stdio: 'inherit' });
  if (r.status !== 0) throw new Error(`${cmd} ${argv.join(' ')} failed`);
}

async function waitFor(url, ms = 20000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    try {
      const r = await fetch(url);
      if (r.ok) return;
    } catch {
      // not yet
    }
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error(`${url} did not come up`);
}

function firefox(profile, file, url, [w, h]) {
  return new Promise((resolve) => {
    const p = spawn('firefox', ['--headless', '--no-remote', '--profile', profile, '--screenshot', file,
      '--window-size', `${w},${h}`, url], { stdio: 'ignore' });
    const timer = setTimeout(() => p.kill('SIGKILL'), 45000);
    p.on('exit', () => {
      clearTimeout(timer);
      resolve(existsSync(file));
    });
  });
}

async function main() {
  const vite = join(APP, 'node_modules', 'vite', 'bin', 'vite.js');
  if (!args.includes('--no-build')) run(process.execPath, [vite, 'build', '--logLevel', 'warn']);
  mkdirSync(OUT, { recursive: true });
  const server = spawn(process.execPath, [vite, 'preview', '--port', String(PORT), '--strictPort'], { cwd: APP, stdio: 'ignore' });
  const profile = mkdtempSync(join(tmpdir(), 'lo-shots-'));
  // Quiet, deterministic profile: no first-run pages, no animations, 1:1 pixels.
  writeFileSync(join(profile, 'user.js'), [
    'user_pref("browser.shell.checkDefaultBrowser", false);',
    'user_pref("browser.startup.homepage_override.mstone", "ignore");',
    'user_pref("datareporting.policy.dataSubmissionEnabled", false);',
    'user_pref("toolkit.telemetry.reportingpolicy.firstRun", false);',
    'user_pref("ui.prefersReducedMotion", 1);',
    'user_pref("layout.css.devPixelsPerPx", "1.0");',
    'user_pref("browser.download.useDownloadDir", false);',
  ].join('\n'));
  let ok = 0;
  let failed = 0;
  try {
    await waitFor(`http://localhost:${PORT}/`);
    for (const [name, query, hash, size] of SHOTS) {
      if (only && !name.includes(only)) continue;
      for (const theme of themes) {
        for (const lang of langs) {
          const q = new URLSearchParams(query);
          q.set('theme', theme);
          q.set('lang', lang);
          if (!q.has('shot')) q.set('shot', '1400');
          q.set('latency', '0');
          const url = `http://localhost:${PORT}/?${q}${hash}`;
          const file = join(OUT, `${name}-${theme}-${lang}.png`);
          rmSync(file, { force: true });
          const done = await firefox(profile, file, url, size ?? [1280, 820]);
          if (done) ok += 1;
          else {
            failed += 1;
            console.error(`failed: ${name} ${theme} ${lang}`);
          }
          process.stdout.write(done ? '.' : 'x');
        }
      }
    }
  } finally {
    process.stdout.write('\n');
    server.kill('SIGTERM');
    rmSync(profile, { recursive: true, force: true });
  }
  console.log(`${ok} screenshots in ${OUT}${failed ? `, ${failed} failed` : ''}`);
  if (failed) process.exit(1);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
