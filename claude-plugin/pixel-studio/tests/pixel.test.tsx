import { expect, test } from 'claude-code/testing'

const PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=='

const PANE = {
  plugin: 'pixel-studio',
  component: 'Pane',
  requestId: 'pixel-canvas',
  props: {
    title: 'pixel',
    isFocused: false,
    bodyColumns: 60,
    placement: 'dock',
    scroll: { offset: 0, bodyRows: 30 },
    view: {},
  },
} as const

const ROW = {
  plugin: 'pixel-studio',
  component: 'ToolUse',
  requestId: 'tu1',
  props: {
    tool_use_id: 'tu1',
    tool: 'mcp__pixel__draw',
    input: { script: 'circle(8, 8, 4, "red");' },
    isRunning: false,
    isErrored: false,
    isInterrupted: false,
  },
} as const

test('a draw call refreshes the pane, the thumbnail and the status line', async ($, on) => {
  const calls: string[] = []
  on('command.register', () => ({ value: { command: 'pixel' } }))
  on('ui.open', () => ({ value: { isPlaced: true } }))
  on('ui.status', () => ({ value: undefined }))
  on('mcp.call', (_, e) => {
    calls.push(`${e.tool}:${String(e.args.action ?? '')}`)
    if (e.tool === 'canvas') {
      return {
        value: {
          isError: false,
          content: [
            {
              type: 'text',
              text: 'canvas "main" 64x32 · active layer "base" · history 1 undo / 0 redo · checkpoints []\nlayers (bottom → top):\n  0: "base" visible opacity 1.00 blend over · content empty',
            },
          ],
        },
      }
    }
    return {
      value: {
        isError: false,
        content: [
          { type: 'text', text: 'view of x=0 y=0 w=64 h=32 at scale x10, image 640x320' },
          // The shape in which Claude Code hands images to $.mcp.call (the Anthropic API one).
          { type: 'image', source: { type: 'base64', media_type: 'image/png', data: PNG } },
          { type: 'text', text: '[Image: source: /tmp/x.png]' },
        ],
      },
    }
  })
  on('tool.call', () => ({ result: [{ type: 'text', text: 'ok in 1.50 ms · 10 script ops' }] }))

  await $.tool.call({ tool: 'mcp__pixel__draw', script: 'circle(8, 8, 4, "red");', tool_use_id: 'tu1' })
  expect(calls).toEqual(['canvas:info', 'view:'])

  const pane = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect((await pane.find({ type: 'Text', text: /main 64×32/ }))?.text).toBe('main 64×32 · 1.50 ms · 1 step')
  expect(await pane.find({ type: 'Image', key: 'canvas' })).toBeTruthy()
  await pane.unmount()

  const desktop = await $.ui.mount({ ...PANE, surface: 'desktop' })
  expect((await desktop.find({ type: 'Text' }))?.text).toContain('main 64×32')
  await desktop.unmount()

  const row = await $.ui.mount({ ...ROW, surface: 'terminal' })
  expect(await row.find({ type: 'Image', key: 'thumb' })).toBeTruthy()
  await row.unmount()
})

test('views and other tools do not trigger a refresh', async ($, on) => {
  const calls: string[] = []
  on('mcp.call', (_, e) => {
    calls.push(e.tool)
    return { value: { isError: false, content: [] } }
  })
  on('tool.call', () => ({ result: [] }))
  await $.tool.call({ tool: 'mcp__pixel__view', mode: 'stats' })
  await $.tool.call({ tool: 'Bash', command: 'ls' })
  expect(calls).toEqual([])
})

test('the pane invites to draw while there is no canvas', async $ => {
  const pane = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect((await pane.find({ type: 'Text' }))?.text).toBe('No canvas yet.')
  await pane.unmount()
})

test('the server bundled with the plugin is found by its tool names', async ($, on) => {
  const servers: string[] = []
  on('mcp.call', (_, e) => {
    servers.push(e.server)
    return { value: { isError: true, content: [{ type: 'text', text: 'no canvas yet' }] } }
  })
  on('tool.call', () => ({ result: [] }))
  await $.tool.call({ tool: 'mcp__plugin_pixel-studio_pixel__draw', script: 'x;' })
  expect(servers[0]).toBe('plugin_pixel-studio_pixel')
})

function shareServer(on: any, exports: unknown[]) {
  on('mcp.call', (_: unknown, e: { args: Record<string, unknown> }) => {
    if (e.args.action === 'info') {
      return { value: { isError: false, content: [{ type: 'text', text: 'canvas "cat" 64x48 · active layer "base"' }] } }
    }
    exports.push(e.args)
    return {
      value: {
        isError: false,
        content: [{ type: 'text', text: 'exported /home/u/proj/.pixel/share/cat.html (40 KB) in 3 ms: self-contained page' }],
      },
    }
  })
}

test('/pixel share exports the page and publishes it as an artifact', async ($, on) => {
  const exports: unknown[] = []
  const published: unknown[] = []
  shareServer(on, exports)
  on('tool.call', (_, e) => {
    published.push(e)
    return { result: { url: 'https://claude.ai/code/artifact/0f3c-cat' } }
  })
  const r = await $.command.run({ command: 'pixel', args: 'share Cat at sunset' } as never)
  expect(exports[0]).toEqual({ action: 'export', path: '.pixel/share/cat.html', artifact: true, title: 'Cat at sunset', _author: 'user' })
  expect(published[0]).toMatchObject({ tool: 'Artifact', action: 'publish', file_path: '/home/u/proj/.pixel/share/cat.html', icon: 'image' })
  expect(r.text).toContain('Published: https://claude.ai/code/artifact/0f3c-cat')

  const again = await $.command.run({ command: 'pixel', args: 'share' } as never)
  expect(published[1]).toMatchObject({ url: 'https://claude.ai/code/artifact/0f3c-cat' })
  expect(again.text).toContain('Updated')
})

test('/pixel share falls back to the local file when artifacts are unavailable', async ($, on) => {
  shareServer(on, [])
  on('tool.call', () => ({ deny: 'Artifacts are not enabled for this account' }))
  const r = await $.command.run({ command: 'pixel', args: 'share' } as never)
  expect(r.text).toContain('Could not publish it as an Artifact: Artifacts are not enabled')
  expect(r.text).toContain('/home/u/proj/.pixel/share/cat.html')
})

// A fake pixel server with a `w`×`h` canvas.
function fakeServer(on: any, log: Record<string, unknown>[], w = 4, h = 4) {
  on('ui.open', () => ({ value: { isPlaced: true } }))
  on('ui.status', () => ({ value: undefined }))
  on('ui.toast', () => ({ value: undefined }))
  on('command.register', () => ({ value: { command: 'pixel' } }))
  on('mcp.call', (_: unknown, e: { tool: string; args: Record<string, unknown> }) => {
    log.push({ tool: e.tool, ...e.args })
    const text = (t: string) => ({ value: { isError: false, content: [{ type: 'text', text: t }] } })
    if (e.tool === 'view') {
      return {
        value: {
          isError: false,
          content: [
            { type: 'text', text: `view of x=0 y=0 w=${w} h=${h} at scale x32, image ${w * 32}x${h * 32}` },
            { type: 'image', source: { type: 'base64', media_type: 'image/png', data: PNG } },
          ],
        },
      }
    }
    if (e.tool === 'canvas' && e.args.action === 'pending') {
      return text('[pixel] The person edited canvas "cat" since your last call (undo x1).')
    }
    if (e.tool === 'canvas') {
      return text(`canvas "cat" ${w}x${h} · active layer "base" · history 0 undo / 0 redo · checkpoints []`)
    }
    return text('ok in 0.20 ms')
  })
}

test('what the person changed reaches the agent with their next message', async ($, on) => {
  const log: Record<string, unknown>[] = []
  fakeServer(on, log)
  on('tool.call', () => ({ result: [] }))
  on('prompt.submit', (_: unknown, e: { text: string; context?: readonly string[] }) => ({ text: e.text, context: e.context }))
  await $.tool.call({ tool: 'mcp__pixel__draw', script: 'x;' })
  const r = await $.prompt.submit({ text: 'make the tail longer' } as never)
  expect(JSON.stringify(r)).toContain('[pixel] The person edited canvas')
})
