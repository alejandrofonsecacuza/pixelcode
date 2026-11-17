# pixelcode

A **pixel-perfect drawing engine** written in Rust, built to be used by **AI agents** (Claude Code
or any other MCP harness) and by people from the terminal.

The agent draws by writing short scripts against a small set of fast, exact primitives, and checks
its work with views designed to cost few tokens. In Claude Code, a plugin shows the canvas live in a
pane next to the chat and lets you share any drawing as an interactive page.

## Principles

1. **Fast end to end.** Native rendering (tiles, spans, parallelism), few round trips (one script
   can run thousands of operations) and cheap feedback (region views, diffs).
2. **No ceiling.** The agent does not pick from a catalog of buttons: it writes a script with
   loops, functions and math that calls native primitives, and it can always verify the result
   with exact values.
3. **Minimal surface.** Four tools: `canvas`, `draw`, `view` and `history`.

## Installation

In Claude Code, one command:

```
/plugin install pixel-studio --marketplace alejandrofonsecacuza/pixelcode
```

Answer `y` to add the marketplace and pick a scope (user: all your projects). That's it: the plugin
ships the MCP server, and on first use it downloads the prebuilt engine for your system (Linux or
macOS, x86_64 or ARM) from GitHub Releases, verifies its SHA-256 and caches it. No Rust toolchain
and no config files to edit.

What you get:

- the `canvas`, `draw`, `view` and `history` tools for the agent;
- a **pane** with the live canvas next to the chat;
- a **thumbnail** under each drawing in the transcript;
- the canvas state in the **status line**;
- the **`/pixel`** command: `show`, `hide`, `undo`, `redo`, `save <path>`, `export <path.html>`,
  `share [title]`, `list`, `use`, `info`.

Images show up in terminals that support a graphics protocol: Kitty, Ghostty or iTerm2 3.5+ (inside
tmux, with `allow-passthrough on`). On Windows, use WSL.

### Sharing a drawing

Any canvas can be exported as **one self-contained HTML page**: a single file with no dependencies
that works offline. It has crisp zoom, a pixel inspector (exact coordinates and color), the
drawing's actual palette (click to copy a color), layers and a **timeline that replays how the
drawing was made**, step by step.

- In Claude Code, **`/pixel share`** publishes it as a claude.ai Artifact and gives you the link
  (private until you share it from its *Share* menu). Sharing again updates the same link.
- **`/pixel export drawing.html`**, or the agent's `canvas export` action, writes the file so you
  can email it or host it on a gist or GitHub Pages.
- From the CLI: `pixel run scene.rhai --size 128x80 -o scene.html`.

### CLI only, without Claude Code

```bash
cargo install --git https://github.com/alejandrofonsecacuza/pixelcode pixel
```

## CLI

```bash
pixel run examples/landscape.rhai --size 256x144 -o landscape.png --scale 4 --show
pixel run - --size 64x64 --show <<< 'circle(32, 32, 20, "#f80");'
pixel show image.png
pixel mcp --live .pixel   # MCP server; writes a PNG per canvas to .pixel/ after each change
pixel watch .pixel        # redraws the newest PNG in .pixel/ whenever it changes
pixel bench               # measures performance on your machine
```

## The four MCP tools

| Tool | What it does |
|---|---|
| `canvas` | `create`, `open` (PNG as a canvas), `import` (PNG into a layer), `save` (PNG, also per layer), `export` (HTML page), `info`, `list`, `use`, `resize`, `close` |
| `draw` | runs a script; it is **atomic** (if it fails, the canvas is unchanged) and returns the timing, the changed region and the `print()` output; with `view: "changed"` it also returns the image in the same call |
| `view` | `image`: zoomed PNG with coordinate rulers and a pixel grid; `pixels`: exact text dump (one symbol per pixel plus a color legend); `stats`: colors, bounding box and hash |
| `history` | `undo`, `redo`, `checkpoint`, `restore`, `list`, `diff` |

Each history step records who made it (the agent or the person), and the shared page's timeline
shows it. When the person changes the canvas from their side (for example `/pixel undo`), the
agent's next tool result starts with a `[pixel]` line describing what changed.

## The language

Scripts are [Rhai](https://rhai.rs) (JS/Rust-like syntax) calling native primitives:

```rhai
layer("sky");
rect(0, 0, W, H, linear(0, 0, 0, H, ["#0b0e2a", "#e0605a"], #{steps: 6, dither: true}));
for i in 0..200 { pixel(rand(0, W - 1), rand(0, H / 2), "#fff"); }

layer("sprite");
grid(40, 40, [
    "..KK..",
    ".KYYK.",
    "KYYYYK",
], #{K: "#000", Y: "#fd0"}, 4);

path("M 10 90 Q 50 60 90 90 Z", #{fill: "#2b1d3f", stroke: "#000", width: 2});
text(4, 4, "hello", #{fill: "#fff", scale: 2});
```

- **Shapes:** `pixel`, `rect`, `circle`, `ellipse`, `line`, `polyline`, `polygon`, `path` (full SVG
  path data: M L H V C S Q T A Z), `flood`, `text` (8×8 font with Latin-1), `grid` (sprites as
  strings), `copy` (with flip, rotation and scale).
- **Paints:** colors, `linear`/`radial` gradients with bands and Bayer dithering, and `pattern` (a
  region tiled as a texture).
- **Layers** with opacity and blend modes (`over`, `replace`, `erase`, `multiply`, `screen`, `add`,
  `darken`, `lighten`), clipping with `clip`, and deterministic `rand`/`noise`.

The full reference lives in [`crates/pixel-script/src/api.rs`](crates/pixel-script/src/api.rs)
(`REFERENCE`). It is the same text the agent receives in the `draw` tool description.

**Coordinates:** fills use continuous coordinates (pixel `(x, y)` covers `[x, x+1)` and is painted
when its center is inside the shape). Lines go through pixel centers: `line(0,0,9,0)` paints
exactly pixels 0..9. There is no antialiasing: output is exact and deterministic.

## Performance

`pixel bench` on a 14-thread machine (4096×4096 canvas):

| Operation | Time |
|---|---|
| Opaque fill 4096² | 2.3 ms (≈7000 Mpx/s) |
| Alpha blend 4096² | 6.7 ms (≈2500 Mpx/s) |
| Dithered radial gradient 4096² | 18 ms |
| 10,000 circles r=12 | 14 ms |
| Star polygon, 2000 vertices | 13.5 ms |
| Thick polyline, 1000 segments | 1.9 ms |
| Composite 4 layers 4096² | 18 ms |
| Snapshot + diff 4096² | 0.3 ms |
| Script with 10,000 `circle()` calls | ~25 ms |

A 16384×16384 canvas (268 M pixels) is filled with a gradient and 2000 translucent circles in about
0.5 s; its downscaled preview takes about 90 ms.

## Design notes

- **64×64 copy-on-write tiles (`Arc`).** Empty tiles take no memory. Cloning a document copies
  pointers, so every `draw` keeps a full undo state almost for free, and diffs skip shared tiles
  without looking at them.
- **Everything rasterizes to horizontal spans.** Spans are normalized (clipped, sorted, merged) and
  painted in one pass: no pixel is blended twice, and tile bands are painted in parallel.
- **Thick strokes as a nonzero union** of segment quads and round joins, in a single pass.
- **Inner strokes for rects and ellipses** by diamond erosion: at 1 px this gives the classic
  8-connected pixel-art outline.
- **Integer blending** (exact `div255`) with fast paths for the opaque and empty cases.
- **Gradients through a precomputed lookup table**, and large polygons in parallel row bands.
- **Views, statistics and saving work in bands:** the whole canvas is never materialized.
- **Hand-written MCP server** (JSON-RPC over stdio), no heavy dependencies.

## Project layout

```
crates/
├── pixel-core     engine: tiled canvas, rasterization, layers, compositing, history, views
├── pixel-script   drawing language (embedded Rhai) on top of pixel-core
└── pixel          binary: MCP server + CLI (run, show, watch, bench)
claude-plugin/
└── pixel-studio   Claude Code plugin: bundled MCP server, live pane, thumbnails, status line, /pixel
examples/          example scripts (.rhai)
```

## Development

```bash
cargo build --release          # binary at target/release/pixel
cargo test --workspace         # engine, language and an end-to-end MCP server test
claude plugin test claude-plugin/pixel-studio
```

To try the plugin from a local checkout:

```bash
claude plugin install pixel-studio --marketplace /path/to/pixelcode --scope project
```

It is read straight from the folder (run `/reload-plugins` after editing), and the launcher uses
`target/release/pixel` when it exists, so nothing is downloaded.

### Releasing

1. Bump the version in `Cargo.toml`, `claude-plugin/pixel-studio/.claude-plugin/plugin.json` and
   `VERSION` in `claude-plugin/pixel-studio/bin/pixel-mcp` (the workflow checks that they match).
2. `git tag v0.1.0 && git push origin v0.1.0`
3. GitHub Actions builds the four binaries and creates the Release with their checksums.

## License

[MIT](LICENSE)
