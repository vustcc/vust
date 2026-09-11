/**
 * @file useScriptLibrary.ts
 * @description 脚本库请求状态与当前临时执行会话。
 */

import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { nodesApi, type NodeSummaryResponse } from '@/api/modules/nodes'
import {
  scriptsApi,
  type CreateScriptRequest,
  type ScriptDetail,
  type ScriptRun,
  type ScriptSummary,
  type UpdateScriptRequest,
} from '@/api/modules/scripts'

const SEARCH_DEBOUNCE_MS = 250
const terminalStatuses = new Set(['succeeded', 'failed', 'timedOut', 'cancelled'])

export interface ScriptRequestState {
  initialLoading: boolean
  refreshing: boolean
  error: string
  warning: string
  loadedAt: Date | null
}

const requestState = (): ScriptRequestState => ({
  initialLoading: false,
  refreshing: false,
  error: '',
  warning: '',
  loadedAt: null,
})

/** 管理脚本资产和当前 Dialog 独占的临时执行。 */
export function useScriptLibrary() {
  const { t } = useI18n()
  const scripts = ref<ScriptSummary[]>([])
  const total = ref(0)
  const nodes = ref<NodeSummaryResponse[]>([])
  const detail = ref<ScriptDetail | null>(null)
  const currentRun = ref<ScriptRun | null>(null)
  const pendingActions = ref(new Set<string>())
  const filters = reactive({
    keyword: '',
    page: 1,
    pageSize: 50,
    sortBy: 'updatedAt' as 'name' | 'updatedAt',
    sortOrder: 'desc' as 'asc' | 'desc',
  })
  const listState = ref<ScriptRequestState>(requestState())
  const detailState = ref<ScriptRequestState>(requestState())
  const saveState = ref<ScriptRequestState>(requestState())
  const nodesState = ref<ScriptRequestState>(requestState())
  const runState = ref<ScriptRequestState>(requestState())

  let mounted = false
  let listSequence = 0
  let detailSequence = 0
  let listController: AbortController | null = null
  let detailController: AbortController | null = null
  let searchTimer: number | null = null

  const isRunActive = computed(
    () => !!currentRun.value && !terminalStatuses.has(currentRun.value.status),
  )
  const errorMessage = (error: unknown, fallback: string) =>
    error instanceof Error && error.message ? error.message : fallback
  const isAbort = (error: unknown) =>
    error instanceof Error && (error.name === 'AbortError' || error.name === 'CanceledError')
  const unwrap = <T>(
    response: { success: boolean; data?: T; message: string },
    fallback: string,
  ) => {
    if (!response.success || response.data === undefined)
      throw new Error(response.message || fallback)
    return response.data
  }
  const criteriaKey = () =>
    JSON.stringify({
      keyword: filters.keyword.trim(),
      page: filters.page,
      pageSize: filters.pageSize,
      sortBy: filters.sortBy,
      sortOrder: filters.sortOrder,
    })

  const performAction = async <T>(key: string, action: () => Promise<T>): Promise<T> => {
    if (pendingActions.value.has(key))
      throw new Error(t('app.scriptManager.messages.duplicateAction'))
    pendingActions.value = new Set(pendingActions.value).add(key)
    try {
      return await action()
    } finally {
      const next = new Set(pendingActions.value)
      next.delete(key)
      pendingActions.value = next
    }
  }
  const isActionPending = (key: string) => pendingActions.value.has(key)

  /** 加载在线节点候选，失败时保留已有数据。 */
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
      nodes.value = unwrap(await nodesApi.list(), t('app.scriptManager.messages.loadNodesFailed'))
      nodesState.value = { ...requestState(), loadedAt: new Date() }
    } catch (error) {
      const message = errorMessage(error, t('app.scriptManager.messages.loadNodesFailed'))
      nodesState.value = {
        ...nodesState.value,
        initialLoading: false,
        refreshing: false,
        error: hasData ? '' : message,
        warning: hasData ? message : '',
      }
    }
  }

  /** 刷新摘要列表，旧搜索响应不能覆盖最新筛选。 */
  const refreshScripts = async () => {
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
        await scriptsApi.list(
          {
            keyword: filters.keyword.trim() || undefined,
            page: filters.page,
            pageSize: filters.pageSize,
            sortBy: filters.sortBy,
            sortOrder: filters.sortOrder,
          },
          controller.signal,
        ),
        t('app.scriptManager.messages.loadFailed'),
      )
      if (!mounted || sequence !== listSequence || key !== criteriaKey()) return
      scripts.value = page.items
      total.value = page.total
      listState.value = { ...requestState(), loadedAt: new Date(page.loadedAt) }
    } catch (error) {
      if (isAbort(error) || !mounted || sequence !== listSequence || key !== criteriaKey()) return
      const message = errorMessage(error, t('app.scriptManager.messages.loadFailed'))
      listState.value = {
        ...listState.value,
        initialLoading: false,
        refreshing: false,
        error: hasData ? '' : message,
        warning: hasData ? message : '',
      }
    }
  }

  /** 按需加载单个脚本正文。 */
  const loadDetail = async (scriptId: string) => {
    detailController?.abort()
    const controller = new AbortController()
    detailController = controller
    const sequence = ++detailSequence
    detailState.value = { ...requestState(), initialLoading: true }
    try {
      const value = unwrap(
        await scriptsApi.detail(scriptId, controller.signal),
        t('app.scriptManager.messages.loadDetailFailed'),
      )
      if (!mounted || sequence !== detailSequence || value.scriptId !== scriptId) return null
      detail.value = value
      detailState.value = { ...requestState(), loadedAt: new Date() }
      return value
    } catch (error) {
      if (isAbort(error) || !mounted || sequence !== detailSequence) return null
      detailState.value = {
        ...requestState(),
        error: errorMessage(error, t('app.scriptManager.messages.loadDetailFailed')),
      }
      return null
    }
  }

  const createScript = async (request: CreateScriptRequest) =>
    performAction('save', async () => {
      saveState.value = { ...requestState(), refreshing: true }
      try {
        const value = unwrap(
          await scriptsApi.create(request),
          t('app.scriptManager.messages.saveFailed'),
        )
        saveState.value = { ...requestState(), loadedAt: new Date() }
        await refreshScripts()
        return value
      } catch (error) {
        saveState.value = {
          ...requestState(),
          error: errorMessage(error, t('app.scriptManager.messages.saveFailed')),
        }
        throw error
      }
    })

  const updateScript = async (scriptId: string, request: UpdateScriptRequest) =>
    performAction('save', async () => {
      saveState.value = { ...requestState(), refreshing: true }
      try {
        const value = unwrap(
          await scriptsApi.update(scriptId, request),
          t('app.scriptManager.messages.saveFailed'),
        )
        detail.value = value
        saveState.value = { ...requestState(), loadedAt: new Date() }
        await refreshScripts()
        return value
      } catch (error) {
        saveState.value = {
          ...requestState(),
          error: errorMessage(error, t('app.scriptManager.messages.saveFailed')),
        }
        throw error
      }
    })

  const removeScript = async (scriptId: string) =>
    performAction(`remove:${scriptId}`, async () => {
      await scriptsApi.remove(scriptId)
      if (detail.value?.scriptId === scriptId) detail.value = null
      await refreshScripts()
    })

  /** 创建当前 Dialog 独占的一次性终端执行。 */
  const startRun = async (scriptId: string, nodeId: string, timeoutSeconds?: number) =>
    performAction(`run:${scriptId}`, async () => {
      runState.value = { ...requestState(), refreshing: true }
      try {
        const value = unwrap(
          await scriptsApi.startRun(scriptId, { nodeId, timeoutSeconds }, crypto.randomUUID()),
          t('app.scriptManager.messages.runFailed'),
        )
        currentRun.value = value
        runState.value = { ...requestState(), loadedAt: new Date() }
        return value
      } catch (error) {
        runState.value = {
          ...requestState(),
          error: errorMessage(error, t('app.scriptManager.messages.runFailed')),
        }
        throw error
      }
    })

  /** 关闭执行会话；服务端成功接受销毁后才清空本地结果。 */
  const dismissRun = async () => {
    const runId = currentRun.value?.runId
    if (!runId) return
    await performAction(`dismiss:${runId}`, async () => scriptsApi.dismissRun(runId))
    currentRun.value = null
    runState.value = requestState()
  }

  watch(
    () => filters.keyword,
    () => {
      filters.page = 1
      if (searchTimer !== null) window.clearTimeout(searchTimer)
      searchTimer = window.setTimeout(refreshScripts, SEARCH_DEBOUNCE_MS)
    },
  )
  watch(
    () => [filters.page, filters.sortBy, filters.sortOrder],
    () => void refreshScripts(),
  )

  onMounted(async () => {
    mounted = true
    await Promise.all([refreshNodes(), refreshScripts()])
  })
  onUnmounted(() => {
    mounted = false
    const runId = currentRun.value?.runId
    if (runId) void scriptsApi.dismissRun(runId)
    listController?.abort()
    detailController?.abort()
    if (searchTimer !== null) window.clearTimeout(searchTimer)
  })

  return {
    scripts,
    total,
    nodes,
    detail,
    currentRun,
    isRunActive,
    filters,
    listState,
    detailState,
    saveState,
    nodesState,
    runState,
    refreshNodes,
    refreshScripts,
    loadDetail,
    createScript,
    updateScript,
    removeScript,
    startRun,
    dismissRun,
    isActionPending,
  }
}
