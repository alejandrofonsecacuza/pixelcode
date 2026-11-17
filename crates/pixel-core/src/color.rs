//! Straight-alpha RGBA colors and blend modes in integer arithmetic.
//!
//! A pixel is stored as a little-endian `u32`: bytes `[r, g, b, a]`.

use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::rgba(r, g, b, 255)
    }

    #[inline(always)]
    pub const fn to_px(self) -> u32 {
        (self.r as u32) | (self.g as u32) << 8 | (self.b as u32) << 16 | (self.a as u32) << 24
    }

    #[inline(always)]
    pub const fn from_px(p: u32) -> Self {
        Self {
            r: p as u8,
            g: (p >> 8) as u8,
            b: (p >> 16) as u8,
            a: (p >> 24) as u8,
        }
    }

    /// Accepts `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` (with or without `#`) and basic CSS names.
    pub fn parse(s: &str) -> Result<Self, String> {
        let t = s.trim();
        let hex = t.strip_prefix('#').unwrap_or(t);
        if let Some(c) = Self::parse_hex(hex) {
            return Ok(c);
        }
        named(&t.to_ascii_lowercase()).ok_or_else(|| {
            format!("invalid color {s:?}: use #rgb, #rrggbb, #rrggbbaa, rgb(), hsl() or a basic CSS name")
        })
    }

    fn parse_hex(h: &str) -> Option<Self> {
        if h.is_empty() || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let v = |i: usize, n: usize| u8::from_str_radix(&h[i..i + n], 16).ok();
        match h.len() {
            3 => Some(Self::rgb(v(0, 1)? * 17, v(1, 1)? * 17, v(2, 1)? * 17)),
            4 => Some(Self::rgba(
                v(0, 1)? * 17,
                v(1, 1)? * 17,
                v(2, 1)? * 17,
                v(3, 1)? * 17,
            )),
            6 => Some(Self::rgb(v(0, 2)?, v(2, 2)?, v(4, 2)?)),
            8 => Some(Self::rgba(v(0, 2)?, v(2, 2)?, v(4, 2)?, v(6, 2)?)),
            _ => None,
        }
    }

    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    /// `h` in degrees; `s`, `l` and `a` in 0..1.
    pub fn from_hsl(h: f64, s: f64, l: f64, a: f64) -> Self {
        let h = h.rem_euclid(360.0) / 360.0;
        let s = s.clamp(0.0, 1.0);
        let l = l.clamp(0.0, 1.0);
        let q = if l < 0.5 {
            l * (1.0 + s)
        } else {
            l + s - l * s
        };
        let p = 2.0 * l - q;
        let hue = |mut t: f64| {
            if t < 0.0 {
                t += 1.0;
            }
            if t > 1.0 {
                t -= 1.0;
            }
            if t < 1.0 / 6.0 {
                p + (q - p) * 6.0 * t
            } else if t < 0.5 {
                q
            } else if t < 2.0 / 3.0 {
                p + (q - p) * (2.0 / 3.0 - t) * 6.0
            } else {
                p
            }
        };
        let (r, g, b) = if s == 0.0 {
            (l, l, l)
        } else {
            (hue(h + 1.0 / 3.0), hue(h), hue(h - 1.0 / 3.0))
        };
        Self::rgba(unit(r), unit(g), unit(b), unit(a))
    }

    pub fn mix(self, o: Color, t: f64) -> Color {
        let t = t.clamp(0.0, 1.0);
        let l = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t).round() as u8;
        Color::rgba(
            l(self.r, o.r),
            l(self.g, o.g),
            l(self.b, o.b),
            l(self.a, o.a),
        )
    }

    pub fn lighten(self, t: f64) -> Color {
        self.mix(Color::WHITE.with_alpha(self.a), t)
    }

    pub fn darken(self, t: f64) -> Color {
        self.mix(Color::BLACK.with_alpha(self.a), t)
    }

    pub fn with_alpha(self, a: u8) -> Color {
        Color { a, ..self }
    }
}

fn unit(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn named(n: &str) -> Option<Color> {
    Some(match n {
        "transparent" | "none" => Color::TRANSPARENT,
        "black" => Color::BLACK,
        "white" => Color::WHITE,
        "red" => Color::rgb(255, 0, 0),
        "lime" => Color::rgb(0, 255, 0),
        "green" => Color::rgb(0, 128, 0),
        "blue" => Color::rgb(0, 0, 255),
        "yellow" => Color::rgb(255, 255, 0),
        "cyan" | "aqua" => Color::rgb(0, 255, 255),
        "magenta" | "fuchsia" => Color::rgb(255, 0, 255),
        "gray" | "grey" => Color::rgb(128, 128, 128),
        "silver" => Color::rgb(192, 192, 192),
        "orange" => Color::rgb(255, 165, 0),
        "purple" => Color::rgb(128, 0, 128),
        "pink" => Color::rgb(255, 192, 203),
        "brown" => Color::rgb(165, 42, 42),
        "navy" => Color::rgb(0, 0, 128),
        "teal" => Color::rgb(0, 128, 128),
        "olive" => Color::rgb(128, 128, 0),
        "maroon" => Color::rgb(128, 0, 0),
        _ => return None,
    })
}

impl fmt::Debug for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Exact `round(x / 255)` for `x <= 65535`, without a division.
#[inline(always)]
pub fn div255(x: u32) -> u32 {
    let x = x + 128;
    (x + (x >> 8)) >> 8
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BlendMode {
    #[default]
    Over,
    Replace,
    Erase,
    Multiply,
    Screen,
    Add,
    Darken,
    Lighten,
}

impl BlendMode {
    pub const NAMES: &'static str = "over, replace, erase, multiply, screen, add, darken, lighten";

    pub fn parse(s: &str) -> Result<Self, String> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "over" | "normal" | "src-over" => Self::Over,
            "replace" | "copy" | "src" => Self::Replace,
            "erase" | "clear" | "dst-out" => Self::Erase,
            "multiply" => Self::Multiply,
            "screen" => Self::Screen,
            "add" | "plus" => Self::Add,
            "darken" => Self::Darken,
            "lighten" => Self::Lighten,
            _ => return Err(format!("unknown blend mode {s:?} (use: {})", Self::NAMES)),
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Over => "over",
            Self::Replace => "replace",
            Self::Erase => "erase",
            Self::Multiply => "multiply",
            Self::Screen => "screen",
            Self::Add => "add",
            Self::Darken => "darken",
            Self::Lighten => "lighten",
        }
    }
}

#[inline(always)]
fn ch(p: u32, s: u32) -> u32 {
    (p >> s) & 255
}

/// Porter-Duff source-over with straight alpha. The common cases (opaque, empty) skip the arithmetic.
#[inline]
pub fn over(dst: u32, src: u32) -> u32 {
    let sa = src >> 24;
    if sa == 255 {
        return src;
    }
    if sa == 0 {
        return dst;
    }
    let da = dst >> 24;
    if da == 0 {
        return src;
    }
    let inv = 255 - sa;
    if da == 255 {
        let r = div255(ch(src, 0) * sa + ch(dst, 0) * inv);
        let g = div255(ch(src, 8) * sa + ch(dst, 8) * inv);
        let b = div255(ch(src, 16) * sa + ch(dst, 16) * inv);
        return r | g << 8 | b << 16 | 0xff00_0000;
    }
    let dw = div255(da * inv);
    let oa = sa + dw;
    let f = |s: u32| (ch(src, s) * sa + ch(dst, s) * dw + oa / 2) / oa;
    f(0) | f(8) << 8 | f(16) << 16 | oa << 24
}

#[inline]
pub fn blend(dst: u32, src: u32, mode: BlendMode) -> u32 {
    match mode {
        BlendMode::Over => over(dst, src),
        BlendMode::Replace => src,
        BlendMode::Erase => {
            let sa = src >> 24;
            if sa == 0 {
                return dst;
            }
            let na = div255((dst >> 24) * (255 - sa));
            if na == 0 {
                0
            } else {
                (dst & 0x00ff_ffff) | na << 24
            }
        }
        _ => {
            let sa = src >> 24;
            if sa == 0 {
                return dst;
            }
            let da = dst >> 24;
            if da == 0 {
                return src;
            }
            let mut out = src & 0xff00_0000;
            for s in [0, 8, 16] {
                let cs = ch(src, s);
                let cb = ch(dst, s);
                let b = match mode {
                    BlendMode::Multiply => div255(cs * cb),
                    BlendMode::Screen => cs + cb - div255(cs * cb),
                    BlendMode::Add => (cs + cb).min(255),
                    BlendMode::Darken => cs.min(cb),
                    _ => cs.max(cb),
                };
                // W3C: Cs' = (1 - αb)·Cs + αb·B(Cb, Cs)
                out |= div255(cs * (255 - da) + b * da) << s;
            }
            over(dst, out)
        }
    }
}

#[inline(always)]
pub fn scale_alpha(p: u32, op: u32) -> u32 {
    if op >= 255 {
        p
    } else {
        (p & 0x00ff_ffff) | div255((p >> 24) * op) << 24
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn div255_exact() {
        for x in 0..=65025u32 {
            assert_eq!(div255(x), (x as f64 / 255.0).round() as u32, "x={x}");
        }
    }

    #[test]
    fn parse_formats() {
        assert_eq!(Color::parse("#f00").unwrap(), Color::rgb(255, 0, 0));
        assert_eq!(
            Color::parse("00ff0080").unwrap(),
            Color::rgba(0, 255, 0, 128)
        );
        assert_eq!(Color::parse("navy").unwrap(), Color::rgb(0, 0, 128));
        assert!(Color::parse("#12").is_err());
        assert_eq!(
            Color::from_px(Color::rgba(1, 2, 3, 4).to_px()),
            Color::rgba(1, 2, 3, 4)
        );
    }

    #[test]
    fn over_rules() {
        let red = Color::rgb(255, 0, 0).to_px();
        let half_blue = Color::rgba(0, 0, 255, 128).to_px();
        assert_eq!(over(red, 0), red);
        assert_eq!(over(0, half_blue), half_blue);
        let c = Color::from_px(over(red, half_blue));
        assert_eq!(c.a, 255);
        assert!(c.r == 127 && c.b == 128, "{c:?}");
        let e = blend(red, Color::rgba(0, 0, 0, 255).to_px(), BlendMode::Erase);
        assert_eq!(e, 0);
    }

    #[test]
    fn hsl_primaries() {
        assert_eq!(Color::from_hsl(0.0, 1.0, 0.5, 1.0), Color::rgb(255, 0, 0));
        assert_eq!(Color::from_hsl(120.0, 1.0, 0.5, 1.0), Color::rgb(0, 255, 0));
        assert_eq!(Color::from_hsl(240.0, 1.0, 0.5, 1.0), Color::rgb(0, 0, 255));
    }
}
