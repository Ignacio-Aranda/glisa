// Small SVG helpers shared by the brand generators.
'use strict';

const r1 = (n) => Math.round(n * 10) / 10;

/** Closed (or open) Catmull-Rom spline through points, as a cubic Bezier path. */
function smooth(points, { closed = true, tension = 1 } = {}) {
  const n = points.length;
  const at = (i) => (closed ? points[(i + n) % n] : points[Math.max(0, Math.min(n - 1, i))]);
  let d = `M${r1(points[0][0])} ${r1(points[0][1])}`;
  const last = closed ? n : n - 1;
  for (let i = 0; i < last; i++) {
    const p0 = at(i - 1), p1 = at(i), p2 = at(i + 1), p3 = at(i + 2);
    const c1 = [p1[0] + ((p2[0] - p0[0]) / 6) * tension, p1[1] + ((p2[1] - p0[1]) / 6) * tension];
    const c2 = [p2[0] - ((p3[0] - p1[0]) / 6) * tension, p2[1] - ((p3[1] - p1[1]) / 6) * tension];
    d += ` C${r1(c1[0])} ${r1(c1[1])} ${r1(c2[0])} ${r1(c2[1])} ${r1(p2[0])} ${r1(p2[1])}`;
  }
  return closed ? d + 'Z' : d;
}

/** Point on a Catmull-Rom spline (closed) at parameter t in [0, n). */
function splinePoint(points, t) {
  const n = points.length;
  const i = Math.floor(t) % n;
  const u = t - Math.floor(t);
  const p0 = points[(i - 1 + n) % n], p1 = points[i], p2 = points[(i + 1) % n], p3 = points[(i + 2) % n];
  const f = (a, b, c, d) =>
    0.5 * (2 * b + (-a + c) * u + (2 * a - 5 * b + 4 * c - d) * u * u + (-a + 3 * b - 3 * c + d) * u * u * u);
  return [f(p0[0], p1[0], p2[0], p3[0]), f(p0[1], p1[1], p2[1], p3[1])];
}

/** Rounded rectangle path. */
function rrect(x, y, w, h, r) {
  r = Math.min(r, w / 2, h / 2);
  return (
    `M${r1(x + r)} ${r1(y)}H${r1(x + w - r)}A${r} ${r} 0 0 1 ${r1(x + w)} ${r1(y + r)}` +
    `V${r1(y + h - r)}A${r} ${r} 0 0 1 ${r1(x + w - r)} ${r1(y + h)}H${r1(x + r)}` +
    `A${r} ${r} 0 0 1 ${r1(x)} ${r1(y + h - r)}V${r1(y + r)}A${r} ${r} 0 0 1 ${r1(x + r)} ${r1(y)}Z`
  );
}

/** Ellipse as a path (so it can share helpers that take `d`). */
function ellipse(cx, cy, rx, ry) {
  return `M${r1(cx - rx)} ${r1(cy)}A${rx} ${ry} 0 1 0 ${r1(cx + rx)} ${r1(cy)}A${rx} ${ry} 0 1 0 ${r1(cx - rx)} ${r1(cy)}Z`;
}

function attrs(o) {
  return Object.entries(o)
    .filter(([, v]) => v !== undefined && v !== null && v !== false)
    .map(([k, v]) => `${k}="${v}"`)
    .join(' ');
}

function path(d, a = {}) {
  return `<path ${attrs({ d, ...a })}/>`;
}

module.exports = { r1, smooth, splinePoint, rrect, ellipse, attrs, path };
