import { defineStore } from 'pinia'
import { computed, ref, watch } from 'vue'
import { useWebSocket } from '@/composables/useWebSocket'
import { useToastStore } from './toast'
import { useNodeStore } from './node'
import { useI18n } from 'vue-i18n'

// --- TypeScript 接口定义 ---

/**
 * 定义从客户端发送到服务器的 WebSocket 消息结构。
 */
interface ClientWsMessage {
  type: 'subscribeLogs' | 'unsubscribeLogs'
  payload: {
    container_id: string
  }
}

/**
 * 定义从服务器接收的日志快照/追加消息的载荷。
 */
interface LogPayload {
  container_id: string
  lines: string[]
}

/**
 * 定义从服务器接收的通用消息（如结束、错误）的载荷。
 */
interface MessagePayload {
  container_id: string
  message: string
}

/**
 * 定义从服务器接收的所有 WebSocket 消息的类型联合。
 */
type ServerWsMessage =
  | { kind: 'snapshot'; payload: LogPayload }
  | { kind: 'append'; payload: LogPayload }
  | { kind: 'end'; payload: MessagePayload }
  | { kind: 'error'; payload: MessagePayload }
  | { kind: 'heartbeat' }

/**
 * 一个 Pinia store，用于管理一个全局的、共享的 WebSocket 连接。
 *
 * @description
 * 此 store 封装了 `useWebSocket` composable，并增加了基于引用计数的连接管理逻辑。
 * 它允许多个组件共享同一个 WebSocket 连接，按需订阅和取消订阅数据流（如容器日志），
 * 并在没有任何组件需要连接时自动断开，以节省资源。
 *
 * @returns `useWebSocketStore`
 */
export const useWebSocketStore = defineStore('webSocket', () => {
  const { t } = useI18n()
  const toastStore = useToastStore()
  /**
   * `ref<ServerWsMessage | null>`: 最近一次从服务器收到的、已解析的 WebSocket 消息。
   */
  const lastMessage = ref<ServerWsMessage | null>(null)
  /**
   * `ref<Set<string>>`: 一个集合，用于跟踪当前所有活跃的日志订阅（以 containerId 作为标识）。
   * 这是实现引用计数的核心。
   */
  const subscriptions = ref(new Set<string>())
  const activeNodeId = ref<string | null>(null)

  const nodeStore = useNodeStore()
  /**
   * 构建 WebSocket 连接的完整 URL。
   */
  const buildWsUrl = (nodeId?: string) => {
    const base = '/api/v1'
    const path =
      nodeId && nodeId !== 'local'
        ? `/node/${encodeURIComponent(nodeId)}/agent/websocket/events/ws`
        : '/agent/websocket/events/ws'

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

  const wsUrl = computed(() => buildWsUrl(activeNodeId.value ?? nodeStore.currentNodeId))

  const {
    connect,
    disconnect: wsDisconnect, // 重命名原始的 disconnect
    send: wsSend,
    connected,
    connecting,
    lastError,
  } = useWebSocket({
    url: wsUrl,
    autoConnect: false, // 设置为 false，按需连接
    autoReconnect: true,
    reconnectInterval: 5000,
    onMessage: (event) => {
      try {
        lastMessage.value = JSON.parse(event.data)
      } catch (err) {
        console.error('Failed to parse WebSocket message:', err)
        lastMessage.value = null
      }
    },
    onError: (event) => {
      console.error('WebSocket Error:', event)
      toastStore.error(t('websocket.connectionError'))
    },
    onOpen: () => {
      // 当连接（或重连）成功时，重新订阅所有当前活跃的日志流
      for (const containerId of subscriptions.value) {
        sendMessage({
          type: 'subscribeLogs',
          payload: { container_id: containerId },
        })
      }
    },
    onClose: () => {
      // 通知由 useWebSocket 的重连逻辑处理
    },
  })

  /**
   * 封装原始的 disconnect，以立即同步更新状态。
   * 这可以防止在快速重新连接时出现竞态条件。
   */
  const disconnect = () => {
    wsDisconnect()
    connected.value = false
    connecting.value = false
  }

  watch(
    () => nodeStore.currentNodeId,
    (newId, oldId) => {
      if (newId === oldId) return
      if (!activeNodeId.value && (connected.value || connecting.value)) {
        disconnect()
        if (subscriptions.value.size > 0) {
          connect()
        }
      }
    },
  )

  /**
   * 向 WebSocket 服务器发送一个类型化的消息。
   * @param message - 要发送的 `ClientWsMessage`。
   */
  const sendMessage = (message: ClientWsMessage) => {
    if (!connected.value) {
      console.warn('WebSocket is not connected. Message not sent.')
      return
    }
    wsSend(JSON.stringify(message))
  }

  /**
   * 订阅一个容器的实时日志。
   *
   * 如果 WebSocket 未连接，此函数会触发连接。
   * 如果已连接，则直接发送订阅消息。
   * @param containerId - 要订阅日志的容器 ID。
   */
  const subscribeToContainerLogs = (containerId: string, nodeId?: string) => {
    const targetNodeId = nodeId || nodeStore.currentNodeId || 'local'
    if (activeNodeId.value && activeNodeId.value !== targetNodeId) {
      disconnect()
      subscriptions.value.clear()
    }
    activeNodeId.value = targetNodeId
    subscriptions.value.add(containerId)
    if (!connected.value && !connecting.value) {
      connect()
    } else if (connected.value) {
      sendMessage({
        type: 'subscribeLogs',
        payload: { container_id: containerId },
      })
    }
    // 如果正在连接 (`connecting` is true)，`onOpen` 回调会处理订阅
  }

  /**
   * 取消订阅一个容器的实时日志。
   *
   * 如果这是最后一个活跃的订阅，此函数会断开 WebSocket 连接。
   * @param containerId - 要取消订阅的容器 ID。
   */
  const unsubscribeFromContainerLogs = (containerId: string, nodeId?: string) => {
    if (nodeId && activeNodeId.value && nodeId !== activeNodeId.value) return
    if (connected.value) {
      sendMessage({
        type: 'unsubscribeLogs',
        payload: { container_id: containerId },
      })
    }
    subscriptions.value.delete(containerId)
    if (subscriptions.value.size === 0) {
      disconnect()
      activeNodeId.value = null
    }
  }

  return {
    // --- State ---
    /**
     * `ref<boolean>`: 当前 WebSocket 是否已连接并准备好通信。
     */
    connected,
    /**
     * `ref<boolean>`: 是否正在尝试建立连接。
     */
    connecting,
    /**
     * `ref<ServerWsMessage | null>`: 最近一次从服务器收到的、已解析的 WebSocket 消息。
     */
    lastMessage,
    /**
     * `ref<string | null>`: 最近一次发生的错误信息。
     */
    lastError,

    // --- Actions ---
    /**
     * 订阅一个容器的实时日志。
     */
    subscribeToContainerLogs,
    /**
     * 取消订阅一个容器的实时日志。
     */
    unsubscribeFromContainerLogs,
  }
})
