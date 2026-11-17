//! The 4 tools: `canvas`, `draw`, `view`, `history`. Independent of the MCP transport.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use pixel_core::view::{self, ViewOpts};
use pixel_core::*;
use pixel_script::RunOptions;
use serde_json::Value;

pub enum Content {
    Text(String),
    Png(Vec<u8>),
}

pub type ToolResult = Result<Vec<Content>, String>;

pub struct Canvas {
    pub doc: Document,
    pub history: History,
    /// The canvas as the agent left it at the end of its last call.
    agent_view: Option<Document>,
    /// The person's changes since then.
    user_log: Vec<String>,
}

impl Canvas {
    fn new(doc: Document) -> Self {
        Self {
            agent_view: None,
            doc,
            history: History::default(),
            user_log: Vec::new(),
        }
    }

    /// Records a change with its author; the person's changes are queued for the agent.
    fn record(&mut self, before: Document, step: Step) {
        if step.author == Author::User {
            self.user_log.push(step.label.clone());
        }
        self.history.record(before, step);
    }
}

pub struct Workspace {
    pub canvases: BTreeMap<String, Canvas>,
    pub current: Option<String>,
    pub live: Option<PathBuf>,
    pub run_opts: RunOptions,
}

// ---------- argument helpers ----------

fn s<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str)
}

fn i(a: &Value, k: &str) -> Result<Option<i64>, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .or_else(|| v.as_f64().map(|f| f.round() as i64))
            .map(Some)
            .ok_or_else(|| format!("`{k}` must be a number")),
    }
}

fn b(a: &Value, k: &str) -> Option<bool> {
    a.get(k).and_then(Value::as_bool)
}

/// Who is calling. The pixel-studio plugin marks what the person does with `_author: "user"`.
fn author(a: &Value) -> Author {
    if s(a, "_author") == Some("user") {
        Author::User
    } else {
        Author::Agent
    }
}

fn step(a: &Value, default_label: impl Into<String>) -> Step {
    Step {
        author: author(a),
        label: s(a, "_label").map_or_else(|| default_label.into(), str::to_string),
    }
}

/// First meaningful line of a script, used to label the step.
fn script_label(script: &str) -> String {
    let line = script
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("//"))
        .unwrap_or("");
    let mut out: String = line.chars().take(48).collect();
    if line.chars().count() > 48 {
        out.push('…');
    }
    format!("draw: {out}")
}

/// `undo x1, undo x2, draw: …` → `undo x2, draw:`.
fn summarize(labels: &[String]) -> String {
    let mut groups: Vec<(String, usize)> = Vec::new();
    for l in labels {
        let key = l.split_whitespace().next().unwrap_or(l).to_string();
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some(g) => g.1 += 1,
            None => groups.push((key, 1)),
        }
    }
    groups
        .iter()
        .map(|(k, n)| {
            if *n > 1 {
                format!("{k} x{n}")
            } else {
                k.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn dims(w: Option<i64>, h: Option<i64>) -> Result<(u32, u32), String> {
    let (w, h) = (w.unwrap_or(256), h.unwrap_or(256));
    if w < 1 || h < 1 || w > MAX_SIDE as i64 || h > MAX_SIDE as i64 {
        return Err(format!("width/height must be between 1 and {MAX_SIDE}"));
    }
    Ok((w as u32, h as u32))
}

fn ms(t: Instant) -> String {
    format!("{:.2} ms", t.elapsed().as_secs_f64() * 1000.0)
}

/// View parameters shared by `view`, `draw` and `history`.
struct ViewArgs {
    region: Option<IRect>,
    zoom: Option<u32>,
    grid: Option<bool>,
    mode: String,
    layer: Option<String>,
    max: u32,
    rulers: bool,
}

impl ViewArgs {
    fn parse(a: &Value) -> Result<Self, String> {
        let region = match (i(a, "x")?, i(a, "y")?, i(a, "w")?, i(a, "h")?) {
            (None, None, None, None) => None,
            (x, y, w, h) => Some(IRect::new(
                x.unwrap_or(0) as i32,
                y.unwrap_or(0) as i32,
                w.unwrap_or(i32::MAX as i64 / 4) as i32,
                h.unwrap_or(i32::MAX as i64 / 4) as i32,
            )),
        };
        Ok(Self {
            region,
            zoom: i(a, "scale")?.map(|z| z.clamp(1, 64) as u32),
            grid: b(a, "grid"),
            mode: s(a, "mode").unwrap_or("image").to_string(),
            layer: s(a, "layer").map(str::to_string),
            max: i(a, "max")?.map_or(1024, |m| m.clamp(64, 4096) as u32),
            rulers: b(a, "rulers").unwrap_or(true),
        })
    }
}

fn render(doc: &Document, va: &ViewArgs) -> ToolResult {
    let region = va.region.unwrap_or(doc.bounds()).intersect(&doc.bounds());
    if region.is_empty() {
        return Err(format!(
            "view region is outside the canvas ({}x{})",
            doc.width, doc.height
        ));
    }
    let layer = match &va.layer {
        Some(n) => Some(
            doc.layer_index(n)
                .ok_or_else(|| format!("no layer named {n:?}"))?,
        ),
        None => None,
    };
    let fetch = |r: IRect| match layer {
        Some(i) => doc.layers[i].raster.read_region(r),
        None => doc.composite(r),
    };
    match va.mode.as_str() {
        "image" => {
            let t = Instant::now();
            let mut opts = ViewOpts {
                zoom: va.zoom,
                max_side: va.max,
                grid: va.grid,
                rulers: va.rulers,
                pre_shrink: 1,
            };
            opts.pre_shrink = view::shrink_for(region, &opts);
            let img = doc.composite_downscaled(region, opts.pre_shrink, layer);
            let (out, meta) = view::render(&img, region, &opts);
            let png = out.encode_png(true)?;
            let text = format!("{} (rendered in {})", meta.describe(region), ms(t));
            Ok(vec![Content::Text(text), Content::Png(png)])
        }
        "pixels" => {
            if region.area() > 128 * 128 {
                return Err(format!(
                    "mode \"pixels\" is limited to 16384 pixels (e.g. 128x128); requested {region}. Narrow x/y/w/h."
                ));
            }
            Ok(vec![Content::Text(view::pixels_text(
                &fetch(region),
                region,
            )?)])
        }
        "stats" => {
            let mut st = view::Stats::new(region);
            let mut y = region.y0;
            while y < region.y1 {
                let band = IRect {
                    x0: region.x0,
                    y0: y,
                    x1: region.x1,
                    y1: (y + 256).min(region.y1),
                };
                st.add(&fetch(band), band.x0, band.y0);
                y = band.y1;
            }
            Ok(vec![Content::Text(st.text())])
        }
        m => Err(format!(
            "unknown view mode {m:?} (use image, pixels or stats)"
        )),
    }
}

impl Workspace {
    pub fn new(live: Option<PathBuf>) -> Self {
        Self {
            canvases: BTreeMap::new(),
            current: None,
            live,
            run_opts: RunOptions::default(),
        }
    }

    pub fn call(&mut self, tool: &str, a: &Value) -> ToolResult {
        let who = author(a);
        let creates =
            tool == "canvas" && matches!(s(a, "action"), Some("create" | "open" | "list"));
        let target = if creates { None } else { self.resolve(a).ok() };
        let brief = match (&target, who) {
            (Some(n), Author::Agent) => self.briefing(n),
            _ => None,
        };
        let res = match tool {
            "canvas" => self.canvas(a),
            "draw" => self.draw(a),
            "view" => self.view(a),
            "history" => self.history(a),
            t => Err(format!("unknown tool {t:?}")),
        };
        if who == Author::Agent {
            let seen = target.or_else(|| if creates { self.current.clone() } else { None });
            if let Some(n) = seen {
                self.agent_saw(&n);
            }
        }
        match (res, brief) {
            (Ok(mut v), Some(b)) => {
                v.insert(0, Content::Text(b));
                Ok(v)
            }
            (Err(e), Some(b)) => Err(format!("{b}\n\n{e}")),
            (r, None) => r,
        }
    }

    /// What the agent needs to know about the person before going on: their changes.
    fn briefing(&self, name: &str) -> Option<String> {
        let c = self.canvases.get(name)?;
        let mut out: Vec<String> = Vec::new();
        if !c.user_log.is_empty() {
            let mut t = format!(
                "[pixel] The person edited canvas {name:?} since your last call ({}).",
                summarize(&c.user_log)
            );
            if let Some(d) = c.agent_view.as_ref().map(|v| c.doc.diff(v))
                && let Some(bb) = d.bbox
            {
                let _ = write!(t, " {} px changed within {bb}.", d.pixels);
            }
            t.push_str(" Look at that area before drawing over it, and keep the person's edits unless asked otherwise.");
            out.push(t);
        }
        (!out.is_empty()).then(|| out.join("\n"))
    }

    fn agent_saw(&mut self, name: &str) {
        if let Some(c) = self.canvases.get_mut(name) {
            c.agent_view = Some(c.doc.clone());
            c.user_log.clear();
        }
    }

    fn resolve(&self, a: &Value) -> Result<String, String> {
        if let Some(n) = s(a, "canvas").or_else(|| s(a, "name")) {
            if self.canvases.contains_key(n) {
                return Ok(n.to_string());
            }
            return Err(format!(
                "no canvas named {n:?}; open canvases: {:?}",
                self.canvases.keys().collect::<Vec<_>>()
            ));
        }
        self.current
            .clone()
            .filter(|n| self.canvases.contains_key(n))
            .ok_or_else(|| {
                "no canvas yet: call canvas {action: \"create\", width, height} first".to_string()
            })
    }

    fn insert(&mut self, name: &str, doc: Document) {
        self.canvases.insert(name.to_string(), Canvas::new(doc));
        self.current = Some(name.to_string());
        self.live_write(name);
    }

    /// Writes the canvas PNG into the "live" directory in the background (for `pixel watch`).
    fn live_write(&self, name: &str) {
        let (Some(dir), Some(c)) = (&self.live, self.canvases.get(name)) else {
            return;
        };
        let doc = c.doc.clone();
        let path = dir.join(format!("{name}.png"));
        std::thread::spawn(move || {
            let side = doc.width.max(doc.height);
            let img = doc.composite_downscaled(doc.bounds(), side.div_ceil(2048), None);
            if let Ok(png) = img.encode_png(true) {
                let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
                let tmp = path.with_extension("png.tmp");
                if std::fs::write(&tmp, png).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
            }
        });
    }

    fn info(&self, name: &str) -> String {
        let c = &self.canvases[name];
        let d = &c.doc;
        let (u, r) = c.history.depth();
        let cps: Vec<&String> = c.history.checkpoints.keys().collect();
        let mut t = format!(
            "canvas {name:?} {}x{} · active layer {:?} · history {u} undo / {r} redo · checkpoints {cps:?}\nlayers (bottom → top):\n",
            d.width, d.height, d.layers[d.active].name
        );
        for (idx, l) in d.layers.iter().enumerate() {
            let bbox = l
                .raster
                .bbox()
                .map_or("empty".to_string(), |b| b.to_string());
            let _ = writeln!(
                t,
                "  {idx}: {:?} {} opacity {:.2} blend {} · content {bbox}",
                l.name,
                if l.visible { "visible" } else { "hidden" },
                l.opacity as f64 / 255.0,
                l.blend.name()
            );
        }
        t.trim_end().to_string()
    }

    // ---------- canvas ----------
    fn canvas(&mut self, a: &Value) -> ToolResult {
        let action = s(a, "action").ok_or("`action` is required")?;
        match action {
            "create" => {
                let name = s(a, "name").unwrap_or("main");
                if self.canvases.contains_key(name) && !b(a, "replace").unwrap_or(false) {
                    return Err(format!(
                        "canvas {name:?} already exists (pass replace: true to overwrite it)"
                    ));
                }
                let (w, h) = dims(i(a, "width")?, i(a, "height")?)?;
                let mut doc = Document::new(w, h);
                if let Some(bg) = s(a, "background") {
                    doc.layers[0].raster.fill_all(Color::parse(bg)?.to_px());
                }
                self.insert(name, doc);
                Ok(vec![Content::Text(format!("created\n{}", self.info(name)))])
            }
            "open" => {
                let path = s(a, "path").ok_or("`path` is required")?;
                let img =
                    Image::decode_png(&std::fs::read(path).map_err(|e| format!("{path}: {e}"))?)?;
                let stem = Path::new(path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("image");
                let name = s(a, "name").unwrap_or(stem).to_string();
                dims(Some(img.width as i64), Some(img.height as i64))?;
                let mut doc = Document::new(img.width, img.height);
                for y in 0..img.height {
                    let row = &img.px[(y * img.width) as usize..((y + 1) * img.width) as usize];
                    doc.layers[0].raster.write_row(y as i32, 0, row);
                }
                self.insert(&name, doc);
                Ok(vec![Content::Text(format!(
                    "opened {path}\n{}",
                    self.info(&name)
                ))])
            }
            "import" => {
                let name = self.resolve(a)?;
                let path = s(a, "path").ok_or("`path` is required")?;
                let img =
                    Image::decode_png(&std::fs::read(path).map_err(|e| format!("{path}: {e}"))?)?;
                let stem = Path::new(path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("image");
                let layer = s(a, "layer").unwrap_or(stem).to_string();
                let (x, y) = (
                    i(a, "x")?.unwrap_or(0) as i32,
                    i(a, "y")?.unwrap_or(0) as i32,
                );
                let c = self.canvases.get_mut(&name).unwrap();
                let before = c.doc.clone();
                c.record(before, step(a, format!("import {stem}")));
                let li = c.doc.ensure_layer(&layer);
                for row in 0..img.height {
                    let data =
                        &img.px[(row * img.width) as usize..((row + 1) * img.width) as usize];
                    c.doc.layers[li].raster.write_row(y + row as i32, x, data);
                }
                self.live_write(&name);
                Ok(vec![Content::Text(format!(
                    "imported {path} into layer {layer:?} at ({x},{y})\n{}",
                    self.info(&name)
                ))])
            }
            "save" => {
                let name = self.resolve(a)?;
                let path = PathBuf::from(
                    s(a, "path").ok_or("`path` is required (e.g. \"out/sprite.png\")")?,
                );
                let t = Instant::now();
                let c = &self.canvases[&name];
                if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                let scale = i(a, "scale")?.unwrap_or(1).clamp(1, 64) as u32;
                let file =
                    std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                c.doc
                    .write_png(std::io::BufWriter::new(file), scale, false)?;
                let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let mut msg = format!(
                    "saved {} ({bytes} bytes, x{scale}) in {}",
                    path.display(),
                    ms(t)
                );
                if b(a, "layers").unwrap_or(false) {
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("canvas");
                    for l in &c.doc.layers {
                        let p = path.with_file_name(format!("{stem}.{}.png", l.name));
                        std::fs::write(
                            &p,
                            l.raster
                                .read_region(c.doc.bounds())
                                .upscale(scale)
                                .encode_png(false)?,
                        )
                        .map_err(|e| e.to_string())?;
                        let _ = write!(msg, "\n  layer {:?} → {}", l.name, p.display());
                    }
                }
                Ok(vec![Content::Text(msg)])
            }
            "export" => {
                let name = self.resolve(a)?;
                let path = PathBuf::from(
                    s(a, "path").map_or_else(|| format!("{name}.html"), str::to_string),
                );
                if path.extension().is_none_or(|x| x != "html") {
                    return Err("export writes an .html page: use a path ending in .html (for PNG use action \"save\")".into());
                }
                let t = Instant::now();
                let c = &self.canvases[&name];
                let past: &[(Document, Step)] = if b(a, "history").unwrap_or(true) {
                    c.history.entries()
                } else {
                    &[]
                };
                let page = crate::share::build(
                    &name,
                    s(a, "title"),
                    &c.doc,
                    past,
                    b(a, "artifact").unwrap_or(false),
                )?;
                if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                std::fs::write(&path, &page.html)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                let abs = std::fs::canonicalize(&path).unwrap_or(path);
                let mut msg = format!(
                    "exported {} ({} KB) in {}: self-contained page with zoom, pixel inspector, palette",
                    abs.display(),
                    page.html.len() / 1024,
                    ms(t)
                );
                if page.frames > 1 {
                    let _ = write!(
                        msg,
                        " and a timeline of {} of {} steps",
                        page.frames, page.steps
                    );
                }
                if page.scale > 1 {
                    let _ = write!(msg, "; image downscaled 1/{} to fit the page", page.scale);
                }
                msg.push_str(". Open it in a browser or share the file; it works offline.");
                Ok(vec![Content::Text(msg)])
            }
            "pending" => {
                let name = self.resolve(a)?;
                Ok(vec![Content::Text(
                    self.briefing(&name).unwrap_or_default(),
                )])
            }
            "info" => {
                let name = self.resolve(a)?;
                Ok(vec![Content::Text(self.info(&name))])
            }
            "list" => {
                if self.canvases.is_empty() {
                    return Ok(vec![Content::Text("no canvases open".into())]);
                }
                let mut t = String::new();
                for (n, c) in &self.canvases {
                    let mark = if self.current.as_deref() == Some(n) {
                        " (current)"
                    } else {
                        ""
                    };
                    let _ = writeln!(
                        t,
                        "{n:?} {}x{} · {} layers{mark}",
                        c.doc.width,
                        c.doc.height,
                        c.doc.layers.len()
                    );
                }
                Ok(vec![Content::Text(t.trim_end().to_string())])
            }
            "use" => {
                let name = self.resolve(a)?;
                self.current = Some(name.clone());
                Ok(vec![Content::Text(format!("current canvas: {name:?}"))])
            }
            "resize" => {
                let name = self.resolve(a)?;
                let c = self.canvases.get_mut(&name).unwrap();
                let (w, h) = dims(
                    i(a, "width")?.or(Some(c.doc.width as i64)),
                    i(a, "height")?.or(Some(c.doc.height as i64)),
                )?;
                let before = c.doc.clone();
                c.record(before, step(a, format!("resize {w}x{h}")));
                c.doc.resize(
                    w,
                    h,
                    i(a, "dx")?.unwrap_or(0) as i32,
                    i(a, "dy")?.unwrap_or(0) as i32,
                );
                self.live_write(&name);
                Ok(vec![Content::Text(format!(
                    "resized\n{}",
                    self.info(&name)
                ))])
            }
            "close" => {
                let name = self.resolve(a)?;
                self.canvases.remove(&name);
                if self.current.as_deref() == Some(&name) {
                    self.current = self.canvases.keys().next().cloned();
                }
                Ok(vec![Content::Text(format!("closed {name:?}"))])
            }
            other => Err(format!(
                "unknown canvas action {other:?} (create, open, import, save, export, info, list, use, resize, close)"
            )),
        }
    }

    // ---------- draw ----------
    fn draw(&mut self, a: &Value) -> ToolResult {
        let name = self.resolve(a)?;
        let script = s(a, "script").ok_or("`script` is required")?;
        let t = Instant::now();
        let c = self.canvases.get_mut(&name).unwrap();
        let (doc, report) = match pixel_script::run(&c.doc, script, &self.run_opts) {
            Ok(v) => v,
            Err(e) => {
                let mut m = format!("script error (canvas unchanged): {}", e.message);
                if !e.logs.is_empty() {
                    let _ = write!(m, "\noutput before the error:\n{}", e.logs.join("\n"));
                }
                return Err(m);
            }
        };
        let diff = doc.diff(&c.doc);
        let before = std::mem::replace(&mut c.doc, doc);
        if diff.pixels > 0
            || before.layers.len() != c.doc.layers.len()
            || before.active != c.doc.active
        {
            c.record(before, step(a, script_label(script)));
        }
        let mut msg = format!(
            "ok in {} · {} script ops · active layer {:?}\n",
            ms(t),
            report.operations,
            c.doc.layers[c.doc.active].name
        );
        match diff.bbox {
            Some(bb) => {
                let _ = write!(msg, "changed {} px within {bb}", diff.pixels);
            }
            None => msg.push_str("no pixels changed"),
        }
        if !report.logs.is_empty() {
            let _ = write!(msg, "\noutput:\n{}", report.logs.join("\n"));
        }
        let mut out = vec![Content::Text(msg)];
        let view_arg = a.get("view").cloned().unwrap_or(Value::Null);
        let va = match &view_arg {
            Value::Bool(true) => Some(ViewArgs::parse(&Value::Null)?),
            Value::String(v) if v == "changed" => {
                let mut va = ViewArgs::parse(&Value::Null)?;
                va.region = diff.bbox.map(|b| b.expand(2));
                Some(va)
            }
            Value::Object(_) => Some(ViewArgs::parse(&view_arg)?),
            _ => None,
        };
        if let Some(va) = va {
            out.extend(render(&c.doc, &va)?);
        }
        self.live_write(&name);
        Ok(out)
    }

    // ---------- view ----------
    fn view(&mut self, a: &Value) -> ToolResult {
        let name = self.resolve(a)?;
        let c = &self.canvases[&name];
        render(&c.doc, &ViewArgs::parse(a)?)
    }

    // ---------- history ----------
    fn history(&mut self, a: &Value) -> ToolResult {
        let name = self.resolve(a)?;
        let action = s(a, "action").ok_or("`action` is required")?;
        let c = self.canvases.get_mut(&name).unwrap();
        let text = match action {
            "undo" | "redo" => {
                let steps = i(a, "steps")?.unwrap_or(1).max(1);
                let mut done: Vec<String> = Vec::new();
                for _ in 0..steps {
                    let st = if action == "undo" {
                        c.history.undo(&mut c.doc)
                    } else {
                        c.history.redo(&mut c.doc)
                    };
                    match st {
                        Some(st) => done.push(format!("{} ({})", st.label, st.author.name())),
                        None => break,
                    }
                }
                if author(a) == Author::User && !done.is_empty() {
                    c.user_log.push(format!("{action} x{}", done.len()));
                }
                let (u, r) = c.history.depth();
                format!(
                    "{action} x{} (now {u} undo / {r} redo available){}{}",
                    done.len(),
                    if done.is_empty() { "" } else { ": " },
                    done.join(", ")
                )
            }
            "checkpoint" => {
                let cp = s(a, "name").ok_or("`name` is required")?;
                c.history.checkpoints.insert(cp.to_string(), c.doc.clone());
                format!("checkpoint {cp:?} saved (costs almost no memory: tiles are shared)")
            }
            "restore" => {
                let cp = s(a, "name").ok_or("`name` is required")?;
                let snap = c
                    .history
                    .checkpoints
                    .get(cp)
                    .cloned()
                    .ok_or_else(|| format!("no checkpoint {cp:?}"))?;
                let before = std::mem::replace(&mut c.doc, snap);
                c.record(before, step(a, format!("restore {cp}")));
                format!("restored checkpoint {cp:?} (undo reverts the restore)")
            }
            "list" => {
                let (u, r) = c.history.depth();
                let mut t = format!(
                    "{u} undo / {r} redo; checkpoints: {:?}",
                    c.history.checkpoints.keys().collect::<Vec<_>>()
                );
                let entries = c.history.entries();
                if !entries.is_empty() {
                    t.push_str("\nrecent steps (oldest → newest):");
                    let start = entries.len().saturating_sub(15);
                    for (k, (_, st)) in entries.iter().enumerate().skip(start) {
                        let _ = write!(t, "\n  {:>3}. {:<6} {}", k + 1, st.author.name(), st.label);
                    }
                }
                t
            }
            "diff" => {
                let pick = |k: &str, default: &str| -> Result<Document, String> {
                    match s(a, k).unwrap_or(default) {
                        "current" => Ok(c.doc.clone()),
                        "previous" => c
                            .history
                            .previous()
                            .cloned()
                            .ok_or_else(|| "no previous state".to_string()),
                        cp => c
                            .history
                            .checkpoints
                            .get(cp)
                            .cloned()
                            .ok_or_else(|| format!("no checkpoint {cp:?}")),
                    }
                };
                let (from, to) = (pick("from", "previous")?, pick("to", "current")?);
                let d = to.diff(&from);
                let mut t = match d.bbox {
                    Some(bb) => format!("{} px differ within {bb}", d.pixels),
                    None => "identical".to_string(),
                };
                if b(a, "view").unwrap_or(false)
                    && let Some(bb) = d.bbox
                {
                    let mut va = ViewArgs::parse(&Value::Null)?;
                    va.region = Some(bb.expand(2));
                    let mut out = vec![Content::Text(std::mem::take(&mut t))];
                    out.extend(render(&to, &va)?);
                    return Ok(out);
                }
                t
            }
            other => {
                return Err(format!(
                    "unknown history action {other:?} (undo, redo, checkpoint, restore, list, diff)"
                ));
            }
        };
        if matches!(action, "undo" | "redo" | "restore") {
            self.live_write(&name);
        }
        Ok(vec![Content::Text(text)])
    }
}
