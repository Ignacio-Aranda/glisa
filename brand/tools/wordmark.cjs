// Name-dependent assets: the wordmark and the README banner, regenerated from the name.
//
//   node brand/tools/wordmark.cjs "Glisa"
//
// Writes brand/logo/wordmark*.svg (+ PNG previews), app/src/lib/brand/logo-wordmark.svg
// and brand/readme-banner.{svg,png}. Text is converted to outlines (Inter, SIL OFL), so the
// files never depend on installed fonts. Needs opentype.js, @fontsource/inter and
// @resvg/resvg-js (see package.json).
'use strict';

const fs = require('fs');
const path = require('path');
const opentype = require('opentype.js');
const { mark } = require('./lib/mark.cjs');
const { toPng } = require('./lib/render.cjs');

const ROOT = path.resolve(__dirname, '../..');
const write = (p, s) => {
  const f = path.join(ROOT, p);
  fs.mkdirSync(path.dirname(f), { recursive: true });
  fs.writeFileSync(f, s);
};

const fonts = {};
function font(weight) {
  if (!fonts[weight]) {
    const file = require.resolve(`@fontsource/inter/files/inter-latin-${weight}-normal.woff`);
    const buf = fs.readFileSync(file);
    fonts[weight] = opentype.parse(buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength));
  }
  return fonts[weight];
}

/** Lay out text as one outline path. tracking in em. Returns { d, width, bbox }. */
function textPath(str, { weight = 600, size = 100, x = 0, y = 0, tracking = 0 } = {}) {
  const f = font(weight);
  const scale = size / f.unitsPerEm;
  const glyphs = [...str].map((ch) => f.charToGlyph(ch)); // no shaping needed for Latin
  let pen = x;
  const p = new opentype.Path();
  glyphs.forEach((g, i) => {
    const gp = g.getPath(pen, y, size);
    p.extend(gp);
    pen += g.advanceWidth * scale;
    if (i < glyphs.length - 1) pen += f.getKerningValue(g, glyphs[i + 1]) * scale + tracking * size;
  });
  const bb = p.getBoundingBox();
  return { d: p.toPathData(2), width: pen - x, bbox: bb };
}

const r2 = (n) => Math.round(n * 100) / 100;

/** Mark + name. color: text fill. Returns { svg body, w, h }. */
function wordmark(name, { color = 'currentColor', size = 100, idp = 'wm', markColors = {} } = {}) {
  const cap = 0.727 * size; // Inter cap height
  // The tilted mark covers x 14.3..50.2 and y 7.2..57.8 of its 64-unit box (1 unit margin kept).
  const markH = cap * 1.62;
  const k = markH / 50.6;
  const gap = size * 0.26;
  const midY = 32.5 * k; // visual centre of the mark
  const baseline = midY + cap / 2; // centre the caps on the mark
  const tx = 37 * k + gap;
  const text = textPath(name, { weight: 600, size, x: tx, y: baseline, tracking: -0.018 });
  const top = Math.min(6.2 * k, text.bbox.y1);
  const bottom = Math.max(58.8 * k, text.bbox.y2);
  const w = Math.ceil(text.bbox.x2 + size * 0.02);
  const h = Math.ceil(bottom - top);
  const body =
    `<g transform="translate(0 ${r2(-top)})">` +
    `<g transform="translate(${r2(-13.3 * k)} 0) scale(${r2(k)})">${mark(idp, markColors)}</g>` +
    `<path d="${text.d}" fill="${color}"/>` +
    `</g>`;
  return { body, w, h };
}

function wordmarkSvg(name, color, idp) {
  const { body, w, h } = wordmark(name, { color, idp });
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${w} ${h}" width="${w}" height="${h}" fill="none" role="img" aria-label="${name}">${body}</svg>\n`;
}

// ------------------------------------------------------------------ banner

function pill(x, y, label, { size = 21, weight = 500 } = {}) {
  const t = textPath(label, { weight, size, x: 0, y: 0 });
  const padX = size * 0.8, h = size * 1.85;
  const w = t.width + 2 * padX;
  return {
    w,
    svg: (dx) => {
      const tt = textPath(label, { weight, size, x: x + dx + padX, y: y + h / 2 + size * 0.36 });
      return (
        `<rect x="${r2(x + dx)}" y="${r2(y)}" width="${r2(w)}" height="${r2(h)}" rx="${r2(h / 2)}" fill="#FFFFFF" fill-opacity="0.14"/>` +
        `<path d="${tt.d}" fill="#FFFFFF"/>`
      );
    },
  };
}

/**
 * README banner: flat Iris, the mark, the wordmark, one line and the platforms.
 * No device art and no images, so it matches the app icon.
 */
function banner(name) {
  const W = 1600, H = 560;
  const { ICON } = require('./logo.cjs');
  const wm = wordmark(name, { color: '#FFFFFF', size: 128, idp: 'bn', markColors: { button: ICON.button, body: ICON.body } });
  const wmX = (W - wm.w) / 2, wmY = 118;
  const line = 'Your mouse and keyboard, set up your way';
  const lt = textPath(line, { weight: 500, size: 34, x: 0, y: 0, tracking: -0.005 });
  const l1 = textPath(line, { weight: 500, size: 34, x: (W - lt.width) / 2, y: wmY + wm.h + 74, tracking: -0.005 });
  const labels = ['Linux', 'Windows', 'macOS', 'Free and open source'];
  const py = wmY + wm.h + 118;
  const ps = labels.map((l) => pill(0, py, l));
  const total = ps.reduce((n, p) => n + p.w, 0) + 14 * (ps.length - 1);
  let px = (W - total) / 2;
  let pills = '';
  for (const p of ps) {
    pills += p.svg(px);
    px += p.w + 14;
  }
  const body =
    `<defs>` +
    `<linearGradient id="bn-bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#644FEE"/><stop offset="1" stop-color="#523DDB"/></linearGradient>` +
    `<clipPath id="bn-clip"><rect width="${W}" height="${H}" rx="28"/></clipPath>` +
    `</defs>` +
    `<g clip-path="url(#bn-clip)">` +
    `<rect width="${W}" height="${H}" fill="url(#bn-bg)"/>` +
    `<g transform="translate(${r2(wmX)} ${wmY})">${wm.body}</g>` +
    `<path d="${l1.d}" fill="#FFFFFF" fill-opacity="0.88"/>` +
    pills +
    `</g>`;
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" width="${W}" height="${H}" role="img" aria-label="${name}">${body}</svg>\n`;
}

function main() {
  const name = process.argv[2] || 'Glisa';
  const cur = wordmarkSvg(name, 'currentColor', 'wm');
  write('brand/logo/wordmark.svg', cur);
  write('app/src/lib/brand/logo-wordmark.svg', cur);
  const onLight = wordmarkSvg(name, '#17161F', 'wl');
  const onDark = wordmarkSvg(name, '#F4F3FA', 'wd');
  write('brand/logo/wordmark-on-light.svg', onLight);
  write('brand/logo/wordmark-on-dark.svg', onDark);
  write('brand/logo/wordmark-on-light.png', toPng(onLight, { width: 1200 }));
  write('brand/logo/wordmark-on-dark.png', toPng(onDark, { width: 1200 }));
  const b = banner(name);
  write('brand/readme-banner.svg', b);
  write('brand/readme-banner.png', toPng(b, { width: 1600 }));
  console.log(`wordmark and banner written for "${name}"`);
}

if (require.main === module) main();

module.exports = { textPath, wordmark };
