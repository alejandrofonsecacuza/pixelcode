//! `pixel`: CLI and MCP server for the pixelcode drawing engine.

mod mcp;
mod share;
mod term;
mod tools;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime};

use clap::{Parser, Subcommand};
use pixel_core::raster::{self, Cap, FillRule};
use pixel_core::span::normalize;
use pixel_core::*;

#[derive(Parser)]
#[command(
    name = "pixel",
    version,
    about = "Pixel-perfect drawing engine for AI agents (CLI + MCP server)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// MCP server over stdio (for Claude Code or another harness).
    Mcp {
        /// Directory where a PNG per canvas is written after every change (for `pixel watch`).
        #[arg(long)]
        live: Option<PathBuf>,
    },
    /// Runs a drawing script ("-" reads stdin) and saves or shows the result.
    Run {
        script: PathBuf,
        /// Canvas size, e.g. 512x512.
        #[arg(long, default_value = "256x256")]
        size: String,
        /// Starting PNG (sets the canvas size).
        #[arg(long)]
        input: Option<PathBuf>,
        /// Background color.
        #[arg(long)]
        bg: Option<String>,
        /// Output file (.png, or .html for a shareable page).
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Integer upscale when saving.
        #[arg(long, default_value_t = 1)]
        scale: u32,
        /// Show the result in the terminal.
        #[arg(long)]
        show: bool,
    },
    /// Shows a PNG in the terminal (Kitty protocol or half blocks).
    Show {
        file: PathBuf,
        #[arg(long)]
        halfblocks: bool,
    },
    /// Watches a PNG (or the newest one in a directory) and redraws it when it changes.
    Watch {
        path: PathBuf,
        #[arg(long, default_value_t = 150)]
        interval: u64,
        #[arg(long)]
        halfblocks: bool,
    },
    /// Measures engine performance on this machine.
    Bench {
        #[arg(long, default_value_t = 4096)]
        size: u32,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let r = match cli.cmd {
        Cmd::Mcp { live } => mcp::serve(tools::Workspace::new(live)).map_err(|e| e.to_string()),
        Cmd::Run {
            script,
            size,
            input,
            bg,
            out,
            scale,
            show,
        } => run(&script, &size, input, bg, out, scale, show),
        Cmd::Show { file, halfblocks } => show(&file, halfblocks),
        Cmd::Watch {
            path,
            interval,
            halfblocks,
        } => watch(&path, interval, halfblocks),
        Cmd::Bench { size } => {
            bench(size);
            Ok(())
        }
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn load_png(p: &Path) -> Result<Image, String> {
    Image::decode_png(&std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?)
}

fn run(
    script: &Path,
    size: &str,
    input: Option<PathBuf>,
    bg: Option<String>,
    out: Option<PathBuf>,
    scale: u32,
    show: bool,
) -> Result<(), String> {
    let src = if script.as_os_str() == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| e.to_string())?;
        s
    } else {
        std::fs::read_to_string(script).map_err(|e| format!("{}: {e}", script.display()))?
    };
    let mut doc = match &input {
        Some(p) => {
            let img = load_png(p)?;
            let mut d = Document::new(img.width, img.height);
            for y in 0..img.height {
                d.layers[0].raster.write_row(
                    y as i32,
                    0,
                    &img.px[(y * img.width) as usize..((y + 1) * img.width) as usize],
                );
            }
            d
        }
        None => {
            let (w, h) = size
                .split_once(['x', 'X'])
                .ok_or("--size must look like 512x512")?;
            let (w, h): (u32, u32) = (
                w.parse().map_err(|_| "bad width")?,
                h.parse().map_err(|_| "bad height")?,
            );
            if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
                return Err(format!("size must be 1..={MAX_SIDE} per side"));
            }
            Document::new(w, h)
        }
    };
    if let Some(bg) = bg {
        doc.layers[0].raster.fill_all(Color::parse(&bg)?.to_px());
    }
    let (doc, report) = pixel_script::run(&doc, &src, &Default::default()).map_err(|e| {
        let mut m = e.message;
        if !e.logs.is_empty() {
            m = format!("{}\n{m}", e.logs.join("\n"));
        }
        m
    })?;
    for l in &report.logs {
        println!("{l}");
    }
    eprintln!(
        "ok: {}x{} in {:.2} ms ({} ops, {} layers)",
        doc.width,
        doc.height,
        report.elapsed.as_secs_f64() * 1000.0,
        report.operations,
        doc.layers.len()
    );
    let img = doc.composite_all();
    if let Some(out) = out {
        if let Some(dir) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        if out.extension().is_some_and(|x| x == "html") {
            let name = out.file_stem().and_then(|s| s.to_str()).unwrap_or("canvas");
            let page = share::build(name, None, &doc, &[], false)?;
            std::fs::write(&out, &page.html).map_err(|e| e.to_string())?;
            eprintln!(
                "saved {} ({} bytes, shareable page)",
                out.display(),
                page.html.len()
            );
            if show {
                term::show(&img, false).map_err(|e| e.to_string())?;
            }
            return Ok(());
        }
        let png = img.upscale(scale.clamp(1, 64)).encode_png(false)?;
        std::fs::write(&out, &png).map_err(|e| e.to_string())?;
        eprintln!("saved {} ({} bytes)", out.display(), png.len());
    }
    if show {
        term::show(&img, false).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn show(file: &Path, halfblocks: bool) -> Result<(), String> {
    term::show(&load_png(file)?, halfblocks).map_err(|e| e.to_string())
}

fn newest_png(dir: &Path) -> Option<(PathBuf, SystemTime)> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "png"))
        .filter_map(|e| Some((e.path(), e.metadata().ok()?.modified().ok()?)))
        .max_by_key(|(_, t)| *t)
}

fn watch(path: &Path, interval: u64, halfblocks: bool) -> Result<(), String> {
    let mut last: Option<(PathBuf, SystemTime)> = None;
    loop {
        let cur = if path.is_dir() {
            newest_png(path)
        } else {
            std::fs::metadata(path)
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| (path.to_path_buf(), t))
        };
        if cur.is_some() && cur != last {
            let (p, _) = cur.clone().unwrap();
            if let Ok(img) = load_png(&p) {
                let mut o = std::io::stdout().lock();
                if !halfblocks && term::kitty_supported() {
                    let _ = write!(o, "\x1b_Ga=d,q=2\x1b\\");
                }
                let _ = writeln!(
                    o,
                    "\x1b[2J\x1b[H{} · {}x{}",
                    p.display(),
                    img.width,
                    img.height
                );
                drop(o);
                term::show(&img, halfblocks).map_err(|e| e.to_string())?;
                last = cur;
            }
        } else if cur.is_none() && last.is_none() {
            eprint!("\rwaiting for {} ...", path.display());
        }
        std::thread::sleep(Duration::from_millis(interval.max(16)));
    }
}

fn best_of<T>(n: usize, mut f: impl FnMut() -> T) -> Duration {
    (0..n)
        .map(|_| {
            let t = Instant::now();
            std::hint::black_box(f());
            t.elapsed()
        })
        .min()
        .unwrap()
}

fn bench(size: u32) {
    let n = size as f64 * size as f64;
    let s = size as f64;
    let bounds = IRect::new(0, 0, size as i32, size as i32);
    let red = Paint::solid(Color::rgba(220, 40, 40, 255));
    let glass = Paint::solid(Color::rgba(40, 90, 220, 140));
    let mut rows: Vec<(String, Duration, Option<f64>)> = Vec::new();
    let mut add = |name: String, d: Duration, px: Option<f64>| rows.push((name, d, px));

    let mut full = Vec::new();
    raster::rect(0.0, 0.0, s, s, bounds, &mut full);

    let mut r = Raster::new(size, size);
    add(
        format!("fill {size}² opaque"),
        best_of(5, || r.paint_spans(&full, &red, BlendMode::Over)),
        Some(n),
    );
    add(
        format!("alpha blend {size}²"),
        best_of(5, || r.paint_spans(&full, &glass, BlendMode::Over)),
        Some(n),
    );
    let g = Gradient::even(&[Color::rgb(10, 10, 40), Color::rgb(250, 200, 80)], 6, true);
    let gp = Paint::Gradient(g, GradientShape::radial(s / 2.0, s / 2.0, s / 2.0));
    add(
        format!("dithered radial gradient {size}²"),
        best_of(3, || r.paint_spans(&full, &gp, BlendMode::Replace)),
        Some(n),
    );

    let mut r2 = Raster::new(size, size);
    let mut buf = Vec::new();
    add(
        "10,000 circles r=12".into(),
        best_of(3, || {
            for i in 0..10_000u64 {
                let x = (i * 7919 % size as u64) as f64;
                let y = (i * 104_729 % size as u64) as f64;
                buf.clear();
                buf.extend(
                    raster::ellipse_rows(x, y, 12.0, 12.0, 0, size as i32)
                        .into_iter()
                        .filter(|s| s.x0 < s.x1),
                );
                normalize(&mut buf, bounds);
                r2.paint_spans(&buf, &red, BlendMode::Over);
            }
        }),
        None,
    );
    let star: Vec<(f64, f64)> = (0..2000)
        .map(|i| {
            let a = i as f64 / 2000.0 * std::f64::consts::TAU;
            let rr = if i % 2 == 0 { s * 0.49 } else { s * 0.22 };
            (s / 2.0 + rr * a.cos(), s / 2.0 + rr * a.sin())
        })
        .collect();
    add(
        "star polygon, 2000 vertices".into(),
        best_of(3, || {
            let mut sp = Vec::new();
            raster::polygon(
                std::slice::from_ref(&star),
                FillRule::NonZero,
                bounds,
                &mut sp,
            );
            normalize(&mut sp, bounds);
            r2.paint_spans(&sp, &red, BlendMode::Over);
        }),
        None,
    );
    let wave: Vec<(f64, f64)> = (0..1000)
        .map(|i| {
            (
                i as f64 * s / 1000.0,
                s / 2.0 + (i as f64 * 0.1).sin() * s * 0.2,
            )
        })
        .collect();
    add(
        "thick polyline, 1000 segments w=5".into(),
        best_of(3, || {
            let mut sp = Vec::new();
            raster::stroke_polyline(&wave, false, 5.0, Cap::Round, bounds, &mut sp);
            normalize(&mut sp, bounds);
            r2.paint_spans(&sp, &red, BlendMode::Over);
        }),
        None,
    );
    let empty = Raster::new(size, size);
    add(
        format!("flood fill {size}²"),
        best_of(3, || {
            let mut sp = Vec::new();
            raster::flood(&empty, 1, 1, 0, bounds, &mut sp);
            sp
        }),
        Some(n),
    );

    let mut d = Document::new(size, size);
    d.layers[0].raster.fill_all(Color::WHITE.to_px());
    for k in 0..3 {
        let li = d.ensure_layer(&format!("l{k}"));
        let mut sp = Vec::new();
        raster::rect(k as f64 * s / 8.0, 0.0, s * 0.6, s, bounds, &mut sp);
        d.layers[li]
            .raster
            .paint_spans(&sp, &glass, BlendMode::Over);
    }
    add(
        format!("composite {size}² x4 layers"),
        best_of(3, || d.composite_all()),
        Some(n),
    );
    add(
        format!("snapshot + diff {size}²"),
        best_of(5, || {
            let snap = d.clone();
            d.layers[0].raster.set(7, 7, 0xff00_00ff);
            d.diff(&snap)
        }),
        None,
    );
    let script = "for i in 0..10000 { circle(rand(0, W), rand(0, H), 6, hsl(i % 360, 0.7, 0.5)); }";
    let sd = Document::new(size, size);
    add(
        "script: 10,000 circle() calls".into(),
        best_of(3, || {
            pixel_script::run(&sd, script, &Default::default()).unwrap()
        }),
        None,
    );
    let img = d.composite(IRect::new(0, 0, 1024, 1024));
    add(
        "encode PNG 1024² (fast)".into(),
        best_of(3, || img.encode_png(true).unwrap()),
        None,
    );

    println!("pixelcode bench · {} threads", rayon_threads());
    println!("{:<38} {:>10} {:>12}", "operation", "time", "throughput");
    for (name, d, px) in rows {
        let ms = d.as_secs_f64() * 1000.0;
        let tp = px.map_or(String::new(), |p| {
            format!("{:.0} Mpx/s", p / d.as_secs_f64() / 1e6)
        });
        println!("{name:<38} {ms:>8.2}ms {tp:>12}");
    }
}

fn rayon_threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}
