/** What the pane shows: the current canvas, already rendered. */
export type Snapshot = {
  canvas: string
  /** Base64 PNG (a view from the pixel server, without rulers or grid). */
  png: string
  /** PNG size in pixels. */
  width: number
  height: number
  /** Status text, e.g. `main 256×144 · 1.20 ms · 3 steps`. */
  info: string
}

/** The canvas right after one specific call to a pixel tool. */
export type Thumb = { id: string; png: string; width: number; height: number }

declare module 'claude-code' {
  interface PluginState {
    'pixel-studio': {
      current: Shaped<Snapshot | null>
      thumbs: Shaped<Thumb[]>
    }
  }
}
