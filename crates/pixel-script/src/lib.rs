//! # pixel-script
//!
//! The agents' brush: a Rhai script (variables, loops, functions, math) that calls native
//! `pixel-core` primitives. One call can paint thousands of shapes, so round trips with the
//! model stop being the dominant cost.
//!
//! Every run is atomic: if the script fails, the original document is left untouched.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pixel_core::raster::{self, Cap, FillRule};
use pixel_core::span::normalize;
use pixel_core::*;
use rhai::{Array, Dynamic, Engine, EvalAltResult, Map, Scope};

mod api;

pub use api::REFERENCE;

type R<T> = Result<T, Box<EvalAltResult>>;

#[derive(Clone, Debug)]
pub struct RunOptions {
    pub max_operations: u64,
    pub timeout: Duration,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            max_operations: 200_000_000,
            timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RunReport {
    pub logs: Vec<String>,
    pub operations: u64,
    pub elapsed: Duration,
}

#[derive(Clone, Debug)]
pub struct ScriptError {
    pub message: String,
    pub logs: Vec<String>,
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Color value in scripts.
pub type ScriptColor = Color;

/// Non-solid paint (gradient or pattern) in scripts.
#[derive(Clone)]
pub struct PaintValue(pub Paint);

pub(crate) struct Ctx {
    pub doc: Document,
    pub clip: IRect,
    pub rng: u64,
    pub logs: Vec<String>,
    buf: Vec<Span>,
}

#[derive(Clone)]
pub(crate) struct Style {
    pub fill: Option<Paint>,
    pub stroke: Option<Paint>,
    pub width: f64,
    pub blend: BlendMode,
    pub rule: FillRule,
    pub cap: Cap,
    pub tolerance: u8,
    pub scale: f64,
    pub dx: f64,
    pub dy: f64,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            fill: None,
            stroke: None,
            width: 1.0,
            blend: BlendMode::Over,
            rule: FillRule::NonZero,
            cap: Cap::Round,
            tolerance: 0,
            scale: 1.0,
            dx: 0.0,
            dy: 0.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Fill,
    Stroke,
}

pub(crate) fn err<T>(msg: impl Into<String>) -> R<T> {
    Err(msg.into().into())
}

pub(crate) fn num(d: &Dynamic, what: &str) -> R<f64> {
    if let Ok(i) = d.as_int() {
        return Ok(i as f64);
    }
    if let Ok(f) = d.as_float() {
        return Ok(f);
    }
    err(format!("{what}: expected a number, got {}", d.type_name()))
}

pub(crate) fn int(d: &Dynamic, what: &str) -> R<i64> {
    Ok(num(d, what)?.round() as i64)
}

pub(crate) fn color(d: &Dynamic) -> R<Color> {
    if let Some(c) = d.clone().try_cast::<Color>() {
        return Ok(c);
    }
    if d.is_string() {
        return Color::parse(&d.clone().into_string().unwrap_or_default()).or_else(err);
    }
    err(format!(
        "expected a color (\"#rrggbb\", rgb(), hsl()), got {}",
        d.type_name()
    ))
}

pub(crate) fn paint(d: &Dynamic) -> R<Paint> {
    if let Some(p) = d.clone().try_cast::<PaintValue>() {
        return Ok(p.0);
    }
    Ok(Paint::solid(color(d)?))
}

/// 0..1 for floats, 0..255 for integers.
pub(crate) fn alpha(d: &Dynamic, what: &str) -> R<u8> {
    if let Ok(i) = d.as_int() {
        return Ok(i.clamp(0, 255) as u8);
    }
    Ok((num(d, what)?.clamp(0.0, 1.0) * 255.0).round() as u8)
}

pub(crate) fn style(d: &Dynamic, role: Role) -> R<Style> {
    let mut s = Style::default();
    if let Some(m) = d.clone().try_cast::<Map>() {
        for (k, v) in m {
            match k.as_str() {
                "fill" => s.fill = Some(paint(&v)?),
                "stroke" => s.stroke = Some(paint(&v)?),
                "color" => match role {
                    Role::Fill => s.fill = Some(paint(&v)?),
                    Role::Stroke => s.stroke = Some(paint(&v)?),
                },
                "width" => s.width = num(&v, "width")?,
                "blend" => {
                    s.blend = BlendMode::parse(&v.into_string().unwrap_or_default()).or_else(err)?
                }
                "rule" => {
                    s.rule = match v.into_string().unwrap_or_default().as_str() {
                        "nonzero" => FillRule::NonZero,
                        "evenodd" => FillRule::EvenOdd,
                        o => {
                            return err(format!(
                                "rule must be \"nonzero\" or \"evenodd\", got {o:?}"
                            ));
                        }
                    }
                }
                "cap" => {
                    s.cap = match v.into_string().unwrap_or_default().as_str() {
                        "round" => Cap::Round,
                        "square" => Cap::Square,
                        "butt" => Cap::Butt,
                        o => return err(format!("cap must be round, square or butt, got {o:?}")),
                    }
                }
                "tolerance" => s.tolerance = num(&v, "tolerance")?.clamp(0.0, 255.0) as u8,
                "scale" => s.scale = num(&v, "scale")?,
                "dx" => s.dx = num(&v, "dx")?,
                "dy" => s.dy = num(&v, "dy")?,
                other => {
                    return err(format!(
                        "unknown style key {other:?} (valid: fill, stroke, color, width, blend, rule, cap, tolerance, scale, dx, dy)"
                    ));
                }
            }
        }
        if s.fill.is_none() && s.stroke.is_none() {
            return err("style map needs at least `fill`, `stroke` or `color`");
        }
    } else {
        let p = paint(d)?;
        match role {
            Role::Fill => s.fill = Some(p),
            Role::Stroke => s.stroke = Some(p),
        }
    }
    Ok(s)
}

pub(crate) fn points(a: &Array, s: &Style) -> R<Vec<(f64, f64)>> {
    let mut out = Vec::with_capacity(a.len());
    let tf = |x: f64, y: f64| (x * s.scale + s.dx, y * s.scale + s.dy);
    if a.iter().all(|v| v.is_array()) {
        for v in a {
            let p = v.clone().cast::<Array>();
            if p.len() != 2 {
                return err("points must be [[x, y], ...] or [x0, y0, x1, y1, ...]");
            }
            out.push(tf(num(&p[0], "x")?, num(&p[1], "y")?));
        }
    } else {
        if !a.len().is_multiple_of(2) {
            return err("flat point list must have an even number of values");
        }
        for c in a.chunks(2) {
            out.push(tf(num(&c[0], "x")?, num(&c[1], "y")?));
        }
    }
    Ok(out)
}

impl Ctx {
    fn new(doc: Document) -> Self {
        let clip = doc.bounds();
        Self {
            doc,
            clip,
            rng: 0x9E37_79B9_7F4A_7C15,
            logs: Vec::new(),
            buf: Vec::new(),
        }
    }

    pub fn layer(&mut self) -> &mut Layer {
        let i = self.doc.active.min(self.doc.layers.len() - 1);
        &mut self.doc.layers[i]
    }

    /// Normalizes the span buffer and paints it on the active layer.
    pub fn flush(&mut self, paint: &Paint, mode: BlendMode) {
        let mut buf = std::mem::take(&mut self.buf);
        normalize(&mut buf, self.clip);
        self.layer().raster.paint_spans(&buf, paint, mode);
        buf.clear();
        self.buf = buf;
    }

    pub fn buf(&mut self) -> &mut Vec<Span> {
        &mut self.buf
    }

    pub fn fill_stroke(
        &mut self,
        s: &Style,
        fill: impl FnOnce(&mut Vec<Span>, IRect),
        stroke: impl FnOnce(&mut Vec<Span>, IRect),
    ) {
        let clip = self.clip;
        if let Some(p) = &s.fill {
            fill(&mut self.buf, clip);
            self.flush(p, s.blend);
        }
        if let Some(p) = &s.stroke {
            stroke(&mut self.buf, clip);
            self.flush(p, s.blend);
        }
    }

    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, s: &Style) {
        let sw = s.width.round().max(1.0) as i32;
        self.fill_stroke(
            s,
            |b, c| raster::rect(x, y, w, h, c, b),
            |b, c| raster::rect_stroke(x, y, w, h, sw, c, b),
        );
    }

    pub fn ellipse(&mut self, cx: f64, cy: f64, rx: f64, ry: f64, s: &Style) {
        let sw = s.width.round().max(1.0) as i32;
        let clip = self.clip;
        let rows = raster::ellipse_rows(cx, cy, rx, ry, clip.y0 - sw - 1, clip.y1 + sw + 1);
        self.fill_stroke(
            s,
            |b, _| b.extend(rows.iter().filter(|r| r.x0 < r.x1)),
            |b, _| raster::inner_stroke(&rows, sw, b),
        );
    }

    pub fn polygon(&mut self, contours: &[Vec<(f64, f64)>], closed: bool, s: &Style) {
        let (rule, width, cap) = (s.rule, s.width, s.cap);
        self.fill_stroke(
            s,
            |b, c| raster::polygon(contours, rule, c, b),
            |b, c| {
                for pts in contours {
                    raster::stroke_polyline(pts, closed, width, cap, c, b);
                }
            },
        );
    }

    pub fn rand_u64(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn image_paint(img: Image, ox: i32, oy: i32, scale: u32, repeat: bool) -> Paint {
        Paint::Image {
            img: Arc::new(img),
            ox,
            oy,
            scale: scale.max(1),
            repeat,
        }
    }
}

/// Runs a script on a copy of the document and returns the result.
/// The copy is cheap (copy-on-write); the original is never touched.
pub fn run(
    doc: &Document,
    script: &str,
    opts: &RunOptions,
) -> Result<(Document, RunReport), ScriptError> {
    let start = Instant::now();
    let ctx = Rc::new(RefCell::new(Ctx::new(doc.clone())));
    let ops = Rc::new(Cell::new(0u64));
    let logs = Rc::new(RefCell::new(Vec::<String>::new()));
    let result = {
        let mut engine = Engine::new();
        engine.set_max_operations(opts.max_operations);
        engine.set_max_call_levels(64);
        engine.set_max_expr_depths(128, 64);
        engine.set_max_string_size(1 << 20);
        engine.set_max_array_size(4 << 20);
        engine.set_max_map_size(1 << 16);
        {
            let logs = logs.clone();
            engine.on_print(move |s| logs.borrow_mut().push(s.to_string()));
        }
        {
            let logs = logs.clone();
            engine.on_debug(move |s, _, pos| logs.borrow_mut().push(format!("[{pos}] {s}")));
        }
        {
            let ops = ops.clone();
            let timeout = opts.timeout;
            engine.on_progress(move |n| {
                ops.set(n);
                if n % 4096 == 0 && start.elapsed() > timeout {
                    Some(Dynamic::from(format!("timeout after {:?}", timeout)))
                } else {
                    None
                }
            });
        }
        api::register(&mut engine, ctx.clone());
        let mut scope = Scope::new();
        scope.push_constant("W", doc.width as i64);
        scope.push_constant("H", doc.height as i64);
        scope.push_constant("PI", std::f64::consts::PI);
        scope.push_constant("TAU", std::f64::consts::TAU);
        engine.run_with_scope(&mut scope, script)
    };
    let logs = std::mem::take(&mut *logs.borrow_mut());
    match result {
        Ok(()) => {
            let mut c = ctx.borrow_mut();
            let mut out = std::mem::replace(&mut c.doc, Document::new(1, 1));
            for l in &mut out.layers {
                match doc.layer_index(&l.name) {
                    Some(i) => l.raster.compact_changed(&doc.layers[i].raster),
                    None => l.raster.compact(),
                }
            }
            let mut all_logs = logs;
            all_logs.append(&mut c.logs);
            Ok((
                out,
                RunReport {
                    logs: all_logs,
                    operations: ops.get(),
                    elapsed: start.elapsed(),
                },
            ))
        }
        Err(e) => Err(ScriptError {
            message: describe_error(&e),
            logs,
        }),
    }
}

fn describe_error(e: &EvalAltResult) -> String {
    match e {
        EvalAltResult::ErrorTooManyOperations(_) => {
            format!(
                "{e} — the script exceeded the operation budget; use bigger primitives (rect/path/grid/gradients) instead of per-pixel loops"
            )
        }
        EvalAltResult::ErrorTerminated(v, pos) => format!("script terminated: {v} ({pos})"),
        _ => e.to_string(),
    }
}

#[cfg(test)]
mod tests;
