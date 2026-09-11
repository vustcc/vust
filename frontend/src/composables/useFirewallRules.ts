/**
 * @file useFirewallRules.ts
 * @description 防火墙规则列表、详情、快照与 latest-request-wins 状态机。
 */

import { computed, onScopeDispose, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { FirewallRuleDetail, FirewallRuleListPage } from '@/api/generated'
import {
  firewallApi,
  type FirewallAction,
  type FirewallEngine,
  type FirewallFamily,
  type FirewallRuleListParams,
  type FirewallRuleSummary,
} from '@/api/modules/firewall'
import { useNodeStore } from '@/stores/node'

export type FirewallLoadPhase =
  | 'idle'
  | 'initialLoading'
  | 'ready'
  | 'empty'
  | 'refreshing'
  | 'stale'
  | 'initialError'

const FILTER_DEBOUNCE_MS = 250

interface ApiFailure extends Error {
  errorCode?: string
  messageKey?: string
}

const responseFailure = (message: string, errorCode?: string, messageKey?: string): ApiFailure => {
  const failure = new Error(message) as ApiFailure
  failure.errorCode = errorCode
  failure.messageKey = messageKey
  return failure
}

/** 维护当前 Node 的防火墙规则观察状态。 */
export function useFirewallRules() {
  const { t, te } = useI18n()
  const nodeStore = useNodeStore()
  const page = ref<FirewallRuleListPage | null>(null)
  const phase = ref<FirewallLoadPhase>('idle')
  const errorFailure = ref<ApiFailure | null>(null)
  const warningFailure = ref<ApiFailure | null>(null)
  const loadedAt = ref<Date | null>(null)
  const keyword = ref('')
  const engine = ref<FirewallEngine | 'all'>('all')
  const family = ref<FirewallFamily | 'all'>('all')
  const action = ref<FirewallAction | 'all'>('all')
  const currentPage = ref(1)
  const pageSize = ref(100)
  const manualRefreshing = ref(false)
  const detail = ref<FirewallRuleDetail | null>(null)
  const detailLoading = ref(false)
  const detailFailure = ref<ApiFailure | null>(null)

  let requestSequence = 0
  let queryRevision = 0
  let detailSequence = 0
  let listController: AbortController | null = null
  let detailController: AbortController | null = null
  let filterTimer: ReturnType<typeof setTimeout> | null = null
  let disposed = false

  const currentNodeId = () => nodeStore.currentNodeId || 'local'
  const normalizeFailure = (failure: unknown): ApiFailure =>
    failure instanceof Error
      ? (failure as ApiFailure)
      : responseFailure(t('app.firewallManager.messages.loadFailed'))
  const translateFailure = (failure: ApiFailure | null) => {
    if (!failure) return ''
    if (failure.messageKey && te(failure.messageKey)) return t(failure.messageKey)
    const errorCodeKey = failure.errorCode ? `api.errors.${failure.errorCode}` : ''
    if (errorCodeKey && te(errorCodeKey)) return t(errorCodeKey)
    return t('app.firewallManager.messages.loadFailed')
  }
  const error = computed(() => translateFailure(errorFailure.value))
  const warning = computed(() => translateFailure(warningFailure.value))
  const detailError = computed(() => translateFailure(detailFailure.value))

  const buildParams = (reuseSnapshot: boolean): FirewallRuleListParams => ({
    snapshotId: reuseSnapshot ? page.value?.snapshotId : undefined,
    query: keyword.value.trim() || undefined,
    engine: engine.value === 'all' ? undefined : engine.value,
    family: family.value === 'all' ? undefined : family.value,
    action: action.value === 'all' ? undefined : action.value,
    page: currentPage.value,
    pageSize: pageSize.value,
    sortBy: 'ruleOrder',
    sortOrder: 'asc',
  })

  /** 加载列表；旧请求被取消且无法提交状态。 */
  const loadRules = async (reuseSnapshot = true): Promise<boolean | undefined> => {
    listController?.abort()
    const controller = new AbortController()
    listController = controller
    const sequence = ++requestSequence
    const revision = queryRevision
    const nodeId = currentNodeId()
    const hasData = page.value !== null
    phase.value = hasData ? 'refreshing' : 'initialLoading'
    if (!hasData) errorFailure.value = null
    warningFailure.value = null
    try {
      const client = firewallApi.forNode(nodeId)
      let params = buildParams(reuseSnapshot)
      let response = await client.listRules(params, controller.signal)
      if (
        !response.success &&
        response.errorCode === 'FIREWALL_SNAPSHOT_EXPIRED' &&
        params.snapshotId
      ) {
        params = { ...params, snapshotId: undefined }
        response = await client.listRules(params, controller.signal)
      }
      if (!response.success || !response.data) {
        throw responseFailure(response.message, response.errorCode, response.messageKey)
      }
      if (
        disposed ||
        controller.signal.aborted ||
        sequence !== requestSequence ||
        revision !== queryRevision ||
        nodeId !== currentNodeId()
      ) {
        return undefined
      }
      page.value = response.data
      loadedAt.value = new Date(response.data.collectedAt)
      errorFailure.value = null
      warningFailure.value = null
      phase.value = response.data.availableTotal === 0 ? 'empty' : 'ready'
      return true
    } catch (failure) {
      if (
        disposed ||
        controller.signal.aborted ||
        sequence !== requestSequence ||
        revision !== queryRevision ||
        nodeId !== currentNodeId()
      ) {
        return undefined
      }
      const normalized = normalizeFailure(failure)
      if (page.value) {
        warningFailure.value = normalized
        phase.value = 'stale'
      } else {
        errorFailure.value = normalized
        phase.value = 'initialError'
      }
      return false
    } finally {
      if (listController === controller) listController = null
    }
  }

  /** 手动重新采集规则；重复点击不会重叠。 */
  const refresh = async () => {
    if (manualRefreshing.value) return
    manualRefreshing.value = true
    currentPage.value = 1
    queryRevision += 1
    try {
      await loadRules(false)
    } finally {
      manualRefreshing.value = false
    }
  }

  /** 按当前不可变快照加载规则详情。 */
  const loadDetail = async (rule: FirewallRuleSummary) => {
    const snapshotId = page.value?.snapshotId
    if (!snapshotId || !rule.capabilities.canViewDetail || detailLoading.value) return
    detailController?.abort()
    const controller = new AbortController()
    detailController = controller
    const sequence = ++detailSequence
    const nodeId = currentNodeId()
    detail.value = null
    detailFailure.value = null
    detailLoading.value = true
    try {
      const response = await firewallApi
        .forNode(nodeId)
        .fetchRuleDetail(rule.ruleId, snapshotId, controller.signal)
      if (!response.success || !response.data) {
        throw responseFailure(response.message, response.errorCode, response.messageKey)
      }
      if (
        disposed ||
        controller.signal.aborted ||
        sequence !== detailSequence ||
        nodeId !== currentNodeId() ||
        snapshotId !== page.value?.snapshotId
      ) {
        return
      }
      detail.value = response.data
    } catch (failure) {
      if (disposed || controller.signal.aborted || sequence !== detailSequence) return
      detailFailure.value = normalizeFailure(failure)
    } finally {
      if (detailController === controller) {
        detailController = null
        detailLoading.value = false
      }
    }
  }

  const clearDetail = () => {
    detailController?.abort()
    detailController = null
    detailSequence += 1
    detail.value = null
    detailFailure.value = null
    detailLoading.value = false
  }

  const scheduleFilterLoad = () => {
    if (filterTimer) clearTimeout(filterTimer)
    filterTimer = setTimeout(() => {
      filterTimer = null
      currentPage.value = 1
      queryRevision += 1
      clearDetail()
      void loadRules(true)
    }, FILTER_DEBOUNCE_MS)
  }

  watch([keyword, engine, family, action], scheduleFilterLoad)

  const goToPage = (nextPage: number) => {
    if (nextPage === currentPage.value || nextPage < 1) return
    currentPage.value = nextPage
    queryRevision += 1
    clearDetail()
    void loadRules(true)
  }

  watch(
    () => nodeStore.currentNodeId,
    () => {
      listController?.abort()
      requestSequence += 1
      queryRevision += 1
      if (filterTimer) clearTimeout(filterTimer)
      filterTimer = null
      page.value = null
      errorFailure.value = null
      warningFailure.value = null
      loadedAt.value = null
      currentPage.value = 1
      clearDetail()
      void loadRules(false)
    },
    { immediate: true },
  )

  onScopeDispose(() => {
    disposed = true
    requestSequence += 1
    detailSequence += 1
    listController?.abort()
    detailController?.abort()
    if (filterTimer) clearTimeout(filterTimer)
  })

  return {
    action,
    currentPage,
    detail,
    detailError,
    detailLoading,
    engine,
    error,
    family,
    goToPage,
    keyword,
    loadedAt,
    manualRefreshing,
    page,
    pageSize,
    phase,
    warning,
    clearDetail,
    loadDetail,
    refresh,
    retry: () => loadRules(false),
    rules: computed(() => page.value?.entries ?? []),
  }
}
