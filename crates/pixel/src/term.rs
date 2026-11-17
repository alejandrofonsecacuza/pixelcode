//! Showing images in the terminal: the Kitty graphics protocol (Kitty, Ghostty, WezTerm), or
//! true-color `▀` half blocks in any other modern terminal.

use std::io::{self, Write};

use base64::Engine as _;
use pixel_core::view::{self, ViewOpts};
use pixel_core::{Color, IRect, Image};

pub fn kitty_supported() -> bool {
    let term = std::env::var("TERM").unwrap_or_default();
    let prog = std::env::var("TERM_PROGRAM").unwrap_or_default();
    std::env::var_os("KITTY_WINDOW_ID").is_some()
        || term.contains("kitty")
        || term.contains("ghostty")
        || prog.eq_ignore_ascii_case("wezterm")
        || prog.eq_ignore_ascii_case("ghostty")
}

fn term_cols() -> u32 {
    terminal_size::terminal_size().map_or(80, |(w, _)| w.0 as u32)
}

pub fn kitty(png: &[u8], out: &mut impl Write) -> io::Result<()> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    let chunks: Vec<&[u8]> = b64.as_bytes().chunks(4096).collect();
    for (i, c) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        if i == 0 {
            write!(out, "\x1b_Ga=T,f=100,q=2,m={more};")?;
        } else {
            write!(out, "\x1b_Gm={more};")?;
        }
        out.write_all(c)?;
        out.write_all(b"\x1b\\")?;
    }
    writeln!(out)
}

pub fn halfblocks(img: &Image, max_cols: u32, out: &mut impl Write) -> io::Result<()> {
    let w = img.width.max(1);
    let scaled = if w > max_cols {
        img.downscale(w.div_ceil(max_cols))
    } else {
        img.upscale((max_cols / w).clamp(1, 4))
    };
    let flat = scaled.over_checker(4);
    let bg = Color::rgb(0, 0, 0).to_px();
    for y in (0..flat.height).step_by(2) {
        for x in 0..flat.width {
            let t = Color::from_px(flat.get(x as i32, y as i32));
            let b = Color::from_px(if y + 1 < flat.height {
                flat.get(x as i32, y as i32 + 1)
            } else {
                bg
            });
            write!(
                out,
                "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m▀",
                t.r, t.g, t.b, b.r, b.g, b.b
            )?;
        }
        writeln!(out, "\x1b[0m")?;
    }
    Ok(())
}

/// Shows an image in the terminal using the best available method.
pub fn show(img: &Image, force_halfblocks: bool) -> io::Result<()> {
    let mut out = io::stdout().lock();
    if !force_halfblocks && kitty_supported() {
        let region = IRect::new(0, 0, img.width as i32, img.height as i32);
        let opts = ViewOpts {
            max_side: 720,
            grid: Some(false),
            rulers: false,
            ..ViewOpts::default()
        };
        let (v, _) = view::render(img, region, &opts);
        kitty(&v.encode_png(true).map_err(io::Error::other)?, &mut out)?;
    } else {
        halfblocks(img, term_cols().saturating_sub(1).max(8), &mut out)?;
    }
    out.flush()
}
