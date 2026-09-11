import { computed, ref, type ComputedRef, type Ref } from 'vue'
import { useWebSocket } from '@/composables/useWebSocket'
import { useNodeStore } from '@/stores/node'

type ShellType = 'bash' | 'sh'

interface TerminalStartPayload {
  container_id: string
  shell: ShellType
  cols: number
  rows: number
}

interface TerminalInputPayload {
  session_id: string
  data: string
}

interface TerminalResizePayload {
  session_id: string
  cols: number
  rows: number
}

interface TerminalClosePayload {
  session_id: string
}

type TerminalClientMessage =
  | { type: 'terminalStart'; payload: TerminalStartPayload }
  | { type: 'terminalInput'; payload: TerminalInputPayload }
  | { type: 'terminalResize'; payload: TerminalResizePayload }
  | { type: 'terminalClose'; payload: TerminalClosePayload }

type TerminalServerMessage =
  | {
      kind: 'terminalStarted'
      payload: { session_id: string; shell: ShellType }
    }
  | {
      kind: 'terminalOutput'
      payload: { session_id: string; data: string }
    }
  | {
      kind: 'terminalExit'
      payload: { session_id: string; exit_code?: number | null }
    }
  | {
      kind: 'terminalError'
      payload: { message: string }
    }

const buildTerminalWsUrl = (nodeId?: string) => {
  const base = '/api/v1'
  const path =
    nodeId && nodeId !== 'local'
      ? `/node/${encodeURIComponent(nodeId)}/agent/websocket/terminal/ws`
      : '/agent/websocket/terminal/ws'

  if (base.startsWith('http')) {
    const url = new URL(base)
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
    url.pathname = url.pathname.replace(/\/$/, '') + path
    return url.toString()
  }

  const origin = window.location.origin.replace(/^http/, 'ws')
  const prefix = base ? base.replace(/\/$/, '') : '/api/v1'
  return `${origin}${prefix}${path}`
}

export const useDockerTerminalWs = (nodeId?: Ref<string> | ComputedRef<string>) => {
  const nodeStore = useNodeStore()
  const wsUrl = computed(() => buildTerminalWsUrl(nodeId?.value ?? nodeStore.currentNodeId))

  const lastTerminalMessage = ref<TerminalServerMessage | null>(null)
  const sessionId = ref<string | null>(null)
  const pendingStart = ref<TerminalStartPayload | null>(null)

  const { connect, disconnect, send, connected, connecting, lastError } = useWebSocket({
    url: wsUrl,
    autoConnect: false,
    autoReconnect: false,
    onOpen: () => {
      if (pendingStart.value) {
        sendMessage({
          type: 'terminalStart',
          payload: pendingStart.value,
        })
      }
    },
    onMessage: (event) => {
      try {
        const parsed = JSON.parse(event.data) as TerminalServerMessage
        lastTerminalMessage.value = parsed
        if (parsed.kind === 'terminalStarted') {
          sessionId.value = parsed.payload.session_id
          pendingStart.value = null
        }
      } catch {
        lastTerminalMessage.value = null
      }
    },
    onClose: () => {
      pendingStart.value = null
      sessionId.value = null
    },
  })

  const sendMessage = (message: TerminalClientMessage) => {
    if (!connected.value) return
    send(JSON.stringify(message))
  }

  const openSession = (payload: TerminalStartPayload) => {
    pendingStart.value = payload
    sessionId.value = null
    lastTerminalMessage.value = null
    connect()
  }

  const writeInput = (data: string) => {
    if (!sessionId.value || !connected.value) return
    sendMessage({
      type: 'terminalInput',
      payload: {
        session_id: sessionId.value,
        data,
      },
    })
  }

  const resizeTerminal = (cols: number, rows: number) => {
    if (!sessionId.value || !connected.value) return
    sendMessage({
      type: 'terminalResize',
      payload: {
        session_id: sessionId.value,
        cols,
        rows,
      },
    })
  }

  const closeSession = () => {
    if (sessionId.value && connected.value) {
      sendMessage({
        type: 'terminalClose',
        payload: { session_id: sessionId.value },
      })
    }
    pendingStart.value = null
    sessionId.value = null
    disconnect()
  }

  return {
    connected,
    connecting,
    lastError,
    sessionId,
    lastTerminalMessage,
    openSession,
    writeInput,
    resizeTerminal,
    closeSession,
  }
}
