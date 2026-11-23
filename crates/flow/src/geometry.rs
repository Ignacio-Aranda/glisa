//! Screen geometry for Flow. Rectangles are `[x, y, w, h]` in the desktop's global logical
//! coordinates (what KWin / X11 / Windows / macOS report for the cursor).

use std::time::Instant;

pub type Rect = [i64; 4];
pub const DEFAULT_SCREEN: Rect = [0, 0, 1920, 1080];
/// Pixels inside the edge where an arriving cursor is placed.
pub const ENTRY_INSET: i64 = 6;

/// Valid rectangles only (positive size, sane coordinates), at most 16.
pub fn clean_rects(rects: &[Rect]) -> Vec<Rect> {
    rects
        .iter()
        .copied()
        .filter(|[x, y, w, h]| {
            *w > 0
                && *h > 0
                && x.abs() < 1 << 20
                && y.abs() < 1 << 20
                && *w < 1 << 16
                && *h < 1 << 16
        })
        .take(16)
        .collect()
}

pub fn contains(r: &Rect, x: f64, y: f64) -> bool {
    r[0] as f64 <= x && x < (r[0] + r[2]) as f64 && r[1] as f64 <= y && y < (r[1] + r[3]) as f64
}

pub fn output_at(rects: &[Rect], x: f64, y: f64) -> Option<Rect> {
    rects.iter().copied().find(|r| contains(r, x, y))
}

pub fn bounding(rects: &[Rect]) -> Rect {
    if rects.is_empty() {
        return DEFAULT_SCREEN;
    }
    let x0 = rects.iter().map(|r| r[0]).min().unwrap_or(0);
    let y0 = rects.iter().map(|r| r[1]).min().unwrap_or(0);
    let x1 = rects.iter().map(|r| r[0] + r[2]).max().unwrap_or(0);
    let y1 = rects.iter().map(|r| r[1] + r[3]).max().unwrap_or(0);
    [x0, y0, x1 - x0, y1 - y0]
}

/// (side, output) when (x, y) touches an OUTER edge of the desktop (an edge with no other
/// output behind it), else None.
pub fn edge_at(rects: &[Rect], x: f64, y: f64) -> Option<(&'static str, Rect)> {
    let o = output_at(rects, x, y)?;
    let [gx, gy, gw, gh] = o.map(|v| v as f64);
    if x < gx + 1.0 && output_at(rects, gx - 1.0, y).is_none() {
        return Some(("left", o));
    }
    if x >= gx + gw - 1.0 && output_at(rects, gx + gw, y).is_none() {
        return Some(("right", o));
    }
    if y < gy + 1.0 && output_at(rects, x, gy - 1.0).is_none() {
        return Some(("top", o));
    }
    if y >= gy + gh - 1.0 && output_at(rects, x, gy + gh).is_none() {
        return Some(("bottom", o));
    }
    None
}

pub fn distance_from(side: &str, r: &Rect, x: f64, y: f64) -> f64 {
    let [gx, gy, gw, gh] = r.map(|v| v as f64);
    match side {
        "left" => x - gx,
        "right" => gx + gw - 1.0 - x,
        "top" => y - gy,
        _ => gy + gh - 1.0 - y,
    }
}

/// True inside the dead zone at either end of that output's edge.
pub fn in_corner(r: &Rect, side: &str, x: f64, y: f64, corner_px: i64) -> bool {
    if corner_px <= 0 {
        return false;
    }
    let (lo, size, along) = if side == "left" || side == "right" {
        (r[1] as f64, r[3] as f64, y)
    } else {
        (r[0] as f64, r[2] as f64, x)
    };
    let corner = (corner_px as f64).min(size / 4.0);
    along < lo + corner || along >= lo + size - corner
}

/// 0..1 position along that side of the whole desktop.
pub fn relative(rects: &[Rect], side: &str, x: f64, y: f64) -> f64 {
    let [bx, by, bw, bh] = bounding(rects).map(|v| v as f64);
    let t = if side == "left" || side == "right" {
        (y - by) / (bh - 1.0).max(1.0)
    } else {
        (x - bx) / (bw - 1.0).max(1.0)
    };
    t.clamp(0.0, 1.0)
}

fn nearest(rects: &[Rect], along: i64, start: usize, size: usize) -> Rect {
    *rects
        .iter()
        .min_by_key(|r| {
            (along - r[start])
                .abs()
                .min((along - (r[start] + r[size] - 1)).abs())
        })
        .expect("at least one rectangle")
}

/// Where an arriving cursor goes: `t` along `side` of this desktop, `inset` px inside the
/// outermost output there.
pub fn entry_point_inset(rects: &[Rect], side: &str, t: f64, inset: i64) -> (i64, i64) {
    let rects: Vec<Rect> = if rects.is_empty() {
        vec![DEFAULT_SCREEN]
    } else {
        rects.to_vec()
    };
    let t = if t.is_finite() { t } else { 0.5 }.clamp(0.0, 1.0);
    let [bx, by, bw, bh] = bounding(&rects);
    if side == "left" || side == "right" {
        let mut along = by + (t * (bh - 1) as f64).round_ties_even() as i64;
        let mut span: Vec<Rect> = rects
            .iter()
            .copied()
            .filter(|r| r[1] <= along && along < r[1] + r[3])
            .collect();
        if span.is_empty() {
            // a gap in the layout: nearest output vertically
            let r0 = nearest(&rects, along, 1, 3);
            along = along.max(r0[1]).min(r0[1] + r0[3] - 1);
            span = vec![r0];
        }
        let r = if side == "left" {
            *span.iter().min_by_key(|r| r[0]).expect("span")
        } else {
            *span.iter().rev().max_by_key(|r| r[0] + r[2]).expect("span") // first of the widest, as Python's max()
        };
        let inset = inset.min(r[2] - 1);
        let x = if side == "left" {
            r[0] + inset
        } else {
            r[0] + r[2] - 1 - inset
        };
        return (x, along);
    }
    let mut along = bx + (t * (bw - 1) as f64).round_ties_even() as i64;
    let mut span: Vec<Rect> = rects
        .iter()
        .copied()
        .filter(|r| r[0] <= along && along < r[0] + r[2])
        .collect();
    if span.is_empty() {
        let r0 = nearest(&rects, along, 0, 2);
        along = along.max(r0[0]).min(r0[0] + r0[2] - 1);
        span = vec![r0];
    }
    let r = if side == "top" {
        *span.iter().min_by_key(|r| r[1]).expect("span")
    } else {
        *span.iter().rev().max_by_key(|r| r[1] + r[3]).expect("span")
    };
    let inset = inset.min(r[3] - 1);
    let y = if side == "top" {
        r[1] + inset
    } else {
        r[1] + r[3] - 1 - inset
    };
    (along, y)
}

pub fn entry_point(rects: &[Rect], side: &str, t: f64) -> (i64, i64) {
    entry_point_inset(rects, side, t, ENTRY_INSET)
}

/// The outer-edge pixel at `t` along `side` (used by the fake backend).
pub fn point_on_edge(rects: &[Rect], side: &str, t: f64) -> (i64, i64) {
    entry_point_inset(rects, side, t, 0)
}

/// A stretch of an outer edge of the desktop where the drag glow is shown: `len` pixels of
/// `side` of the output `output`, starting `start` pixels from its top (left and right
/// sides) or its left (top and bottom).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlowEdge {
    pub side: &'static str,
    pub output: Rect,
    pub start: i64,
    pub len: i64,
}

/// Every stretch of `side` of every output with nothing beyond it: exactly where
/// [`edge_at`] (and the KWin script) report that side.
pub fn outer_edges(rects: &[Rect], side: &'static str) -> Vec<GlowEdge> {
    let rects = clean_rects(rects);
    let vertical = side == "left" || side == "right";
    let mut out = Vec::new();
    for r in &rects {
        let [x, y, w, h] = *r;
        // The line just beyond that side, and the span along it.
        let (beyond, lo, len) = match side {
            "left" => (x - 1, y, h),
            "right" => (x + w, y, h),
            "top" => (y - 1, x, w),
            _ => (y + h, x, w),
        };
        // Spans of other outputs that cover the line beyond: not outer there.
        let mut covered: Vec<(i64, i64)> = rects
            .iter()
            .filter(|o| *o != r)
            .filter(|[ox, oy, ow, oh]| {
                if vertical {
                    *ox <= beyond && beyond < ox + ow
                } else {
                    *oy <= beyond && beyond < oy + oh
                }
            })
            .map(|[ox, oy, ow, oh]| {
                if vertical {
                    (*oy, oy + oh)
                } else {
                    (*ox, ox + ow)
                }
            })
            .collect();
        covered.sort();
        let mut at = lo;
        for (a, b) in covered.into_iter().chain([(lo + len, lo + len)]) {
            let (a, b) = (a.clamp(lo, lo + len), b.clamp(lo, lo + len));
            if a > at {
                out.push(GlowEdge {
                    side,
                    output: *r,
                    start: at - lo,
                    len: a - at,
                });
            }
            at = at.max(b);
        }
    }
    out
}

/// An event of [`EdgeTracker`].
#[derive(Debug, Clone, PartialEq)]
pub enum EdgeEvent {
    Edge {
        side: &'static str,
        x: f64,
        y: f64,
        rect: Rect,
    },
    Leave {
        side: &'static str,
    },
}

/// Turns pointer positions into edge/leave events, exactly like the KWin script does:
/// `edge` when the pointer reaches an outer edge (and again while it moves along it),
/// `leave` once it is `release_px` away from it.
pub struct EdgeTracker {
    pub release_px: f64,
    pub resend_px: f64,
    pub resend_s: f64,
    pub side: Option<&'static str>,
    pub rect: Option<Rect>,
    last: (f64, f64, Option<f64>),
}

impl Default for EdgeTracker {
    fn default() -> Self {
        Self::new(32.0)
    }
}

impl EdgeTracker {
    pub fn new(release_px: f64) -> Self {
        Self {
            release_px,
            resend_px: 24.0,
            resend_s: 0.12,
            side: None,
            rect: None,
            last: (f64::INFINITY, f64::INFINITY, None),
        }
    }

    pub fn reset(&mut self) {
        self.side = None;
        self.rect = None;
        self.last = (f64::INFINITY, f64::INFINITY, None);
    }

    /// `now` in seconds on any monotonic clock; `suppressed`: a window is being moved or a
    /// mouse button is held (never an edge then).
    pub fn feed(
        &mut self,
        x: f64,
        y: f64,
        rects: &[Rect],
        now: f64,
        suppressed: bool,
    ) -> Vec<EdgeEvent> {
        let mut out = Vec::new();
        let hit = if suppressed {
            None
        } else {
            edge_at(rects, x, y)
        };
        let (lx, ly, lt) = self.last;
        if let Some((side, rect)) = hit {
            let moved = (x - lx).abs() + (y - ly).abs();
            let since = lt.map_or(f64::INFINITY, |lt| now - lt);
            if Some(side) != self.side
                || moved >= self.resend_px
                || (moved > 0.0 && since >= self.resend_s)
            {
                if let Some(old) = self.side.filter(|old| *old != side) {
                    out.push(EdgeEvent::Leave { side: old });
                }
                self.side = Some(side);
                self.rect = Some(rect);
                self.last = (x, y, Some(now));
                out.push(EdgeEvent::Edge { side, x, y, rect });
            } else {
                self.rect = Some(rect); // sliding along the edge onto another output
            }
        } else if let Some(side) = self.side {
            let r = self.rect;
            if r.is_none_or(|r| {
                !contains(&r, x, y) || distance_from(side, &r, x, y) > self.release_px
            }) {
                out.push(EdgeEvent::Leave { side });
                self.reset();
            }
        }
        out
    }
}

/// Seconds since an arbitrary process-wide origin (for [`EdgeTracker::feed`]).
pub fn now_s() -> f64 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO: [Rect; 2] = [[0, 0, 1920, 1080], [1920, 0, 1280, 1024]];

    #[test]
    fn outer_edges_only() {
        assert_eq!(edge_at(&TWO, 0.0, 500.0).unwrap().0, "left");
        assert!(edge_at(&TWO, 1919.0, 500.0).is_none()); // inner edge between monitors
        assert_eq!(edge_at(&TWO, 3199.0, 500.0).unwrap().0, "right");
        assert_eq!(edge_at(&TWO, 1919.0, 1050.0).unwrap().0, "right"); // below the smaller one
        assert_eq!(edge_at(&TWO, 2500.0, 1023.0).unwrap().0, "bottom");
        assert!(edge_at(&TWO, 900.0, 500.0).is_none());
    }

    #[test]
    fn glow_edges_follow_the_outer_edges() {
        let e = |side, output, start, len| GlowEdge {
            side,
            output,
            start,
            len,
        };
        let [a, b] = TWO;
        // side by side: the outer side of each, and the stretch of the inner one below the
        // smaller monitor
        assert_eq!(
            outer_edges(&TWO, "right"),
            vec![e("right", a, 1024, 56), e("right", b, 0, 1024)]
        );
        assert_eq!(outer_edges(&TWO, "left"), vec![e("left", a, 0, 1080)]);
        // the taller one sticks out below the smaller one: that stretch is outer too
        let tall = [[0, 0, 1920, 1080], [1920, 0, 2560, 1440]];
        assert_eq!(
            outer_edges(&tall, "left"),
            vec![e("left", tall[0], 0, 1080), e("left", tall[1], 1080, 360)]
        );
        assert_eq!(
            outer_edges(&TWO, "bottom"),
            vec![e("bottom", a, 0, 1920), e("bottom", b, 0, 1280)]
        );
        assert_eq!(outer_edges(&TWO, "top").len(), 2);
        // a monitor on top of another one, centred
        let stacked = [[0, 0, 1920, 1080], [320, -1024, 1280, 1024]];
        assert_eq!(
            outer_edges(&stacked, "top"),
            vec![
                e("top", stacked[0], 0, 320),
                e("top", stacked[0], 1600, 320),
                e("top", stacked[1], 0, 1280)
            ]
        );
        // a glow edge is an outer edge, as edge_at sees it (corners left out: two sides)
        for (rects, side) in [
            (&tall[..], "left"),
            (&stacked[..], "top"),
            (&TWO[..], "right"),
        ] {
            for g in outer_edges(rects, side) {
                let [x, y, w, h] = g.output;
                for i in [g.start + 1, g.start + g.len - 2] {
                    let (px, py) = match side {
                        "left" => (x, y + i),
                        "right" => (x + w - 1, y + i),
                        "top" => (x + i, y),
                        _ => (x + i, y + h - 1),
                    };
                    assert_eq!(
                        edge_at(rects, px as f64, py as f64).map(|e| e.0),
                        Some(side),
                        "{g:?} at {i}"
                    );
                }
            }
        }
        assert!(outer_edges(&[], "left").is_empty());
    }

    #[test]
    fn corners_relative_entry() {
        let r = [0, 0, 1920, 1080];
        assert!(in_corner(&r, "left", 0.0, 10.0, 48));
        assert!(in_corner(&r, "left", 0.0, 1070.0, 48));
        assert!(!in_corner(&r, "left", 0.0, 540.0, 48));
        assert!((relative(&[r], "right", 1919.0, 539.5) - 0.5).abs() < 1e-9);
        assert_eq!(entry_point(&[r], "left", 0.5), (6, 540));
        assert_eq!(entry_point(&TWO, "right", 0.5), (3199 - 6, 540));
        assert_eq!(entry_point(&TWO, "right", 1.0), (1919 - 6, 1079)); // only the big one there
        assert_eq!(entry_point(&TWO, "top", 0.0), (0, 6));
        assert_eq!(entry_point(&[[0, 0, 2560, 1440]], "left", 0.25), (6, 360));
        assert_eq!(
            entry_point(&[[0, 0, 1920, 1080]], "right", 0.75),
            (1913, 809)
        );
    }

    #[test]
    fn tracker() {
        let mut tr = EdgeTracker::new(32.0);
        let rects = [[0, 0, 1920, 1080]];
        assert!(tr.feed(900.0, 500.0, &rects, 0.0, false).is_empty());
        let ev = tr.feed(1919.0, 500.0, &rects, 0.0, false);
        assert!(
            matches!(ev[0], EdgeEvent::Edge { side: "right", x, y, .. } if x == 1919.0 && y == 500.0)
        );
        assert!(tr.feed(1919.0, 500.0, &rects, 1.0, false).is_empty()); // no movement: nothing
        assert!(matches!(
            tr.feed(1919.0, 530.0, &rects, 0.0, false)[0],
            EdgeEvent::Edge { .. }
        ));
        assert!(tr.feed(1900.0, 530.0, &rects, 0.0, false).is_empty()); // inside the release band
        assert_eq!(
            tr.feed(1880.0, 530.0, &rects, 0.0, false),
            vec![EdgeEvent::Leave { side: "right" }]
        );
        assert!(tr.feed(1919.0, 500.0, &rects, 0.0, true).is_empty()); // dragging
    }
}
