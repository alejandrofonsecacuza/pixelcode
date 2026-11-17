//! Minimal MCP server over stdio (JSON-RPC 2.0, one message per line).
//! Hand-written: only `initialize`, `tools/list` and `tools/call` are needed.

use std::io::{self, BufRead, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};

use base64::Engine as _;
use serde_json::{Value, json};

use crate::tools::{Content, Workspace};

const INSTRUCTIONS: &str = "pixelcode is a fast, pixel-perfect drawing engine. Workflow: \
1) canvas {action:\"create\", width, height} (up to 16384 px per side; sparse tiles make big canvases cheap). \
2) draw {script} — one script can contain thousands of operations (loops, functions, math), so batch work into few calls; \
pass view:\"changed\" or view:true to get an image back in the same call. Failed scripts leave the canvas untouched. \
3) Verify with view: mode \"image\" (zoomed PNG with coordinate rulers and pixel grid), \"pixels\" (exact per-pixel dump as text \
for small regions) or \"stats\" (colors, bbox, hash). Zoom into regions instead of viewing a huge canvas at once. \
4) history: checkpoint before risky changes, undo/redo, diff. 5) canvas {action:\"save\", path} writes a PNG; \
canvas {action:\"export\", path:\"x.html\"} writes a shareable interactive page (in Claude Code it can be published as an Artifact). \
The person can also change the canvas (for example undo from their side); your next tool result then starts with a \
[pixel] line describing what changed.";

fn tools() -> Value {
    let draw_desc = format!(
        "Run a drawing script on a canvas. The whole script is atomic (all or nothing) and returns timing, \
the changed region and print() output.\n\n{}",
        pixel_script::REFERENCE
    );
    let view_props = json!({
        "x": {"type": "integer"}, "y": {"type": "integer"}, "w": {"type": "integer"}, "h": {"type": "integer"},
        "scale": {"type": "integer", "description": "integer zoom (1-64); default: fit into `max` px"},
        "max": {"type": "integer", "description": "max image side in px when auto-fitting (default 1024)"},
        "grid": {"type": "boolean", "description": "pixel grid overlay (default: on when zoom >= 8)"},
        "rulers": {"type": "boolean", "description": "coordinate rulers (default true)"},
        "mode": {"type": "string", "enum": ["image", "pixels", "stats"]},
        "layer": {"type": "string", "description": "show only this layer"}
    });
    json!([
        {
            "name": "canvas",
            "description": "Manage canvases. Actions: create {name?='main', width, height, background?, replace?}; \
    open {path, name?} (PNG as new canvas); import {path, layer?, x?, y?} (PNG into a layer); \
    save {path, scale?, layers?} (export PNG; layers:true also writes one PNG per layer); \
    export {path?='<name>.html', title?, history?=true, artifact?=false} (self-contained shareable HTML page: zoom, pixel inspector, \
    palette, layers and a replayable timeline of the drawing history; works offline); info; list; use {name}; \
    resize {width, height, dx?, dy?}; close.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["create", "open", "import", "save", "export", "info", "list", "use", "resize", "close", "pending"]},
                    "title": {"type": "string"}, "history": {"type": "boolean"},
                    "artifact": {"type": "boolean", "description": "export: omit the <!doctype>/<html> wrapper, for publishing as a claude.ai Artifact"},
                    "name": {"type": "string"}, "canvas": {"type": "string"},
                    "width": {"type": "integer"}, "height": {"type": "integer"},
                    "background": {"type": "string"}, "replace": {"type": "boolean"},
                    "path": {"type": "string"}, "layer": {"type": "string"},
                    "x": {"type": "integer"}, "y": {"type": "integer"}, "dx": {"type": "integer"}, "dy": {"type": "integer"},
                    "scale": {"type": "integer"}, "layers": {"type": "boolean"}
                },
                "required": ["action"]
            }
        },
        {
            "name": "draw",
            "description": draw_desc,
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "Rhai drawing script"},
                    "canvas": {"type": "string", "description": "defaults to the current canvas"},
                    "view": {
                        "description": "optional preview in the same call: true (whole canvas), \"changed\" (changed region) or a view object",
                        "anyOf": [{"type": "boolean"}, {"type": "string", "enum": ["changed"]}, {"type": "object", "properties": view_props}]
                    }
                },
                "required": ["script"]
            }
        },
        {
            "name": "view",
            "description": "Look at a canvas region. mode image: PNG zoomed with integer scale, coordinate rulers and \
    pixel grid (big regions are downscaled: zoom in for exact work). mode pixels: exact text dump, one symbol per pixel \
    plus a color legend (max 128x128). mode stats: distinct colors, content bbox, coverage, hash.",
            "inputSchema": {"type": "object", "properties": view_props.as_object().cloned().map(|mut m| {
                m.insert("canvas".into(), json!({"type": "string"}));
                Value::Object(m)
            }).unwrap()}
        },
        {
            "name": "history",
            "description": "Undo/redo and checkpoints (cheap: tiles are shared copy-on-write). Actions: undo {steps?}; \
    redo {steps?}; checkpoint {name}; restore {name}; list; diff {from?='previous', to?='current', view?} where \
    from/to are 'current', 'previous' or a checkpoint name.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["undo", "redo", "checkpoint", "restore", "list", "diff"]},
                    "canvas": {"type": "string"}, "name": {"type": "string"}, "steps": {"type": "integer"},
                    "from": {"type": "string"}, "to": {"type": "string"}, "view": {"type": "boolean"}
                },
                "required": ["action"]
            }
        }
    ])
}

fn content_json(c: Content) -> Value {
    match c {
        Content::Text(t) => json!({"type": "text", "text": t}),
        Content::Png(p) => json!({
            "type": "image",
            "data": base64::engine::general_purpose::STANDARD.encode(p),
            "mimeType": "image/png"
        }),
    }
}

fn reply(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: &Value, code: i64, msg: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}})
}

pub fn handle(ws: &mut Workspace, msg: &Value) -> Option<Value> {
    let method = msg.get("method").and_then(Value::as_str);
    let id = msg.get("id").cloned();
    let (Some(method), Some(id)) = (method, id) else {
        return None; // notifications and responses get no reply
    };
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => {
            let version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2025-06-18");
            reply(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "pixelcode", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS
                }),
            )
        }
        "ping" => reply(&id, json!({})),
        "tools/list" => reply(&id, json!({"tools": tools()})),
        "resources/list" => reply(&id, json!({"resources": []})),
        "prompts/list" => reply(&id, json!({"prompts": []})),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let res = catch_unwind(AssertUnwindSafe(|| ws.call(name, &args))).unwrap_or_else(|p| {
                let m = p
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()));
                Err(format!(
                    "internal error: {}",
                    m.unwrap_or_else(|| "panic".into())
                ))
            });
            match res {
                Ok(content) => reply(
                    &id,
                    json!({"content": content.into_iter().map(content_json).collect::<Vec<_>>(), "isError": false}),
                ),
                Err(e) => reply(
                    &id,
                    json!({"content": [{"type": "text", "text": e}], "isError": true}),
                ),
            }
        }
        m => error(&id, -32601, &format!("method not found: {m}")),
    })
}

pub fn serve(mut ws: Workspace) -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let responses: Vec<Value> = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => batch.iter().filter_map(|m| handle(&mut ws, m)).collect(),
            Ok(msg) => handle(&mut ws, &msg).into_iter().collect(),
            Err(e) => vec![error(&Value::Null, -32700, &format!("parse error: {e}"))],
        };
        for r in responses {
            writeln!(stdout, "{r}")?;
        }
        stdout.flush()?;
    }
    Ok(())
}
