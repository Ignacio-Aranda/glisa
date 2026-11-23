// Rasterize our SVGs without a browser (resvg). resvg has no CSS variables, so the
// device classes are resolved to concrete colours for a theme first.
//
// Needs @resvg/resvg-js: `npm i --prefix brand/tools` (see brand/tools/package.json) or
// NODE_PATH pointing at a node_modules that has it.
'use strict';

const tokens = require('../tokens.cjs');

/** "rgb(1 2 3 / 0.4)" | "#abc" | "#aabbcc" -> { hex, alpha } */
function parseColor(v) {
  v = String(v).trim();
  let m = v.match(/^rgba?\(\s*([\d.]+)[ ,]+([\d.]+)[ ,]+([\d.]+)\s*(?:[/,]\s*([\d.]+%?))?\s*\)$/);
  if (m) {
    const hx = [m[1], m[2], m[3]].map((c) => Math.round(+c).toString(16).padStart(2, '0')).join('');
    let a = m[4] === undefined ? 1 : m[4].endsWith('%') ? parseFloat(m[4]) / 100 : +m[4];
    return { hex: '#' + hx, alpha: a };
  }
  if (/^#[0-9a-f]{3}$/i.test(v)) v = '#' + [...v.slice(1)].map((c) => c + c).join('');
  return { hex: v, alpha: 1 };
}

/**
 * Build a resvg-friendly <style> from a class map
 * { 'lo-body': { fill: '--dev-body' }, 'lo-hl': { opacity: '--dev-hl' }, ... }.
 */
function themeStyle(classMap, theme, { strokeScale = 1 } = {}) {
  const t = tokens.resolve(theme);
  let css = '';
  for (const [cls, props] of Object.entries(classMap)) {
    const decl = [];
    for (const [prop, val] of Object.entries(props)) {
      const raw = typeof val === 'string' && val.startsWith('--') ? t[val] : val;
      if (prop === 'fill' || prop === 'stroke' || prop === 'stop-color') {
        const c = parseColor(raw);
        decl.push(`${prop}:${c.hex}`);
        const op = prop === 'stop-color' ? 'stop-opacity' : `${prop}-opacity`;
        if (c.alpha !== 1) decl.push(`${op}:${c.alpha}`);
      } else if (prop === 'vector-effect') {
        // resvg ignores it: emulate non-scaling strokes with strokeScale (px per unit).
      } else if (prop === 'stroke-width') {
        decl.push(`stroke-width:${parseFloat(raw) / strokeScale}`);
      } else {
        decl.push(`${prop}:${raw}`);
      }
    }
    css += `.${cls}{${decl.join(';')}}`;
  }
  return css;
}

/** Inject a <style> right after the root <svg ...> tag. */
function withStyle(svg, css) {
  return svg.replace(/<svg\b[^>]*>/, (m) => `${m}<style>${css}</style>`);
}

function toPng(svg, { width, background } = {}) {
  const { Resvg } = require('@resvg/resvg-js');
  const r = new Resvg(svg, {
    fitTo: width ? { mode: 'width', value: width } : { mode: 'original' },
    background,
    font: { loadSystemFonts: false },
  });
  return r.render().asPng();
}

module.exports = { parseColor, themeStyle, withStyle, toPng };
