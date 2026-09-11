import { defineComponent } from 'vue'
import { createPinia } from 'pinia'
import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useNodeStore } from '@/stores/node'
import { useSystemMonitor } from '@/composables/useSystemMonitor'
import { systemMonitoringApi } from '@/api/modules/systemMonitoring'
import type { SystemMonitoringOverview, SystemMonitoringSeriesPage } from '@/api/generated'

vi.mock('@/api/modules/systemMonitoring', () => ({
  systemMonitoringApi: {
    fetchOverview: vi.fn(),
    fetchSeries: vi.fn(),
  },
}))

const api = vi.mocked(systemMonitoringApi)

const overview = (cpuPercent: number): SystemMonitoringOverview => ({
  ownership: 'system',
  observedAt: '2026-07-15T00:00:00Z',
  snapshotStatus: 'fresh',
  coveragePercent: 100,
  sources: [],
  metrics: {
    cpuPercent,
    memoryUsedBytes: 1,
    memoryTotalBytes: 2,
    memoryPercent: 50,
    loadAverage1m: 0,
    loadAverage5m: 0,
    loadAverage15m: 0,
    diskReadBytesPerSecond: null,
    diskWriteBytesPerSecond: null,
    networkReceiveBytesPerSecond: null,
    networkTransmitBytesPerSecond: null,
  },
  history: {
    state: 'running',
    sampleIntervalSeconds: 60,
    retentionDays: 7,
    lastSampledAt: '2026-07-15T00:00:00Z',
  },
  capabilities: { canManageCollection: true, canClearHistory: true },
})

const emptySeries: SystemMonitoringSeriesPage = {
  range: '24h',
  from: '2026-07-14T00:00:00Z',
  to: '2026-07-15T00:00:00Z',
  resolutionSeconds: 300,
  seriesStatus: 'empty',
  expectedPointCount: 288,
  actualPointCount: 0,
  coveragePercent: 0,
  points: [],
  pageInfo: { limit: 500, hasMore: false, nextCursor: null },
}

const response = <T>(data: T) => ({
  success: true,
  code: 200,
  message: '',
  data,
})

const deferred = <T>() => {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, resolve, reject }
}

const Harness = defineComponent({
  setup() {
    return useSystemMonitor()
  },
  template: '<div />',
})

const mountHarness = () => {
  const pinia = createPinia()
  const i18n = createI18n({
    legacy: false,
    locale: 'zh',
    messages: {
      zh: {
        app: {
          nodes: { master: '本机' },
          systemMonitor: { fetchFailed: '加载失败' },
        },
      },
    },
  })
  const wrapper = mount(Harness, { global: { plugins: [pinia, i18n] } })
  return { wrapper, nodeStore: useNodeStore(pinia) }
}

describe('useSystemMonitor', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    localStorage.clear()
    api.fetchSeries.mockResolvedValue(response(emptySeries))
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('节点切换后旧节点响应不能覆盖新状态', async () => {
    const oldRequest = deferred<ReturnType<typeof response<SystemMonitoringOverview>>>()
    const newRequest = deferred<ReturnType<typeof response<SystemMonitoringOverview>>>()
    api.fetchOverview
      .mockImplementationOnce(() => oldRequest.promise)
      .mockImplementationOnce(() => newRequest.promise)
    const { wrapper, nodeStore } = mountHarness()
    nodeStore.setNodes([
      { id: 'local', name: '本机', address: '', status: 'online', tags: [] },
      { id: 'node-2', name: '节点 2', address: '', status: 'online', tags: [] },
    ])
    nodeStore.currentNodeId = 'node-2'
    await flushPromises()

    oldRequest.resolve(response(overview(10)))
    await flushPromises()
    expect(api.fetchOverview).toHaveBeenCalledTimes(2)
    newRequest.resolve(response(overview(80)))
    await flushPromises()

    expect(wrapper.vm.overview?.metrics.cpuPercent).toBe(80)
    wrapper.unmount()
  })

  it('重复刷新合并且请求不重叠', async () => {
    const first = deferred<ReturnType<typeof response<SystemMonitoringOverview>>>()
    api.fetchOverview
      .mockImplementationOnce(() => first.promise)
      .mockResolvedValue(response(overview(20)))
    const { wrapper } = mountHarness()
    await wrapper.vm.refreshOverview()
    void wrapper.vm.refreshOverview()
    void wrapper.vm.refreshOverview()
    expect(api.fetchOverview).toHaveBeenCalledTimes(1)

    first.resolve(response(overview(10)))
    await flushPromises()
    expect(api.fetchOverview).toHaveBeenCalledTimes(2)
    wrapper.unmount()
  })

  it('同节点刷新失败保留旧数据并产生非阻塞警告', async () => {
    api.fetchOverview
      .mockResolvedValueOnce(response(overview(30)))
      .mockRejectedValueOnce(new Error('temporary failure'))
    const { wrapper } = mountHarness()
    await flushPromises()
    await wrapper.vm.refreshOverview()

    expect(wrapper.vm.overview?.metrics.cpuPercent).toBe(30)
    expect(wrapper.vm.overviewState.warning).toBe('temporary failure')
    expect(wrapper.vm.overviewState.error).toBe('')
    wrapper.unmount()
  })

  it('自动轮询不触发手动刷新按钮状态', async () => {
    api.fetchOverview.mockResolvedValue(response(overview(30)))
    const { wrapper } = mountHarness()
    await flushPromises()

    expect(wrapper.vm.manualRefreshing).toBe(false)
    await vi.advanceTimersByTimeAsync(5_000)
    expect(wrapper.vm.manualRefreshing).toBe(false)

    wrapper.unmount()
  })

  it('切换时间范围时保留旧趋势直到新响应提交', async () => {
    const nextSeries = deferred<ReturnType<typeof response<SystemMonitoringSeriesPage>>>()
    api.fetchOverview.mockResolvedValue(response(overview(30)))
    api.fetchSeries
      .mockResolvedValueOnce(response(emptySeries))
      .mockImplementationOnce(() => nextSeries.promise)
    const { wrapper } = mountHarness()
    await flushPromises()

    wrapper.vm.timeRange = '7d'
    await flushPromises()
    expect(wrapper.vm.series?.range).toBe('24h')
    expect(wrapper.vm.seriesState.refreshing).toBe(true)

    nextSeries.resolve(response({ ...emptySeries, range: '7d' }))
    await flushPromises()
    expect(wrapper.vm.series?.range).toBe('7d')
    expect(wrapper.vm.seriesState.refreshing).toBe(false)

    wrapper.unmount()
  })
})
