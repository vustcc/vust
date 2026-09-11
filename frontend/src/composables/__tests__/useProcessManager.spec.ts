import { defineComponent, nextTick } from 'vue'
import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { NetworkConnectionListPage, ProcessListPage } from '@/api/generated'
import { useProcessManager } from '@/composables/useProcessManager'

const api = vi.hoisted(() => ({
  forNode: vi.fn(),
  listProcesses: vi.fn(),
  listNetworkConnections: vi.fn(),
  terminate: vi.fn(),
  createForceKillConfirmation: vi.fn(),
  forceKill: vi.fn(),
}))

vi.mock('@/api/modules/process', () => ({ processApi: { forNode: api.forNode } }))

const response = <T>(data: T) => ({ success: true, code: 200, message: '', data })

const processPage = (name: string): ProcessListPage => ({
  entries: [
    {
      processId: 'a'.repeat(64),
      pid: 42,
      name,
      parentPid: 1,
      threadCount: 1,
      userName: 'root',
      state: 'running',
      management: { kind: 'custom' },
      capabilities: { canTerminate: true, canForceKill: true },
    },
  ],
  page: 1,
  pageSize: 100,
  availableTotal: 1,
  total: 1,
  counts: { running: 1 },
  sampledAt: '2026-07-16T00:00:00.000Z',
  coverage: {
    status: 'complete',
    scannedCount: 1,
    succeededCount: 1,
    failedCount: 0,
    warnings: [],
  },
})

const networkPage = (): NetworkConnectionListPage => ({
  entries: [],
  page: 1,
  pageSize: 100,
  availableTotal: 0,
  total: 0,
  byState: {},
  byProtocol: {},
  sampledAt: '2026-07-16T00:00:00.000Z',
  coverage: {
    status: 'complete',
    scannedCount: 0,
    succeededCount: 0,
    failedCount: 0,
    warnings: [],
  },
})

const deferred = <T>() => {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((complete) => {
    resolve = complete
  })
  return { promise, resolve }
}

const Harness = defineComponent({
  setup() {
    return useProcessManager('node-a')
  },
  template: '<div />',
})

describe('useProcessManager', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    api.forNode.mockReturnValue({
      listProcesses: api.listProcesses,
      listNetworkConnections: api.listNetworkConnections,
      terminate: api.terminate,
      createForceKillConfirmation: api.createForceKillConfirmation,
      forceKill: api.forceKill,
    })
    api.listProcesses.mockResolvedValue(response(processPage('initial')))
    api.listNetworkConnections.mockResolvedValue(response(networkPage()))
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('固定节点且并发筛选变化只提交最新查询结果', async () => {
    const first = deferred<ReturnType<typeof response<ProcessListPage>>>()
    const latest = deferred<ReturnType<typeof response<ProcessListPage>>>()
    api.listProcesses.mockReset()
    api.listProcesses.mockReturnValueOnce(first.promise).mockReturnValueOnce(latest.promise)
    const wrapper = mount(Harness)

    wrapper.vm.processQuery.query = 'old'
    await nextTick()
    wrapper.vm.processQuery.query = 'latest'
    await nextTick()
    expect(api.forNode).toHaveBeenCalledWith('node-a')
    expect(api.listProcesses).toHaveBeenCalledTimes(1)

    first.resolve(response(processPage('old result')))
    await flushPromises()
    expect(api.listProcesses).toHaveBeenCalledTimes(2)
    expect(api.listProcesses.mock.calls[1]?.[0]).toMatchObject({ query: 'latest' })
    latest.resolve(response(processPage('latest result')))
    await flushPromises()

    expect(wrapper.vm.processPage?.entries[0]?.name).toBe('latest result')
    wrapper.unmount()
  })

  it('后台刷新失败时保留旧进程数据并进入 stale', async () => {
    const wrapper = mount(Harness)
    await flushPromises()
    api.listProcesses.mockResolvedValueOnce({
      success: false,
      code: 503,
      message: 'sampler unavailable',
      data: null,
    })

    await wrapper.vm.refresh()

    expect(wrapper.vm.processPage?.entries[0]?.name).toBe('initial')
    expect(wrapper.vm.processPhase).toBe('stale')
    expect(wrapper.vm.processError).toBe('sampler unavailable')
    wrapper.unmount()
  })

  it('仅轮询活动标签且轮询请求不重叠', async () => {
    vi.useFakeTimers()
    const wrapper = mount(Harness)
    await flushPromises()
    expect(api.listProcesses).toHaveBeenCalledTimes(1)

    wrapper.vm.setActiveView('network')
    await flushPromises()
    expect(api.listNetworkConnections).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(3000)
    expect(api.listProcesses).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(2000)
    expect(api.listNetworkConnections).toHaveBeenCalledTimes(2)
    wrapper.unmount()
  })

  it('区分连接来源不完整与进程归属不完整', async () => {
    const ownerPartialPage = networkPage()
    ownerPartialPage.coverage.ownerCoveragePercent = 62.4
    api.listNetworkConnections.mockResolvedValue(response(ownerPartialPage))
    const wrapper = mount(Harness)

    wrapper.vm.setActiveView('network')
    await flushPromises()

    expect(wrapper.vm.networkPartial).toBe(false)
    expect(wrapper.vm.networkOwnerPartial).toBe(true)
    expect(wrapper.vm.networkOwnerCoveragePercent).toBe(62.4)
    wrapper.unmount()
  })
})
