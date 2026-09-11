/**
 * @file useContainerLogs.ts
 * @description 容器日志 composable，封装 LogsModal 和 LogsPanel 共享的 ~70% 逻辑。
 *
 * 职责：
 * - WebSocket 消息监听与过滤（按 container_id）
 * - 日志行累积与 MAX_LINES=1000 限制
 * - snapshot / append / end / error 消息分发
 * - 基于滚动位置的自动滚动
 * - REST 回退（`dockerApi.forNode().fetchContainerLogs()`）
 * - 模式管理（realtime / latest）
 * - 状态文本计算
 * - 组件卸载时取消订阅
 */

import { computed, nextTick, onBeforeUnmount, ref, watch, type ComputedRef, type Ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { dockerApi } from '@/api/modules/docker'
import { useWebSocketStore } from '@/stores/websocket'
import { useNodeStore } from '@/stores/node'

/** 日志模式：实时流 或 最新快照 */
export type LogMode = 'realtime' | 'latest'

/** composable 入参选项 */
export interface UseContainerLogsOptions {
  /** 当前容器 ID（响应式） */
  containerId: Ref<string | null> | ComputedRef<string | null>
  /** 容器名称，仅用于 UI 显示 */
  containerName?: Ref<string | null> | ComputedRef<string | null>
  /**
   * 是否处于活跃状态。
   * - LogsModal：映射到 `visible`
   * - LogsPanel：映射到 `active` prop
   */
  active: Ref<boolean>
  /** 目标节点 ID，未提供时回退到 `useNodeStore().currentNodeId` */
  nodeId?: Ref<string> | ComputedRef<string>
}

/**
 * 容器日志 composable。
 *
 * 将 WebSocket 实时日志订阅、REST 拉取、日志行缓冲、自动滚动等逻辑
 * 封装为可复用的组合式函数，供 LogsModal 与 LogsPanel 共同消费。
 */
export function useContainerLogs(options: UseContainerLogsOptions) {
  const { containerId, active, nodeId } = options

  const { t } = useI18n()
  const wsStore = useWebSocketStore()
  const nodeStore = useNodeStore()

  /** 解析实际使用的节点 ID */
  const resolvedNodeId = computed(() => nodeId?.value ?? nodeStore.currentNodeId)

  // --------------- 状态 ---------------

  /** 内存中保留的最大日志行数 */
  const MAX_LINES = 1000

  /** 当前日志模式 */
  const logMode = ref<LogMode>('realtime')

  /** 日志行缓冲 */
  const logLines = ref<string[]>([])

  /** 日志行拼接为文本（Panel 使用 pre 渲染） */
  const logText = computed(() => logLines.value.join('\n'))

  /** 是否正在加载 */
  const isLoading = ref(false)

  /** 当前加载或实时流错误。 */
  const logError = ref<string | null>(null)

  /** 用户是否暂停将实时日志追加到可见缓冲。 */
  const isPaused = ref(false)

  /** 暂停期间暂存的新日志。 */
  const pausedLines = ref<string[]>([])

  /** 实时流是否已结束 */
  const streamEnded = ref(false)

  /** 是否已经订阅了实时流 */
  const realtimeSubscribed = ref(false)

  /** 组件存活标记，防止卸载后异步操作写入 DOM */
  const isAlive = ref(true)
  let requestVersion = 0

  /** 日志滚动容器的模板引用，由消费者通过 `ref` 绑定 */
  const autoScrollEl = ref<HTMLElement | null>(null)

  /** 模式切换标签定义 */
  const logModeTabs = computed(() => [
    { label: t('app.docker.containers.logsPanel.realtimeLogs'), name: 'realtime' },
    { label: t('app.docker.containers.logsPanel.latest'), name: 'latest' },
  ])

  // --------------- 状态文本 ---------------

  /**
   * 根据当前连接状态和模式，计算用户可读的状态文本。
   */
  const statusText = computed(() => {
    if (!containerId.value) return t('app.docker.containers.logsPanel.selectContainerFirst')
    if (!active.value) return t('app.docker.containers.logsPanel.paused')
    if (logMode.value === 'latest') {
      return t('app.docker.containers.logsPanel.latestManual')
    }
    if (isPaused.value) return t('app.docker.containers.logsPanel.paused')
    if (streamEnded.value) return t('app.docker.containers.logsPanel.streamEnded')
    if (wsStore.connecting) return t('app.docker.containers.logsPanel.connecting')
    if (wsStore.connected) return t('app.docker.containers.logsPanel.connected')
    return wsStore.lastError || t('app.docker.containers.logsPanel.disconnected')
  })

  // --------------- 滚动控制 ---------------

  /**
   * 异步滚动日志容器到底部。
   * 使用双层 `requestAnimationFrame` 保证 DOM 更新后才执行。
   */
  const scrollToBottom = async () => {
    await nextTick()
    if (!isAlive.value) return
    const el = autoScrollEl.value
    if (el) {
      requestAnimationFrame(() => {
        if (!isAlive.value) return
        el.scrollTop = el.scrollHeight
        requestAnimationFrame(() => {
          if (!isAlive.value) return
          el.scrollTop = el.scrollHeight
        })
      })
    }
  }

  // --------------- 日志行操作 ---------------

  /**
   * 设置日志行（快照 / 首次加载），可选择是否滚动到底部。
   */
  const setLines = (lines: string[], shouldScroll = true) => {
    logLines.value = lines.slice(-MAX_LINES)
    if (shouldScroll) {
      void scrollToBottom()
    }
  }

  /**
   * 追加日志行，仅在用户滚动条接近底部时自动滚动。
   */
  const appendLines = (lines: string[]) => {
    const el = autoScrollEl.value
    const isScrolledToBottom = el ? el.scrollHeight - el.scrollTop <= el.clientHeight + 20 : true
    logLines.value = [...logLines.value, ...lines].slice(-MAX_LINES)
    if (isScrolledToBottom) {
      void scrollToBottom()
    }
  }

  /** 清空日志行 */
  const clearLogs = () => {
    logLines.value = []
  }

  // --------------- WebSocket 消息监听 ---------------

  /**
   * 监听全局 WebSocket store 中的 `lastMessage`，
   * 过滤出与当前容器相关的消息并分发处理。
   */
  watch(
    () => wsStore.lastMessage,
    (message) => {
      if (!active.value) return
      if (!message || !containerId.value) return
      if (message.kind === 'heartbeat') return
      if (message.payload?.container_id !== containerId.value) return

      isLoading.value = false
      const { kind, payload } = message
      if (kind === 'snapshot') {
        setLines(payload.lines, true)
      } else if (kind === 'append') {
        if (isPaused.value) {
          pausedLines.value = [...pausedLines.value, ...payload.lines].slice(-MAX_LINES)
        } else {
          appendLines(payload.lines)
        }
      } else if (kind === 'end') {
        streamEnded.value = true
        realtimeSubscribed.value = false
      } else if (kind === 'error') {
        realtimeSubscribed.value = false
        logError.value = payload.message || t('app.docker.containers.logsPanel.fetchError')
      }
    },
    { deep: true },
  )

  // --------------- REST 回退 ---------------

  /**
   * 通过 REST API 加载最新的 100 行日志。
   */
  const loadLatestLogs = async () => {
    if (!containerId.value) {
      logError.value = t('app.docker.containers.logsPanel.missingContainerForLogs')
      return
    }
    const requestId = containerId.value
    const requestNodeId = resolvedNodeId.value
    const currentRequest = ++requestVersion
    isLoading.value = true
    logError.value = null
    try {
      const client = dockerApi.forNode(requestNodeId)
      const res = await client.fetchContainerLogs(requestId, 100)
      if (
        !isAlive.value ||
        currentRequest !== requestVersion ||
        containerId.value !== requestId ||
        resolvedNodeId.value !== requestNodeId
      ) {
        return
      }
      if (res.success && res.data) {
        setLines(res.data, false)
      } else {
        logError.value = t('app.docker.containers.logsPanel.latestFailed', {
          message: res.message || t('common.unknownError'),
        })
      }
    } finally {
      if (currentRequest === requestVersion) isLoading.value = false
      if (active.value && logMode.value === 'latest' && logLines.value.length > 0) {
        await scrollToBottom()
      }
    }
  }

  // --------------- 实时流控制 ---------------

  /**
   * 启动实时日志模式：清空日志、设置加载态、发送 WS 订阅。
   */
  const startRealtime = () => {
    if (!containerId.value) {
      logError.value = t('app.docker.containers.logsPanel.missingContainerForRealtime')
      return
    }
    streamEnded.value = false
    isLoading.value = true
    logError.value = null
    isPaused.value = false
    pausedLines.value = []
    logLines.value = []
    wsStore.subscribeToContainerLogs(containerId.value, resolvedNodeId.value)
    realtimeSubscribed.value = true
  }

  /**
   * 停止实时日志模式：发送 WS 取消订阅。
   * @param id - 可选的容器 ID，默认取当前 containerId。
   */
  const stopRealtime = (id?: string | null) => {
    const target = id ?? containerId.value
    if (target) {
      wsStore.unsubscribeFromContainerLogs(target, resolvedNodeId.value)
    }
    realtimeSubscribed.value = false
  }

  /** 暂停或恢复实时日志展示；恢复时按原顺序合并暂存行。 */
  const togglePaused = () => {
    if (logMode.value !== 'realtime') return
    isPaused.value = !isPaused.value
    if (!isPaused.value && pausedLines.value.length > 0) {
      appendLines(pausedLines.value)
      pausedLines.value = []
    }
  }

  // --------------- 模式切换 ---------------

  /**
   * 处理日志模式切换（"实时日志" vs "最新100条"）。
   */
  const switchMode = (mode: LogMode) => {
    if (logMode.value === mode) return
    logMode.value = mode
    if (!active.value) return
    if (mode === 'realtime') {
      startRealtime()
    } else {
      stopRealtime()
      void loadLatestLogs()
    }
  }

  // --------------- 自动响应 containerId / active 变化 ---------------

  /** 当 containerId 变化时，清理旧订阅并按当前模式重新加载 */
  watch(
    containerId,
    (next, prev) => {
      if (prev) {
        stopRealtime(prev)
      }
      streamEnded.value = false
      logLines.value = []
      logError.value = null
      isPaused.value = false
      pausedLines.value = []
      requestVersion += 1
      if (!next) return
      if (!active.value) return
      if (logMode.value === 'realtime') {
        startRealtime()
      } else {
        void loadLatestLogs()
      }
    },
    { immediate: true },
  )

  /** 当 active 变化时，按需启停 */
  watch(active, (isActive) => {
    if (!containerId.value) return
    if (!isActive) {
      requestVersion += 1
      stopRealtime(containerId.value)
      isLoading.value = false
      isPaused.value = false
      pausedLines.value = []
      return
    }
    if (logMode.value === 'realtime') {
      startRealtime()
    } else {
      void loadLatestLogs()
    }
  })

  // --------------- 生命周期清理 ---------------

  onBeforeUnmount(() => {
    isAlive.value = false
    requestVersion += 1
    stopRealtime()
  })

  // --------------- 返回 ---------------

  return {
    /** 日志行数组 */
    logLines,
    /** 日志行拼接文本 */
    logText,
    /** 加载状态 */
    isLoading,
    logError,
    isPaused,
    pausedLineCount: computed(() => pausedLines.value.length),
    /** 当前日志模式 */
    logMode,
    /** 模式切换标签定义 */
    logModeTabs,
    /** 用户可读状态文本 */
    statusText,
    /** 实时流是否已结束 */
    streamEnded,
    /** 是否已订阅实时流 */
    realtimeSubscribed,
    /** 组件存活标记 */
    isAlive,
    /** 日志滚动容器元素引用 */
    autoScrollEl,
    /** 滚动到底部 */
    scrollToBottom,
    /** 清空日志 */
    clearLogs,
    /** 通过 REST 加载最新日志 */
    loadLatestLogs,
    /** 启动实时日志订阅 */
    startRealtime,
    /** 停止实时日志订阅 */
    stopRealtime,
    togglePaused,
    /** 切换日志模式 */
    switchMode,
  }
}
