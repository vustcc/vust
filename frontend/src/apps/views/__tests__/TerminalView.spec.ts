import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import TerminalView from '../TerminalView.vue'
import zh from '@/locales/zh'
import { terminalApi } from '@/api/modules/terminal'

const xtermSpies = vi.hoisted(() => ({
  write: vi.fn(),
  focus: vi.fn(),
  clear: vi.fn(),
  reset: vi.fn(),
}))

vi.mock('@/api/modules/terminal', () => ({
  terminalApi: {
    fetchAccess: vi.fn(),
    getWebSocketUrl: vi.fn(() => 'ws://localhost/terminal'),
  },
}))

vi.mock('@/stores/theme', () => ({
  useThemeStore: () => ({ currentTheme: 'light' }),
  buildTerminalTheme: () => ({}),
}))

const updateWindowRuntimeState = vi.fn()
vi.mock('@/stores/window-manager', () => ({
  useWindowManagerStore: () => ({ updateWindowRuntimeState }),
}))

vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    cols = 80
    rows = 24
    options: Record<string, unknown> = {}
    textarea = document.createElement('textarea')
    loadAddon() {}
    open() {}
    onData() {}
    onResize() {}
    write = xtermSpies.write
    focus = xtermSpies.focus
    clear = xtermSpies.clear
    reset = xtermSpies.reset
    dispose() {}
  },
}))

vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class {
    fit() {}
  },
}))

class ResizeObserverStub {
  observe() {}
  disconnect() {}
}

class FakeWebSocket {
  static readonly CONNECTING = 0
  static readonly OPEN = 1
  static readonly CLOSED = 3
  static instances: FakeWebSocket[] = []

  readyState = FakeWebSocket.CONNECTING
  binaryType = ''
  onopen: (() => void) | null = null
  onmessage: ((event: MessageEvent) => void) | null = null
  onerror: (() => void) | null = null
  onclose: (() => void) | null = null

  constructor() {
    FakeWebSocket.instances.push(this)
  }

  send() {}

  open() {
    this.readyState = FakeWebSocket.OPEN
    this.onopen?.()
  }

  message(data: string | ArrayBuffer) {
    this.onmessage?.(new MessageEvent('message', { data }))
  }

  close() {
    this.readyState = FakeWebSocket.CLOSED
  }
}

const api = vi.mocked(terminalApi)

describe('TerminalView', () => {
  beforeEach(() => {
    vi.stubGlobal('ResizeObserver', ResizeObserverStub)
    vi.stubGlobal('WebSocket', FakeWebSocket)
    FakeWebSocket.instances = []
    xtermSpies.write.mockClear()
    xtermSpies.focus.mockClear()
    xtermSpies.clear.mockClear()
    xtermSpies.reset.mockClear()
    api.fetchAccess.mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: {
        ownership: 'system',
        nodeId: 'node-a',
        nodeName: '节点 A',
        availability: 'available',
        shell: 'bash',
        idleTimeoutSeconds: 1_800,
        capabilities: { canStartSession: true },
        unavailableReason: null,
      },
    })
  })

  it('只使用窗口 payload 中固定的目标节点并提供稳定标记', async () => {
    const wrapper = mount(TerminalView, {
      props: {
        windowId: 'terminal-1',
        payload: { nodeId: 'node-a', nodeName: '节点 A' },
      },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })
    await flushPromises()

    expect(api.fetchAccess).toHaveBeenCalledWith('node-a')
    expect(wrapper.get('[data-page="terminal"]').exists()).toBe(true)
    expect(wrapper.get('[data-ui="terminal-toolbar"]').text()).toContain('节点 A')
    expect(wrapper.get('[data-ui="terminal-canvas"]').attributes('aria-label')).toContain('节点 A')
    expect(updateWindowRuntimeState).toHaveBeenCalledWith(
      'terminal-1',
      expect.objectContaining({ allowsNodeSwitch: false }),
    )
    wrapper.unmount()
  })

  it('直接连接、控制事件不写入转录，断开后显示非活动状态且重连清屏', async () => {
    const wrapper = mount(TerminalView, {
      props: { payload: { nodeId: 'node-a', nodeName: '节点 A' } },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })
    await flushPromises()
    await wrapper.get('[data-ui="terminal-connect"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-ui="terminal-confirm-dialog"]').exists()).toBe(false)
    expect(FakeWebSocket.instances).toHaveLength(1)

    const socket = FakeWebSocket.instances[0]!
    socket.open()
    socket.message(
      JSON.stringify({
        kind: 'started',
        payload: {
          sessionId: 'session-1',
          shell: 'bash',
          startedAt: '2026-07-16T00:00:00Z',
          idleTimeoutSeconds: 1_800,
        },
      }),
    )
    expect(xtermSpies.write).not.toHaveBeenCalled()

    socket.message(new TextEncoder().encode('root@localhost:~# ').buffer)
    await flushPromises()
    socket.message(
      JSON.stringify({
        kind: 'exited',
        payload: {
          sessionId: 'session-1',
          exitCode: 0,
          reason: 'processExited',
          endedAt: '2026-07-16T00:00:01Z',
        },
      }),
    )
    await flushPromises()

    expect(xtermSpies.write).toHaveBeenCalledTimes(1)
    expect(wrapper.get('[data-ui="terminal-inactive"]').text()).toContain('退出码：0')

    xtermSpies.clear.mockClear()
    xtermSpies.reset.mockClear()
    await wrapper.get('[data-ui="terminal-reconnect"]').trigger('click')
    await flushPromises()
    expect(xtermSpies.reset).toHaveBeenCalledTimes(1)
    expect(xtermSpies.clear).toHaveBeenCalledTimes(1)
    expect(FakeWebSocket.instances).toHaveLength(2)
    wrapper.unmount()
  })
})
