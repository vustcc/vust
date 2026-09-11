import { defineComponent } from 'vue'
import { createPinia } from 'pinia'
import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { FirewallRuleListPage } from '@/api/generated'
import { useFirewallRules } from '@/composables/useFirewallRules'
import { useNodeStore } from '@/stores/node'

const api = vi.hoisted(() => ({
  forNode: vi.fn(),
  listRules: vi.fn(),
  fetchRuleDetail: vi.fn(),
}))

vi.mock('@/api/modules/firewall', () => ({ firewallApi: { forNode: api.forNode } }))

const response = <T>(data: T) => ({ success: true, code: 200, message: '', data })
const page = (nodeId: string, action: 'accept' | 'drop' = 'accept'): FirewallRuleListPage => ({
  node: { nodeId, nodeName: nodeId },
  snapshotId: '00000000-0000-4000-8000-000000000001',
  collectedAt: '2026-07-17T00:00:00.000Z',
  collection: { status: 'complete', coveragePercent: 100, sources: [], warnings: [] },
  entries: [
    {
      ruleId: 'a'.repeat(64),
      kind: 'rule',
      engine: 'nftables',
      family: 'inet',
      table: 'filter',
      chain: 'input',
      position: 1,
      action,
      protocol: 'tcp',
      sourceAddress: '10.0.0.0/8',
      sourcePorts: [],
      destinationPorts: ['22'],
      capabilities: { canViewDetail: true },
    },
  ],
  page: 1,
  pageSize: 100,
  availableTotal: 1,
  total: 1,
  capabilities: { canViewDetail: true },
})

const deferred = <T>() => {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise
  })
  return { promise, resolve }
}

const Harness = defineComponent({
  setup() {
    return useFirewallRules()
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
        api: { errors: { FIREWALL_UNAVAILABLE: '当前无法读取节点防火墙规则。' } },
        app: {
          nodes: { master: '本机', targetNodeMissing: '节点不存在' },
          firewallManager: { messages: { loadFailed: '加载失败' } },
        },
      },
      en: {
        api: {
          errors: { FIREWALL_UNAVAILABLE: 'Firewall rules are currently unavailable.' },
        },
        app: {
          nodes: { master: 'Local', targetNodeMissing: 'Node missing' },
          firewallManager: { messages: { loadFailed: 'Load failed' } },
        },
      },
    },
  })
  const wrapper = mount(Harness, { global: { plugins: [pinia, i18n] } })
  return { wrapper, nodeStore: useNodeStore(pinia), i18n }
}

describe('useFirewallRules', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.clearAllMocks()
    localStorage.clear()
    api.forNode.mockReturnValue({
      listRules: api.listRules,
      fetchRuleDetail: api.fetchRuleDetail,
    })
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('节点快速切换时旧响应不能覆盖当前节点', async () => {
    const oldRequest = deferred<ReturnType<typeof response<FirewallRuleListPage>>>()
    const newRequest = deferred<ReturnType<typeof response<FirewallRuleListPage>>>()
    api.listRules
      .mockImplementationOnce(() => oldRequest.promise)
      .mockImplementationOnce(() => newRequest.promise)
    const { wrapper, nodeStore } = mountHarness()
    nodeStore.setNodes([
      { id: 'local', name: '本机', address: '', status: 'online', tags: [] },
      { id: 'node-2', name: '节点 2', address: '', status: 'online', tags: [] },
    ])
    nodeStore.currentNodeId = 'node-2'
    await flushPromises()

    oldRequest.resolve(response(page('local', 'accept')))
    await flushPromises()
    expect(wrapper.vm.page).toBeNull()

    newRequest.resolve(response(page('node-2', 'drop')))
    await flushPromises()
    expect(wrapper.vm.page?.node.nodeId).toBe('node-2')
    expect(wrapper.vm.page?.entries[0]?.action).toBe('drop')
    wrapper.unmount()
  })

  it('同节点刷新失败保留旧规则并进入 stale', async () => {
    api.listRules.mockResolvedValueOnce(response(page('local'))).mockResolvedValueOnce({
      success: false,
      code: 503,
      message: 'temporary failure',
      errorCode: 'FIREWALL_UNAVAILABLE',
      data: null,
    })
    const { wrapper, i18n } = mountHarness()
    await flushPromises()
    await wrapper.vm.refresh()

    expect(wrapper.vm.page?.entries).toHaveLength(1)
    expect(wrapper.vm.phase).toBe('stale')
    expect(wrapper.vm.warning).toBe('当前无法读取节点防火墙规则。')

    i18n.global.locale.value = 'en'
    await flushPromises()
    expect(wrapper.vm.warning).toBe('Firewall rules are currently unavailable.')
    wrapper.unmount()
  })

  it('快照过期时自动去掉 snapshotId 重建一次', async () => {
    api.listRules
      .mockResolvedValueOnce(response(page('local')))
      .mockResolvedValueOnce({
        success: false,
        code: 410,
        message: 'expired',
        errorCode: 'FIREWALL_SNAPSHOT_EXPIRED',
        data: null,
      })
      .mockResolvedValueOnce(response(page('local', 'drop')))
    const { wrapper } = mountHarness()
    await flushPromises()

    wrapper.vm.goToPage(2)
    await flushPromises()
    expect(api.listRules).toHaveBeenNthCalledWith(
      2,
      expect.objectContaining({ snapshotId: expect.any(String), page: 2 }),
      expect.any(AbortSignal),
    )
    expect(api.listRules).toHaveBeenNthCalledWith(
      3,
      expect.objectContaining({ snapshotId: undefined, page: 2 }),
      expect.any(AbortSignal),
    )
    expect(wrapper.vm.page?.entries[0]?.action).toBe('drop')
    wrapper.unmount()
  })

  it('重复手动刷新只提交一个新快照请求', async () => {
    api.listRules.mockResolvedValueOnce(response(page('local')))
    const refreshRequest = deferred<ReturnType<typeof response<FirewallRuleListPage>>>()
    api.listRules.mockImplementationOnce(() => refreshRequest.promise)
    const { wrapper } = mountHarness()
    await flushPromises()

    void wrapper.vm.refresh()
    void wrapper.vm.refresh()
    expect(api.listRules).toHaveBeenCalledTimes(2)
    refreshRequest.resolve(response(page('local', 'drop')))
    await flushPromises()
    expect(wrapper.vm.manualRefreshing).toBe(false)
    wrapper.unmount()
  })
})
