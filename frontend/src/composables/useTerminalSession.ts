/**
 * @file useTerminalSession.ts
 * @description 宿主机终端访问检查与单会话 WebSocket 状态机。
 */

import { computed, onBeforeUnmount, ref } from 'vue'
import type { TerminalAccess, TerminalClientMessage, TerminalServerMessage } from '@/api/generated'
import { terminalApi } from '@/api/modules/terminal'

const MAX_INPUT_FRAME_BYTES = 16 * 1024
const MIN_COLS = 20
const MAX_COLS = 500
const MIN_ROWS = 5
const MAX_ROWS = 200

export type TerminalAccessState = 'idle' | 'checking' | 'ready' | 'unavailable' | 'failed'
export type TerminalSessionState =
  | 'idle'
  | 'connecting'
  | 'starting'
  | 'active'
  | 'idleWarning'
  | 'closing'
  | 'ended'
  | 'failed'

type TerminalErrorPayload = Extract<TerminalServerMessage, { kind: 'error' }>['payload']
type TerminalExitPayload = Extract<TerminalServerMessage, { kind: 'exited' }>['payload']

export interface TerminalSessionOptions {
  onOutput: (bytes: Uint8Array) => void
  onControl?: (message: TerminalServerMessage) => void
  createWebSocket?: (url: string) => WebSocket
}

/** 判断当前终端网格尺寸能否发送给 Agent。 */
const isValidSize = (cols: number, rows: number) =>
  Number.isInteger(cols) &&
  Number.isInteger(rows) &&
  cols >= MIN_COLS &&
  cols <= MAX_COLS &&
  rows >= MIN_ROWS &&
  rows <= MAX_ROWS

/** 管理固定节点上的一次性宿主机终端会话。 */
export function useTerminalSession(nodeId: string, options: TerminalSessionOptions) {
  const access = ref<TerminalAccess | null>(null)
  const accessState = ref<TerminalAccessState>('idle')
  const accessError = ref('')
  const sessionState = ref<TerminalSessionState>('idle')
  const sessionError = ref<TerminalErrorPayload | null>(null)
  const exitResult = ref<TerminalExitPayload | null>(null)
  const sessionId = ref<string | null>(null)
  const actualShell = ref<'bash' | 'sh' | null>(null)
  const idleWarningSeconds = ref<number | null>(null)

  let socket: WebSocket | null = null
  let accessSequence = 0
  let connectionAttempt = 0
  let accessPromise: Promise<void> | null = null
  const createWebSocket = options.createWebSocket ?? ((url: string) => new WebSocket(url))

  /** 发送文本控制帧。 */
  const sendControl = (message: TerminalClientMessage) => {
    if (!socket || socket.readyState !== WebSocket.OPEN) return false
    socket.send(JSON.stringify(message))
    return true
  }

  /** 获取终端可用性；重复检查合并为同一个请求。 */
  const checkAccess = () => {
    if (accessPromise) return accessPromise
    const sequence = ++accessSequence
    accessState.value = 'checking'
    accessError.value = ''
    accessPromise = terminalApi
      .fetchAccess(nodeId)
      .then((response) => {
        if (sequence !== accessSequence) return
        if (!response.success || !response.data) {
          throw new Error(response.message || 'TERMINAL_UNAVAILABLE')
        }
        access.value = response.data
        accessState.value =
          response.data.availability === 'available' && response.data.capabilities.canStartSession
            ? 'ready'
            : 'unavailable'
      })
      .catch((error: unknown) => {
        if (sequence !== accessSequence) return
        access.value = null
        accessState.value = 'failed'
        accessError.value = error instanceof Error ? error.message : 'TERMINAL_UNAVAILABLE'
      })
      .finally(() => {
        if (sequence === accessSequence) accessPromise = null
      })
    return accessPromise
  }

  /** 提交服务端控制事件，旧连接事件由 attempt ID 丢弃。 */
  const handleControl = (attempt: number, raw: string) => {
    if (attempt !== connectionAttempt) return
    let message: TerminalServerMessage
    try {
      message = JSON.parse(raw) as TerminalServerMessage
    } catch {
      sessionError.value = {
        code: 'TERMINAL_PROTOCOL_VIOLATION',
        message: 'Invalid terminal control frame',
        recoverable: false,
      }
      sessionState.value = 'failed'
      socket?.close()
      return
    }
    options.onControl?.(message)
    if (message.kind === 'started') {
      sessionId.value = message.payload.sessionId
      actualShell.value = message.payload.shell
      idleWarningSeconds.value = null
      sessionState.value = 'active'
      return
    }
    if (message.kind === 'idleWarning') {
      idleWarningSeconds.value = message.payload.expiresInSeconds
      sessionState.value = 'idleWarning'
      return
    }
    if (message.kind === 'exited') {
      exitResult.value = message.payload
      idleWarningSeconds.value = null
      sessionState.value = 'ended'
      socket?.close()
      return
    }
    sessionError.value = message.payload
    if (!message.payload.recoverable) {
      sessionState.value = 'failed'
      socket?.close()
    }
  }

  /** 处理原始 PTY 输出，Blob 转换完成后仍校验连接代次。 */
  const handleSocketData = async (attempt: number, data: unknown) => {
    if (attempt !== connectionAttempt) return
    if (data instanceof ArrayBuffer) {
      options.onOutput(new Uint8Array(data))
      return
    }
    if (data instanceof Blob) {
      const buffer = await data.arrayBuffer()
      if (attempt === connectionAttempt) options.onOutput(new Uint8Array(buffer))
      return
    }
    if (typeof data === 'string') handleControl(attempt, data)
  }

  /** 建立新会话；WebSocket open 仅表示传输建立，started 才表示 PTY 可用。 */
  const connect = (cols: number, rows: number) => {
    if (accessState.value !== 'ready' || !isValidSize(cols, rows)) return false
    if (
      ['connecting', 'starting', 'active', 'idleWarning', 'closing'].includes(sessionState.value)
    ) {
      return false
    }
    const attempt = ++connectionAttempt
    sessionError.value = null
    exitResult.value = null
    sessionId.value = null
    actualShell.value = null
    idleWarningSeconds.value = null
    sessionState.value = 'connecting'

    const nextSocket = createWebSocket(terminalApi.getWebSocketUrl(nodeId))
    socket = nextSocket
    nextSocket.binaryType = 'arraybuffer'
    nextSocket.onopen = () => {
      if (attempt !== connectionAttempt || socket !== nextSocket) return
      sessionState.value = 'starting'
      sendControl({ type: 'start', payload: { cols, rows } })
    }
    nextSocket.onmessage = (event) => {
      void handleSocketData(attempt, event.data)
    }
    nextSocket.onerror = () => {
      if (attempt !== connectionAttempt || socket !== nextSocket) return
      sessionError.value = {
        code: 'TERMINAL_UNAVAILABLE',
        message: 'Terminal WebSocket failed',
        recoverable: false,
      }
      sessionState.value = 'failed'
    }
    nextSocket.onclose = () => {
      if (attempt !== connectionAttempt || socket !== nextSocket) return
      socket = null
      if (!['ended', 'failed', 'idle'].includes(sessionState.value)) {
        sessionError.value = {
          code: 'TERMINAL_IO_FAILED',
          message: 'Terminal connection closed unexpectedly',
          recoverable: false,
        }
        sessionState.value = 'failed'
      }
    }
    return true
  }

  /** 发送终端输入，并按 16 KiB 上限拆分二进制帧。 */
  const writeInput = (value: string) => {
    if (!socket || !['active', 'idleWarning'].includes(sessionState.value)) return false
    const bytes = new TextEncoder().encode(value)
    for (let offset = 0; offset < bytes.byteLength; offset += MAX_INPUT_FRAME_BYTES) {
      socket.send(bytes.slice(offset, offset + MAX_INPUT_FRAME_BYTES))
    }
    if (sessionState.value === 'idleWarning') sessionState.value = 'active'
    idleWarningSeconds.value = null
    return true
  }

  /** 调整活动 PTY 的尺寸。 */
  const resize = (cols: number, rows: number) => {
    if (!['active', 'idleWarning'].includes(sessionState.value) || !isValidSize(cols, rows)) {
      return false
    }
    return sendControl({ type: 'resize', payload: { cols, rows } })
  }

  /** 请求 Agent 关闭当前会话。 */
  const disconnect = () => {
    if (!socket || !['active', 'idleWarning'].includes(sessionState.value)) return false
    sessionState.value = 'closing'
    return sendControl({ type: 'close' })
  }

  /** 取消尚未启动的连接尝试。 */
  const cancel = () => {
    if (!['connecting', 'starting'].includes(sessionState.value)) return false
    connectionAttempt += 1
    socket?.close()
    socket = null
    sessionState.value = 'ended'
    return true
  }

  /** 页面卸载时终止连接，不保留后台会话。 */
  const dispose = () => {
    accessSequence += 1
    connectionAttempt += 1
    if (socket?.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify({ type: 'close' } satisfies TerminalClientMessage))
    }
    socket?.close()
    socket = null
  }

  onBeforeUnmount(dispose)

  return {
    access,
    accessState,
    accessError,
    sessionState,
    sessionError,
    exitResult,
    sessionId,
    actualShell,
    idleWarningSeconds,
    canConnect: computed(
      () =>
        accessState.value === 'ready' && ['idle', 'ended', 'failed'].includes(sessionState.value),
    ),
    checkAccess,
    connect,
    writeInput,
    resize,
    disconnect,
    cancel,
    dispose,
  }
}
