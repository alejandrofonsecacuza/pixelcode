//! Horizontal spans: the intermediate representation of every shape.
//!
//! Each primitive is rasterized into a list of `[x0, x1)` spans per row. The spans are then
//! normalized (clipped, sorted, merged) and painted in one pass, so no pixel is ever blended
//! twice and painting can run in parallel across tile bands.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    pub y: i32,
    pub x0: i32,
    pub x1: i32,
}

impl Span {
    #[inline(always)]
    pub fn new(y: i32, x0: i32, x1: i32) -> Self {
        Self { y, x0, x1 }
    }
}

/// Half-open integer rectangle `[x0, x1) × [y0, y1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl IRect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self {
            x0: x,
            y0: y,
            x1: x.saturating_add(w),
            y1: y.saturating_add(h),
        }
    }

    pub fn w(&self) -> i32 {
        (self.x1 - self.x0).max(0)
    }

    pub fn h(&self) -> i32 {
        (self.y1 - self.y0).max(0)
    }

    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    pub fn area(&self) -> u64 {
        self.w() as u64 * self.h() as u64
    }

    pub fn intersect(&self, o: &IRect) -> IRect {
        IRect {
            x0: self.x0.max(o.x0),
            y0: self.y0.max(o.y0),
            x1: self.x1.min(o.x1),
            y1: self.y1.min(o.y1),
        }
    }

    pub fn union(&self, o: &IRect) -> IRect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        IRect {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }

    pub fn expand(&self, d: i32) -> IRect {
        IRect {
            x0: self.x0 - d,
            y0: self.y0 - d,
            x1: self.x1 + d,
            y1: self.y1 + d,
        }
    }
}

impl std::fmt::Display for IRect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "x={} y={} w={} h={}",
            self.x0,
            self.y0,
            self.w(),
            self.h()
        )
    }
}

/// Clips to `clip`, sorts by (y, x0) and merges overlapping or touching spans.
pub fn normalize(spans: &mut Vec<Span>, clip: IRect) {
    spans.retain_mut(|s| {
        if s.y < clip.y0 || s.y >= clip.y1 {
            return false;
        }
        s.x0 = s.x0.max(clip.x0);
        s.x1 = s.x1.min(clip.x1);
        s.x0 < s.x1
    });
    if spans.windows(2).any(|w| w[0] > w[1]) {
        spans.sort_unstable();
    }
    let mut w = 0usize;
    for i in 0..spans.len() {
        let s = spans[i];
        if w > 0 {
            let last = &mut spans[w - 1];
            if last.y == s.y && s.x0 <= last.x1 {
                last.x1 = last.x1.max(s.x1);
                continue;
            }
        }
        spans[w] = s;
        w += 1;
    }
    spans.truncate(w);
}

pub fn area(spans: &[Span]) -> u64 {
    spans.iter().map(|s| (s.x1 - s.x0).max(0) as u64).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_and_clips() {
        let mut v = vec![
            Span::new(1, 5, 9),
            Span::new(0, 0, 3),
            Span::new(1, 0, 5),
            Span::new(1, 8, 20),
            Span::new(9, 0, 4),
        ];
        normalize(&mut v, IRect::new(0, 0, 10, 5));
        assert_eq!(v, vec![Span::new(0, 0, 3), Span::new(1, 0, 10)]);
    }
}
