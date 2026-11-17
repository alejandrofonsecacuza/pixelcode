# pixelcode

Pixel-perfect drawing engine in Rust for AI agents (MCP server + CLI), plus a Claude Code plugin.
See README.md.

## Commands
- `cargo build --release`: binary at `target/release/pixel` (the plugin launcher uses it when present)
- `cargo test --workspace`: includes an end-to-end MCP server test (`crates/pixel/tests/mcp.rs`)
- `claude plugin test claude-plugin/pixel-studio`: plugin tests
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all`
- `./target/release/pixel bench`: measure before and after touching hot paths

## Principles (non-negotiable)
1. Fast end to end: do not trade performance for simplicity in algorithms; measure with `pixel bench`.
2. No ceiling for the agent: capability goes into the language (`pixel-script`), not into more tools.
3. Minimal surface: only 4 MCP tools (`canvas`, `draw`, `view`, `history`).

## Conventions
- Every shape rasterizes to `Span`s painted with `Raster::paint_spans` (never pixel by pixel in hot paths).
- Nothing may materialize a whole canvas: views, stats and saving work in bands.
- When adding or changing a language function, update `REFERENCE` in `crates/pixel-script/src/api.rs`
  (it is what the agent reads) and add a test in `crates/pixel-script/src/tests.rs`.
- Watch for names that clash with Rhai built-ins (it happened with `clear` on strings).
- The MCP server ships with the plugin (`claude-plugin/pixel-studio/.mcp.json` → `bin/pixel-mcp`); there is no
  `.mcp.json` at the root. Its tools are named `mcp__plugin_pixel-studio_pixel__*`; the plugin detects the server name.
- Plugin calls carry `_author: "user"` (and optionally `_label`): history records who made each step, and the agent's
  next call gets the `[pixel]` notice from `Workspace::briefing`. A call without `_author` counts as the agent's and
  marks everything as seen, so the plugin must never omit it.
- Panel lessons: overlaying a `Client` on an `Image` erased the image in Kitty, and a `Client` that draws the canvas
  cell by cell exceeds its 100,000-character limit. Editing and marking from the panel were tried and removed: the
  person asks the agent for changes in the chat.
- When releasing, `VERSION` in the launcher, `plugin.json` and `Cargo.toml` must match (`release.yml` checks it).
