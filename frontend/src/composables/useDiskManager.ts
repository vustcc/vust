/**
 * @file useDiskManager.ts
 * @description 磁盘清单、按需详情与串行 operation 跟踪状态。
 */

import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  disksApi,
  type CreateDiskOperationRequest,
  type DiskDetail,
  type DiskInventory,
  type DiskOperation,
} from '@/api/modules/disks'

const POLL_MS = 2_000
const terminal = new Set(['succeeded', 'partial', 'failed', 'canceled'])

export interface DiskRequestState {
  initialLoading: boolean
  refreshing: boolean
  error: string
  warning: string
  loadedAt: Date | null
}

const emptyState = (): DiskRequestState => ({
  initialLoading: false,
  refreshing: false,
  error: '',
  warning: '',
  loadedAt: null,
})

/** 创建绑定窗口打开时节点快照的磁盘领域状态。 */
export function useDiskManager(nodeId: string) {
  const { t } = useI18n()
  const inventory = ref<DiskInventory | null>(null)
  const detail = ref<DiskDetail | null>(null)
  const operation = ref<DiskOperation | null>(null)
  const inventoryState = ref<DiskRequestState>(emptyState())
  const detailState = ref<DiskRequestState>(emptyState())
  const operationState = ref<DiskRequestState>(emptyState())
  const submitting = ref(false)
  const cancelling = ref(false)

  let mounted = false
  let inventorySequence = 0
  let detailSequence = 0
  let inventoryController: AbortController | null = null
  let detailController: AbortController | null = null
  let pollController: AbortController | null = null
  let pollTimer: number | null = null
  let polling = false
  const storageKey = `vust.diskManager.activeOperation.${nodeId}`

  const activeOperation = computed(
    () => operation.value !== null && !terminal.has(operation.value.status),
  )
  const errorText = (error: unknown, fallback: string) =>
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

  /** 首次加载或后台刷新；刷新失败保留旧清单。 */
  const refresh = async () => {
    inventoryController?.abort()
    const controller = new AbortController()
    inventoryController = controller
    const sequence = ++inventorySequence
    const snapshot = nodeId
    const hasData = inventory.value !== null
    inventoryState.value = {
      ...inventoryState.value,
      initialLoading: !hasData,
      refreshing: hasData,
      error: hasData ? '' : inventoryState.value.error,
      warning: '',
    }
    try {
      const value = unwrap(
        await disksApi.inventory(snapshot, controller.signal),
        t('app.diskManager.messages.loadFailed'),
      )
      if (
        !mounted ||
        controller.signal.aborted ||
        sequence !== inventorySequence ||
        snapshot !== nodeId
      )
        return
      inventory.value = value
      inventoryState.value = { ...emptyState(), loadedAt: new Date(value.collectedAt) }
    } catch (error) {
      if (isAbort(error) || !mounted || sequence !== inventorySequence) return
      const message = errorText(error, t('app.diskManager.messages.loadFailed'))
      inventoryState.value = {
        ...inventoryState.value,
        initialLoading: false,
        refreshing: false,
        error: hasData ? '' : message,
        warning: hasData ? message : '',
      }
    }
  }

  /** 按需加载详情并校验 diskId 快照。 */
  const loadDetail = async (diskId: string) => {
    detailController?.abort()
    const controller = new AbortController()
    detailController = controller
    const sequence = ++detailSequence
    detailState.value = { ...emptyState(), initialLoading: true }
    try {
      const value = unwrap(
        await disksApi.detail(nodeId, diskId, controller.signal),
        t('app.diskManager.messages.detailFailed'),
      )
      if (!mounted || sequence !== detailSequence || value.diskId !== diskId) return null
      detail.value = value
      detailState.value = { ...emptyState(), loadedAt: new Date() }
      return value
    } catch (error) {
      if (isAbort(error) || !mounted || sequence !== detailSequence) return null
      detailState.value = {
        ...emptyState(),
        error: errorText(error, t('app.diskManager.messages.detailFailed')),
      }
      return null
    }
  }

  /** 幂等提交操作，重复点击不产生第二个请求。 */
  const submit = async (request: CreateDiskOperationRequest) => {
    if (submitting.value || activeOperation.value) return null
    submitting.value = true
    operationState.value = { ...emptyState(), refreshing: true }
    const key = crypto.randomUUID()
    try {
      const value = unwrap(
        await disksApi.createOperation(nodeId, request, key),
        t('app.diskManager.messages.operationFailed'),
      )
      operation.value = value
      sessionStorage.setItem(storageKey, value.operationId)
      operationState.value = { ...emptyState(), loadedAt: new Date() }
      schedulePoll(0)
      return value
    } catch (error) {
      operationState.value = {
        ...emptyState(),
        error: errorText(error, t('app.diskManager.messages.operationFailed')),
      }
      throw error
    } finally {
      submitting.value = false
    }
  }

  const poll = async () => {
    const operationId = operation.value?.operationId || sessionStorage.getItem(storageKey)
    if (!mounted || !operationId || polling || document.hidden) return
    polling = true
    pollController?.abort()
    const controller = new AbortController()
    pollController = controller
    try {
      const value = unwrap(
        await disksApi.operation(nodeId, operationId, controller.signal),
        t('app.diskManager.messages.operationLoadFailed'),
      )
      if (!mounted || controller.signal.aborted) return
      operation.value = value
      operationState.value = { ...emptyState(), loadedAt: new Date() }
      if (terminal.has(value.status)) {
        sessionStorage.removeItem(storageKey)
        await refresh()
        if (detail.value) await loadDetail(detail.value.diskId)
        return
      }
    } catch (error) {
      if (!isAbort(error)) {
        operationState.value.warning = errorText(
          error,
          t('app.diskManager.messages.operationLoadFailed'),
        )
      }
    } finally {
      polling = false
    }
    schedulePoll()
  }

  const schedulePoll = (delay = POLL_MS) => {
    if (pollTimer !== null) window.clearTimeout(pollTimer)
    if (!mounted || document.hidden) return
    pollTimer = window.setTimeout(() => void poll(), delay)
  }

  const cancel = async () => {
    if (!operation.value?.capabilities.canCancel || cancelling.value) return
    cancelling.value = true
    try {
      operation.value = unwrap(
        await disksApi.cancelOperation(nodeId, operation.value.operationId),
        t('app.diskManager.messages.cancelFailed'),
      )
      schedulePoll(0)
    } finally {
      cancelling.value = false
    }
  }

  const onVisibility = () => {
    if (!document.hidden && (activeOperation.value || sessionStorage.getItem(storageKey)))
      schedulePoll(0)
  }

  onMounted(() => {
    mounted = true
    document.addEventListener('visibilitychange', onVisibility)
    void refresh()
    if (sessionStorage.getItem(storageKey)) schedulePoll(0)
  })
  onUnmounted(() => {
    mounted = false
    inventoryController?.abort()
    detailController?.abort()
    pollController?.abort()
    if (pollTimer !== null) window.clearTimeout(pollTimer)
    document.removeEventListener('visibilitychange', onVisibility)
  })

  return {
    inventory,
    detail,
    operation,
    inventoryState,
    detailState,
    operationState,
    submitting,
    cancelling,
    activeOperation,
    refresh,
    loadDetail,
    submit,
    cancel,
  }
}
