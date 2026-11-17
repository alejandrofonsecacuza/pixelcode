//! Views designed for agents: a zoomed image with rulers and a pixel grid, an exact text dump
//! of pixels, and statistics. The goal is for the agent to verify what it drew while spending
//! as few tokens as possible.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::color::{Color, over};
use crate::image::Image;
use crate::span::IRect;
use crate::text;

#[derive(Clone, Copy, Debug)]
pub struct ViewOpts {
    /// Forced integer zoom; `None` = fit into `max_side`.
    pub zoom: Option<u32>,
    pub max_side: u32,
    /// `None` = automatic (on at zoom >= 8).
    pub grid: Option<bool>,
    pub rulers: bool,
    /// The input image was already downscaled by this factor (banded compositing).
    pub pre_shrink: u32,
}

impl Default for ViewOpts {
    fn default() -> Self {
        Self {
            zoom: None,
            max_side: 1024,
            grid: None,
            rulers: true,
            pre_shrink: 1,
        }
    }
}

/// Downscale factor needed for `region` to fit `max_side` (1 = no downscale).
pub fn shrink_for(region: IRect, opts: &ViewOpts) -> u32 {
    if opts.zoom.is_some() {
        return 1;
    }
    let side = region.w().max(region.h()).max(1) as u32;
    if side <= opts.max_side {
        1
    } else {
        side.div_ceil(opts.max_side.max(1))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ViewMeta {
    /// Each canvas pixel covers `zoom × zoom` image pixels…
    pub zoom: u32,
    /// …or each image pixel averages `shrink × shrink` canvas pixels.
    pub shrink: u32,
    pub margin_x: u32,
    pub margin_y: u32,
    pub width: u32,
    pub height: u32,
}

impl ViewMeta {
    pub fn describe(&self, region: IRect) -> String {
        let scale = if self.shrink > 1 {
            format!("1/{} (downscaled preview)", self.shrink)
        } else {
            format!("x{}", self.zoom)
        };
        let mut s = format!(
            "view of {region} at scale {scale}, image {}x{}",
            self.width, self.height
        );
        if self.margin_x > 0 {
            let _ = write!(
                s,
                "; rulers show canvas coordinates (image px = {} + (canvas - {}) * scale horizontally, {} + (canvas - {}) * scale vertically)",
                self.margin_x, region.x0, self.margin_y, region.y0
            );
        }
        s
    }
}

const RULER_BG: u32 = Color::rgb(0x1e, 0x1e, 0x24).to_px();
const RULER_FG: u32 = Color::rgb(0xd8, 0xd8, 0xe0).to_px();
const RULER_TICK: u32 = Color::rgb(0x80, 0x80, 0x90).to_px();

pub fn draw_text(img: &mut Image, x: i32, y: i32, s: &str, color: u32) {
    let mut sp = Vec::new();
    text::spans(x, y, s, 1, &mut sp);
    for span in sp {
        for xx in span.x0..span.x1 {
            img.blend_px(xx, span.y, color);
        }
    }
}

fn digits(v: i32) -> u32 {
    v.to_string().len() as u32
}

/// Renders an already composited region (`img`, whose canvas origin is `region.x0/y0`).
pub fn render(img: &Image, region: IRect, opts: &ViewOpts) -> (Image, ViewMeta) {
    let (w, h) = (img.width.max(1), img.height.max(1));
    let side = w.max(h);
    let (zoom, shrink) = match opts.zoom {
        _ if opts.pre_shrink > 1 => (1, opts.pre_shrink),
        Some(z) if z >= 1 => (z.min(64), 1),
        _ if side <= opts.max_side => ((opts.max_side / side).clamp(1, 32), 1),
        _ => (1, side.div_ceil(opts.max_side.max(1))),
    };
    let scaled = if opts.pre_shrink > 1 {
        img.clone()
    } else if shrink > 1 {
        img.downscale(shrink)
    } else {
        img.upscale(zoom)
    };
    let mut body = scaled.over_checker(8);
    let grid = opts.grid.unwrap_or(zoom >= 8) && zoom >= 4;
    if grid {
        let minor = Color::rgba(0, 0, 0, 40).to_px();
        let major = Color::rgba(0, 0, 0, 110).to_px();
        for i in 0..img.width as i32 {
            let c = if (region.x0 + i) % 8 == 0 {
                major
            } else {
                minor
            };
            let x = i * zoom as i32;
            for y in 0..body.height as i32 {
                body.blend_px(x, y, c);
            }
        }
        for j in 0..img.height as i32 {
            let c = if (region.y0 + j) % 8 == 0 {
                major
            } else {
                minor
            };
            let y = j * zoom as i32;
            for x in 0..body.width as i32 {
                body.blend_px(x, y, c);
            }
        }
    }
    if !opts.rulers {
        let meta = ViewMeta {
            zoom,
            shrink,
            margin_x: 0,
            margin_y: 0,
            width: body.width,
            height: body.height,
        };
        return (body, meta);
    }
    let max_coord = region.x1.max(region.y1);
    let mx = digits(max_coord) * 8 + 6;
    let my = 12u32;
    let mut out = Image::filled(body.width + mx, body.height + my, RULER_BG);
    for y in 0..body.height {
        let dst = ((y + my) * out.width + mx) as usize;
        let src = (y * body.width) as usize;
        out.px[dst..dst + body.width as usize]
            .copy_from_slice(&body.px[src..src + body.width as usize]);
    }
    let scale = zoom as f64 / shrink as f64;
    let pick = |min_px: f64| -> i32 {
        const STEPS: [i32; 20] = [
            1, 2, 4, 5, 8, 10, 16, 20, 25, 32, 50, 64, 100, 128, 200, 250, 256, 500, 512, 1000,
        ];
        STEPS
            .iter()
            .copied()
            .find(|&s| s as f64 * scale >= min_px)
            .unwrap_or(((min_px / scale).ceil() as i32 / 1000 + 1) * 1000)
    };
    let step_x = pick((digits(max_coord) * 8 + 6) as f64);
    let step_y = pick(12.0);
    let first = |o: i32, s: i32| o.div_euclid(s) * s + if o.rem_euclid(s) == 0 { 0 } else { s };
    let mut c = first(region.x0, step_x);
    while c < region.x1 {
        let px = mx as i32 + ((c - region.x0) as f64 * scale) as i32;
        for y in (my as i32 - 3)..my as i32 {
            out.set(px, y, RULER_TICK);
        }
        let label = c.to_string();
        if px + 1 + label.len() as i32 * 8 <= out.width as i32 {
            draw_text(&mut out, px + 1, 1, &label, RULER_FG);
        }
        c += step_x;
    }
    let mut c = first(region.y0, step_y);
    while c < region.y1 {
        let py = my as i32 + ((c - region.y0) as f64 * scale) as i32;
        for x in (mx as i32 - 3)..mx as i32 {
            out.set(x, py, RULER_TICK);
        }
        if py + 8 <= out.height as i32 {
            draw_text(&mut out, 1, py + 1, &c.to_string(), RULER_FG);
        }
        c += step_y;
    }
    let meta = ViewMeta {
        zoom,
        shrink,
        margin_x: mx,
        margin_y: my,
        width: out.width,
        height: out.height,
    };
    (out, meta)
}

const SYMBOLS: &str =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789#$%&*+=?@<>/~^";

/// Exact text dump: one symbol per pixel plus a color legend.
pub fn pixels_text(img: &Image, region: IRect) -> Result<String, String> {
    let syms: Vec<char> = SYMBOLS.chars().collect();
    let mut legend: Vec<u32> = Vec::new();
    let mut map: HashMap<u32, char> = HashMap::new();
    for &p in &img.px {
        if p >> 24 == 0 || map.contains_key(&p) {
            continue;
        }
        if legend.len() == syms.len() {
            return Err(format!(
                "region has more than {} distinct colors; use a smaller region or mode \"stats\"",
                syms.len()
            ));
        }
        map.insert(p, syms[legend.len()]);
        legend.push(p);
    }
    let mut s = format!(
        "pixels of {region}; '.' = transparent; row label = y, column ruler = x mod 10 (starting at x={})\n",
        region.x0
    );
    for (i, p) in legend.iter().enumerate() {
        let _ = write!(s, "{}={} ", syms[i], Color::from_px(*p).to_hex());
    }
    s.push('\n');
    let lw = digits(region.y1.max(region.y0.abs()) + 1) as usize;
    let _ = write!(s, "{:>lw$} ", "");
    for x in region.x0..region.x0 + img.width as i32 {
        s.push(char::from(b'0' + x.rem_euclid(10) as u8));
    }
    s.push('\n');
    for y in 0..img.height {
        let _ = write!(s, "{:>lw$} ", region.y0 + y as i32);
        for x in 0..img.width {
            let p = img.px[(y * img.width + x) as usize];
            s.push(if p >> 24 == 0 { '.' } else { map[&p] });
        }
        s.push('\n');
    }
    Ok(s)
}

/// Statistics accumulated band by band (the whole region never has to be in memory).
pub struct Stats {
    region: IRect,
    counts: HashMap<u32, u64>,
    bbox: Option<IRect>,
    opaque: u64,
    total: u64,
    hash: u64,
}

impl Stats {
    pub fn new(region: IRect) -> Self {
        let hash = 0xcbf2_9ce4_8422_2325 ^ ((region.w() as u64) << 32 | region.h() as u64);
        Self {
            region,
            counts: HashMap::new(),
            bbox: None,
            opaque: 0,
            total: 0,
            hash,
        }
    }

    /// Adds a composited band whose canvas origin is `(x0, y0)`. Bands must arrive in row
    /// order so the hash matches `Image::hash`.
    pub fn add(&mut self, img: &Image, x0: i32, y0: i32) {
        for y in 0..img.height {
            let mut row_min = None;
            let mut row_max = 0;
            for x in 0..img.width {
                let p = img.px[(y * img.width + x) as usize];
                self.hash = (self.hash ^ p as u64)
                    .wrapping_mul(0x0100_0000_01b3)
                    .rotate_left(5);
                *self.counts.entry(p).or_default() += 1;
                if p >> 24 != 0 {
                    self.opaque += 1;
                    row_min.get_or_insert(x);
                    row_max = x;
                }
            }
            if let Some(m) = row_min {
                let r = IRect::new(x0 + m as i32, y0 + y as i32, (row_max - m + 1) as i32, 1);
                self.bbox = Some(self.bbox.map_or(r, |b| b.union(&r)));
            }
        }
        self.total += img.width as u64 * img.height as u64;
    }

    pub fn text(&self) -> String {
        let total = self.total.max(1);
        let mut top: Vec<(u32, u64)> = self.counts.iter().map(|(k, v)| (*k, *v)).collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut s = format!(
            "stats of {}: {} distinct colors, {:.1}% non-transparent, hash {:016x}\n",
            self.region,
            top.len(),
            self.opaque as f64 * 100.0 / total as f64,
            self.hash
        );
        match self.bbox {
            Some(b) => {
                let _ = writeln!(s, "content bbox: {b}");
            }
            None => s.push_str("content bbox: empty\n"),
        }
        s.push_str("top colors: ");
        for (p, n) in top.iter().take(16) {
            let name = if p >> 24 == 0 {
                "transparent".to_string()
            } else {
                Color::from_px(*p).to_hex()
            };
            let _ = write!(s, "{name} {:.1}%, ", *n as f64 * 100.0 / total as f64);
        }
        s.truncate(s.trim_end_matches([',', ' ']).len());
        s
    }
}

pub fn stats_text(img: &Image, region: IRect) -> String {
    let mut st = Stats::new(region);
    st.add(img, region.x0, region.y0);
    st.text()
}

/// Composites over an opaque background (for terminals without transparency).
pub fn flatten(img: &Image, bg: u32) -> Image {
    let mut out = img.clone();
    for p in &mut out.px {
        *p = over(bg, *p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_zoom_and_rulers() {
        let img = Image::new(16, 8);
        let (out, meta) = render(&img, IRect::new(0, 0, 16, 8), &ViewOpts::default());
        assert_eq!(meta.zoom, 32);
        assert_eq!(out.width, 16 * 32 + meta.margin_x);
        let big = Image::new(4000, 100);
        let (_, meta) = render(&big, IRect::new(0, 0, 4000, 100), &ViewOpts::default());
        assert_eq!(meta.shrink, 4);
    }

    #[test]
    fn pixel_dump_roundtrip() {
        let mut img = Image::new(3, 2);
        img.set(1, 0, Color::rgb(255, 0, 0).to_px());
        let t = pixels_text(&img, IRect::new(10, 20, 3, 2)).unwrap();
        assert!(t.contains("A=#ff0000"), "{t}");
        assert!(t.contains("20 .A."), "{t}");
        assert!(t.contains("21 ..."), "{t}");
    }
}
