/**
 * @file useSystemMonitor.ts
 * @description 系统监控页面状态、latest-request-wins 与非重叠轮询。
 */

import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { SystemMonitoringOverview, SystemMonitoringSeriesPage } from '@/api/generated'
import { systemMonitoringApi, type SystemMonitoringRange } from '@/api/modules/systemMonitoring'
import { useNodeStore } from '@/stores/node'
import { resolveThroughputUnit } from '@/utils/units'

const OVERVIEW_REFRESH_MS = 5_000
const SERIES_REFRESH_MS = 60_000

interface DataRequestState {
  loading: boolean
  refreshing: boolean
  error: string
  warning: string
  loadedAt: Date | null
}

const initialRequestState = (): DataRequestState => ({
  loading: false,
  refreshing: false,
  error: '',
  warning: '',
  loadedAt: null,
})

/** 将服务端 UTC 时间按当前 locale 展示。 */
export const formatMonitoringTime = (value: string | null) =>
  value ? new Date(value).toLocaleString() : ''

/** 系统监控数据管理。 */
export function useSystemMonitor() {
  const { t } = useI18n()
  const nodeStore = useNodeStore()
  const overview = ref<SystemMonitoringOverview | null>(null)
  const series = ref<SystemMonitoringSeriesPage | null>(null)
  const overviewState = ref<DataRequestState>(initialRequestState())
  const seriesState = ref<DataRequestState>(initialRequestState())
  const timeRange = ref<SystemMonitoringRange>('24h')
  const manualRefreshing = ref(false)

  let overviewSequence = 0
  let seriesSequence = 0
  let overviewInFlight = false
  let seriesInFlight = false
  let overviewPending = false
  let seriesPending = false
  let overviewTimer: number | null = null
  let seriesTimer: number | null = null
  let mounted = false

  const currentNodeId = () => nodeStore.currentNodeId || 'local'
  const resolveMessage = (error: unknown) =>
    error instanceof Error && error.message ? error.message : t('app.systemMonitor.fetchFailed')

  /** 获取实时概览；并发调用合并为最多一次后续刷新。 */
  const refreshOverview = async () => {
    if (overviewInFlight) {
      overviewPending = true
      return
    }
    overviewInFlight = true
    const nodeId = currentNodeId()
    const sequence = ++overviewSequence
    const hasData = overview.value !== null
    overviewState.value = {
      ...overviewState.value,
      loading: !hasData,
      refreshing: hasData,
      error: hasData ? overviewState.value.error : '',
      warning: '',
    }
    try {
      const response = await systemMonitoringApi.fetchOverview(nodeId)
      if (!response.success || !response.data) {
        throw new Error(response.message || t('app.systemMonitor.fetchFailed'))
      }
      if (sequence !== overviewSequence || nodeId !== currentNodeId()) return
      overview.value = response.data
      overviewState.value = {
        loading: false,
        refreshing: false,
        error: '',
        warning: '',
        loadedAt: new Date(),
      }
    } catch (error) {
      if (sequence !== overviewSequence || nodeId !== currentNodeId()) return
      const message = resolveMessage(error)
      overviewState.value = {
        ...overviewState.value,
        loading: false,
        refreshing: false,
        error: overview.value ? '' : message,
        warning: overview.value ? message : '',
      }
    } finally {
      overviewInFlight = false
      if (overviewPending && mounted) {
        overviewPending = false
        void refreshOverview()
      }
    }
  }

  /** 获取历史趋势；节点和范围均参与响应提交校验。 */
  const refreshSeries = async () => {
    if (seriesInFlight) {
      seriesPending = true
      return
    }
    seriesInFlight = true
    const nodeId = currentNodeId()
    const range = timeRange.value
    const sequence = ++seriesSequence
    const hasData = series.value !== null
    seriesState.value = {
      ...seriesState.value,
      loading: !hasData,
      refreshing: hasData,
      error: hasData ? seriesState.value.error : '',
      warning: '',
    }
    try {
      const response = await systemMonitoringApi.fetchSeries(nodeId, range)
      if (!response.success || !response.data) {
        throw new Error(response.message || t('app.systemMonitor.fetchFailed'))
      }
      if (sequence !== seriesSequence || nodeId !== currentNodeId() || range !== timeRange.value) {
        return
      }
      series.value = response.data
      seriesState.value = {
        loading: false,
        refreshing: false,
        error: '',
        warning: '',
        loadedAt: new Date(),
      }
    } catch (error) {
      if (sequence !== seriesSequence || nodeId !== currentNodeId() || range !== timeRange.value) {
        return
      }
      const message = resolveMessage(error)
      seriesState.value = {
        ...seriesState.value,
        loading: false,
        refreshing: false,
        error: series.value ? '' : message,
        warning: series.value ? message : '',
      }
    } finally {
      seriesInFlight = false
      if (seriesPending && mounted) {
        seriesPending = false
        void refreshSeries()
      }
    }
  }

  /** 刷新两类数据，不改变手动刷新按钮状态。 */
  const refreshAllData = () => Promise.all([refreshOverview(), refreshSeries()])

  /** 手动刷新两类数据，重复操作会合并而不会重叠。 */
  const refreshAll = async () => {
    if (manualRefreshing.value) return
    manualRefreshing.value = true
    try {
      await refreshAllData()
    } finally {
      manualRefreshing.value = false
    }
  }

  const scheduleOverview = async () => {
    await refreshOverview()
    if (mounted) overviewTimer = window.setTimeout(scheduleOverview, OVERVIEW_REFRESH_MS)
  }

  const scheduleSeries = async () => {
    await refreshSeries()
    if (mounted) seriesTimer = window.setTimeout(scheduleSeries, SERIES_REFRESH_MS)
  }

  const resetForNode = () => {
    overviewSequence += 1
    seriesSequence += 1
    overviewPending = false
    seriesPending = false
    overview.value = null
    series.value = null
    overviewState.value = initialRequestState()
    seriesState.value = initialRequestState()
    void refreshAllData()
  }

  watch(
    () => nodeStore.currentNodeId,
    () => resetForNode(),
  )

  watch(timeRange, () => {
    seriesSequence += 1
    seriesPending = false
    void refreshSeries()
  })

  onMounted(() => {
    mounted = true
    void scheduleOverview()
    void scheduleSeries()
  })

  onUnmounted(() => {
    mounted = false
    overviewSequence += 1
    seriesSequence += 1
    if (overviewTimer !== null) window.clearTimeout(overviewTimer)
    if (seriesTimer !== null) window.clearTimeout(seriesTimer)
  })

  const points = computed(() => series.value?.points ?? [])
  const labels = computed(() => points.value.map((point) => formatMonitoringTime(point.sampledAt)))
  const metric = <K extends keyof (typeof points.value)[number]['metrics']>(key: K) =>
    computed(() => points.value.map((point) => point.metrics[key]))
  const load1 = metric('loadAverage1m')
  const load5 = metric('loadAverage5m')
  const load15 = metric('loadAverage15m')
  const cpu = metric('cpuPercent')
  const memory = metric('memoryPercent')
  const diskRead = metric('diskReadBytesPerSecond')
  const diskWrite = metric('diskWriteBytesPerSecond')
  const netRx = metric('networkReceiveBytesPerSecond')
  const netTx = metric('networkTransmitBytesPerSecond')
  const numericValues = (values: Array<number | null>) =>
    values.filter((value): value is number => value !== null)
  const diskScale = computed(() =>
    resolveThroughputUnit(numericValues([...diskRead.value, ...diskWrite.value])),
  )
  const netScale = computed(() =>
    resolveThroughputUnit(numericValues([...netRx.value, ...netTx.value])),
  )
  const busy = computed(
    () =>
      overviewState.value.loading ||
      overviewState.value.refreshing ||
      seriesState.value.loading ||
      seriesState.value.refreshing,
  )

  return {
    overview,
    series,
    overviewState,
    seriesState,
    timeRange,
    points,
    labels,
    load1,
    load5,
    load15,
    cpu,
    memory,
    diskRead,
    diskWrite,
    netRx,
    netTx,
    diskScale,
    netScale,
    busy,
    manualRefreshing,
    refreshOverview,
    refreshSeries,
    refreshAll,
  }
}
