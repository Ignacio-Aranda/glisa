// Name-independent logo assets: mark, app icon, tray icons, favicon.
//
//   node brand/tools/logo.cjs        (needs @resvg/resvg-js for the PNGs, see package.json)
//
// Writes brand/logo/mark*.svg, brand/icon/app-icon*.{svg,png}, brand/tray/*,
// app/src/lib/brand/logo.svg and app/src/lib/brand/favicon.svg.
// The wordmark and the README banner depend on the name: see wordmark.cjs.
'use strict';

const fs = require('fs');
const path = require('path');
const { smooth } = require('./lib/svg.cjs');
const { mark, monoMark, trayGlyph } = require('./lib/mark.cjs');

const ROOT = path.resolve(__dirname, '../..');
const out = (p) => path.join(ROOT, p);
const write = (p, s) => {
  fs.mkdirSync(path.dirname(out(p)), { recursive: true });
  fs.writeFileSync(out(p), s);
};
const svgDoc = (w, h, body, extra = '') =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}"${extra}>${body}</svg>\n`;

/** Superellipse ("squircle") path, close to the continuous corners of macOS icons. */
function squircle(cx, cy, r, n = 5, pts = 120) {
  const P = [];
  for (let i = 0; i < pts; i++) {
    const t = (i / pts) * Math.PI * 2;
    const c = Math.cos(t), s = Math.sin(t);
    P.push([cx + r * Math.sign(c) * Math.abs(c) ** (2 / n), cy + r * Math.sign(s) * Math.abs(s) ** (2 / n)]);
  }
  return smooth(P);
}

// ------------------------------------------------------------------ mark

function markSvg() {
  return svgDoc(64, 64, mark('lo-mark'), ' fill="none" role="img" aria-label="logo"');
}
function markMonoSvg() {
  return svgDoc(64, 64, monoMark('lo-mono'), ' role="img" aria-label="logo"');
}

// ------------------------------------------------------------------ app icon

/** Flat brand colours shared by the icon, favicon and banner. */
const ICON = { tile: '#5B45E5', button: '#FFFFFF', body: '#C3BAFF' };

/**
 * App icon: a flat Iris squircle with the flat mark. `size` canvas, `tile` squircle size
 * (macOS grid: 1024 canvas, 824 tile).
 */
function appIcon({ size = 1024, tile = 824 } = {}) {
  const c = size / 2;
  const d = squircle(c, c, tile / 2);
  const k = (tile * 0.7) / 64; // mark scale
  const m = mark('ic', { button: ICON.button, body: ICON.body, gap: 2.4 });
  const body =
    `<path d="${d}" fill="${ICON.tile}"/>` +
    `<g transform="translate(${c - 32 * k} ${c - 32 * k}) scale(${k})">${m}</g>`;
  return svgDoc(size, size, body);
}

/** Favicon: the same tile with simpler geometry for 16-32 px. */
function favicon() {
  const d = squircle(32, 32, 31, 4.6, 64);
  const m = mark('fv', { button: ICON.button, body: ICON.body, ring: false, gap: 3.4 });
  const body = `<path d="${d}" fill="${ICON.tile}"/><g transform="translate(5.5 5.5) scale(0.83)">${m}</g>`;
  return svgDoc(64, 64, body);
}

// ------------------------------------------------------------------ tray

const TRAY = {
  template: { color: '#000000', note: 'macOS menu bar template image (system tints it)' },
  light: { color: '#1D1B2C', note: 'for light panels (Linux, Windows light taskbar)' },
  dark: { color: '#F4F3FA', note: 'for dark panels (Linux, Windows dark taskbar)' },
  color: { color: '#6A56F2', note: 'brand colour, readable on light and dark panels' },
};

function traySvg(kind, px = 16) {
  return svgDoc(px, px, trayGlyph(TRAY[kind].color, px));
}

// ------------------------------------------------------------------ main

function main() {
  const { toPng } = require('./lib/render.cjs');
  const mk = markSvg();
  write('brand/logo/mark.svg', mk);
  write('app/src/lib/brand/logo.svg', mk);
  write('brand/logo/mark-mono.svg', markMonoSvg());
  write('brand/logo/mark-512.png', toPng(mk, { width: 512 }));

  const icon = appIcon();
  write('brand/icon/app-icon.svg', icon);
  write('brand/icon/app-icon.png', toPng(icon, { width: 1024 }));
  // Full-bleed variant for platforms that don't expect the macOS padding (Windows tiles,
  // some Linux docks): the same tile at 94 % of the canvas.
  const full = appIcon({ tile: 964 });
  write('brand/icon/app-icon-full.svg', full);
  write('brand/icon/app-icon-full.png', toPng(full, { width: 1024 }));

  write('app/src/lib/brand/favicon.svg', favicon());

  for (const kind of Object.keys(TRAY)) {
    write(`brand/tray/tray-${kind}.svg`, traySvg(kind));
    for (const px of [16, 22, 24, 32]) write(`brand/tray/tray-${kind}-${px}.png`, toPng(traySvg(kind, px), { width: px }));
  }
  // macOS also wants @2x of its 16 and 22 pt images.
  write('brand/tray/tray-template-16@2x.png', toPng(traySvg('template', 32), { width: 32 }));
  write('brand/tray/tray-template-22@2x.png', toPng(traySvg('template', 44), { width: 44 }));
  console.log('logo assets written');
}

if (require.main === module) main();

module.exports = { appIcon, favicon, squircle, TRAY, ICON };
