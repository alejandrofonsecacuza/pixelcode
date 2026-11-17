//! Text with an 8×8 bitmap font (font8x8, public domain): ASCII, Latin-1, box and block glyphs.

use font8x8::UnicodeFonts;

use crate::span::Span;

pub const GLYPH: i32 = 8;

pub fn glyph(c: char) -> [u8; 8] {
    font8x8::BASIC_FONTS
        .get(c)
        .or_else(|| font8x8::LATIN_FONTS.get(c))
        .or_else(|| font8x8::BOX_FONTS.get(c))
        .or_else(|| font8x8::BLOCK_FONTS.get(c))
        .or_else(|| font8x8::GREEK_FONTS.get(c))
        .or_else(|| font8x8::MISC_FONTS.get(c))
        .unwrap_or_else(|| font8x8::BASIC_FONTS.get('?').unwrap_or([0; 8]))
}

/// Width and height of a text in pixels (multiple lines with `\n`).
pub fn measure(text: &str, scale: i32) -> (i32, i32) {
    let lines: Vec<&str> = text.split('\n').collect();
    let w = lines
        .iter()
        .map(|l| l.chars().count() as i32)
        .max()
        .unwrap_or(0);
    (w * GLYPH * scale, lines.len() as i32 * GLYPH * scale)
}

/// Rasterizes text into spans with its top-left corner at `(x, y)`.
pub fn spans(x: i32, y: i32, text: &str, scale: i32, out: &mut Vec<Span>) {
    let scale = scale.max(1);
    for (li, line) in text.split('\n').enumerate() {
        let ly = y + li as i32 * GLYPH * scale;
        for (ci, ch) in line.chars().enumerate() {
            let g = glyph(ch);
            let gx = x + ci as i32 * GLYPH * scale;
            for (row, bits) in g.iter().enumerate() {
                let mut col = 0;
                while col < 8 {
                    if bits & (1 << col) == 0 {
                        col += 1;
                        continue;
                    }
                    let start = col;
                    while col < 8 && bits & (1 << col) != 0 {
                        col += 1;
                    }
                    for sy in 0..scale {
                        out.push(Span::new(
                            ly + row as i32 * scale + sy,
                            gx + start * scale,
                            gx + col * scale,
                        ));
                    }
                }
            }
        }
    }
}
