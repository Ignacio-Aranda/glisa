// The logo mark: a mouse seen from above, tilted, with its left button picked out and a
// hotspot ring on it (the same ring the app draws on device illustrations).
// Geometry on a 64-unit grid; everything here returns SVG fragments.
'use strict';

const EGG = 'M32 7C42.5 7 48 15 48 27V40C48 51.5 41 58 32 58C23 58 16 51.5 16 40V27C16 15 21.5 7 32 7Z';
const SPLIT = 28;
const TILT = -14;
const HOT = [24.6, 18.6];

/** Brand colours: Iris is the only accent; lavender is Iris at low strength. */
const IRIS = '#5B45E5';
const LAVENDER = '#C8C0FF';

/**
 * Colour mark, flat. `p` prefixes ids. Options:
 *  button: fill of the left button; body: fill of the rest; hot: fill of the hotspot dot;
 *  ring: true draws the hotspot as a ring (else a plain dot hole); tilt in degrees.
 */
function mark(p, o = {}) {
  const { button = IRIS, body = LAVENDER, hot = null, ring = true, tilt = TILT, gap = 2.6 } = o;
  const seam = `M32 2V${SPLIT}M8 ${SPLIT + 1.6}C21 ${SPLIT - 1.2} 43 ${SPLIT - 1.2} 56 ${SPLIT + 1.6}`;
  const [hx, hy] = HOT;
  const hole = ring ? 4.7 : 3.3;
  return (
    `<defs>` +
    `<clipPath id="${p}-l"><path d="M0 0H32V${SPLIT}C22 ${SPLIT} 12 ${SPLIT + 1} 0 ${SPLIT + 2}Z"/></clipPath>` +
    `<mask id="${p}-m" maskUnits="userSpaceOnUse" x="-8" y="-8" width="80" height="80">` +
    `<rect x="-8" y="-8" width="80" height="80" fill="#fff"/>` +
    `<path d="${seam}" stroke="#000" stroke-width="${gap}" fill="none"/>` +
    `<circle cx="${hx}" cy="${hy}" r="${hole}" fill="#000"/></mask>` +
    `</defs>` +
    `<g transform="rotate(${tilt} 32 32)">` +
    `<g mask="url(#${p}-m)"><path d="${EGG}" fill="${body}"/><path d="${EGG}" fill="${button}" clip-path="url(#${p}-l)"/></g>` +
    (ring ? `<circle cx="${hx}" cy="${hy}" r="2.3" fill="${hot ?? button}"/>` : '') +
    `</g>`
  );
}

/** Single-colour mark (fill = `color`, default currentColor). */
function monoMark(p, { color = 'currentColor', tilt = TILT, ring = true, gap = 2.8 } = {}) {
  const seam = `M32 2V${SPLIT}M8 ${SPLIT + 1.6}C21 ${SPLIT - 1.2} 43 ${SPLIT - 1.2} 56 ${SPLIT + 1.6}`;
  const [hx, hy] = HOT;
  return (
    `<defs><mask id="${p}-m" maskUnits="userSpaceOnUse" x="-8" y="-8" width="80" height="80">` +
    `<rect x="-8" y="-8" width="80" height="80" fill="#fff"/>` +
    `<path d="${seam}" stroke="#000" stroke-width="${gap}" fill="none"/>` +
    `<circle cx="${hx}" cy="${hy}" r="${ring ? 4.7 : 3.3}" fill="#000"/></mask></defs>` +
    `<g transform="rotate(${tilt} 32 32)" fill="${color}">` +
    `<path d="${EGG}" mask="url(#${p}-m)"/>` +
    (ring ? `<circle cx="${hx}" cy="${hy}" r="2.3"/>` : '') +
    `</g>`
  );
}

/**
 * Tray glyph drawn for a given pixel size: upright, seams snapped to whole pixels
 * (1 px below 32 px, 2 px at 32 and above) and a dot hole for the hotspot, so it stays
 * crisp at 16, 22, 24 and 32 px. The viewBox is 0 0 px px.
 */
function trayGlyph(color, px = 16) {
  const s = px / 16;
  const g = px >= 32 ? 2 : 1;
  const f = (n) => Math.round(n * s * 100) / 100;
  const egg =
    `M${f(8)} ${f(0.5)}C${f(10.9)} ${f(0.5)} ${f(12.5)} ${f(2.6)} ${f(12.5)} ${f(5.6)}V${f(10.6)}` +
    `C${f(12.5)} ${f(13.6)} ${f(10.6)} ${f(15.5)} ${f(8)} ${f(15.5)}C${f(5.4)} ${f(15.5)} ${f(3.5)} ${f(13.6)} ${f(3.5)} ${f(10.6)}` +
    `V${f(5.6)}C${f(3.5)} ${f(2.6)} ${f(5.1)} ${f(0.5)} ${f(8)} ${f(0.5)}Z`;
  const vx = Math.round(8 * s - g / 2);
  const hy = Math.round(6.6 * s);
  const r = Math.max(1.1, 1.15 * s);
  return (
    `<defs><mask id="t-m" maskUnits="userSpaceOnUse" x="0" y="0" width="${px}" height="${px}">` +
    `<rect width="${px}" height="${px}" fill="#fff"/>` +
    `<rect x="${vx}" y="0" width="${g}" height="${hy}" fill="#000"/>` +
    `<rect x="0" y="${hy}" width="${px}" height="${g}" fill="#000"/>` +
    `<circle cx="${f(5.9)}" cy="${f(3.7)}" r="${Math.round(r * 100) / 100}" fill="#000"/></mask></defs>` +
    `<path d="${egg}" fill="${color}" mask="url(#t-m)"/>`
  );
}

module.exports = { mark, monoMark, trayGlyph, EGG, IRIS, LAVENDER };
