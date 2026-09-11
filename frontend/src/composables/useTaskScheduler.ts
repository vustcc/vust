/**
 * @file useTaskScheduler.ts
 * @description 计划任务页面的独立请求状态、latest-request-wins 与串行恢复轮询。
 */

import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type {
  CreateScheduledTaskBatchRequest,
  CreateScheduledTaskMigrationRequest,
  CreateScheduledTaskRequest,
  ScheduledTaskBatch,
  ScheduledTaskDetail,
  ScheduledTaskOperation,
  ScheduledTaskRun,
  ScheduledTaskRunOutput,
  ScheduledTaskSummary,
  UpdateScheduledTaskRequest,
} from '@/api/generated/scheduled-tasks'
import type { NodeSummaryResponse } from '@/api/modules/nodes'
import { nodesApi } from '@/api/modules/nodes'
import { taskApi } from '@/api/modules/task'

const POLL_INTERVAL_MS = 2_000
const SEARCH_DEBOUNCE_MS = 250
const TRACKING_KEY = 'vust.scheduledTasks.activeTracking'

export interface TaskRequestState {
  initialLoading: boolean
  refreshing: boolean
  error: string
  warning: string
  loadedAt: Date | null
}

const requestState = (): TaskRequestState => ({
  initialLoading: false,
  refreshing: false,
  error: '',
  warning: '',
  loadedAt: null,
})

const terminalOperationStatuses = new Set(['succeeded', 'partial', 'failed', 'cancelled'])
const terminalRunStatuses = new Set(['succeeded', 'failed', 'timedOut', 'cancelled'])

/** 计划任务页面领域状态。 */
export function useTaskScheduler() {
  const { t } = useI18n()
  const nodes = ref<NodeSummaryResponse[]>([])
  const tasks = ref<ScheduledTaskSummary[]>([])
  const total = ref(0)
  const filters = reactive({
    nodeId: '',
    keyword: '',
    enabled: undefined as boolean | undefined,
    deploymentStatus: '',
    page: 1,
    pageSize: 50,
    sortBy: 'updatedAt' as 'name' | 'nextRunAt' | 'updatedAt',
    sortOrder: 'desc' as 'asc' | 'desc',
  })
  const nodesState = ref<TaskRequestState>(requestState())
  const listState = ref<TaskRequestState>(requestState())
  const detailState = ref<TaskRequestState>(requestState())
  const runsState = ref<TaskRequestState>(requestState())
  const outputState = ref<TaskRequestState>(requestState())
  const detail = ref<ScheduledTaskDetail | null>(null)
  const runs = ref<ScheduledTaskRun[]>([])
  const runsTotal = ref(0)
  const output = ref<ScheduledTaskRunOutput | null>(null)
  const trackedOperations = ref<Record<string, ScheduledTaskOperation>>({})
  const trackedRuns = ref<Record<string, ScheduledTaskRun>>({})
  const trackedBatches = ref<Record<string, ScheduledTaskBatch>>({})
  const pendingActions = ref<Set<string>>(new Set())

  let mounted = false
  let listSequence = 0
  let detailSequence = 0
  let runsSequence = 0
  let outputSequence = 0
  let listController: AbortController | null = null
  let detailController: AbortController | null = null
  let runsController: AbortController | null = null
  let outputController: AbortController | null = null
  let searchTimer: number | null = null
  let pollTimer: number | null = null
  let polling = false

  const errorMessage = (error: unknown, fallback: string) =>
    error instanceof Error && error.message ? error.message : fallback
  const isAbort = (error: unknown) =>
    error instanceof Error && (error.name === 'AbortError' || error.name === 'CanceledError')
  const unwrap = <T>(
    response: { success: boolean; data?: T; message: string },
    fallback: string,
  ) => {
    if (!response.success || !response.data) throw new Error(response.message || fallback)
    return response.data
  }
  const criteriaKey = () =>
    JSON.stringify({
      nodeId: filters.nodeId,
      keyword: filters.keyword.trim(),
      enabled: filters.enabled,
      deploymentStatus: filters.deploymentStatus,
      page: filters.page,
      pageSize: filters.pageSize,
      sortBy: filters.sortBy,
      sortOrder: filters.sortOrder,
    })

  /** 独立加载节点选项。 */
  const refreshNodes = async () => {
    const hasData = nodes.value.length > 0
    nodesState.value = {
      ...nodesState.value,
      initialLoading: !hasData,
      refreshing: hasData,
      error: hasData ? '' : nodesState.value.error,
      warning: '',
    }
    try {
      const data = unwrap(await nodesApi.list(), t('app.taskScheduler.messages.loadNodesFailed'))
      nodes.value = data
      nodesState.value = {
        initialLoading: false,
        refreshing: false,
        error: '',
        warning: '',
        loadedAt: new Date(),
      }
    } catch (error) {
      const message = errorMessage(error, t('app.taskScheduler.messages.loadNodesFailed'))
      nodesState.value = {
        ...nodesState.value,
        initialLoading: false,
        refreshing: false,
        error: hasData ? '' : message,
        warning: hasData ? message : '',
      }
    }
  }

  /** 获取任务摘要；节点、筛选快照与请求序列共同决定响应是否可提交。 */
  const refreshTasks = async () => {
    listController?.abort()
    const controller = new AbortController()
    listController = controller
    const sequence = ++listSequence
    const key = criteriaKey()
    const hasData = listState.value.loadedAt !== null
    listState.value = {
      ...listState.value,
      initialLoading: !hasData,
      refreshing: hasData,
      error: hasData ? '' : listState.value.error,
      warning: '',
    }
    try {
      const page = unwrap(
        await taskApi.list(
          {
            nodeId: filters.nodeId || undefined,
            keyword: filters.keyword.trim() || undefined,
            enabled: filters.enabled,
            deploymentStatus: filters.deploymentStatus || undefined,
            page: filters.page,
            pageSize: filters.pageSize,
            sortBy: filters.sortBy,
            sortOrder: filters.sortOrder,
          },
          controller.signal,
        ),
        t('app.taskScheduler.messages.loadTasksFailed'),
      )
      if (!mounted || sequence !== listSequence || key !== criteriaKey()) return
      tasks.value = page.items
      total.value = page.total
      listState.value = {
        initialLoading: false,
        refreshing: false,
        error: '',
        warning: '',
        loadedAt: new Date(page.loadedAt),
      }
    } catch (error) {
      if (isAbort(error) || !mounted || sequence !== listSequence || key !== criteriaKey()) return
      const message = errorMessage(error, t('app.taskScheduler.messages.loadTasksFailed'))
      listState.value = {
        ...listState.value,
        initialLoading: false,
        refreshing: false,
        error: hasData ? '' : message,
        warning: hasData ? message : '',
      }
    }
  }

  /** 按需加载任务详情，旧详情请求不能覆盖新选择。 */
  const loadDetail = async (taskId: string) => {
    detailController?.abort()
    const controller = new AbortController()
    detailController = controller
    const sequence = ++detailSequence
    detailState.value = { ...requestState(), initialLoading: true }
    try {
      const value = unwrap(
        await taskApi.detail(taskId, controller.signal),
        t('app.taskScheduler.messages.loadDetailFailed'),
      )
      if (!mounted || sequence !== detailSequence) return null
      detail.value = value
      detailState.value = { ...requestState(), loadedAt: new Date() }
      return value
    } catch (error) {
      if (isAbort(error) || !mounted || sequence !== detailSequence) return null
      detailState.value = {
        ...requestState(),
        error: errorMessage(error, t('app.taskScheduler.messages.loadDetailFailed')),
      }
      return null
    }
  }

  /** 按任务分页加载执行记录。 */
  const loadRuns = async (taskId: string, page = 1, pageSize = 50) => {
    runsController?.abort()
    const controller = new AbortController()
    runsController = controller
    const sequence = ++runsSequence
    runsState.value = { ...requestState(), initialLoading: true }
    try {
      const value = unwrap(
        await taskApi.listRuns(taskId, page, pageSize, controller.signal),
        t('app.taskScheduler.messages.loadRunsFailed'),
      )
      if (!mounted || sequence !== runsSequence) return
      runs.value = value.items
      runsTotal.value = value.total
      runsState.value = { ...requestState(), loadedAt: new Date(value.loadedAt) }
    } catch (error) {
      if (isAbort(error) || !mounted || sequence !== runsSequence) return
      runsState.value = {
        ...requestState(),
        error: errorMessage(error, t('app.taskScheduler.messages.loadRunsFailed')),
      }
    }
  }

  /** 分页读取受限输出；切换运行记录时旧输出不会被提交。 */
  const loadOutput = async (taskId: string, runId: string, offsetBytes = 0) => {
    outputController?.abort()
    const controller = new AbortController()
    outputController = controller
    const sequence = ++outputSequence
    outputState.value = { ...requestState(), initialLoading: true }
    try {
      const value = unwrap(
        await taskApi.runOutput(taskId, runId, offsetBytes, 65_536, controller.signal),
        t('app.taskScheduler.messages.loadOutputFailed'),
      )
      if (!mounted || sequence !== outputSequence) return null
      output.value = value
      outputState.value = { ...requestState(), loadedAt: new Date() }
      return value
    } catch (error) {
      if (isAbort(error) || !mounted || sequence !== outputSequence) return null
      outputState.value = {
        ...requestState(),
        error: errorMessage(error, t('app.taskScheduler.messages.loadOutputFailed')),
      }
      return null
    }
  }

  const isActionPending = (key: string) => pendingActions.value.has(key)
  const withSubmissionLock = async <T>(
    key: string,
    action: () => Promise<T>,
  ): Promise<T | null> => {
    if (pendingActions.value.has(key)) return null
    pendingActions.value = new Set(pendingActions.value).add(key)
    try {
      return await action()
    } finally {
      const next = new Set(pendingActions.value)
      next.delete(key)
      pendingActions.value = next
    }
  }

  const persistTracking = () => {
    const operations = Object.values(trackedOperations.value)
      .filter((item) => !terminalOperationStatuses.has(item.status))
      .map((item) => item.operationId)
    const activeRuns = Object.values(trackedRuns.value)
      .filter((item) => !terminalRunStatuses.has(item.status))
      .map((item) => ({ taskId: item.taskId, runId: item.runId }))
    const batches = Object.values(trackedBatches.value)
      .filter((item) => !terminalOperationStatuses.has(item.status))
      .map((item) => item.batchId)
    sessionStorage.setItem(TRACKING_KEY, JSON.stringify({ operations, runs: activeRuns, batches }))
  }

  const schedulePoll = () => {
    if (pollTimer !== null) window.clearTimeout(pollTimer)
    const hasActive =
      Object.values(trackedOperations.value).some(
        (item) => !terminalOperationStatuses.has(item.status),
      ) ||
      Object.values(trackedRuns.value).some((item) => !terminalRunStatuses.has(item.status)) ||
      Object.values(trackedBatches.value).some(
        (item) => !terminalOperationStatuses.has(item.status),
      )
    if (mounted && !document.hidden && hasActive) {
      pollTimer = window.setTimeout(pollTracked, POLL_INTERVAL_MS)
    }
  }

  /** 活动运行与后台操作串行轮询，请求完成后才安排下一轮。 */
  const pollTracked = async () => {
    if (polling || !mounted || document.hidden) return
    polling = true
    let reachedTerminal = false
    try {
      for (const item of Object.values(trackedOperations.value)) {
        if (terminalOperationStatuses.has(item.status)) continue
        const response = await taskApi.operation(item.operationId)
        if (response.success && response.data) {
          trackedOperations.value = {
            ...trackedOperations.value,
            [item.operationId]: response.data,
          }
          reachedTerminal ||= terminalOperationStatuses.has(response.data.status)
        }
      }
      for (const item of Object.values(trackedRuns.value)) {
        if (terminalRunStatuses.has(item.status)) continue
        const response = await taskApi.runDetail(item.taskId, item.runId)
        if (response.success && response.data) {
          trackedRuns.value = { ...trackedRuns.value, [item.runId]: response.data }
          reachedTerminal ||= terminalRunStatuses.has(response.data.status)
        }
      }
      for (const item of Object.values(trackedBatches.value)) {
        if (terminalOperationStatuses.has(item.status)) continue
        const response = await taskApi.batch(item.batchId)
        if (response.success && response.data) {
          trackedBatches.value = { ...trackedBatches.value, [item.batchId]: response.data }
          reachedTerminal ||= terminalOperationStatuses.has(response.data.status)
        }
      }
      persistTracking()
      if (reachedTerminal) await refreshTasks()
    } finally {
      polling = false
      schedulePoll()
    }
  }

  const trackOperation = (operation: ScheduledTaskOperation) => {
    trackedOperations.value = {
      ...trackedOperations.value,
      [operation.operationId]: operation,
    }
    persistTracking()
    schedulePoll()
  }
  const trackRun = (run: ScheduledTaskRun) => {
    trackedRuns.value = { ...trackedRuns.value, [run.runId]: run }
    persistTracking()
    schedulePoll()
  }
  const trackBatch = (batch: ScheduledTaskBatch) => {
    trackedBatches.value = { ...trackedBatches.value, [batch.batchId]: batch }
    persistTracking()
    schedulePoll()
  }

  const createTask = (payload: CreateScheduledTaskRequest) =>
    withSubmissionLock('create', async () => {
      const value = unwrap(
        await taskApi.create(payload),
        t('app.taskScheduler.messages.saveFailed'),
      )
      trackOperation(value.operation)
      await refreshTasks()
      return value
    })
  const updateTask = (taskId: string, payload: UpdateScheduledTaskRequest) =>
    withSubmissionLock(`update:${taskId}`, async () => {
      const value = unwrap(
        await taskApi.update(taskId, payload),
        t('app.taskScheduler.messages.saveFailed'),
      )
      trackOperation(value.operation)
      await refreshTasks()
      return value
    })
  const setTaskEnabled = (taskId: string, enabled: boolean) =>
    withSubmissionLock(`state:${taskId}`, async () => {
      const value = unwrap(
        await taskApi.updateState(taskId, { enabled }),
        t('app.taskScheduler.messages.toggleFailed'),
      )
      trackOperation(value.operation)
      await refreshTasks()
      return value
    })
  const removeTask = (taskId: string) =>
    withSubmissionLock(`remove:${taskId}`, async () => {
      const value = unwrap(
        await taskApi.remove(taskId),
        t('app.taskScheduler.messages.deleteFailed'),
      )
      trackOperation(value)
      await refreshTasks()
      return value
    })
  const startRun = (taskId: string) =>
    withSubmissionLock(`run:${taskId}`, async () => {
      const value = unwrap(
        await taskApi.startRun(taskId),
        t('app.taskScheduler.messages.runFailed'),
      )
      trackRun(value)
      await refreshTasks()
      return value
    })
  const cancelRun = (taskId: string, runId: string) =>
    withSubmissionLock(`cancel-run:${runId}`, async () => {
      const value = unwrap(
        await taskApi.cancelRun(taskId, runId),
        t('app.taskScheduler.messages.cancelRunFailed'),
      )
      trackRun(value)
      return value
    })
  const cancelOperation = (operationId: string) =>
    withSubmissionLock(`cancel-operation:${operationId}`, async () => {
      const value = unwrap(
        await taskApi.cancelOperation(operationId),
        t('app.taskScheduler.messages.cancelOperationFailed'),
      )
      trackOperation(value)
      return value
    })
  const migrateTask = (taskId: string, payload: CreateScheduledTaskMigrationRequest) =>
    withSubmissionLock(`migrate:${taskId}`, async () => {
      const value = unwrap(
        await taskApi.migrate(taskId, payload),
        t('app.taskScheduler.messages.migrateFailed'),
      )
      trackOperation(value)
      await refreshTasks()
      return value
    })
  const createBatch = (payload: CreateScheduledTaskBatchRequest) =>
    withSubmissionLock('batch', async () => {
      const value = unwrap(
        await taskApi.createBatch(payload),
        t('app.taskScheduler.messages.batchFailed'),
      )
      trackBatch(value)
      await refreshTasks()
      return value
    }) as Promise<ScheduledTaskBatch | null>

  const restoreTracking = async () => {
    try {
      const saved = JSON.parse(sessionStorage.getItem(TRACKING_KEY) || '{}') as {
        operations?: string[]
        runs?: Array<{ taskId: string; runId: string }>
        batches?: string[]
      }
      for (const operationId of saved.operations ?? []) {
        const response = await taskApi.operation(operationId)
        if (response.success && response.data) trackOperation(response.data)
      }
      for (const item of saved.runs ?? []) {
        const response = await taskApi.runDetail(item.taskId, item.runId)
        if (response.success && response.data) trackRun(response.data)
      }
      for (const batchId of saved.batches ?? []) {
        const response = await taskApi.batch(batchId)
        if (response.success && response.data) trackBatch(response.data)
      }
    } catch {
      sessionStorage.removeItem(TRACKING_KEY)
    }
  }

  const handleVisibility = () => {
    if (document.hidden) {
      if (pollTimer !== null) window.clearTimeout(pollTimer)
      pollTimer = null
      return
    }
    void pollTracked()
  }

  watch(
    () => [
      filters.nodeId,
      filters.keyword,
      filters.enabled,
      filters.deploymentStatus,
      filters.page,
      filters.pageSize,
      filters.sortBy,
      filters.sortOrder,
    ],
    (current, previous) => {
      if (!mounted) return
      const keywordChanged = current[1] !== previous?.[1]
      if (searchTimer !== null) window.clearTimeout(searchTimer)
      if (keywordChanged) {
        searchTimer = window.setTimeout(() => void refreshTasks(), SEARCH_DEBOUNCE_MS)
      } else {
        void refreshTasks()
      }
    },
  )

  onMounted(() => {
    mounted = true
    document.addEventListener('visibilitychange', handleVisibility)
    void Promise.all([refreshNodes(), refreshTasks(), restoreTracking()])
  })

  onUnmounted(() => {
    mounted = false
    listSequence += 1
    detailSequence += 1
    runsSequence += 1
    outputSequence += 1
    listController?.abort()
    detailController?.abort()
    runsController?.abort()
    outputController?.abort()
    if (searchTimer !== null) window.clearTimeout(searchTimer)
    if (pollTimer !== null) window.clearTimeout(pollTimer)
    document.removeEventListener('visibilitychange', handleVisibility)
  })

  const activeOperations = computed(() =>
    Object.values(trackedOperations.value).filter(
      (item) => !terminalOperationStatuses.has(item.status),
    ),
  )
  const activeRuns = computed(() =>
    Object.values(trackedRuns.value).filter((item) => !terminalRunStatuses.has(item.status)),
  )
  const activeBatches = computed(() =>
    Object.values(trackedBatches.value).filter(
      (item) => !terminalOperationStatuses.has(item.status),
    ),
  )

  return {
    nodes,
    tasks,
    total,
    filters,
    nodesState,
    listState,
    detailState,
    runsState,
    outputState,
    detail,
    runs,
    runsTotal,
    output,
    trackedOperations,
    trackedRuns,
    trackedBatches,
    activeOperations,
    activeRuns,
    activeBatches,
    pendingActions,
    isActionPending,
    refreshNodes,
    refreshTasks,
    loadDetail,
    loadRuns,
    loadOutput,
    createTask,
    updateTask,
    setTaskEnabled,
    removeTask,
    startRun,
    cancelRun,
    cancelOperation,
    migrateTask,
    createBatch,
    trackOperation,
    trackRun,
    trackBatch,
  }
}
