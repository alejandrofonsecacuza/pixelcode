# pixel-studio

Claude Code plugin for the **pixelcode** drawing engine. It ships the engine's MCP server and shows
the canvas inside Claude Code:

- **Live pane** next to the chat with the current canvas. It opens by itself on the agent's first drawing.
- **Canvas thumbnail** under every agent call that changes it (`draw`, `canvas create/open/import/resize`,
  `history undo/redo/restore`).
- **Status line** with the canvas name, size, active layer and last draw time.
- **`/pixel`**: `show`, `hide`, `undo [n]`, `redo [n]`, `save <path> [scale]`, `export <path.html>`, `share [title]`,
  `list`, `use <name>`, `info`. When you change the canvas with `/pixel`, the agent is told so it does not lose track.
- **`/pixel share`** publishes the canvas as a claude.ai Artifact: an interactive page with zoom, a pixel inspector, the
  palette, layers and a replayable history. If your account has no Artifacts, it leaves the HTML file for you to share.

It needs a terminal that can show images: Kitty, Ghostty or iTerm2 3.5+ (inside tmux, with `allow-passthrough on`).
Other surfaces get the canvas info as text.

## Installation

```
/plugin install pixel-studio --marketplace alejandrofonsecacuza/pixelcode
```

From a shell: `claude plugin install pixel-studio --marketplace alejandrofonsecacuza/pixelcode`.

The plugin bundles the MCP server; on first use it downloads the prebuilt engine (Linux/macOS) and verifies its
SHA-256. To use your own binary, set `PIXEL_BIN=/path/to/pixel`.
