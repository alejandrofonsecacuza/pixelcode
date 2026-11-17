//! Document: a stack of same-sized layers, composited in parallel.

use rayon::prelude::*;

use crate::canvas::Raster;
use crate::color::{BlendMode, blend, over, scale_alpha};
use crate::image::Image;
use crate::span::IRect;

#[derive(Clone)]
pub struct Layer {
    pub name: String,
    pub visible: bool,
    pub opacity: u8,
    pub blend: BlendMode,
    pub raster: Raster,
}

impl Layer {
    pub fn new(name: &str, w: u32, h: u32) -> Self {
        Self {
            name: name.to_string(),
            visible: true,
            opacity: 255,
            blend: BlendMode::Over,
            raster: Raster::new(w, h),
        }
    }
}

/// Cloning a document copies O(layers × tiles) pointers: the basis of the history.
#[derive(Clone)]
pub struct Document {
    pub width: u32,
    pub height: u32,
    /// Bottom to top.
    pub layers: Vec<Layer>,
    /// The layer scripts draw on.
    pub active: usize,
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.layers.iter().map(|l| l.name.as_str()).collect();
        write!(
            f,
            "Document({}x{}, layers {:?}, active {})",
            self.width, self.height, names, self.active
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Diff {
    pub pixels: u64,
    pub bbox: Option<IRect>,
}

impl Document {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            layers: vec![Layer::new("base", width, height)],
            active: 0,
        }
    }

    pub fn bounds(&self) -> IRect {
        IRect::new(0, 0, self.width as i32, self.height as i32)
    }

    pub fn layer_index(&self, name: &str) -> Option<usize> {
        self.layers.iter().position(|l| l.name == name)
    }

    /// Returns the layer's index, creating it on top if it does not exist.
    pub fn ensure_layer(&mut self, name: &str) -> usize {
        match self.layer_index(name) {
            Some(i) => i,
            None => {
                self.layers.push(Layer::new(name, self.width, self.height));
                self.layers.len() - 1
            }
        }
    }

    pub fn remove_layer(&mut self, name: &str) -> Result<(), String> {
        let i = self
            .layer_index(name)
            .ok_or_else(|| format!("no layer named {name:?}"))?;
        if self.layers.len() == 1 {
            return Err("cannot remove the only layer".into());
        }
        self.layers.remove(i);
        if self.active >= self.layers.len() || self.active > i {
            self.active = self.active.saturating_sub(1).min(self.layers.len() - 1);
        }
        Ok(())
    }

    pub fn move_layer(&mut self, name: &str, to: usize) -> Result<(), String> {
        let i = self
            .layer_index(name)
            .ok_or_else(|| format!("no layer named {name:?}"))?;
        let active_name = self.layers[self.active].name.clone();
        let l = self.layers.remove(i);
        let to = to.min(self.layers.len());
        self.layers.insert(to, l);
        self.active = self.layer_index(&active_name).unwrap_or(0);
        Ok(())
    }

    pub fn resize(&mut self, width: u32, height: u32, dx: i32, dy: i32) {
        for l in &mut self.layers {
            l.raster = l.raster.resized(width, height, dx, dy);
        }
        self.width = width;
        self.height = height;
    }

    /// Composites the visible layers of a region. Rows run in parallel; empty tiles cost nothing.
    pub fn composite(&self, r: IRect) -> Image {
        let r = r.intersect(&self.bounds());
        let mut img = Image::new(r.w() as u32, r.h() as u32);
        let w = r.w() as usize;
        if w == 0 || r.h() == 0 {
            return img;
        }
        let layers: Vec<&Layer> = self
            .layers
            .iter()
            .filter(|l| l.visible && l.opacity > 0)
            .collect();
        img.px.par_chunks_mut(w).enumerate().for_each(|(row, out)| {
            let y = r.y0 + row as i32;
            for l in &layers {
                let op = l.opacity as u32;
                let mode = l.blend;
                l.raster.for_row_segments(y, r.x0, r.x1, |x, seg| {
                    let o = &mut out[(x - r.x0) as usize..][..seg.len()];
                    if mode == BlendMode::Over && op == 255 {
                        for (d, &s) in o.iter_mut().zip(seg) {
                            *d = over(*d, s);
                        }
                    } else {
                        for (d, &s) in o.iter_mut().zip(seg) {
                            *d = blend(*d, scale_alpha(s, op), mode);
                        }
                    }
                });
            }
        });
        img
    }

    /// Area-averaged `k×k` downscaled composite, band by band: a 16384² canvas can be
    /// previewed without materializing 1 GB of pixels.
    pub fn composite_downscaled(&self, r: IRect, k: u32, layer: Option<usize>) -> Image {
        let r = r.intersect(&self.bounds());
        let k = k.max(1);
        let fetch = |band: IRect| match layer {
            Some(i) => self.layers[i].raster.read_region(band),
            None => self.composite(band),
        };
        if k == 1 {
            return fetch(r);
        }
        let (ow, oh) = ((r.w() as u32).div_ceil(k), (r.h() as u32).div_ceil(k));
        let mut out = Image::new(ow, oh);
        if ow == 0 {
            return out;
        }
        out.px
            .par_chunks_mut(ow as usize)
            .enumerate()
            .for_each(|(oy, row)| {
                let y0 = r.y0 + (oy as u32 * k) as i32;
                let band = IRect {
                    x0: r.x0,
                    y0,
                    x1: r.x1,
                    y1: (y0 + k as i32).min(r.y1),
                };
                let small = fetch(band).downscale(k);
                row.copy_from_slice(&small.px[..ow as usize]);
            });
        out
    }

    /// Streams the composited PNG in row bands (bounded memory even for huge canvases).
    pub fn write_png<W: std::io::Write>(&self, w: W, scale: u32, fast: bool) -> Result<(), String> {
        let scale = scale.max(1);
        let (ow, oh) = (self.width * scale, self.height * scale);
        let mut enc = png::Encoder::new(w, ow, oh);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(if fast {
            png::Compression::Fast
        } else {
            png::Compression::Balanced
        });
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        let mut stream = writer.stream_writer().map_err(|e| e.to_string())?;
        let band = (256 / scale as i32).max(1);
        let mut y = 0;
        while y < self.height as i32 {
            let r = IRect::new(0, y, self.width as i32, band.min(self.height as i32 - y));
            let img = self.composite(r).upscale(scale);
            std::io::Write::write_all(&mut stream, &img.to_rgba8()).map_err(|e| e.to_string())?;
            y += band;
        }
        stream.finish().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn composite_all(&self) -> Image {
        self.composite(self.bounds())
    }

    /// Layer-by-layer changes against another state (shared tiles are skipped).
    pub fn diff(&self, other: &Document) -> Diff {
        if self.width != other.width || self.height != other.height {
            let b = self.bounds().union(&other.bounds());
            return Diff {
                pixels: b.area(),
                bbox: Some(b),
            };
        }
        let mut pixels = 0;
        let mut bbox: Option<IRect> = None;
        let mut account = |n: u64, b: Option<IRect>| {
            pixels += n;
            if let Some(b) = b {
                bbox = Some(bbox.map_or(b, |q| q.union(&b)));
            }
        };
        for l in &self.layers {
            match other.layer_index(&l.name) {
                Some(j) => {
                    let (n, b) = l.raster.diff(&other.layers[j].raster);
                    let props_changed = {
                        let o = &other.layers[j];
                        o.visible != l.visible || o.opacity != l.opacity || o.blend != l.blend
                    };
                    if props_changed {
                        let bb = l
                            .raster
                            .bbox()
                            .into_iter()
                            .chain(other.layers[j].raster.bbox())
                            .reduce(|a, b| a.union(&b));
                        account(bb.map_or(0, |b| b.area()), bb);
                    } else {
                        account(n, b);
                    }
                }
                None => {
                    let bb = l.raster.bbox();
                    account(bb.map_or(0, |b| b.area()), bb);
                }
            }
        }
        for l in &other.layers {
            if self.layer_index(&l.name).is_none() {
                let bb = l.raster.bbox();
                account(bb.map_or(0, |b| b.area()), bb);
            }
        }
        Diff { pixels, bbox }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;

    #[test]
    fn composite_respects_order_and_opacity() {
        let mut d = Document::new(4, 4);
        d.layers[0].raster.set(1, 1, Color::rgb(255, 0, 0).to_px());
        let top = d.ensure_layer("top");
        d.layers[top]
            .raster
            .set(1, 1, Color::rgb(0, 0, 255).to_px());
        assert_eq!(d.composite_all().get(1, 1), Color::rgb(0, 0, 255).to_px());
        d.layers[top].opacity = 0;
        assert_eq!(d.composite_all().get(1, 1), Color::rgb(255, 0, 0).to_px());
        d.layers[top].visible = false;
        d.layers[top].opacity = 255;
        assert_eq!(d.composite_all().get(1, 1), Color::rgb(255, 0, 0).to_px());
    }

    #[test]
    fn diff_after_clone_is_empty() {
        let mut d = Document::new(200, 200);
        d.layers[0].raster.fill_all(Color::WHITE.to_px());
        let snap = d.clone();
        assert_eq!(
            d.diff(&snap),
            Diff {
                pixels: 0,
                bbox: None
            }
        );
        d.layers[0].raster.set(150, 20, 0);
        assert_eq!(d.diff(&snap).bbox, Some(IRect::new(150, 20, 1, 1)));
    }
}
