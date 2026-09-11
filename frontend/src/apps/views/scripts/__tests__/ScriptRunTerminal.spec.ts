import { createPinia } from 'pinia'
import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import ScriptRunTerminal from '../ScriptRunTerminal.vue'

let onData: ((value: string) => void) | undefined
const write = vi.fn()
const focus = vi.fn()
const dismiss = vi.fn()
vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    cols = 80
    rows = 24
    options: Record<string, unknown> = {}
    loadAddon() {}
    open() {}
    dispose() {}
    write = write
    focus = focus
    onData(handler: (value: string) => void) {
      onData = handler
    }
  },
}))
vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class {
    fit() {}
  },
}))

class FakeWebSocket {
  static OPEN = 1
  static instances: FakeWebSocket[] = []
  readyState = 1
  binaryType = ''
  sent: unknown[] = []
  onopen: (() => void) | null = null
  onmessage: ((event: { data: unknown }) => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  constructor(public url: string) {
    FakeWebSocket.instances.push(this)
  }
  send(value: unknown) {
    this.sent.push(value)
  }
  close() {
    this.onclose?.()
  }
}

describe('ScriptRunTerminal', () => {
  beforeEach(() => {
    FakeWebSocket.instances = []
    write.mockClear()
    focus.mockClear()
    onData = undefined
    dismiss.mockReset()
    vi.stubGlobal('fetch', dismiss.mockResolvedValue(new Response()))
    vi.stubGlobal('WebSocket', FakeWebSocket)
    vi.stubGlobal(
      'ResizeObserver',
      class {
        observe() {}
        disconnect() {}
      },
    )
  })
  it('发送控制帧和原始输入，接收二进制输出且不重连', async () => {
    const wrapper = mount(ScriptRunTerminal, {
      props: { runId: 'run 1' },
      global: { plugins: [createPinia()] },
    })
    await flushPromises()
    const socket = FakeWebSocket.instances[0]!
    expect(socket.url).toContain('/script-runs/run%201/ws')
    socket.onopen?.()
    expect(JSON.parse(socket.sent[0] as string)).toEqual({
      type: 'start',
      payload: { cols: 80, rows: 24 },
    })
    onData?.('hello')
    expect(Array.from(socket.sent[1] as Uint8Array)).toEqual(
      Array.from(new TextEncoder().encode('hello')),
    )
    socket.onmessage?.({ data: new Uint8Array([65, 66]).buffer })
    expect(write).toHaveBeenCalled()
    const escaped = vi.fn()
    document.addEventListener('keydown', escaped)
    await wrapper.trigger('keydown', { key: 'Escape' })
    expect(escaped).not.toHaveBeenCalled()
    document.removeEventListener('keydown', escaped)
    socket.onerror?.()
    socket.onclose?.()
    expect(wrapper.emitted('disconnected')).toHaveLength(1)
    expect(FakeWebSocket.instances).toHaveLength(1)
    wrapper.unmount()
  })

  it('收到终态控制帧后关闭连接不会误报中断', async () => {
    const wrapper = mount(ScriptRunTerminal, {
      props: { runId: 'completed-run' },
      global: { plugins: [createPinia()] },
    })
    await flushPromises()
    const socket = FakeWebSocket.instances[0]!
    socket.onmessage?.({
      data: JSON.stringify({
        kind: 'exited',
        payload: {
          runId: 'completed-run',
          exitCode: 0,
          status: 'succeeded',
          endedAt: new Date().toISOString(),
        },
      }),
    })

    socket.onclose?.()

    expect(wrapper.emitted('disconnected')).toBeUndefined()
    wrapper.unmount()
  })

  it('页面卸载时关闭会话并以 keepalive 销毁运行', async () => {
    const wrapper = mount(ScriptRunTerminal, {
      props: { runId: 'run/unload' },
      global: { plugins: [createPinia()] },
    })
    await flushPromises()
    const socket = FakeWebSocket.instances[0]!

    window.dispatchEvent(new Event('pagehide'))

    expect(dismiss).toHaveBeenCalledWith('/api/v1/script-runs/run%2Funload', {
      method: 'DELETE',
      keepalive: true,
    })
    expect(socket.sent).toContain(JSON.stringify({ type: 'close' }))
    wrapper.unmount()
  })
})
