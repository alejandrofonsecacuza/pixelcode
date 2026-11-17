//! Flat RGBA image (one contiguous `Vec<u32>`): the output of compositing and the PNG boundary.

use crate::color::{Color, over};
use crate::span::IRect;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub px: Vec<u32>,
}

impl Image {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            px: vec![0; width as usize * height as usize],
        }
    }

    pub fn filled(width: u32, height: u32, c: u32) -> Self {
        Self {
            width,
            height,
            px: vec![c; width as usize * height as usize],
        }
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> u32 {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return 0;
        }
        self.px[y as usize * self.width as usize + x as usize]
    }

    #[inline]
    pub fn set(&mut self, x: i32, y: i32, c: u32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        self.px[y as usize * self.width as usize + x as usize] = c;
    }

    #[inline]
    pub fn blend_px(&mut self, x: i32, y: i32, c: u32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let i = y as usize * self.width as usize + x as usize;
        self.px[i] = over(self.px[i], c);
    }

    pub fn fill_rect(&mut self, r: IRect, c: u32) {
        let r = r.intersect(&IRect::new(0, 0, self.width as i32, self.height as i32));
        for y in r.y0..r.y1 {
            let row = y as usize * self.width as usize;
            for v in &mut self.px[row + r.x0 as usize..row + r.x1 as usize] {
                *v = over(*v, c);
            }
        }
    }

    pub fn crop(&self, r: IRect) -> Image {
        let r = r.intersect(&IRect::new(0, 0, self.width as i32, self.height as i32));
        let mut out = Image::new(r.w() as u32, r.h() as u32);
        for y in 0..r.h() {
            let src = (r.y0 + y) as usize * self.width as usize + r.x0 as usize;
            let dst = y as usize * out.width as usize;
            out.px[dst..dst + r.w() as usize].copy_from_slice(&self.px[src..src + r.w() as usize]);
        }
        out
    }

    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(self.px.len() * 4);
        for p in &self.px {
            v.extend_from_slice(&p.to_le_bytes());
        }
        v
    }

    pub fn from_rgba8(width: u32, height: u32, data: &[u8]) -> Self {
        let px = data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        Self { width, height, px }
    }

    pub fn encode_png(&self, fast: bool) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, self.width.max(1), self.height.max(1));
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_compression(if fast {
                png::Compression::Fast
            } else {
                png::Compression::Balanced
            });
            let mut w = enc.write_header().map_err(|e| e.to_string())?;
            if self.px.is_empty() {
                w.write_image_data(&[0, 0, 0, 0])
                    .map_err(|e| e.to_string())?;
            } else {
                w.write_image_data(&self.to_rgba8())
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(out)
    }

    pub fn decode_png(bytes: &[u8]) -> Result<Image, String> {
        let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
        dec.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = dec.read_info().map_err(|e| format!("png: {e}"))?;
        let size = reader.output_buffer_size().ok_or("png: image too large")?;
        let mut buf = vec![0u8; size];
        let info = reader
            .next_frame(&mut buf)
            .map_err(|e| format!("png: {e}"))?;
        let (w, h) = (info.width, info.height);
        let data = &buf[..info.buffer_size()];
        let px: Vec<u32> = match info.color_type {
            png::ColorType::Rgba => data
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
            png::ColorType::Rgb => data
                .as_chunks::<3>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], 255]))
                .collect(),
            png::ColorType::GrayscaleAlpha => data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes([c[0], c[0], c[0], c[1]]))
                .collect(),
            png::ColorType::Grayscale => data
                .iter()
                .map(|&g| u32::from_le_bytes([g, g, g, 255]))
                .collect(),
            other => return Err(format!("png: unsupported color type {other:?}")),
        };
        Ok(Image {
            width: w,
            height: h,
            px,
        })
    }

    /// Nearest-neighbour upscale: each pixel becomes a `k×k` block.
    pub fn upscale(&self, k: u32) -> Image {
        if k <= 1 {
            return self.clone();
        }
        let (w, h) = (self.width * k, self.height * k);
        let mut out = Image::new(w, h);
        for y in 0..self.height as usize {
            let src = &self.px[y * self.width as usize..(y + 1) * self.width as usize];
            let row0 = y * k as usize * w as usize;
            {
                let dst = &mut out.px[row0..row0 + w as usize];
                for (i, &p) in src.iter().enumerate() {
                    dst[i * k as usize..(i + 1) * k as usize].fill(p);
                }
            }
            for r in 1..k as usize {
                out.px
                    .copy_within(row0..row0 + w as usize, row0 + r * w as usize);
            }
        }
        out
    }

    /// Area-average downscale (alpha weighted) over `k×k` blocks.
    pub fn downscale(&self, k: u32) -> Image {
        if k <= 1 {
            return self.clone();
        }
        let (w, h) = (self.width.div_ceil(k), self.height.div_ceil(k));
        let mut out = Image::new(w, h);
        for oy in 0..h {
            for ox in 0..w {
                let (mut r, mut g, mut b, mut a, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
                for y in oy * k..((oy + 1) * k).min(self.height) {
                    for x in ox * k..((ox + 1) * k).min(self.width) {
                        let c = Color::from_px(self.px[(y * self.width + x) as usize]);
                        let ca = c.a as u64;
                        r += c.r as u64 * ca;
                        g += c.g as u64 * ca;
                        b += c.b as u64 * ca;
                        a += ca;
                        n += 1;
                    }
                }
                if let (Some(r), Some(g), Some(b)) =
                    (r.checked_div(a), g.checked_div(a), b.checked_div(a))
                {
                    out.px[(oy * w + ox) as usize] =
                        Color::rgba(r as u8, g as u8, b as u8, (a / n) as u8).to_px();
                }
            }
        }
        out
    }

    /// Composites over a checkerboard (`cell` px squares) to show transparency.
    pub fn over_checker(&self, cell: u32) -> Image {
        let light = Color::rgb(0xe6, 0xe6, 0xe6).to_px();
        let dark = Color::rgb(0xc4, 0xc4, 0xc4).to_px();
        let cell = cell.max(1);
        let mut out = self.clone();
        for y in 0..self.height {
            for x in 0..self.width {
                let i = (y * self.width + x) as usize;
                let bg = if ((x / cell) + (y / cell)).is_multiple_of(2) {
                    light
                } else {
                    dark
                };
                out.px[i] = over(bg, self.px[i]);
            }
        }
        out
    }

    pub fn flip_x(&self) -> Image {
        let mut out = self.clone();
        for row in out.px.chunks_mut(self.width.max(1) as usize) {
            row.reverse();
        }
        out
    }

    pub fn flip_y(&self) -> Image {
        let mut out = Image::new(self.width, self.height);
        let w = self.width as usize;
        for y in 0..self.height as usize {
            let src = (self.height as usize - 1 - y) * w;
            out.px[y * w..(y + 1) * w].copy_from_slice(&self.px[src..src + w]);
        }
        out
    }

    /// Rotates 90° clockwise.
    pub fn rotate_cw(&self) -> Image {
        let (w, h) = (self.width, self.height);
        let mut out = Image::new(h, w);
        for y in 0..h {
            for x in 0..w {
                out.px[(x * h + (h - 1 - y)) as usize] = self.px[(y * w + x) as usize];
            }
        }
        out
    }

    /// Fast, stable content hash (to check that two states are identical).
    pub fn hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ ((self.width as u64) << 32 | self.height as u64);
        for &p in &self.px {
            h = (h ^ p as u64).wrapping_mul(0x0100_0000_01b3).rotate_left(5);
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_roundtrip() {
        let mut img = Image::new(3, 2);
        img.set(0, 0, Color::rgba(255, 0, 0, 255).to_px());
        img.set(2, 1, Color::rgba(0, 0, 255, 100).to_px());
        let bytes = img.encode_png(true).unwrap();
        assert_eq!(Image::decode_png(&bytes).unwrap(), img);
    }

    #[test]
    fn scale_and_rotate() {
        let mut img = Image::new(2, 1);
        img.set(1, 0, 7);
        let up = img.upscale(3);
        assert_eq!((up.width, up.height), (6, 3));
        assert_eq!(up.get(5, 2), 7);
        assert_eq!(up.get(2, 2), 0);
        let r = img.rotate_cw();
        assert_eq!((r.width, r.height), (1, 2));
        assert_eq!(r.get(0, 1), 7);
        assert_eq!(img.flip_x().get(0, 0), 7);
    }
}
