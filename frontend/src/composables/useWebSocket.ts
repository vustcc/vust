import { computed, isRef, onBeforeUnmount, ref, watch } from 'vue'
import type { Ref } from 'vue'

type WebSocketData = Parameters<WebSocket['send']>[0]

export interface UseWebSocketOptions {
  /**
   * 目标 WebSocket 地址。可以是字符串或 ref。
   */
  url: string | Ref<string>
  /**
   * 可选的 WebSocket 子协议列表。
   */
  protocols?: string[]
  /**
   * 是否在 setup 时自动尝试连接。
   * @default true
   */
  autoConnect?: boolean
  /**
   * 是否在连接非正常关闭后自动重连。
   * @default true
   */
  autoReconnect?: boolean
  /**
   * 两次重连之间的间隔毫秒数。
   * @default 3000
   */
  reconnectInterval?: number
  /**
   * 最大重连尝试次数。超出后将停止重试。
   * @default Infinity
   */
  maxReconnectAttempts?: number
  /**
   * WebSocket 连接成功打开时的回调函数。
   */
  onOpen?: (event: Event) => void
  /**
   * 收到 WebSocket 消息时的回调函数。
   */
  onMessage?: (event: MessageEvent) => void
  /**
   * WebSocket 连接关闭时的回调函数。
   */
  onClose?: (event: CloseEvent) => void
  /**
   * WebSocket 连接发生错误时的回调函数。
   */
  onError?: (event: Event) => void
}

type ListenerCleanup = () => void

/**
 * 一个用于管理 WebSocket 连接的 Vue Composable 函数。
 *
 * 它封装了连接、断开、发送消息、自动重连等常用功能，并以响应式的方式暴露连接状态和数据。
 *
 * @param options - `UseWebSocketOptions` 配置对象。
 * @returns 返回一个包含 WebSocket 状态和控制函数的对象。
 */
export function useWebSocket(options: UseWebSocketOptions) {
  const socket = ref<WebSocket | null>(null)
  const connected = ref(false)
  const connecting = ref(false)
  const lastMessage = ref<MessageEvent | null>(null)
  const lastError = ref<string | null>(null)
  const reconnectAttempts = ref(0)
  const isReconnecting = ref(false)

  const urlSource = computed(() => {
    return isRef(options.url) ? options.url.value : options.url
  })

  const resolvedProtocols = computed(() => options.protocols ?? [])
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null
  let manualClose = false
  let cleanupListeners: ListenerCleanup | null = null

  const scheduleReconnect = () => {
    if (reconnectTimer) {
      clearTimeout(reconnectTimer)
    }
    reconnectTimer = setTimeout(() => {
      reconnectAttempts.value += 1
      connect()
    }, options.reconnectInterval ?? 3000)
  }

  const attachListeners = (ws: WebSocket): ListenerCleanup => {
    const onOpen = (event: Event) => {
      connected.value = true
      connecting.value = false
      manualClose = false
      isReconnecting.value = false
      reconnectAttempts.value = 0
      lastError.value = null
      options.onOpen?.(event)
    }
    const onMessage = (event: MessageEvent) => {
      lastMessage.value = event
      options.onMessage?.(event)
    }
    const onClose = (event: CloseEvent) => {
      connected.value = false
      connecting.value = false
      options.onClose?.(event)
      if (!manualClose) {
        scheduleReconnect()
      }
    }
    const onError = (event: Event) => {
      lastError.value = 'WebSocket 连接异常'
      options.onError?.(event)
    }

    ws.addEventListener('open', onOpen)
    ws.addEventListener('message', onMessage)
    ws.addEventListener('close', onClose)
    ws.addEventListener('error', onError)

    return () => {
      ws.removeEventListener('open', onOpen)
      ws.removeEventListener('message', onMessage)
      ws.removeEventListener('close', onClose)
      ws.removeEventListener('error', onError)
    }
  }

  const cleanupSocket = () => {
    if (cleanupListeners) {
      cleanupListeners()
    }
    cleanupListeners = null
    if (socket.value) {
      socket.value.close()
      socket.value = null
    }
  }

  const connect = () => {
    const url = urlSource.value?.trim()
    if (!url) {
      lastError.value = '未提供 WebSocket 地址'
      return
    }
    cleanupSocket()
    connecting.value = true
    lastError.value = null
    manualClose = false

    const ws = resolvedProtocols.value.length
      ? new WebSocket(url, resolvedProtocols.value)
      : new WebSocket(url)
    socket.value = ws
    cleanupListeners = attachListeners(ws)
  }

  const disconnect = () => {
    manualClose = true
    isReconnecting.value = false
    if (reconnectTimer) {
      clearTimeout(reconnectTimer)
    }
    reconnectAttempts.value = 0
    cleanupSocket()
  }

  const send = (data: WebSocketData) => {
    if (!connected.value || !socket.value) {
      console.warn('WebSocket is not connected, failed to send data')
      return
    }
    socket.value.send(data)
  }

  watch(
    urlSource,
    (newUrl, oldUrl) => {
      if (newUrl && newUrl !== oldUrl && (options.autoConnect ?? true)) {
        reconnectAttempts.value = 0
        connect()
      }
    },
    { immediate: false },
  )

  if (options.autoConnect ?? true) {
    connect()
  }

  onBeforeUnmount(() => {
    disconnect()
  })

  return {
    /**
     * 手动建立 WebSocket 连接。
     * 如果已经连接，会先断开旧连接再建立新连接。
     */
    connect,
    /**
     * 手动断开 WebSocket 连接。
     * 调用此函数会阻止自动重连。
     */
    disconnect,
    /**
     * 通过 WebSocket 发送数据。
     * @param data - 要发送的数据 (string, Blob, ArrayBuffer等)。
     */
    send,
    /**
     * `ref<boolean>`: 当前 WebSocket 是否已连接并准备好通信。
     */
    connected,
    /**
     * `ref<boolean>`: 是否正在尝试建立连接。
     */
    connecting,
    /**
     * `ref<number>`: 当前已尝试的重连次数。
     */
    reconnectAttempts,
    /**
     * `ref<boolean>`: 是否正在进行重连。
     */
    isReconnecting,
    /**
     * `ref<MessageEvent | null>`: 最近一次收到的消息事件。
     */
    lastMessage,
    /**
     * `ref<string | null>`: 最近一次发生的错误信息。
     */
    lastError,
  }
}
