//! Raster made of 64×64 copy-on-write tiles.
//!
//! - Empty tiles take no memory (`None`): huge, sparse canvases are cheap.
//! - Each tile lives in an `Arc`: cloning a raster (checkpoint, undo) costs a handful of
//!   pointers, and only tiles modified afterwards get copied.
//! - Tile bands are independent, so painting is spread across cores.

use std::sync::Arc;

use rayon::prelude::*;

use crate::color::BlendMode;
use crate::paint::{Paint, fill_segment};
use crate::span::{IRect, Span, area};

pub const TILE_SHIFT: i32 = 6;
pub const TILE: i32 = 1 << TILE_SHIFT;
const MASK: i32 = TILE - 1;
const TILE_PX: usize = (TILE * TILE) as usize;
/// Below this many pixels, splitting work across threads is not worth it.
const PAR_THRESHOLD: u64 = 1 << 15;

#[derive(Clone)]
pub struct Tile(pub [u32; TILE_PX]);

type Slot = Option<Arc<Tile>>;

#[inline]
fn tile_mut(slot: &mut Slot) -> &mut Tile {
    Arc::make_mut(slot.get_or_insert_with(|| Arc::new(Tile([0; TILE_PX]))))
}

#[derive(Clone)]
pub struct Raster {
    width: u32,
    height: u32,
    tiles_x: usize,
    tiles_y: usize,
    tiles: Vec<Slot>,
}

impl Raster {
    pub fn new(width: u32, height: u32) -> Self {
        let tiles_x = (width as usize).div_ceil(TILE as usize);
        let tiles_y = (height as usize).div_ceil(TILE as usize);
        Self {
            width,
            height,
            tiles_x,
            tiles_y,
            tiles: vec![None; tiles_x * tiles_y],
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn bounds(&self) -> IRect {
        IRect::new(0, 0, self.width as i32, self.height as i32)
    }

    /// Tiles with allocated memory (the rest are transparent and free).
    pub fn allocated_tiles(&self) -> usize {
        self.tiles.iter().filter(|t| t.is_some()).count()
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> u32 {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return 0;
        }
        match &self.tiles[(y >> TILE_SHIFT) as usize * self.tiles_x + (x >> TILE_SHIFT) as usize] {
            Some(t) => t.0[((y & MASK) << TILE_SHIFT | (x & MASK)) as usize],
            None => 0,
        }
    }

    #[inline]
    pub fn set(&mut self, x: i32, y: i32, px: u32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let i = (y >> TILE_SHIFT) as usize * self.tiles_x + (x >> TILE_SHIFT) as usize;
        if px == 0 && self.tiles[i].is_none() {
            return;
        }
        tile_mut(&mut self.tiles[i]).0[((y & MASK) << TILE_SHIFT | (x & MASK)) as usize] = px;
    }

    pub fn clear(&mut self) {
        self.tiles.fill(None);
    }

    /// Fills everything with one color, sharing a single tile across all positions.
    pub fn fill_all(&mut self, px: u32) {
        if px == 0 {
            self.clear();
            return;
        }
        let shared = Some(Arc::new(Tile([px; TILE_PX])));
        self.tiles.fill(shared);
    }

    /// Paints spans that are already normalized (sorted, disjoint and in bounds).
    pub fn paint_spans(&mut self, spans: &[Span], paint: &Paint, mode: BlendMode) {
        if spans.is_empty() {
            return;
        }
        let tx = self.tiles_x;
        let first_band = spans[0].y >> TILE_SHIFT;
        let last_band = spans[spans.len() - 1].y >> TILE_SHIFT;
        if first_band == last_band || area(spans) < PAR_THRESHOLD {
            for s in spans {
                let b = (s.y >> TILE_SHIFT) as usize;
                paint_span(&mut self.tiles[b * tx..(b + 1) * tx], *s, paint, mode);
            }
            return;
        }
        let mut rows: Vec<&mut [Slot]> = self.tiles.chunks_mut(tx).collect();
        let mut jobs: Vec<(&mut [Slot], &[Span])> =
            Vec::with_capacity((last_band - first_band + 1) as usize);
        let mut i = 0;
        while i < spans.len() {
            let band = spans[i].y >> TILE_SHIFT;
            let mut j = i + 1;
            while j < spans.len() && spans[j].y >> TILE_SHIFT == band {
                j += 1;
            }
            jobs.push((std::mem::take(&mut rows[band as usize]), &spans[i..j]));
            i = j;
        }
        jobs.into_par_iter().for_each(|(row, ss)| {
            for s in ss {
                paint_span(row, *s, paint, mode);
            }
        });
    }

    /// Walks the segments of a row that have allocated tiles (empty ones are skipped).
    #[inline]
    pub fn for_row_segments(&self, y: i32, x0: i32, x1: i32, mut f: impl FnMut(i32, &[u32])) {
        if y < 0 || y >= self.height as i32 {
            return;
        }
        let x0 = x0.max(0);
        let x1 = x1.min(self.width as i32);
        let band = (y >> TILE_SHIFT) as usize * self.tiles_x;
        let ly = ((y & MASK) << TILE_SHIFT) as usize;
        let mut x = x0;
        while x < x1 {
            let ti = (x >> TILE_SHIFT) as usize;
            let xe = x1.min(((ti as i32) + 1) << TILE_SHIFT);
            if let Some(t) = &self.tiles[band + ti] {
                let a = ly + (x & MASK) as usize;
                f(x, &t.0[a..a + (xe - x) as usize]);
            }
            x = xe;
        }
    }

    /// Writes a row of pixels as-is (replace), without allocating tiles for empty runs.
    pub fn write_row(&mut self, y: i32, x0: i32, data: &[u32]) {
        if y < 0 || y >= self.height as i32 {
            return;
        }
        let band = (y >> TILE_SHIFT) as usize * self.tiles_x;
        let ly = ((y & MASK) << TILE_SHIFT) as usize;
        let start = x0.max(0);
        let end = (x0 + data.len() as i32).min(self.width as i32);
        let mut x = start;
        while x < end {
            let ti = (x >> TILE_SHIFT) as usize;
            let xe = end.min(((ti as i32) + 1) << TILE_SHIFT);
            let src = &data[(x - x0) as usize..(xe - x0) as usize];
            let slot = &mut self.tiles[band + ti];
            if !(slot.is_none() && src.iter().all(|&p| p == 0)) {
                let a = ly + (x & MASK) as usize;
                tile_mut(slot).0[a..a + src.len()].copy_from_slice(src);
            }
            x = xe;
        }
    }

    pub fn read_region(&self, r: IRect) -> crate::image::Image {
        let r = r.intersect(&self.bounds());
        let mut img = crate::image::Image::new(r.w() as u32, r.h() as u32);
        let w = r.w() as usize;
        if w == 0 {
            return img;
        }
        img.px.par_chunks_mut(w).enumerate().for_each(|(row, out)| {
            self.for_row_segments(r.y0 + row as i32, r.x0, r.x1, |x, seg| {
                let o = (x - r.x0) as usize;
                out[o..o + seg.len()].copy_from_slice(seg);
            });
        });
        img
    }

    /// Smallest box containing non-transparent pixels.
    pub fn bbox(&self) -> Option<IRect> {
        (0..self.tiles.len())
            .into_par_iter()
            .filter_map(|i| {
                let t = self.tiles[i].as_ref()?;
                let (tx, ty) = (
                    (i % self.tiles_x) as i32 * TILE,
                    (i / self.tiles_x) as i32 * TILE,
                );
                let mut r: Option<IRect> = None;
                for ly in 0..TILE.min(self.height as i32 - ty) {
                    for lx in 0..TILE.min(self.width as i32 - tx) {
                        if t.0[(ly << TILE_SHIFT | lx) as usize] >> 24 != 0 {
                            let p = IRect::new(tx + lx, ty + ly, 1, 1);
                            r = Some(r.map_or(p, |q| q.union(&p)));
                        }
                    }
                }
                r
            })
            .reduce_with(|a, b| a.union(&b))
    }

    /// Differing pixels and their bounding box. Shared tiles are skipped without looking at them.
    pub fn diff(&self, other: &Raster) -> (u64, Option<IRect>) {
        if self.width != other.width || self.height != other.height {
            let b = self.bounds().union(&other.bounds());
            return (b.area(), Some(b));
        }
        (0..self.tiles.len())
            .into_par_iter()
            .filter_map(|i| {
                let (a, b) = (&self.tiles[i], &other.tiles[i]);
                if let (Some(a), Some(b)) = (a, b)
                    && Arc::ptr_eq(a, b)
                {
                    return None;
                }
                if a.is_none() && b.is_none() {
                    return None;
                }
                let (tx, ty) = (
                    (i % self.tiles_x) as i32 * TILE,
                    (i / self.tiles_x) as i32 * TILE,
                );
                let px = |s: &Slot, k: usize| s.as_ref().map_or(0, |t| t.0[k]);
                let mut n = 0u64;
                let mut r: Option<IRect> = None;
                for ly in 0..TILE.min(self.height as i32 - ty) {
                    for lx in 0..TILE.min(self.width as i32 - tx) {
                        let k = (ly << TILE_SHIFT | lx) as usize;
                        if px(a, k) != px(b, k) {
                            n += 1;
                            let p = IRect::new(tx + lx, ty + ly, 1, 1);
                            r = Some(r.map_or(p, |q| q.union(&p)));
                        }
                    }
                }
                r.map(|r| (n, r))
            })
            .reduce_with(|(n1, r1), (n2, r2)| (n1 + n2, r1.union(&r2)))
            .map_or((0, None), |(n, r)| (n, Some(r)))
    }

    /// New size; the content is shifted by `(dx, dy)`.
    pub fn resized(&self, width: u32, height: u32, dx: i32, dy: i32) -> Raster {
        let mut out = Raster::new(width, height);
        let mut buf = vec![0u32; self.width as usize];
        for y in 0..self.height as i32 {
            let ny = y + dy;
            if ny < 0 || ny >= height as i32 {
                continue;
            }
            buf.fill(0);
            self.for_row_segments(y, 0, self.width as i32, |x, seg| {
                buf[x as usize..x as usize + seg.len()].copy_from_slice(seg);
            });
            out.write_row(ny, dx, &buf);
        }
        out
    }

    /// Frees tiles that became fully transparent.
    pub fn compact(&mut self) {
        for slot in &mut self.tiles {
            if let Some(t) = slot
                && t.0.iter().all(|&p| p == 0)
            {
                *slot = None;
            }
        }
    }

    /// Like `compact`, but only checks tiles that changed since `before`.
    pub fn compact_changed(&mut self, before: &Raster) {
        if self.tiles.len() != before.tiles.len() {
            self.compact();
            return;
        }
        for (slot, b) in self.tiles.iter_mut().zip(&before.tiles) {
            if let Some(t) = slot {
                if let Some(b) = b
                    && Arc::ptr_eq(t, b)
                {
                    continue;
                }
                if t.0.iter().all(|&p| p == 0) {
                    *slot = None;
                }
            }
        }
    }

    pub fn tiles_y(&self) -> usize {
        self.tiles_y
    }
}

#[inline]
fn paint_span(row: &mut [Slot], s: Span, paint: &Paint, mode: BlendMode) {
    let ly = ((s.y & MASK) << TILE_SHIFT) as usize;
    let mut x = s.x0;
    while x < s.x1 {
        let ti = (x >> TILE_SHIFT) as usize;
        let xe = s.x1.min(((ti as i32) + 1) << TILE_SHIFT);
        let slot = &mut row[ti];
        if slot.is_none() && mode == BlendMode::Erase {
            x = xe;
            continue;
        }
        let a = ly + (x & MASK) as usize;
        fill_segment(
            &mut tile_mut(slot).0[a..a + (xe - x) as usize],
            x,
            s.y,
            paint,
            mode,
        );
        x = xe;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;

    #[test]
    fn sparse_and_cow() {
        let mut r = Raster::new(300, 200);
        assert_eq!(r.allocated_tiles(), 0);
        r.set(130, 70, 5);
        assert_eq!(r.get(130, 70), 5);
        assert_eq!(r.allocated_tiles(), 1);
        let snap = r.clone();
        r.set(131, 70, 6);
        assert_eq!(snap.get(131, 70), 0);
        let (n, b) = r.diff(&snap);
        assert_eq!(n, 1);
        assert_eq!(b, Some(IRect::new(131, 70, 1, 1)));
    }

    #[test]
    fn paint_parallel_matches_sequential() {
        let red = Paint::solid(Color::rgba(255, 0, 0, 200));
        let spans: Vec<Span> = (0..500).map(|y| Span::new(y, 3, 497)).collect();
        let mut a = Raster::new(500, 500);
        a.paint_spans(&spans, &red, BlendMode::Over);
        let mut b = Raster::new(500, 500);
        for s in &spans {
            b.paint_spans(std::slice::from_ref(s), &red, BlendMode::Over);
        }
        assert_eq!(a.diff(&b).0, 0);
        assert_eq!(a.bbox(), Some(IRect::new(3, 0, 494, 500)));
    }

    #[test]
    fn resize_moves_content() {
        let mut r = Raster::new(10, 10);
        r.set(2, 3, 9);
        let r2 = r.resized(20, 20, 5, 1);
        assert_eq!(r2.get(7, 4), 9);
    }
}
