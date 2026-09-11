import { computed, onScopeDispose, reactive, ref, watch } from 'vue'
import type {
  NetworkConnectionListPage,
  ProcessForceKillConfirmation,
  ProcessListPage,
  ProcessSignalResult,
} from '@/api/generated'
import {
  processApi,
  type NetworkConnectionListParams,
  type ProcessListParams,
} from '@/api/modules/process'

export type ProcessManagerActiveView = 'process' | 'network'
export type ProcessLoadPhase =
  | 'idle'
  | 'initialLoading'
  | 'ready'
  | 'refreshing'
  | 'stale'
  | 'initialError'

const PROCESS_POLL_INTERVAL_MS = 3000
const NETWORK_POLL_INTERVAL_MS = 5000

const createIdempotencyKey = () => {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) return crypto.randomUUID()
  return `00000000-0000-4000-8000-${Date.now().toString(16).padStart(12, '0').slice(-12)}`
}

/**
 * 为进程管理窗口维护固定节点、独立加载状态和非重叠 HTTP 轮询。
 */
export const useProcessManager = (nodeId: string) => {
  const scopedApi = processApi.forNode(nodeId)
  const activeView = ref<ProcessManagerActiveView>('process')
  const processPage = ref<ProcessListPage | null>(null)
  const networkPage = ref<NetworkConnectionListPage | null>(null)
  const processPhase = ref<ProcessLoadPhase>('idle')
  const networkPhase = ref<ProcessLoadPhase>('idle')
  const processError = ref<string | null>(null)
  const networkError = ref<string | null>(null)
  const pendingProcessIds = ref<Set<string>>(new Set())

  const processQuery = reactive<ProcessListParams>({
    page: 1,
    pageSize: 100,
    sortBy: 'pid',
    sortOrder: 'asc',
  })
  const networkQuery = reactive<NetworkConnectionListParams>({
    page: 1,
    pageSize: 100,
    sortBy: 'protocol',
    sortOrder: 'asc',
  })

  let processRequestSequence = 0
  let networkRequestSequence = 0
  let processQueryRevision = 0
  let networkQueryRevision = 0
  let processInFlight = false
  let networkInFlight = false
  let processRequestPending = false
  let networkRequestPending = false
  let pollTimer: ReturnType<typeof setTimeout> | null = null
  let disposed = false

  const clearPoll = () => {
    if (pollTimer !== null) clearTimeout(pollTimer)
    pollTimer = null
  }

  const schedulePoll = (view: ProcessManagerActiveView) => {
    if (disposed || activeView.value !== view) return
    clearPoll()
    const delay = view === 'process' ? PROCESS_POLL_INTERVAL_MS : NETWORK_POLL_INTERVAL_MS
    pollTimer = setTimeout(() => {
      if (view === 'process') void loadProcesses(true)
      else void loadNetworkConnections(true)
    }, delay)
  }

  const loadProcesses = async (background = false): Promise<boolean | undefined> => {
    if (processInFlight) {
      processRequestPending = true
      return undefined
    }
    if (activeView.value === 'process') clearPoll()
    processInFlight = true
    const sequence = ++processRequestSequence
    const revision = processQueryRevision
    const requestNodeId = nodeId
    const params = { ...processQuery }
    if (!processPage.value) processPhase.value = 'initialLoading'
    else if (!background && processPhase.value !== 'stale') processPhase.value = 'refreshing'
    try {
      const response = await scopedApi.listProcesses(params)
      if (
        disposed ||
        requestNodeId !== nodeId ||
        sequence !== processRequestSequence ||
        revision !== processQueryRevision
      ) {
        return undefined
      }
      if (!response.success || !response.data) throw new Error(response.message)
      processPage.value = response.data
      processError.value = null
      processPhase.value = 'ready'
      return true
    } catch (error) {
      if (
        disposed ||
        requestNodeId !== nodeId ||
        sequence !== processRequestSequence ||
        revision !== processQueryRevision
      ) {
        return undefined
      }
      processError.value = error instanceof Error ? error.message : String(error)
      processPhase.value = processPage.value ? 'stale' : 'initialError'
      return false
    } finally {
      processInFlight = false
      if (processRequestPending) {
        processRequestPending = false
        queueMicrotask(() => void loadProcesses(false))
      } else {
        schedulePoll('process')
      }
    }
  }

  const loadNetworkConnections = async (background = false): Promise<boolean | undefined> => {
    if (networkInFlight) {
      networkRequestPending = true
      return undefined
    }
    if (activeView.value === 'network') clearPoll()
    networkInFlight = true
    const sequence = ++networkRequestSequence
    const revision = networkQueryRevision
    const requestNodeId = nodeId
    const params = { ...networkQuery }
    if (!networkPage.value) networkPhase.value = 'initialLoading'
    else if (!background && networkPhase.value !== 'stale') networkPhase.value = 'refreshing'
    try {
      const response = await scopedApi.listNetworkConnections(params)
      if (
        disposed ||
        requestNodeId !== nodeId ||
        sequence !== networkRequestSequence ||
        revision !== networkQueryRevision
      ) {
        return undefined
      }
      if (!response.success || !response.data) throw new Error(response.message)
      networkPage.value = response.data
      networkError.value = null
      networkPhase.value = 'ready'
      return true
    } catch (error) {
      if (
        disposed ||
        requestNodeId !== nodeId ||
        sequence !== networkRequestSequence ||
        revision !== networkQueryRevision
      ) {
        return undefined
      }
      networkError.value = error instanceof Error ? error.message : String(error)
      networkPhase.value = networkPage.value ? 'stale' : 'initialError'
      return false
    } finally {
      networkInFlight = false
      if (networkRequestPending) {
        networkRequestPending = false
        queueMicrotask(() => void loadNetworkConnections(false))
      } else {
        schedulePoll('network')
      }
    }
  }

  const setActiveView = (view: ProcessManagerActiveView) => {
    activeView.value = view
    clearPoll()
    if (view === 'process') void loadProcesses(false)
    else void loadNetworkConnections(false)
  }

  const refresh = () => {
    if (activeView.value === 'process') return loadProcesses(false)
    return loadNetworkConnections(false)
  }

  const withPendingProcess = async <T>(processId: string, operation: () => Promise<T>) => {
    if (pendingProcessIds.value.has(processId)) return undefined
    pendingProcessIds.value = new Set(pendingProcessIds.value).add(processId)
    try {
      return await operation()
    } finally {
      const next = new Set(pendingProcessIds.value)
      next.delete(processId)
      pendingProcessIds.value = next
    }
  }

  const terminate = (processId: string) =>
    withPendingProcess<ProcessSignalResult>(processId, async () => {
      const response = await scopedApi.terminate(processId, {
        idempotencyKey: createIdempotencyKey(),
      })
      if (!response.success || !response.data) throw new Error(response.message)
      void loadProcesses(true)
      return response.data
    })

  const createForceKillConfirmation = async (
    processId: string,
  ): Promise<ProcessForceKillConfirmation> => {
    const response = await scopedApi.createForceKillConfirmation(processId)
    if (!response.success || !response.data) throw new Error(response.message)
    return response.data
  }

  const forceKill = (processId: string, confirmationToken: string) =>
    withPendingProcess<ProcessSignalResult>(processId, async () => {
      const response = await scopedApi.forceKill(processId, {
        idempotencyKey: createIdempotencyKey(),
        confirmationToken,
      })
      if (!response.success || !response.data) throw new Error(response.message)
      void loadProcesses(true)
      return response.data
    })

  watch(
    () => ({ ...processQuery }),
    () => {
      processQueryRevision += 1
      if (activeView.value === 'process') void loadProcesses(false)
    },
  )
  watch(
    () => ({ ...networkQuery }),
    () => {
      networkQueryRevision += 1
      if (activeView.value === 'network') void loadNetworkConnections(false)
    },
  )

  onScopeDispose(() => {
    disposed = true
    processRequestSequence += 1
    networkRequestSequence += 1
    clearPoll()
  })

  void loadProcesses(false)

  return {
    activeView,
    createForceKillConfirmation,
    forceKill,
    networkError,
    networkOwnerCoveragePercent: computed(() => {
      const coverage = networkPage.value?.coverage.ownerCoveragePercent ?? 100
      return coverage < 100 ? Math.floor(coverage * 10) / 10 : 100
    }),
    networkOwnerPartial: computed(() => {
      const coverage = networkPage.value?.coverage.ownerCoveragePercent
      return coverage !== undefined && coverage < 100
    }),
    networkPage,
    networkPartial: computed(() => networkPage.value?.coverage.status === 'partial'),
    networkPhase,
    networkQuery,
    pendingProcessIds,
    processError,
    processPage,
    processPartial: computed(() => processPage.value?.coverage.status === 'partial'),
    processPhase,
    processQuery,
    refresh,
    setActiveView,
    terminate,
  }
}
