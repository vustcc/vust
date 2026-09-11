import { defineComponent } from 'vue'
import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useTerminalSession } from '@/composables/useTerminalSession'
import { terminalApi } from '@/api/modules/terminal'
import type { TerminalAccess, TerminalServerMessage } from '@/api/generated'

vi.mock('@/api/modules/terminal', () => ({
  terminalApi: {
    fetchAccess: vi.fn(),
    getWebSocketUrl: vi.fn(() => 'ws://localhost/api/v1/node/node-a/terminal/ws'),
  },
}))

const api = vi.mocked(terminalApi)
const access: TerminalAccess = {
  ownership: 'system',
  nodeId: 'node-a',
  nodeName: '节点 A',
  availability: 'available',
  shell: 'bash',
  idleTimeoutSeconds: 1_800,
  capabilities: { canStartSession: true },
  unavailableReason: null,
}

class FakeWebSocket {
  readyState = 0
  binaryType = ''
  sent: Array<string | ArrayBufferLike | Blob | ArrayBufferView> = []
  onopen: (() => void) | null = null
  onmessage: ((event: MessageEvent) => void) | null = null
  onerror: (() => void) | null = null
  onclose: (() => void) | null = null

  send(data: string | ArrayBufferLike | Blob | ArrayBufferView) {
    this.sent.push(data)
  }

  open() {
    this.readyState = WebSocket.OPEN
    this.onopen?.()
  }

  message(message: TerminalServerMessage) {
    this.onmessage?.(new MessageEvent('message', { data: JSON.stringify(message) }))
  }

  close() {
    this.readyState = WebSocket.CLOSED
  }
}

const response = <T>(data: T) => ({ success: true, code: 200, message: '', data })

const mountHarness = () => {
  const sockets: FakeWebSocket[] = []
  const outputs: Uint8Array[] = []
  const Harness = defineComponent({
    setup() {
      return useTerminalSession('node-a', {
        onOutput: (bytes) => outputs.push(bytes),
        createWebSocket: () => {
          const socket = new FakeWebSocket()
          sockets.push(socket)
          return socket as unknown as WebSocket
        },
      })
    },
    template: '<div />',
  })
  return { wrapper: mount(Harness), sockets, outputs }
}

describe('useTerminalSession', () => {
  beforeEach(() => {
    api.fetchAccess.mockResolvedValue(response(access))
  })

  it('传输建立后等待 started 才进入活动状态', async () => {
    const { wrapper, sockets } = mountHarness()
    await wrapper.vm.checkAccess()
    expect(wrapper.vm.connect(80, 24)).toBe(true)
    expect(wrapper.vm.sessionState).toBe('connecting')

    sockets[0]!.open()
    expect(wrapper.vm.sessionState).toBe('starting')
    sockets[0]!.message({
      kind: 'started',
      payload: {
        sessionId: 'session-1',
        shell: 'bash',
        startedAt: '2026-07-16T00:00:00Z',
        idleTimeoutSeconds: 1_800,
      },
    })

    expect(wrapper.vm.sessionState).toBe('active')
    expect(wrapper.vm.sessionId).toBe('session-1')
    wrapper.unmount()
  })

  it('旧连接事件不能覆盖新的连接代次', async () => {
    const { wrapper, sockets } = mountHarness()
    await wrapper.vm.checkAccess()
    wrapper.vm.connect(80, 24)
    const oldSocket = sockets[0]!
    wrapper.vm.cancel()
    wrapper.vm.connect(100, 30)
    const currentSocket = sockets[1]!

    oldSocket.open()
    oldSocket.message({
      kind: 'started',
      payload: {
        sessionId: 'old-session',
        shell: 'sh',
        startedAt: '2026-07-16T00:00:00Z',
        idleTimeoutSeconds: 1_800,
      },
    })
    expect(wrapper.vm.sessionState).toBe('connecting')

    currentSocket.open()
    currentSocket.message({
      kind: 'started',
      payload: {
        sessionId: 'new-session',
        shell: 'bash',
        startedAt: '2026-07-16T00:00:01Z',
        idleTimeoutSeconds: 1_800,
      },
    })
    oldSocket.onerror?.()
    expect(wrapper.vm.sessionState).toBe('active')
    expect(wrapper.vm.sessionId).toBe('new-session')
    wrapper.unmount()
  })

  it('输入使用二进制帧并按 16 KiB 拆分', async () => {
    const { wrapper, sockets } = mountHarness()
    await wrapper.vm.checkAccess()
    wrapper.vm.connect(80, 24)
    const socket = sockets[0]!
    socket.open()
    socket.message({
      kind: 'started',
      payload: {
        sessionId: 'session-1',
        shell: 'bash',
        startedAt: '2026-07-16T00:00:00Z',
        idleTimeoutSeconds: 1_800,
      },
    })
    expect(wrapper.vm.writeInput('a'.repeat(16 * 1024 + 8))).toBe(true)

    const binaryFrames = socket.sent.filter((frame) => typeof frame !== 'string') as Uint8Array[]
    expect(binaryFrames).toHaveLength(2)
    expect(binaryFrames[0]!.byteLength).toBe(16 * 1024)
    expect(binaryFrames[1]!.byteLength).toBe(8)
    wrapper.unmount()
  })

  it('真实退出终态被保留且不会被 close 覆盖', async () => {
    const { wrapper, sockets } = mountHarness()
    await wrapper.vm.checkAccess()
    wrapper.vm.connect(80, 24)
    const socket = sockets[0]!
    socket.open()
    socket.message({
      kind: 'started',
      payload: {
        sessionId: 'session-1',
        shell: 'bash',
        startedAt: '2026-07-16T00:00:00Z',
        idleTimeoutSeconds: 1_800,
      },
    })
    socket.message({
      kind: 'exited',
      payload: {
        sessionId: 'session-1',
        exitCode: 7,
        reason: 'processExited',
        endedAt: '2026-07-16T00:00:01Z',
      },
    })
    socket.onclose?.()
    await flushPromises()

    expect(wrapper.vm.sessionState).toBe('ended')
    expect(wrapper.vm.exitResult?.exitCode).toBe(7)
    wrapper.unmount()
  })
})
