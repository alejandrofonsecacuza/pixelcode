// pixel-studio: native integration of the pixelcode engine with Claude Code.
//
// - A "pixel" pane next to the chat with the live canvas.
// - A thumbnail of the canvas under every agent call that changes it.
// - A status line with the canvas, its size, the last draw time and the step count.
// - /pixel to open the pane, undo, save, export or share.
// - When the person changes the canvas (an undo, say), the agent finds out: the server tells it
//   in its next tool result and this mod adds it as context to the next message.
//
// Everything goes through the same MCP server the agent uses ($.mcp.call). Calls from this mod
// carry `_author: "user"`: they belong to the person, never to the agent.

import { atom, read, update } from 'claude-code'
import type { EngineInterface, Register } from 'claude-code'

import type { Snapshot, Thumb } from '../types'

// The server either ships with this plugin (`plugin:pixel-studio:pixel`, tools named
// `mcp__plugin_pixel-studio_pixel__draw`) or is configured by hand as `pixel`.
const TOOL = /^mcp__(.*pixel.*)__(canvas|draw|view|history)$/
const SERVERS = ['plugin:pixel-studio:pixel', 'pixel']
const PANE = 'pixel-canvas'
const MAX_THUMBS = 24
// Bump a tag whenever the shape of its value changes: values stored by older code are then dropped.
const current = atom({ plugin: 'pixel-studio', key: 'current' } as const, null, { shape: 'snap-3' })
const thumbs = atom({ plugin: 'pixel-studio', key: 'thumbs' } as const, [], { shape: 'thumbs-1' })

type Args = Record<string, unknown>
type McpResult = Awaited<ReturnType<EngineInterface['mcp']['call']>>

const NOT_CONNECTED = 'The pixel engine is not connected: check /mcp (server "pixel").'

const HELP = [
  'Usage: /pixel [command]',
  '  (none) | show        open the pane with the live canvas',
  '  hide                 close the pane',
  '  undo [n] | redo [n]   undo or redo n steps',
  '  save <path> [scale]   save the canvas as a PNG',
  '  export <path.html>    interactive page to share (works offline)',
  '  share [title]         publish it as a claude.ai Artifact and get the link',
  '  list                 list open canvases',
  '  use <name>           switch the current canvas',
  '  info                 layers and history of the current canvas',
].join('\n')

let autoOpened = false
/** Artifact already published per canvas in this session: sharing again updates it. */
const shared = new Map<string, string>()
/** The last server that answered or that the agent used; it is tried first. */
let server: string | undefined

function pixelTool(name: string): { server: string; tool: string } | undefined {
  const m = TOOL.exec(name)
  return m?.[1] && m[2] ? { server: m[1], tool: m[2] } : undefined
}

function textOf(r: McpResult): string {
  return r.content
    .filter(b => b.type === 'text')
    .map(b => b.text ?? '')
    .join('\n')
}

/** Base64 PNG from a result. Claude Code hands images over in the Anthropic API shape
 * (`{ source: { data } }`); raw MCP uses `{ data }`. Both are accepted. */
function imageOf(r: McpResult): string | undefined {
  for (const block of r.content) {
    if (block.type !== 'image') continue
    if (typeof block.data === 'string') return block.data
    const source = block.source as { data?: unknown } | undefined
    if (typeof source?.data === 'string') return source.data
  }
  return undefined
}

/** Calls the server as the person, so the notice pending for the agent is not consumed. */
async function call($: EngineInterface, tool: string, args: Args): Promise<McpResult | undefined> {
  const candidates = server ? [server, ...SERVERS.filter(s => s !== server)] : SERVERS
  for (const name of candidates) {
    try {
      const r = await $.mcp.call(name, tool, { ...args, _author: 'user' })
      server = name
      return r
    } catch {
      // that name is not connected: try the next one
    }
  }
  return undefined
}

/** Actions that change what the canvas looks like. */
function changesCanvas(tool: string, a: Args): boolean {
  const action = typeof a.action === 'string' ? a.action : ''
  if (tool === 'draw') return true
  if (tool === 'canvas') return ['create', 'open', 'import', 'resize', 'use', 'close'].includes(action)
  if (tool === 'history') return ['undo', 'redo', 'restore'].includes(action)
  return false
}

/** Cells that show an image without distortion (a cell is about twice as tall as it is wide). */
function fit(w: number, h: number, maxCols: number, maxRows: number): { columns: number; rows: number } {
  const clamp = (v: number) => Math.max(1, Math.min(255, Math.round(v)))
  let columns = Math.max(1, maxCols)
  let rows = (columns * h) / w / 2
  if (rows > maxRows) {
    rows = Math.max(1, maxRows)
    columns = (rows * 2 * w) / h
  }
  return { columns: clamp(columns), rows: clamp(rows) }
}

/** Reads the canvas from the server again and updates the pane, the thumbnail and the status line. */
async function refresh($: EngineInterface, toolUseId?: string, canvas?: string, timing?: string): Promise<void> {
  const info = await call($, 'canvas', canvas ? { action: 'info', canvas } : { action: 'info' })
  if (!info || info.isError) {
    await update($, current, () => null)
    $.ui.status(undefined)
    return
  }
  const first = textOf(info).split('\n')[0] ?? ''
  const m = /canvas "([^"]+)" (\d+)x(\d+) · active layer "([^"]*)"/.exec(first)
  const name = m?.[1] ?? canvas ?? 'canvas'
  const view = await call($, 'view', { canvas: name, rulers: false, grid: false, max: 640 })
  const png = view && !view.isError ? imageOf(view) : undefined
  if (!view || !png) return
  const size = /image (\d+)x(\d+)/.exec(textOf(view))
  const layerCount = textOf(info)
    .split('\n')
    .filter(l => /^\s+\d+: "/.test(l)).length
  const steps = Number(/history (\d+) undo/.exec(first)?.[1] ?? 0)
  const parts = [`${name} ${m?.[2] ?? '?'}×${m?.[3] ?? '?'}`]
  if (layerCount > 1) parts.push(`layer ${m?.[4] ?? '?'} (${layerCount} layers)`)
  if (timing) parts.push(timing)
  if (steps > 0) parts.push(`${steps} ${steps === 1 ? 'step' : 'steps'}`)
  const snap: Snapshot = {
    canvas: name,
    png,
    width: Number(size?.[1] ?? 1),
    height: Number(size?.[2] ?? 1),
    info: parts.join(' · '),
  }
  await update($, current, () => snap)
  if (toolUseId) {
    const thumb: Thumb = { id: toolUseId, png, width: snap.width, height: snap.height }
    await update($, thumbs, list => [...list.filter(t => t.id !== toolUseId), thumb].slice(-MAX_THUMBS))
  }
  $.ui.status(`🎨 ${snap.info}`)
}

// ---------- sharing ----------

/** The current canvas, as the server reports it. */
async function currentCanvas($: EngineInterface): Promise<{ name: string; w: string; h: string } | undefined> {
  const info = await call($, 'canvas', { action: 'info' })
  if (!info || info.isError) return undefined
  const m = /canvas "([^"]+)" (\d+)x(\d+)/.exec(textOf(info))
  return m?.[1] && m[2] && m[3] ? { name: m[1], w: m[2], h: m[3] } : undefined
}

/** Exports the page and returns its absolute path, or the error to show. */
async function exportPage($: EngineInterface, args: Args): Promise<{ path?: string; text: string }> {
  const r = await call($, 'canvas', { action: 'export', ...args })
  if (!r) return { text: NOT_CONNECTED }
  const text = textOf(r)
  if (r.isError) return { text }
  return { path: /exported (.+?\.html) \(/.exec(text)?.[1], text }
}

async function share($: EngineInterface, title: string | undefined): Promise<{ text: string; context?: string[] }> {
  const canvas = await currentCanvas($)
  if (!canvas) return { text: 'There is no canvas to share yet. Ask the agent to draw something first.' }
  const slug = canvas.name.replace(/[^\w.-]+/g, '-')
  const page = await exportPage($, { path: `.pixel/share/${slug}.html`, artifact: true, ...(title ? { title } : {}) })
  if (!page.path) return { text: page.text }
  const previous = shared.get(canvas.name)
  const input = previous
    ? { tool: 'Artifact' as const, action: 'publish' as const, file_path: page.path, url: previous }
    : {
        tool: 'Artifact' as const,
        action: 'publish' as const,
        file_path: page.path,
        icon: 'image',
        description: `${canvas.w}×${canvas.h} px pixel art made with pixelcode, with a pixel inspector and a replayable history.`,
      }
  let published: string | undefined
  let why = ''
  try {
    const ran = await $.tool.call(input)
    if (ran.deny !== undefined) why = ran.deny
    else {
      const raw = JSON.stringify(ran.result ?? '')
      published = /https:\/\/claude\.ai\/[\w/-]+/.exec(raw)?.[0]
      if (!published) why = raw.slice(0, 200)
    }
  } catch (err) {
    why = String(err)
  }
  if (!published) {
    return {
      text: [
        'Could not publish it as an Artifact' + (why ? `: ${why}` : '.'),
        `The page is at ${page.path}: open it in a browser or share the file (it works offline).`,
      ].join('\n'),
    }
  }
  shared.set(canvas.name, published)
  return {
    text: `${previous ? 'Updated' : 'Published'}: ${published}\nIt stays private until you share it from claude.ai. Local file: ${page.path}`,
    context: [`The user shared canvas "${canvas.name}" as a claude.ai artifact: ${published}`],
  }
}

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    const started = await next(e)
    await $.command.register({
      name: 'pixel',
      description: 'pixelcode canvas: live pane, undo/redo, save and share (/pixel help)',
    })
    void refresh($)
    return started
  })

  // After every agent call that changes the canvas: refresh everything.
  on('tool.call', async ($, e, next) => {
    const ran = await next(e)
    const pt = pixelTool(e.tool)
    if (!pt || ran.deny !== undefined || ran.isError) return ran
    server = pt.server
    const tool = pt.tool
    const args = e as unknown as Args
    if (!changesCanvas(tool, args)) return ran
    const timing = /ok in ([\d.]+ ms)/.exec(JSON.stringify(ran.result ?? ''))?.[1]
    const canvas = typeof args.canvas === 'string' ? args.canvas : typeof args.name === 'string' ? args.name : undefined
    await refresh($, e.tool_use_id, tool === 'canvas' && args.action === 'close' ? undefined : canvas, timing)
    if (!autoOpened) {
      autoOpened = true
      $.ui.open({ id: PANE, title: 'pixel' }).catch(() => undefined) // no screen (headless): nothing to open
    }
    return ran
  }).catch(($, e, next) => next(e)) // if the refresh fails, the agent's call is left intact

  // What the person changed (an undo, say) travels as context with their next message.
  on('prompt.submit', async ($, e, next) => {
    const snap = await read($, current)
    if (!snap) return next(e)
    const r = await call($, 'canvas', { action: 'pending', canvas: snap.canvas })
    const text = r && !r.isError ? textOf(r).trim() : ''
    return text ? next({ ...e, context: [...(e.context ?? []), text] }) : next(e)
  }).catch(($, e, next) => next(e))

  on('command.run', { command: 'pixel' }, async ($, e) => {
    const [sub = 'show', ...rest] = e.args.trim().split(/\s+/).filter(Boolean)
    const note = (what: string) => [
      `The user ran "/pixel ${e.args.trim()}" directly: ${what}. The canvas state changed outside your tool calls.`,
    ]
    switch (sub) {
      case 'show': {
        await refresh($)
        await $.ui.open({ id: PANE, title: 'pixel', focus: true })
        return { text: 'pixel pane opened.' }
      }
      case 'hide': {
        await $.ui.close({ id: PANE })
        return { text: 'pixel pane closed.' }
      }
      case 'undo':
      case 'redo': {
        const steps = Math.max(1, Number(rest[0] ?? 1) || 1)
        const r = await call($, 'history', { action: sub, steps })
        if (!r) return { text: NOT_CONNECTED }
        await refresh($)
        return { text: textOf(r), context: r.isError ? undefined : note(`${sub} x${steps}`) }
      }
      case 'save': {
        const [path, scale] = rest
        if (!path) return { text: 'Usage: /pixel save <path.png> [scale]' }
        const args: Args = { action: 'save', path }
        if (scale) args.scale = Number(scale)
        const r = await call($, 'canvas', args)
        return { text: r ? textOf(r) : NOT_CONNECTED }
      }
      case 'use': {
        const name = rest[0]
        if (!name) return { text: 'Usage: /pixel use <name>' }
        const r = await call($, 'canvas', { action: 'use', name })
        if (!r) return { text: NOT_CONNECTED }
        await refresh($)
        return { text: textOf(r), context: r.isError ? undefined : note(`current canvas is now ${name}`) }
      }
      case 'export': {
        const path = rest[0]
        if (!path?.endsWith('.html')) return { text: 'Usage: /pixel export <path.html>' }
        return { text: (await exportPage($, { path })).text }
      }
      case 'share': {
        const title = rest.join(' ').trim() || undefined
        return share($, title)
      }
      case 'list':
      case 'info': {
        const r = await call($, 'canvas', { action: sub })
        return { text: r ? textOf(r) : NOT_CONNECTED }
      }
      default:
        return { text: HELP }
    }
  })

  // Pane: the live canvas, fitted to the available space.
  on('ui.render', { component: 'Pane', requestId: PANE }, async ($, e) => {
    const snap = await read($, current)
    if (e.surface !== 'terminal') {
      const { Text } = $.ui.resolve(e)
      return <Text>{snap ? `🎨 ${snap.info}` : 'No canvas yet.'}</Text>
    }
    const { Box, Text, Image } = $.ui.resolve(e)
    if (!snap) {
      return (
        <Box flexDirection="column">
          <Text dimColor>No canvas yet.</Text>
          <Text dimColor>Ask the agent to draw something, or see /pixel help.</Text>
        </Box>
      )
    }
    const size = fit(snap.width, snap.height, e.props.bodyColumns, Math.max(4, e.props.scroll.bodyRows - 2))
    return (
      <Box flexDirection="column">
        <Text bold>{snap.info}</Text>
        <Image key="canvas" source={{ png: snap.png }} columns={size.columns} rows={size.rows} alt={`canvas ${snap.canvas}`} />
      </Box>
    )
  })

  // Thumbnail under every call that changed the canvas.
  on('ui.render', { component: 'ToolUse' }, async ($, e, next) => {
    const row = await next(e)
    if (!pixelTool(e.props.tool) || e.props.isRunning || e.surface !== 'terminal') return row
    const list = await read($, thumbs)
    const thumb = list.find(t => t.id === e.props.tool_use_id)
    if (!thumb) return row
    const { Box, Image } = $.ui.resolve(e)
    const size = fit(thumb.width, thumb.height, Math.min(48, (e.viewport?.columns ?? 80) - 6), 14)
    return (
      <Box flexDirection="column">
        {row}
        <Box paddingLeft={2}>
          <Image key="thumb" source={{ png: thumb.png }} columns={size.columns} rows={size.rows} alt="canvas thumbnail" />
        </Box>
      </Box>
    )
  })
}
