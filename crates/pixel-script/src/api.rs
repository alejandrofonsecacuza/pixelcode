//! Functions exposed to scripts, and their reference (which is also handed to the agent).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use pixel_core::raster;
use pixel_core::*;
use rhai::{Array, Dynamic, Engine, Map};

use crate::{Ctx, PaintValue, R, Role, alpha, color, err, int, num, paint, points, style};

/// Compact language reference. It is included in the `draw` tool's description.
pub const REFERENCE: &str = r##"Script language: Rhai (JS/Rust-like: let, for i in 0..n, if, fn, arrays [..], maps #{k: v}, closures).
Every statement ends with `;`. String interpolation uses backticks: `x=${x}`.
Constants: W, H (canvas size), PI, TAU. Ints and floats mix freely in drawing calls.

COORDINATES: fills use continuous coords, pixel (x,y) covers [x,x+1)x[y,y+1) and is painted if its CENTER is inside.
  rect(0,0,4,4) paints pixels 0..3. circle(10,10,5) is centered on a pixel CORNER (even diameter);
  circle(10.5,10.5,5) is centered on pixel (10,10) (odd diameter). Lines/strokes pass through pixel centers:
  line(0,0,9,0,c) paints pixels 0..9 inclusive. No antialiasing: output is exact and deterministic.

STYLE argument (last param of shapes): a color/paint ("#rrggbb", "#rrggbbaa", "red", rgb(..), linear(..)) or a map:
  #{fill: paint, stroke: paint, width: 1, blend: "over", rule: "nonzero"|"evenodd", cap: "round"|"square"|"butt",
    scale: 1, dx: 0, dy: 0, tolerance: 0}
  rect/circle/ellipse strokes are INNER strokes (stay inside the filled shape). blend: over, replace, erase,
  multiply, screen, add, darken, lighten. scale/dx/dy transform points of polygon/polyline/path.

SHAPES
  pixel(x, y, style)                    rect(x, y, w, h, style)
  circle(cx, cy, r, style)              ellipse(cx, cy, rx, ry, style)
  line(x0, y0, x1, y1, style)           polyline(points, style)     polygon(points, style)
  path(svg_d, style)   SVG path data: M L H V C S Q T A Z (abs/rel). e.g. path("M2 2 Q 16 30 30 2 Z", "#f80")
  points: [[x,y], [x,y], ...] or flat [x0,y0, x1,y1, ...]
  flood(x, y, style)   bucket fill of the connected region on the active layer (#{fill:.., tolerance: 0..255})
  text(x, y, string, style)   8x8 bitmap font (ASCII + Latin-1 + box/block chars), "\n" for new lines;
                       style map key `scale` = integer glyph scale. text_width(s [, scale]) -> int
  grid(x, y, rows, palette [, scale])   sprite from strings: rows = ["..AA..", ".ABBA."],
                       palette = #{A: "#000", B: "#f00"}; '.', ' ' and unmapped chars are transparent.
  copy(x, y, w, h, dx, dy [, #{from: "layer", flip_x, flip_y, rotate: 90|180|270, scale: int, blend, cut: bool}])

PAINTS (usable wherever a color is)
  linear(x0, y0, x1, y1, colors [, #{steps: n, dither: bool}])    radial(cx, cy, r, colors [, opts])
     colors = ["#000", "#fff"] (evenly spaced) or [[0.0, "#000"], [0.7, "#f00"], [1.0, "#fff"]]
     steps = quantize into n bands (pixel-art style); dither = ordered Bayer dithering between bands.
  pattern(x, y, w, h [, #{from: "layer", scale: int}])   tiles a region of a layer as a repeating paint.

COLORS
  rgb(r,g,b) rgba(r,g,b,a) hsl(h,s,l) hsla(h,s,l,a) hex("#rrggbb")   (a: 0..255 int or 0.0..1.0 float)
  c.lighten(t) c.darken(t) c.mix(other, t) c.alpha(a) c.hex() c.r c.g c.b c.a
  get(x, y) -> color on the active layer      sample(x, y) -> composited color of all layers

LAYERS (bottom to top; drawing goes to the active layer, which persists between calls)
  layer(name [, #{opacity: 0..1, blend: "multiply", visible: bool}])  select or create (on top)
  layers() -> names   current_layer() -> name   remove_layer(name)   move_layer(name, index)
  clear() / clear(color)   clears or fills the active layer (respects clip)
  clip(x, y, w, h) / unclip()   restrict all drawing to a rectangle

UTILS
  seed(n) rand() -> 0..1  rand(a, b) -> int in [a, b]  noise(x, y) -> smooth value noise 0..1
  clamp(v, lo, hi)  lerp(a, b, t)  dist(x0, y0, x1, y1)  atan2(y, x)  sin/cos/sqrt... accept ints too
  print(x) -> returned in the tool result (use it to report measurements back to yourself)."##;

fn channel(d: &Dynamic, what: &str) -> R<u8> {
    Ok(int(d, what)?.clamp(0, 255) as u8)
}

fn unit_or_percent(d: &Dynamic, what: &str) -> R<f64> {
    let v = num(d, what)?;
    Ok(if v > 1.0 { v / 100.0 } else { v })
}

fn opt<'a>(m: &'a Map, k: &str) -> Option<&'a Dynamic> {
    m.get(k)
}

fn opts_map(d: &Dynamic, what: &str) -> R<Map> {
    d.clone().try_cast::<Map>().map_or_else(
        || err(format!("{what}: expected an options map #{{...}}")),
        Ok,
    )
}

fn as_str(d: &Dynamic, what: &str) -> R<String> {
    d.clone()
        .into_string()
        .or_else(|_| err(format!("{what}: expected a string, got {}", d.type_name())))
}

fn as_array(d: &Dynamic, what: &str) -> R<Array> {
    d.clone()
        .try_cast::<Array>()
        .map_or_else(|| err(format!("{what}: expected an array")), Ok)
}

fn truthy(d: Option<&Dynamic>) -> bool {
    d.and_then(|v| v.as_bool().ok()).unwrap_or(false)
}

fn gradient_stops(colors: &Dynamic, opts: Option<&Dynamic>) -> R<Gradient> {
    let arr = as_array(colors, "gradient colors")?;
    if arr.len() < 2 {
        return err("a gradient needs at least 2 colors");
    }
    let (mut steps, mut dither) = (0u32, false);
    if let Some(o) = opts {
        let m = opts_map(o, "gradient")?;
        if let Some(s) = opt(&m, "steps") {
            steps = int(s, "steps")?.clamp(0, 4096) as u32;
        }
        dither = truthy(opt(&m, "dither"));
    }
    if arr.iter().all(|v| v.is_array()) {
        let mut stops = Vec::new();
        for v in &arr {
            let p = v.clone().cast::<Array>();
            if p.len() != 2 {
                return err("gradient stops must be [t, color]");
            }
            stops.push((num(&p[0], "stop")?, color(&p[1])?));
        }
        Ok(Gradient::new(stops, steps, dither))
    } else {
        let cs = arr.iter().map(color).collect::<R<Vec<_>>>()?;
        Ok(Gradient::even(&cs, steps, dither))
    }
}

fn value_noise(seed: u64, x: f64, y: f64) -> f64 {
    let hash = |ix: i64, iy: i64| -> f64 {
        let mut h = (ix as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (iy as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
            ^ seed;
        h ^= h >> 33;
        h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        h ^= h >> 33;
        (h >> 11) as f64 / (1u64 << 53) as f64
    };
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let s = |t: f64| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (s(fx), s(fy));
    let (ix, iy) = (x0 as i64, y0 as i64);
    let a = hash(ix, iy) + (hash(ix + 1, iy) - hash(ix, iy)) * sx;
    let b = hash(ix, iy + 1) + (hash(ix + 1, iy + 1) - hash(ix, iy + 1)) * sx;
    a + (b - a) * sy
}

fn grid(c: &mut Ctx, x: i32, y: i32, rows: &Dynamic, palette: &Dynamic, scale: i32) -> R<()> {
    let rows = as_array(rows, "grid rows")?;
    let pal = opts_map(palette, "grid palette")?;
    let mut lut: HashMap<char, u32> = HashMap::new();
    for (k, v) in pal {
        let mut chars = k.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            return err(format!(
                "grid palette keys must be single characters, got {k:?}"
            ));
        };
        lut.insert(ch, color(&v)?.to_px());
    }
    let lines = rows
        .iter()
        .map(|r| as_str(r, "grid row"))
        .collect::<R<Vec<_>>>()?;
    let w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u32;
    let mut img = Image::new(w, lines.len() as u32);
    let s = scale.max(1);
    let buf = c.buf();
    for (j, line) in lines.iter().enumerate() {
        let mut run: Option<i32> = None;
        let chars: Vec<char> = line.chars().collect();
        for i in 0..=chars.len() {
            let px = chars.get(i).and_then(|ch| {
                if *ch == '.' || *ch == ' ' {
                    None
                } else {
                    lut.get(ch)
                }
            });
            match (px, run) {
                (Some(&p), _) => {
                    img.set(i as i32, j as i32, p);
                    run.get_or_insert(i as i32);
                }
                (None, Some(st)) => {
                    for k in 0..s {
                        buf.push(Span::new(
                            y + j as i32 * s + k,
                            x + st * s,
                            x + i as i32 * s,
                        ));
                    }
                    run = None;
                }
                (None, None) => {}
            }
        }
    }
    let p = Ctx::image_paint(img, x, y, s as u32, false);
    c.flush(&p, BlendMode::Over);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn copy(
    c: &mut Ctx,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    dx: i32,
    dy: i32,
    opts: Option<&Dynamic>,
) -> R<()> {
    let m = match opts {
        Some(o) => opts_map(o, "copy")?,
        None => Map::new(),
    };
    let src = match opt(&m, "from") {
        Some(n) => {
            let n = as_str(n, "from")?;
            c.doc
                .layer_index(&n)
                .map_or_else(|| err(format!("copy: no layer named {n:?}")), Ok)?
        }
        None => c.doc.active,
    };
    let want = IRect::new(x, y, w, h);
    let r = want.intersect(&c.doc.bounds());
    if r.is_empty() {
        return Ok(());
    }
    let (sx, sy) = (r.x0 - x, r.y0 - y);
    let mut img = c.doc.layers[src].raster.read_region(r);
    if truthy(opt(&m, "cut")) {
        let mut spans = Vec::new();
        raster::rect(
            r.x0 as f64,
            r.y0 as f64,
            r.w() as f64,
            r.h() as f64,
            c.doc.bounds(),
            &mut spans,
        );
        c.doc.layers[src]
            .raster
            .paint_spans(&spans, &Paint::Solid(0), BlendMode::Replace);
    }
    if truthy(opt(&m, "flip_x")) {
        img = img.flip_x();
    }
    if truthy(opt(&m, "flip_y")) {
        img = img.flip_y();
    }
    let rot = match opt(&m, "rotate") {
        Some(v) => int(v, "rotate")?.rem_euclid(360),
        None => 0,
    };
    match rot {
        0 => {}
        90 => img = img.rotate_cw(),
        180 => img = img.rotate_cw().rotate_cw(),
        270 => img = img.rotate_cw().rotate_cw().rotate_cw(),
        _ => return err("copy: rotate must be 0, 90, 180 or 270"),
    }
    let scale = match opt(&m, "scale") {
        Some(v) => int(v, "scale")?.clamp(1, 256) as i32,
        None => 1,
    };
    let blend = match opt(&m, "blend") {
        Some(v) => BlendMode::parse(&as_str(v, "blend")?).or_else(err)?,
        None => BlendMode::Over,
    };
    let (ox, oy) = if rot == 0 {
        (dx + sx * scale, dy + sy * scale)
    } else {
        (dx, dy)
    };
    let (iw, ih) = (
        img.width as f64 * scale as f64,
        img.height as f64 * scale as f64,
    );
    let clip = c.clip;
    raster::rect(ox as f64, oy as f64, iw, ih, clip, c.buf());
    let p = Ctx::image_paint(img, ox, oy, scale as u32, false);
    c.flush(&p, blend);
    Ok(())
}

fn clear_with(c: &mut Ctx, col: &Dynamic) -> R<()> {
    let p = paint(col)?;
    let clip = c.clip;
    match p {
        Paint::Solid(px) if clip == c.doc.bounds() => c.layer().raster.fill_all(px),
        p => {
            raster::rect(
                clip.x0 as f64,
                clip.y0 as f64,
                clip.w() as f64,
                clip.h() as f64,
                clip,
                c.buf(),
            );
            c.flush(&p, BlendMode::Replace);
        }
    }
    Ok(())
}

pub(crate) fn register(engine: &mut Engine, ctx: Rc<RefCell<Ctx>>) {
    // ---------- types ----------
    engine
        .register_type_with_name::<Color>("Color")
        .register_fn("to_string", |c: &mut Color| c.to_hex())
        .register_fn("to_debug", |c: &mut Color| c.to_hex())
        .register_fn("hex", |c: &mut Color| c.to_hex())
        .register_get("r", |c: &mut Color| c.r as i64)
        .register_get("g", |c: &mut Color| c.g as i64)
        .register_get("b", |c: &mut Color| c.b as i64)
        .register_get("a", |c: &mut Color| c.a as i64)
        .register_fn("lighten", |c: &mut Color, t: Dynamic| -> R<Color> {
            Ok(c.lighten(num(&t, "lighten")?))
        })
        .register_fn("darken", |c: &mut Color, t: Dynamic| -> R<Color> {
            Ok(c.darken(num(&t, "darken")?))
        })
        .register_fn("alpha", |c: &mut Color, a: Dynamic| -> R<Color> {
            Ok(c.with_alpha(alpha(&a, "alpha")?))
        })
        .register_fn("mix", |c: &mut Color, o: Dynamic, t: Dynamic| -> R<Color> {
            Ok(c.mix(color(&o)?, num(&t, "mix")?))
        })
        .register_fn("==", |a: Color, b: Color| a == b)
        .register_fn("!=", |a: Color, b: Color| a != b);
    engine
        .register_type_with_name::<PaintValue>("Paint")
        .register_fn("to_string", |_: &mut PaintValue| "<paint>".to_string());

    engine.register_fn("rgb", |r: Dynamic, g: Dynamic, b: Dynamic| -> R<Color> {
        Ok(Color::rgb(
            channel(&r, "r")?,
            channel(&g, "g")?,
            channel(&b, "b")?,
        ))
    });
    engine.register_fn(
        "rgba",
        |r: Dynamic, g: Dynamic, b: Dynamic, a: Dynamic| -> R<Color> {
            Ok(Color::rgba(
                channel(&r, "r")?,
                channel(&g, "g")?,
                channel(&b, "b")?,
                alpha(&a, "a")?,
            ))
        },
    );
    engine.register_fn("hsl", |h: Dynamic, s: Dynamic, l: Dynamic| -> R<Color> {
        Ok(Color::from_hsl(
            num(&h, "h")?,
            unit_or_percent(&s, "s")?,
            unit_or_percent(&l, "l")?,
            1.0,
        ))
    });
    engine.register_fn(
        "hsla",
        |h: Dynamic, s: Dynamic, l: Dynamic, a: Dynamic| -> R<Color> {
            let a = alpha(&a, "a")? as f64 / 255.0;
            Ok(Color::from_hsl(
                num(&h, "h")?,
                unit_or_percent(&s, "s")?,
                unit_or_percent(&l, "l")?,
                a,
            ))
        },
    );
    engine.register_fn("hex", |s: &str| -> R<Color> {
        Color::parse(s).or_else(err)
    });

    // ---------- shapes ----------
    let c = ctx.clone();
    engine.register_fn(
        "pixel",
        move |x: Dynamic, y: Dynamic, st: Dynamic| -> R<()> {
            let (x, y) = (int(&x, "x")? as i32, int(&y, "y")? as i32);
            let s = style(&st, Role::Fill)?;
            let p = s.fill.or(s.stroke).unwrap();
            let mut c = c.borrow_mut();
            c.buf().push(Span::new(y, x, x + 1));
            c.flush(&p, s.blend);
            Ok(())
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "rect",
        move |x: Dynamic, y: Dynamic, w: Dynamic, h: Dynamic, st: Dynamic| -> R<()> {
            let s = style(&st, Role::Fill)?;
            c.borrow_mut().rect(
                num(&x, "x")?,
                num(&y, "y")?,
                num(&w, "w")?,
                num(&h, "h")?,
                &s,
            );
            Ok(())
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "circle",
        move |x: Dynamic, y: Dynamic, r: Dynamic, st: Dynamic| -> R<()> {
            let s = style(&st, Role::Fill)?;
            let r = num(&r, "r")?;
            c.borrow_mut()
                .ellipse(num(&x, "cx")?, num(&y, "cy")?, r, r, &s);
            Ok(())
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "ellipse",
        move |x: Dynamic, y: Dynamic, rx: Dynamic, ry: Dynamic, st: Dynamic| -> R<()> {
            let s = style(&st, Role::Fill)?;
            c.borrow_mut().ellipse(
                num(&x, "cx")?,
                num(&y, "cy")?,
                num(&rx, "rx")?,
                num(&ry, "ry")?,
                &s,
            );
            Ok(())
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "line",
        move |x0: Dynamic, y0: Dynamic, x1: Dynamic, y1: Dynamic, st: Dynamic| -> R<()> {
            let mut s = style(&st, Role::Stroke)?;
            if s.stroke.is_none() {
                s.stroke = s.fill.take();
            }
            s.fill = None;
            let pts = vec![
                (num(&x0, "x0")?, num(&y0, "y0")?),
                (num(&x1, "x1")?, num(&y1, "y1")?),
            ];
            c.borrow_mut().polygon(&[pts], false, &s);
            Ok(())
        },
    );

    let c = ctx.clone();
    engine.register_fn("polyline", move |pts: Dynamic, st: Dynamic| -> R<()> {
        let s = style(&st, Role::Stroke)?;
        let p = points(&as_array(&pts, "points")?, &s)?;
        c.borrow_mut().polygon(&[p], false, &s);
        Ok(())
    });

    let c = ctx.clone();
    engine.register_fn("polygon", move |pts: Dynamic, st: Dynamic| -> R<()> {
        let s = style(&st, Role::Fill)?;
        let p = points(&as_array(&pts, "points")?, &s)?;
        c.borrow_mut().polygon(&[p], true, &s);
        Ok(())
    });

    let c = ctx.clone();
    engine.register_fn("path", move |d: Dynamic, st: Dynamic| -> R<()> {
        let s = style(&st, Role::Fill)?;
        let d = as_str(&d, "path")?;
        let subs = path::parse(&d, 0.2 / s.scale.abs().max(1e-3)).or_else(err)?;
        let tf = |(x, y): (f64, f64)| (x * s.scale + s.dx, y * s.scale + s.dy);
        let mut cx = c.borrow_mut();
        if let Some(p) = &s.fill {
            let contours: Vec<Vec<(f64, f64)>> = subs
                .iter()
                .map(|sp| sp.pts.iter().copied().map(tf).collect())
                .collect();
            let clip = cx.clip;
            raster::polygon(&contours, s.rule, clip, cx.buf());
            cx.flush(p, s.blend);
        }
        if let Some(p) = &s.stroke {
            let clip = cx.clip;
            for sp in &subs {
                let pts: Vec<(f64, f64)> = sp.pts.iter().copied().map(tf).collect();
                raster::stroke_polyline(&pts, sp.closed, s.width, s.cap, clip, cx.buf());
            }
            cx.flush(p, s.blend);
        }
        Ok(())
    });

    let c = ctx.clone();
    engine.register_fn(
        "flood",
        move |x: Dynamic, y: Dynamic, st: Dynamic| -> R<()> {
            let s = style(&st, Role::Fill)?;
            let p = s.fill.clone().or(s.stroke.clone()).unwrap();
            let (x, y) = (int(&x, "x")? as i32, int(&y, "y")? as i32);
            let mut cx = c.borrow_mut();
            let mut buf = std::mem::take(cx.buf());
            let active = cx.doc.active;
            raster::flood(
                &cx.doc.layers[active].raster,
                x,
                y,
                s.tolerance,
                cx.clip,
                &mut buf,
            );
            *cx.buf() = buf;
            cx.flush(&p, s.blend);
            Ok(())
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "text",
        move |x: Dynamic, y: Dynamic, t: Dynamic, st: Dynamic| -> R<()> {
            let s = style(&st, Role::Fill)?;
            let p = s.fill.clone().or(s.stroke.clone()).unwrap();
            let t = t.to_string();
            let scale = s.scale.round().max(1.0) as i32;
            let mut cx = c.borrow_mut();
            text::spans(
                int(&x, "x")? as i32,
                int(&y, "y")? as i32,
                &t,
                scale,
                cx.buf(),
            );
            cx.flush(&p, s.blend);
            Ok(())
        },
    );
    engine.register_fn("text_width", |t: Dynamic| -> i64 {
        text::measure(&t.to_string(), 1).0 as i64
    });
    engine.register_fn("text_width", |t: Dynamic, s: Dynamic| -> R<i64> {
        Ok(text::measure(&t.to_string(), int(&s, "scale")?.max(1) as i32).0 as i64)
    });

    let c = ctx.clone();
    engine.register_fn(
        "grid",
        move |x: Dynamic, y: Dynamic, rows: Dynamic, pal: Dynamic| -> R<()> {
            grid(
                &mut c.borrow_mut(),
                int(&x, "x")? as i32,
                int(&y, "y")? as i32,
                &rows,
                &pal,
                1,
            )
        },
    );
    let c = ctx.clone();
    engine.register_fn(
        "grid",
        move |x: Dynamic, y: Dynamic, rows: Dynamic, pal: Dynamic, sc: Dynamic| -> R<()> {
            let s = int(&sc, "scale")?.clamp(1, 1024) as i32;
            grid(
                &mut c.borrow_mut(),
                int(&x, "x")? as i32,
                int(&y, "y")? as i32,
                &rows,
                &pal,
                s,
            )
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "copy",
        move |x: Dynamic, y: Dynamic, w: Dynamic, h: Dynamic, dx: Dynamic, dy: Dynamic| -> R<()> {
            let v = [
                int(&x, "x")?,
                int(&y, "y")?,
                int(&w, "w")?,
                int(&h, "h")?,
                int(&dx, "dx")?,
                int(&dy, "dy")?,
            ];
            copy(
                &mut c.borrow_mut(),
                v[0] as i32,
                v[1] as i32,
                v[2] as i32,
                v[3] as i32,
                v[4] as i32,
                v[5] as i32,
                None,
            )
        },
    );
    let c = ctx.clone();
    engine.register_fn(
        "copy",
        move |x: Dynamic,
              y: Dynamic,
              w: Dynamic,
              h: Dynamic,
              dx: Dynamic,
              dy: Dynamic,
              o: Dynamic|
              -> R<()> {
            let v = [
                int(&x, "x")?,
                int(&y, "y")?,
                int(&w, "w")?,
                int(&h, "h")?,
                int(&dx, "dx")?,
                int(&dy, "dy")?,
            ];
            copy(
                &mut c.borrow_mut(),
                v[0] as i32,
                v[1] as i32,
                v[2] as i32,
                v[3] as i32,
                v[4] as i32,
                v[5] as i32,
                Some(&o),
            )
        },
    );

    // ---------- paints ----------
    engine.register_fn(
        "linear",
        |x0: Dynamic, y0: Dynamic, x1: Dynamic, y1: Dynamic, cs: Dynamic| -> R<PaintValue> {
            let g = gradient_stops(&cs, None)?;
            let shape = GradientShape::linear(
                num(&x0, "x0")?,
                num(&y0, "y0")?,
                num(&x1, "x1")?,
                num(&y1, "y1")?,
            );
            Ok(PaintValue(Paint::Gradient(g, shape)))
        },
    );
    engine.register_fn(
        "linear",
        |x0: Dynamic,
         y0: Dynamic,
         x1: Dynamic,
         y1: Dynamic,
         cs: Dynamic,
         o: Dynamic|
         -> R<PaintValue> {
            let g = gradient_stops(&cs, Some(&o))?;
            let shape = GradientShape::linear(
                num(&x0, "x0")?,
                num(&y0, "y0")?,
                num(&x1, "x1")?,
                num(&y1, "y1")?,
            );
            Ok(PaintValue(Paint::Gradient(g, shape)))
        },
    );
    engine.register_fn(
        "radial",
        |cx: Dynamic, cy: Dynamic, r: Dynamic, cs: Dynamic| -> R<PaintValue> {
            let g = gradient_stops(&cs, None)?;
            Ok(PaintValue(Paint::Gradient(
                g,
                GradientShape::radial(num(&cx, "cx")?, num(&cy, "cy")?, num(&r, "r")?),
            )))
        },
    );
    engine.register_fn(
        "radial",
        |cx: Dynamic, cy: Dynamic, r: Dynamic, cs: Dynamic, o: Dynamic| -> R<PaintValue> {
            let g = gradient_stops(&cs, Some(&o))?;
            Ok(PaintValue(Paint::Gradient(
                g,
                GradientShape::radial(num(&cx, "cx")?, num(&cy, "cy")?, num(&r, "r")?),
            )))
        },
    );

    let pattern = |c: &Ctx, x: i64, y: i64, w: i64, h: i64, o: Option<&Dynamic>| -> R<PaintValue> {
        let m = match o {
            Some(o) => opts_map(o, "pattern")?,
            None => Map::new(),
        };
        let src = match opt(&m, "from") {
            Some(n) => {
                let n = as_str(n, "from")?;
                c.doc
                    .layer_index(&n)
                    .map_or_else(|| err(format!("pattern: no layer named {n:?}")), Ok)?
            }
            None => c.doc.active,
        };
        let scale = match opt(&m, "scale") {
            Some(v) => int(v, "scale")?.clamp(1, 256) as u32,
            None => 1,
        };
        let img = c.doc.layers[src]
            .raster
            .read_region(IRect::new(x as i32, y as i32, w as i32, h as i32));
        if img.width == 0 || img.height == 0 {
            return err("pattern: region is empty or outside the canvas");
        }
        Ok(PaintValue(Ctx::image_paint(
            img, x as i32, y as i32, scale, true,
        )))
    };
    let c = ctx.clone();
    engine.register_fn(
        "pattern",
        move |x: Dynamic, y: Dynamic, w: Dynamic, h: Dynamic| -> R<PaintValue> {
            pattern(
                &c.borrow(),
                int(&x, "x")?,
                int(&y, "y")?,
                int(&w, "w")?,
                int(&h, "h")?,
                None,
            )
        },
    );
    let c = ctx.clone();
    engine.register_fn(
        "pattern",
        move |x: Dynamic, y: Dynamic, w: Dynamic, h: Dynamic, o: Dynamic| -> R<PaintValue> {
            pattern(
                &c.borrow(),
                int(&x, "x")?,
                int(&y, "y")?,
                int(&w, "w")?,
                int(&h, "h")?,
                Some(&o),
            )
        },
    );

    // ---------- reading ----------
    let c = ctx.clone();
    engine.register_fn("get", move |x: Dynamic, y: Dynamic| -> R<Color> {
        let mut c = c.borrow_mut();
        Ok(Color::from_px(
            c.layer()
                .raster
                .get(int(&x, "x")? as i32, int(&y, "y")? as i32),
        ))
    });
    let c = ctx.clone();
    engine.register_fn("sample", move |x: Dynamic, y: Dynamic| -> R<Color> {
        let c = c.borrow();
        let img = c
            .doc
            .composite(IRect::new(int(&x, "x")? as i32, int(&y, "y")? as i32, 1, 1));
        Ok(Color::from_px(img.px.first().copied().unwrap_or(0)))
    });
    let c = ctx.clone();
    engine.register_fn("width", move || c.borrow().doc.width as i64);
    let c = ctx.clone();
    engine.register_fn("height", move || c.borrow().doc.height as i64);

    // ---------- layers ----------
    let c = ctx.clone();
    engine.register_fn("layer", move |name: Dynamic| -> R<()> {
        let n = as_str(&name, "layer")?;
        let mut c = c.borrow_mut();
        let i = c.doc.ensure_layer(&n);
        c.doc.active = i;
        Ok(())
    });
    let c = ctx.clone();
    engine.register_fn("layer", move |name: Dynamic, o: Dynamic| -> R<()> {
        let n = as_str(&name, "layer")?;
        let m = opts_map(&o, "layer")?;
        let mut c = c.borrow_mut();
        let i = c.doc.ensure_layer(&n);
        c.doc.active = i;
        for (k, v) in m {
            match k.as_str() {
                "opacity" => c.doc.layers[i].opacity = alpha(&v, "opacity")?,
                "blend" => {
                    c.doc.layers[i].blend = BlendMode::parse(&as_str(&v, "blend")?).or_else(err)?
                }
                "visible" => {
                    c.doc.layers[i].visible =
                        v.as_bool().or_else(|_| err("visible: expected bool"))?
                }
                o => {
                    return err(format!(
                        "unknown layer option {o:?} (valid: opacity, blend, visible)"
                    ));
                }
            }
        }
        Ok(())
    });
    let c = ctx.clone();
    engine.register_fn("layers", move || -> Array {
        c.borrow()
            .doc
            .layers
            .iter()
            .map(|l| Dynamic::from(l.name.clone()))
            .collect()
    });
    let c = ctx.clone();
    engine.register_fn("current_layer", move || -> String {
        let c = c.borrow();
        c.doc.layers[c.doc.active].name.clone()
    });
    let c = ctx.clone();
    engine.register_fn("remove_layer", move |name: Dynamic| -> R<()> {
        c.borrow_mut()
            .doc
            .remove_layer(&as_str(&name, "remove_layer")?)
            .or_else(err)
    });
    let c = ctx.clone();
    engine.register_fn("move_layer", move |name: Dynamic, to: Dynamic| -> R<()> {
        let to = int(&to, "index")?.max(0) as usize;
        c.borrow_mut()
            .doc
            .move_layer(&as_str(&name, "move_layer")?, to)
            .or_else(err)
    });

    let c = ctx.clone();
    engine.register_fn("clear", move || {
        let mut c = c.borrow_mut();
        let clip = c.clip;
        if clip == c.doc.bounds() {
            c.layer().raster.clear();
        } else {
            raster::rect(
                clip.x0 as f64,
                clip.y0 as f64,
                clip.w() as f64,
                clip.h() as f64,
                clip,
                c.buf(),
            );
            c.flush(&Paint::Solid(0), BlendMode::Replace);
        }
    });
    // `clear(color)`: Rhai ships a native `clear` for strings, so the ImmutableString variant is
    // registered too (engine-registered functions take precedence over packages).
    let c = ctx.clone();
    engine.register_fn("clear", move |col: Dynamic| -> R<()> {
        clear_with(&mut c.borrow_mut(), &col)
    });
    let c = ctx.clone();
    engine.register_fn("clear", move |col: rhai::ImmutableString| -> R<()> {
        clear_with(&mut c.borrow_mut(), &Dynamic::from(col))
    });
    let c = ctx.clone();
    engine.register_fn(
        "clip",
        move |x: Dynamic, y: Dynamic, w: Dynamic, h: Dynamic| -> R<()> {
            let r = IRect::new(
                int(&x, "x")? as i32,
                int(&y, "y")? as i32,
                int(&w, "w")? as i32,
                int(&h, "h")? as i32,
            );
            let mut c = c.borrow_mut();
            c.clip = r.intersect(&c.doc.bounds());
            Ok(())
        },
    );
    let c = ctx.clone();
    engine.register_fn("unclip", move || {
        let mut c = c.borrow_mut();
        c.clip = c.doc.bounds();
    });

    // ---------- utilities ----------
    let c = ctx.clone();
    engine.register_fn("seed", move |n: Dynamic| -> R<()> {
        let n = int(&n, "seed")? as u64;
        c.borrow_mut().rng = n.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        Ok(())
    });
    let c = ctx.clone();
    engine.register_fn("rand", move || -> f64 {
        (c.borrow_mut().rand_u64() >> 11) as f64 / (1u64 << 53) as f64
    });
    let c = ctx.clone();
    engine.register_fn("rand", move |a: Dynamic, b: Dynamic| -> R<i64> {
        let (a, b) = (int(&a, "a")?, int(&b, "b")?);
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let span = (hi - lo) as u64 + 1;
        Ok(lo + (c.borrow_mut().rand_u64() % span) as i64)
    });
    let c = ctx.clone();
    engine.register_fn("noise", move |x: Dynamic, y: Dynamic| -> R<f64> {
        let seed = c.borrow().rng;
        Ok(value_noise(seed, num(&x, "x")?, num(&y, "y")?))
    });
    engine.register_fn(
        "clamp",
        |v: Dynamic, lo: Dynamic, hi: Dynamic| -> R<Dynamic> {
            if let (Ok(v), Ok(lo), Ok(hi)) = (v.as_int(), lo.as_int(), hi.as_int()) {
                return Ok(Dynamic::from(v.clamp(lo.min(hi), hi.max(lo))));
            }
            let (v, lo, hi) = (num(&v, "v")?, num(&lo, "lo")?, num(&hi, "hi")?);
            Ok(Dynamic::from(v.clamp(lo.min(hi), hi.max(lo))))
        },
    );
    engine.register_fn("lerp", |a: Dynamic, b: Dynamic, t: Dynamic| -> R<f64> {
        let (a, b, t) = (num(&a, "a")?, num(&b, "b")?, num(&t, "t")?);
        Ok(a + (b - a) * t)
    });
    engine.register_fn(
        "dist",
        |x0: Dynamic, y0: Dynamic, x1: Dynamic, y1: Dynamic| -> R<f64> {
            Ok((num(&x1, "x1")? - num(&x0, "x0")?).hypot(num(&y1, "y1")? - num(&y0, "y0")?))
        },
    );
    engine.register_fn("atan2", |y: Dynamic, x: Dynamic| -> R<f64> {
        Ok(num(&y, "y")?.atan2(num(&x, "x")?))
    });
    engine.register_fn("sin", |x: i64| (x as f64).sin());
    engine.register_fn("cos", |x: i64| (x as f64).cos());
    engine.register_fn("tan", |x: i64| (x as f64).tan());
    engine.register_fn("sqrt", |x: i64| (x as f64).sqrt());
    engine.register_fn("exp", |x: i64| (x as f64).exp());
    engine.register_fn("ln", |x: i64| (x as f64).ln());
    engine.register_fn("floor", |x: i64| x as f64);
    engine.register_fn("ceil", |x: i64| x as f64);
    engine.register_fn("round", |x: i64| x as f64);
    engine.register_fn("to_float", |x: f64| x);
}
