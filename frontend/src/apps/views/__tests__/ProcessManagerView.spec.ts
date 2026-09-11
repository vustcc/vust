import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ProcessListPage } from '@/api/generated'
import ProcessManagerView from '../ProcessManagerView.vue'
import zh from '@/locales/zh'
import { VustSelect } from '@/components/ui'

const api = vi.hoisted(() => ({
  forNode: vi.fn(),
  listProcesses: vi.fn(),
  listNetworkConnections: vi.fn(),
  terminate: vi.fn(),
  createForceKillConfirmation: vi.fn(),
  forceKill: vi.fn(),
}))
const confirmation = vi.hoisted(() => ({ showConfirmation: vi.fn() }))
const notifications = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), warning: vi.fn() }))

vi.mock('@/api/modules/process', () => ({ processApi: { forNode: api.forNode } }))
vi.mock('@/stores/node', () => ({ useNodeStore: () => ({ currentNodeId: 'global-node' }) }))
vi.mock('@/stores/window-manager', () => ({
  useWindowManagerStore: () => ({ updateWindowRuntimeState: vi.fn() }),
}))
vi.mock('@/stores/confirmation-modal', () => ({
  useConfirmationModalStore: () => confirmation,
}))
vi.mock('@/stores/toast', () => ({ useToastStore: () => notifications }))

const response = <T>(data: T) => ({ success: true, code: 200, message: '', data })
const processId = 'a'.repeat(64)
const page: ProcessListPage = {
  entries: [
    {
      processId,
      pid: 42,
      name: 'worker',
      parentPid: 1,
      threadCount: 2,
      userName: 'root',
      state: 'running',
      cpuPercent: 1,
      memoryPercent: 2,
      connectionCount: 0,
      startedAt: '2026-07-16T00:00:00.000Z',
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
}

describe('ProcessManagerView', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    api.forNode.mockReturnValue({
      listProcesses: api.listProcesses,
      listNetworkConnections: api.listNetworkConnections,
      terminate: api.terminate,
      createForceKillConfirmation: api.createForceKillConfirmation,
      forceKill: api.forceKill,
    })
    api.listProcesses.mockResolvedValue(response(page))
    api.listNetworkConnections.mockResolvedValue(
      response({
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
      }),
    )
    api.createForceKillConfirmation.mockResolvedValue(
      response({ confirmationToken: 'confirm-token', expiresAt: '2026-07-16T00:01:00.000Z' }),
    )
    api.forceKill.mockResolvedValue(
      response({
        idempotencyKey: '00000000-0000-4000-8000-000000000001',
        processId,
        pid: 42,
        processName: 'worker',
        signal: 'kill',
        status: 'delivered',
        deliveredAt: '2026-07-16T00:00:01.000Z',
      }),
    )
    confirmation.showConfirmation.mockResolvedValue(true)
  })

  it('固定使用窗口节点且强制结束必须完成两次确认', async () => {
    const wrapper = mount(ProcessManagerView, {
      props: { payload: { nodeId: 'node-a' } },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })
    await flushPromises()
    expect(api.forNode).toHaveBeenCalledWith('node-a')
    expect(wrapper.get('[data-page="process-manager"]').exists()).toBe(true)

    expect(wrapper.findAll('button').some((button) => button.text() === '退出')).toBe(true)
    const killButton = wrapper.findAll('button').find((button) => button.text() === '结束')
    expect(killButton).toBeDefined()
    await killButton!.trigger('click')
    await flushPromises()

    expect(confirmation.showConfirmation).toHaveBeenCalledTimes(2)
    expect(api.createForceKillConfirmation).toHaveBeenCalledWith(processId)
    expect(api.forceKill).toHaveBeenCalledWith(
      processId,
      expect.objectContaining({ confirmationToken: 'confirm-token' }),
    )
    expect(notifications.success).toHaveBeenCalled()
    wrapper.unmount()
  })

  it('状态筛选只提供当前采样中真实存在的状态', async () => {
    api.listProcesses.mockResolvedValue(
      response({
        ...page,
        entries: [{ ...page.entries[0], state: 'sleeping' }],
        counts: { sleeping: 1 },
      }),
    )
    const wrapper = mount(ProcessManagerView, {
      props: { payload: { nodeId: 'node-a' } },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })
    await flushPromises()

    const statusSelect = wrapper
      .findAllComponents(VustSelect)
      .find((select) => select.props('name') === 'processStatus')
    expect(statusSelect?.props('options')).toEqual([
      { label: '全部状态', value: 'ALL' },
      { label: '睡眠', value: 'sleeping' },
    ])
    wrapper.unmount()
  })

  it('网络状态使用大写下划线格式展示且保留原始筛选值', async () => {
    api.listNetworkConnections.mockResolvedValue(
      response({
        entries: [
          {
            connectionId: 'tcp:1',
            protocol: 'tcp',
            localEndpoint: { address: '127.0.0.1', port: 8080 },
            remoteEndpoint: { address: '127.0.0.1', port: 8081 },
            state: 'synSent',
            owners: [],
          },
        ],
        page: 1,
        pageSize: 100,
        availableTotal: 1,
        total: 1,
        byState: { synSent: 1 },
        byProtocol: { tcp: 1 },
        sampledAt: '2026-07-16T00:00:00.000Z',
        coverage: {
          status: 'complete',
          scannedCount: 1,
          succeededCount: 1,
          failedCount: 0,
          warnings: [],
        },
      }),
    )
    const wrapper = mount(ProcessManagerView, {
      props: { payload: { nodeId: 'node-a' } },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })
    await wrapper.get('[data-slot="network-tab"]').trigger('click')
    await flushPromises()

    expect(wrapper.text()).toContain('SYN_SENT')
    const stateSelect = wrapper
      .findAllComponents(VustSelect)
      .find((select) => select.props('name') === 'networkState')
    expect(stateSelect?.props('options')).toEqual([
      { label: 'ALL', value: 'ALL' },
      { label: 'SYN_SENT', value: 'synSent' },
    ])
    wrapper.unmount()
  })

  it('网络连接完整但进程归属不完整时显示独立提示', async () => {
    api.listNetworkConnections.mockResolvedValue(
      response({
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
          scannedCount: 1,
          succeededCount: 1,
          failedCount: 0,
          ownerCoveragePercent: 75,
          warnings: ['some network connections could not be attributed to a process'],
        },
      }),
    )
    const wrapper = mount(ProcessManagerView, {
      props: { payload: { nodeId: 'node-a' } },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })

    await wrapper.get('[data-slot="network-tab"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-ui="network-owner-partial-warning"]').exists()).toBe(true)
    expect(wrapper.find('[data-ui="network-partial-warning"]').exists()).toBe(false)
    expect(wrapper.text()).toContain('进程归属覆盖率为 75%')
    wrapper.unmount()
  })

  it('网络来源不完整时优先显示连接采集告警', async () => {
    api.listNetworkConnections.mockResolvedValue(
      response({
        entries: [],
        page: 1,
        pageSize: 100,
        availableTotal: 0,
        total: 0,
        byState: {},
        byProtocol: {},
        sampledAt: '2026-07-16T00:00:00.000Z',
        coverage: {
          status: 'partial',
          scannedCount: 1,
          succeededCount: 0,
          failedCount: 1,
          ownerCoveragePercent: 50,
          warnings: ['tcp connection source was unavailable'],
        },
      }),
    )
    const wrapper = mount(ProcessManagerView, {
      props: { payload: { nodeId: 'node-a' } },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })

    await wrapper.get('[data-slot="network-tab"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-ui="network-partial-warning"]').exists()).toBe(true)
    expect(wrapper.find('[data-ui="network-owner-partial-warning"]').exists()).toBe(false)
    expect(wrapper.text()).toContain('部分网络连接来源暂不可用')
    wrapper.unmount()
  })
})
