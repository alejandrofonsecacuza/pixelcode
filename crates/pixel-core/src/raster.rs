//! Shape rasterization into spans.
//!
//! Coordinate convention:
//! - **Fills** (rect, ellipse, polygon, path): continuous coordinates; pixel `(x, y)` covers
//!   `[x, x+1) × [y, y+1)` and is painted when its center falls inside the shape.
//! - **Strokes** of lines and paths go through pixel centers, so `line(0,0,9,0)` paints
//!   exactly pixels 0..=9 of row 0.
//! - **Strokes** of rects and ellipses are inner strokes (they stay inside the filled shape).

use rayon::prelude::*;

use crate::canvas::Raster;
use crate::span::{IRect, Span};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Cap {
    #[default]
    Round,
    Square,
    Butt,
}

/// First pixel whose center is at or after `v`, clamped to `[lo, hi]`.
#[inline]
fn edge(v: f64, lo: i32, hi: i32) -> i32 {
    let e = (v - 0.5).ceil();
    if e.is_nan() {
        lo
    } else {
        e.clamp(lo as f64, hi as f64) as i32
    }
}

pub fn rect(x: f64, y: f64, w: f64, h: f64, clip: IRect, out: &mut Vec<Span>) {
    let (x, w) = if w < 0.0 { (x + w, -w) } else { (x, w) };
    let (y, h) = if h < 0.0 { (y + h, -h) } else { (y, h) };
    let x0 = edge(x, clip.x0, clip.x1);
    let x1 = edge(x + w, clip.x0, clip.x1);
    let y0 = edge(y, clip.y0, clip.y1);
    let y1 = edge(y + h, clip.y0, clip.y1);
    if x0 >= x1 {
        return;
    }
    out.extend((y0..y1).map(|yy| Span::new(yy, x0, x1)));
}

/// Inner border of a rectangle, `width` pixels thick.
pub fn rect_stroke(x: f64, y: f64, w: f64, h: f64, width: i32, clip: IRect, out: &mut Vec<Span>) {
    let big = clip.expand(width + 1);
    let mut rows = Vec::new();
    rect(x, y, w, h, big, &mut rows);
    let Some(first) = rows.first().copied() else {
        return;
    };
    let last = rows[rows.len() - 1];
    let (x0, x1) = (first.x0, first.x1);
    let width = width.max(1);
    for s in rows {
        if s.y < clip.y0 || s.y >= clip.y1 {
            continue;
        }
        if s.y < first.y + width || s.y > last.y - width || x1 - x0 <= 2 * width {
            out.push(s);
        } else {
            out.push(Span::new(s.y, x0, x0 + width));
            out.push(Span::new(s.y, x1 - width, x1));
        }
    }
}

/// Rows of a filled ellipse (one per row, contiguous, possibly empty).
/// Strict inclusion avoids single-pixel nubs at the extremes.
pub fn ellipse_rows(cx: f64, cy: f64, rx: f64, ry: f64, ylo: i32, yhi: i32) -> Vec<Span> {
    let mut out = Vec::new();
    if !(rx > 0.0 && ry > 0.0) || !cx.is_finite() || !cy.is_finite() {
        return out;
    }
    let fy0 = ((cy - ry - 0.5).floor() + 1.0).max(ylo as f64);
    let fy1 = ((cy + ry - 0.5).ceil() - 1.0).min(yhi as f64 - 1.0);
    if fy0 > fy1 {
        return out;
    }
    let lim = 1.0e9;
    for y in fy0 as i32..=fy1 as i32 {
        let dy = (y as f64 + 0.5 - cy) / ry;
        let k = 1.0 - dy * dy;
        if k <= 0.0 {
            out.push(Span::new(y, 0, 0));
            continue;
        }
        let half = rx * k.sqrt();
        let x0 = ((cx - half - 0.5).floor() + 1.0).clamp(-lim, lim) as i32;
        let x1 = (cx + half - 0.5).ceil().clamp(-lim, lim) as i32;
        out.push(Span::new(y, x0, x1));
    }
    out
}

/// Inner stroke of thickness `w` for one-span-per-row shapes: the shape minus its erosion by a
/// diamond of radius `w` (with `w = 1` this is the classic 8-connected pixel-art outline).
pub fn inner_stroke(rows: &[Span], w: i32, out: &mut Vec<Span>) {
    let w = w.max(1);
    let n = rows.len() as i32;
    for i in 0..n {
        let s = rows[i as usize];
        if s.x0 >= s.x1 {
            continue;
        }
        let mut lo = i32::MIN;
        let mut hi = i32::MAX;
        let mut interior = true;
        for k in -w..=w {
            let j = i + k;
            if j < 0 || j >= n {
                interior = false;
                break;
            }
            let r = rows[j as usize];
            if r.x0 >= r.x1 {
                interior = false;
                break;
            }
            let m = w - k.abs();
            lo = lo.max(r.x0 + m);
            hi = hi.min(r.x1 - m);
        }
        if !interior || lo >= hi {
            out.push(s);
        } else {
            if s.x0 < lo {
                out.push(Span::new(s.y, s.x0, lo.min(s.x1)));
            }
            if hi < s.x1 {
                out.push(Span::new(s.y, hi.max(s.x0), s.x1));
            }
        }
    }
}

struct Edge {
    ymin: f64,
    ymax: f64,
    x0: f64,
    slope: f64,
    dir: i32,
}

/// Polygon fill (several implicitly closed contours) with an active edge table.
pub fn polygon(contours: &[Vec<(f64, f64)>], rule: FillRule, clip: IRect, out: &mut Vec<Span>) {
    let mut edges: Vec<Edge> = Vec::new();
    for c in contours {
        let n = c.len();
        if n < 2 {
            continue;
        }
        for i in 0..n {
            let p = c[i];
            let q = c[(i + 1) % n];
            if p.1 == q.1
                || !(p.0.is_finite() && p.1.is_finite() && q.0.is_finite() && q.1.is_finite())
            {
                continue;
            }
            let (a, b, dir) = if p.1 < q.1 { (p, q, 1) } else { (q, p, -1) };
            edges.push(Edge {
                ymin: a.1,
                ymax: b.1,
                x0: a.0,
                slope: (b.0 - a.0) / (b.1 - a.1),
                dir,
            });
        }
    }
    if edges.is_empty() {
        return;
    }
    edges.sort_by(|a, b| a.ymin.total_cmp(&b.ymin));
    let ymin = edges[0].ymin;
    let ymax = edges.iter().map(|e| e.ymax).fold(f64::MIN, f64::max);
    let y0 = edge(ymin, clip.y0, clip.y1);
    let y1 = edge(ymax, clip.y0, clip.y1);
    let rows = (y1 - y0).max(0) as usize;
    // Large polygons: row bands in parallel (each band builds its own active edge table).
    if rows >= 256 && rows * edges.len() >= 1 << 17 {
        let band = 64;
        let parts: Vec<Vec<Span>> = (y0..y1)
            .step_by(band)
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|b0| {
                let mut v = Vec::new();
                scan_rows(&edges, rule, b0, (b0 + band as i32).min(y1), clip, &mut v);
                v
            })
            .collect();
        for p in parts {
            out.extend(p);
        }
    } else {
        scan_rows(&edges, rule, y0, y1, clip, out);
    }
}

fn scan_rows(edges: &[Edge], rule: FillRule, y0: i32, y1: i32, clip: IRect, out: &mut Vec<Span>) {
    let (xlo, xhi) = (clip.x0, clip.x1);
    let mut active: Vec<usize> = Vec::new();
    let mut xs: Vec<(f64, i32)> = Vec::new();
    let mut next = 0;
    for y in y0..y1 {
        let yc = y as f64 + 0.5;
        while next < edges.len() && edges[next].ymin <= yc {
            active.push(next);
            next += 1;
        }
        active.retain(|&e| edges[e].ymax > yc);
        xs.clear();
        for &e in &active {
            let ed = &edges[e];
            xs.push((ed.x0 + (yc - ed.ymin) * ed.slope, ed.dir));
        }
        xs.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let mut push = |xa: f64, xb: f64| {
            let a = edge(xa, xlo, xhi);
            let b = edge(xb, xlo, xhi);
            if a < b {
                out.push(Span::new(y, a, b));
            }
        };
        match rule {
            FillRule::EvenOdd => {
                for pair in xs.as_chunks::<2>().0 {
                    push(pair[0].0, pair[1].0);
                }
            }
            FillRule::NonZero => {
                let mut wind = 0;
                let mut start = 0.0;
                for &(x, d) in &xs {
                    let prev = wind;
                    wind += d;
                    if prev == 0 && wind != 0 {
                        start = x;
                    } else if prev != 0 && wind == 0 {
                        push(start, x);
                    }
                }
            }
        }
    }
}

/// Liang–Barsky clipping of a segment against a rectangle.
fn clip_segment(
    mut x0: f64,
    mut y0: f64,
    mut x1: f64,
    mut y1: f64,
    r: IRect,
) -> Option<(f64, f64, f64, f64)> {
    let (xmin, ymin, xmax, ymax) = (r.x0 as f64, r.y0 as f64, r.x1 as f64, r.y1 as f64);
    let (dx, dy) = (x1 - x0, y1 - y0);
    let mut t0: f64 = 0.0;
    let mut t1: f64 = 1.0;
    for (p, q) in [
        (-dx, x0 - xmin),
        (dx, xmax - x0),
        (-dy, y0 - ymin),
        (dy, ymax - y0),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
        }
    }
    if t0 > t1 {
        return None;
    }
    let (ox, oy) = (x0, y0);
    if t1 < 1.0 {
        x1 = ox + t1 * dx;
        y1 = oy + t1 * dy;
    }
    if t0 > 0.0 {
        x0 = ox + t0 * dx;
        y0 = oy + t0 * dy;
    }
    Some((x0, y0, x1, y1))
}

/// One-pixel line (Bresenham) between pixel centers, endpoints included.
pub fn thin_line(x0: f64, y0: f64, x1: f64, y1: f64, clip: IRect, out: &mut Vec<Span>) {
    let Some((x0, y0, x1, y1)) = clip_segment(x0, y0, x1, y1, clip.expand(2)) else {
        return;
    };
    let (mut x, mut y) = (x0.round() as i32, y0.round() as i32);
    let (xe, ye) = (x1.round() as i32, y1.round() as i32);
    let dx = (xe - x).abs();
    let dy = -(ye - y).abs();
    let sx = if x < xe { 1 } else { -1 };
    let sy = if y < ye { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        out.push(Span::new(y, x, x + 1));
        if x == xe && y == ye {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

pub fn circle_polygon(cx: f64, cy: f64, r: f64) -> Vec<(f64, f64)> {
    let n = ((r * std::f64::consts::TAU / 1.5) as usize).clamp(8, 512);
    (0..n)
        .map(|i| {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

fn orient_positive(mut c: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    let n = c.len();
    let mut a = 0.0;
    for i in 0..n {
        let (p, q) = (c[i], c[(i + 1) % n]);
        a += p.0 * q.1 - q.0 * p.1;
    }
    if a < 0.0 {
        c.reverse();
    }
    c
}

/// Strokes a polyline. With `width <= 1` it uses Bresenham; otherwise it unions per-segment quads,
/// round joins and caps in a single nonzero pass (no double blending).
pub fn stroke_polyline(
    pts: &[(f64, f64)],
    closed: bool,
    width: f64,
    cap: Cap,
    clip: IRect,
    out: &mut Vec<Span>,
) {
    if pts.is_empty() {
        return;
    }
    if width <= 1.0 {
        if pts.len() == 1 {
            thin_line(pts[0].0, pts[0].1, pts[0].0, pts[0].1, clip, out);
        }
        for w in pts.windows(2) {
            thin_line(w[0].0, w[0].1, w[1].0, w[1].1, clip, out);
        }
        if closed && pts.len() > 2 {
            let (a, b) = (pts[pts.len() - 1], pts[0]);
            thin_line(a.0, a.1, b.0, b.1, clip, out);
        }
        return;
    }
    let hw = width / 2.0;
    let p: Vec<(f64, f64)> = pts.iter().map(|&(x, y)| (x + 0.5, y + 0.5)).collect();
    let mut segs: Vec<((f64, f64), (f64, f64))> = p.windows(2).map(|w| (w[0], w[1])).collect();
    if closed && p.len() > 2 {
        segs.push((p[p.len() - 1], p[0]));
    }
    if !closed && cap == Cap::Square && !segs.is_empty() {
        let ext = |a: (f64, f64), b: (f64, f64)| {
            let (dx, dy) = (a.0 - b.0, a.1 - b.1);
            let l = dx.hypot(dy);
            if l > 1e-9 {
                (a.0 + dx / l * hw, a.1 + dy / l * hw)
            } else {
                a
            }
        };
        let f = segs[0];
        segs[0].0 = ext(f.0, f.1);
        let k = segs.len() - 1;
        let l = segs[k];
        segs[k].1 = ext(l.1, l.0);
    }
    let mut contours: Vec<Vec<(f64, f64)>> = Vec::new();
    for &(a, b) in &segs {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l = dx.hypot(dy);
        if l < 1e-9 {
            continue;
        }
        let (nx, ny) = (-dy / l * hw, dx / l * hw);
        contours.push(orient_positive(vec![
            (a.0 + nx, a.1 + ny),
            (b.0 + nx, b.1 + ny),
            (b.0 - nx, b.1 - ny),
            (a.0 - nx, a.1 - ny),
        ]));
    }
    let n = p.len();
    for (i, &v) in p.iter().enumerate() {
        let is_end = !closed && (i == 0 || i == n - 1);
        if (is_end && cap == Cap::Round) || !is_end || contours.is_empty() {
            contours.push(orient_positive(circle_polygon(v.0, v.1, hw)));
        }
    }
    polygon(&contours, FillRule::NonZero, clip, out);
}

/// Scanline flood fill on the raster: 4-connected pixels whose color differs from the seed's by
/// at most `tol` per channel.
pub fn flood(r: &Raster, x: i32, y: i32, tol: u8, clip: IRect, out: &mut Vec<Span>) {
    let clip = clip.intersect(&r.bounds());
    if !clip.contains(x, y) {
        return;
    }
    let target = r.get(x, y);
    let tol = tol as i32;
    let matches = |p: u32| -> bool {
        if (p >> 24) == 0 && (target >> 24) == 0 {
            return true;
        }
        (0..4).all(|i| {
            (((p >> (i * 8)) & 255) as i32 - ((target >> (i * 8)) & 255) as i32).abs() <= tol
        })
    };
    let (cw, ch) = (clip.w() as usize, clip.h() as usize);
    let mut seen = vec![0u64; (cw * ch).div_ceil(64)];
    let idx = |x: i32, y: i32| (y - clip.y0) as usize * cw + (x - clip.x0) as usize;
    let is_seen = |s: &Vec<u64>, i: usize| s[i >> 6] & (1 << (i & 63)) != 0;
    let mut stack = vec![(x, y)];
    while let Some((sx, sy)) = stack.pop() {
        if is_seen(&seen, idx(sx, sy)) || !matches(r.get(sx, sy)) {
            continue;
        }
        let mut l = sx;
        while l > clip.x0 && !is_seen(&seen, idx(l - 1, sy)) && matches(r.get(l - 1, sy)) {
            l -= 1;
        }
        let mut rr = sx;
        while rr + 1 < clip.x1 && !is_seen(&seen, idx(rr + 1, sy)) && matches(r.get(rr + 1, sy)) {
            rr += 1;
        }
        for xx in l..=rr {
            let i = idx(xx, sy);
            seen[i >> 6] |= 1 << (i & 63);
        }
        out.push(Span::new(sy, l, rr + 1));
        for ny in [sy - 1, sy + 1] {
            if ny < clip.y0 || ny >= clip.y1 {
                continue;
            }
            let mut xx = l;
            while xx <= rr {
                if !is_seen(&seen, idx(xx, ny)) && matches(r.get(xx, ny)) {
                    stack.push((xx, ny));
                    while xx <= rr && matches(r.get(xx, ny)) {
                        xx += 1;
                    }
                } else {
                    xx += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::normalize;

    const CLIP: IRect = IRect {
        x0: 0,
        y0: 0,
        x1: 100,
        y1: 100,
    };

    #[test]
    fn rect_is_exact() {
        let mut v = Vec::new();
        rect(2.0, 3.0, 4.0, 5.0, CLIP, &mut v);
        assert_eq!(v.len(), 5);
        assert!(v.iter().all(|s| s.x0 == 2 && s.x1 == 6));
        assert_eq!(v[0].y, 3);
    }

    #[test]
    fn polygon_matches_rect() {
        let mut a = Vec::new();
        polygon(
            &[vec![(2.0, 3.0), (6.0, 3.0), (6.0, 8.0), (2.0, 8.0)]],
            FillRule::NonZero,
            CLIP,
            &mut a,
        );
        let mut b = Vec::new();
        rect(2.0, 3.0, 4.0, 5.0, CLIP, &mut b);
        assert_eq!(a, b);
    }

    #[test]
    fn circle_is_symmetric() {
        let rows = ellipse_rows(10.5, 10.5, 5.0, 5.0, 0, 100);
        for s in &rows {
            assert_eq!(10 - s.x0, s.x1 - 1 - 10, "row {s:?}");
        }
        assert_eq!(rows.first().unwrap().y, 6);
        assert_eq!(rows.last().unwrap().y, 14);
    }

    #[test]
    fn thin_line_endpoints_inclusive() {
        let mut v = Vec::new();
        thin_line(0.0, 0.0, 9.0, 0.0, CLIP, &mut v);
        normalize(&mut v, CLIP);
        assert_eq!(v, vec![Span::new(0, 0, 10)]);
        let mut d = Vec::new();
        thin_line(0.0, 0.0, 5.0, 5.0, CLIP, &mut d);
        assert_eq!(d.len(), 6);
    }

    #[test]
    fn inner_stroke_of_circle_is_ring() {
        let rows = ellipse_rows(20.0, 20.0, 10.0, 10.0, 0, 100);
        let mut v = Vec::new();
        inner_stroke(&rows, 1, &mut v);
        let mid = v.iter().filter(|s| s.y == 19).collect::<Vec<_>>();
        assert_eq!(mid.len(), 2);
        assert!(mid.iter().all(|s| s.x1 - s.x0 == 1));
    }

    #[test]
    fn flood_fills_enclosed_area() {
        let mut r = Raster::new(20, 20);
        let mut border = Vec::new();
        rect_stroke(0.0, 0.0, 10.0, 10.0, 1, r.bounds(), &mut border);
        normalize(&mut border, r.bounds());
        r.paint_spans(
            &border,
            &crate::paint::Paint::Solid(0xff00_00ff),
            crate::color::BlendMode::Over,
        );
        let mut f = Vec::new();
        flood(&r, 5, 5, 0, r.bounds(), &mut f);
        assert_eq!(crate::span::area(&f), 64);
    }

    #[test]
    fn thick_stroke_has_no_overlap() {
        let mut v = Vec::new();
        stroke_polyline(
            &[(5.0, 5.0), (30.0, 5.0), (30.0, 30.0)],
            false,
            4.0,
            Cap::Round,
            CLIP,
            &mut v,
        );
        let n = v.len();
        normalize(&mut v, CLIP);
        assert_eq!(n, v.len(), "nonzero union already yields disjoint spans");
    }
}
