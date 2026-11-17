//! Which color goes into each pixel of a span: solid, gradient (banded and dithered) or image.

use std::sync::Arc;

use crate::color::{BlendMode, Color, blend, over};
use crate::image::Image;

/// 8×8 Bayer matrix for ordered dithering.
const BAYER8: [u8; 64] = [
    0, 32, 8, 40, 2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26, 12, 44, 4, 36, 14, 46, 6, 38, 60,
    28, 52, 20, 62, 30, 54, 22, 3, 35, 11, 43, 1, 33, 9, 41, 51, 19, 59, 27, 49, 17, 57, 25, 15,
    47, 7, 39, 13, 45, 5, 37, 63, 31, 55, 23, 61, 29, 53, 21,
];

#[derive(Clone, Debug)]
pub struct Gradient {
    /// Stops `(t, [r, g, b, a])`, sorted by `t` in 0..1.
    pub stops: Vec<(f64, [f64; 4])>,
    /// 0 = smooth; `n >= 2` = quantize into `n` bands (pixel-art style).
    pub steps: u32,
    /// With bands: Bayer-dithered transitions instead of hard edges.
    pub dither: bool,
    /// Precomputed colors: `LUT_N + 1` samples (smooth) or one per band.
    lut: Arc<[u32]>,
}

const LUT_N: usize = 4096;

impl Gradient {
    pub fn new(mut stops: Vec<(f64, Color)>, steps: u32, dither: bool) -> Self {
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        let stops = stops
            .into_iter()
            .map(|(t, c)| {
                (
                    t.clamp(0.0, 1.0),
                    [c.r as f64, c.g as f64, c.b as f64, c.a as f64],
                )
            })
            .collect();
        let mut g = Self {
            stops,
            steps,
            dither,
            lut: Arc::from(Vec::new()),
        };
        let lut: Vec<u32> = if steps >= 2 {
            (0..steps)
                .map(|k| g.eval(k as f64 / (steps - 1) as f64))
                .collect()
        } else {
            (0..=LUT_N)
                .map(|k| g.eval(k as f64 / LUT_N as f64))
                .collect()
        };
        g.lut = Arc::from(lut);
        g
    }

    /// Evenly spaced stops.
    pub fn even(colors: &[Color], steps: u32, dither: bool) -> Self {
        let n = colors.len().max(2) - 1;
        Self::new(
            colors
                .iter()
                .enumerate()
                .map(|(i, &c)| (i as f64 / n as f64, c))
                .collect(),
            steps,
            dither,
        )
    }

    #[inline]
    pub fn sample(&self, t: f64, x: i32, y: i32) -> u32 {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        if self.steps >= 2 {
            let n = (self.steps - 1) as f64;
            let v = t * n;
            let k = if self.dither {
                let th = BAYER8[((y & 7) * 8 + (x & 7)) as usize] as f64 / 64.0 + 1.0 / 128.0;
                let base = v.floor();
                (base + if v - base > th { 1.0 } else { 0.0 }).min(n)
            } else {
                v.round()
            };
            self.lut[k as usize]
        } else {
            self.lut[(t * LUT_N as f64 + 0.5) as usize]
        }
    }

    /// Exact interpolation between stops (only used to build the lookup table).
    fn eval(&self, t: f64) -> u32 {
        let s = &self.stops;
        if s.is_empty() {
            return 0;
        }
        if t <= s[0].0 {
            return pack(s[0].1);
        }
        let last = s[s.len() - 1];
        if t >= last.0 {
            return pack(last.1);
        }
        let i = s.partition_point(|st| st.0 <= t).max(1) - 1;
        let (t0, c0) = s[i];
        let (t1, c1) = s[i + 1];
        let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
        pack([
            c0[0] + (c1[0] - c0[0]) * f,
            c0[1] + (c1[1] - c0[1]) * f,
            c0[2] + (c1[2] - c0[2]) * f,
            c0[3] + (c1[3] - c0[3]) * f,
        ])
    }
}

#[inline]
fn pack(c: [f64; 4]) -> u32 {
    let u = |v: f64| v.round().clamp(0.0, 255.0) as u32;
    u(c[0]) | u(c[1]) << 8 | u(c[2]) << 16 | u(c[3]) << 24
}

#[derive(Clone, Debug)]
pub enum GradientShape {
    Linear {
        x0: f64,
        y0: f64,
        dx: f64,
        dy: f64,
        inv_len2: f64,
    },
    Radial {
        cx: f64,
        cy: f64,
        inv_r: f64,
    },
}

impl GradientShape {
    pub fn linear(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        let (dx, dy) = (x1 - x0, y1 - y0);
        let l2 = dx * dx + dy * dy;
        Self::Linear {
            x0,
            y0,
            dx,
            dy,
            inv_len2: if l2 > 0.0 { 1.0 / l2 } else { 0.0 },
        }
    }

    pub fn radial(cx: f64, cy: f64, r: f64) -> Self {
        Self::Radial {
            cx,
            cy,
            inv_r: if r > 0.0 { 1.0 / r } else { 0.0 },
        }
    }

    #[inline]
    fn t(&self, px: f64, py: f64) -> f64 {
        match *self {
            Self::Linear {
                x0,
                y0,
                dx,
                dy,
                inv_len2,
            } => ((px - x0) * dx + (py - y0) * dy) * inv_len2,
            Self::Radial { cx, cy, inv_r } => {
                ((px - cx) * (px - cx) + (py - cy) * (py - cy)).sqrt() * inv_r
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum Paint {
    Solid(u32),
    Gradient(Gradient, GradientShape),
    /// Image anchored at `(ox, oy)`, scaled `scale` times; with `repeat` it tiles as a pattern.
    Image {
        img: Arc<Image>,
        ox: i32,
        oy: i32,
        scale: u32,
        repeat: bool,
    },
}

impl Paint {
    pub fn solid(c: Color) -> Self {
        Paint::Solid(c.to_px())
    }

    #[inline]
    pub fn at(&self, x: i32, y: i32) -> u32 {
        match self {
            Paint::Solid(c) => *c,
            Paint::Gradient(g, shape) => g.sample(shape.t(x as f64 + 0.5, y as f64 + 0.5), x, y),
            Paint::Image {
                img,
                ox,
                oy,
                scale,
                repeat,
            } => {
                let s = (*scale).max(1) as i32;
                let mut ix = (x - ox).div_euclid(s);
                let mut iy = (y - oy).div_euclid(s);
                if *repeat {
                    if img.width == 0 || img.height == 0 {
                        return 0;
                    }
                    ix = ix.rem_euclid(img.width as i32);
                    iy = iy.rem_euclid(img.height as i32);
                }
                img.get(ix, iy)
            }
        }
    }
}

/// Paints one contiguous run of a tile row. This is the hottest loop in the engine.
#[inline]
pub fn fill_segment(seg: &mut [u32], x0: i32, y: i32, paint: &Paint, mode: BlendMode) {
    match paint {
        Paint::Solid(c) => {
            let c = *c;
            let a = c >> 24;
            if mode == BlendMode::Replace || (mode == BlendMode::Over && a == 255) {
                seg.fill(c);
            } else if a == 0 {
            } else if mode == BlendMode::Over {
                for d in seg {
                    *d = over(*d, c);
                }
            } else {
                for d in seg {
                    *d = blend(*d, c, mode);
                }
            }
        }
        _ => {
            for (i, d) in seg.iter_mut().enumerate() {
                *d = blend(*d, paint.at(x0 + i as i32, y), mode);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_ends_and_steps() {
        let g = Gradient::even(&[Color::BLACK, Color::WHITE], 0, false);
        assert_eq!(Color::from_px(g.sample(0.0, 0, 0)), Color::BLACK);
        assert_eq!(Color::from_px(g.sample(1.0, 0, 0)), Color::WHITE);
        assert_eq!(Color::from_px(g.sample(0.5, 0, 0)).r, 128);
        let banded = Gradient::even(&[Color::BLACK, Color::WHITE], 3, false);
        assert_eq!(Color::from_px(banded.sample(0.4, 0, 0)).r, 128);
        assert_eq!(Color::from_px(banded.sample(0.2, 0, 0)).r, 0);
    }

    #[test]
    fn pattern_repeats() {
        let mut img = Image::new(2, 2);
        img.set(1, 1, 9);
        let p = Paint::Image {
            img: Arc::new(img),
            ox: 0,
            oy: 0,
            scale: 1,
            repeat: true,
        };
        assert_eq!(p.at(3, 3), 9);
        assert_eq!(p.at(-1, -1), 9);
        assert_eq!(p.at(2, 3), 0);
    }
}
