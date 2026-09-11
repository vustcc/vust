import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { FirewallRuleListPage } from '@/api/generated'
import FirewallManagerView from '../FirewallManagerView.vue'
import zh from '@/locales/zh'

const api = vi.hoisted(() => ({
  forNode: vi.fn(),
  listRules: vi.fn(),
  fetchRuleDetail: vi.fn(),
}))
const windowRuntime = vi.hoisted(() => ({ updateWindowRuntimeState: vi.fn() }))

vi.mock('@/api/modules/firewall', () => ({ firewallApi: { forNode: api.forNode } }))
vi.mock('@/stores/node', () => ({ useNodeStore: () => ({ currentNodeId: 'local' }) }))
vi.mock('@/stores/window-manager', () => ({ useWindowManagerStore: () => windowRuntime }))

const response = <T>(data: T) => ({ success: true, code: 200, message: '', data })
const rulesPage: FirewallRuleListPage = {
  node: { nodeId: 'local', nodeName: '本机' },
  snapshotId: '00000000-0000-4000-8000-000000000001',
  collectedAt: '2026-07-17T00:00:00.000Z',
  collection: {
    status: 'partial',
    coveragePercent: 50,
    sources: [
      { source: 'nftables', status: 'loaded', ruleCount: 1 },
      { source: 'iptablesV4', status: 'failed', ruleCount: 0 },
    ],
    warnings: [{ code: 'SOURCE_FAILED', source: 'iptablesV4' }],
  },
  entries: [
    {
      ruleId: 'a'.repeat(64),
      kind: 'rule',
      engine: 'nftables',
      family: 'inet',
      table: 'filter',
      chain: 'input',
      position: 1,
      action: 'accept',
      protocol: 'tcp',
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
}

const mountView = () =>
  mount(FirewallManagerView, {
    props: { windowId: 'firewall-window' },
    global: {
      plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
    },
  })

describe('FirewallManagerView', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    api.forNode.mockReturnValue({
      listRules: api.listRules,
      fetchRuleDetail: api.fetchRuleDetail,
    })
    api.listRules.mockResolvedValue(response(rulesPage))
  })

  it('使用稳定 DOM 标记、可访问筛选控件和部分采集提示', async () => {
    const wrapper = mountView()
    await flushPromises()

    expect(wrapper.get('[data-page="firewall-manager"]').exists()).toBe(true)
    expect(wrapper.get('[data-ui="toolbar"]').exists()).toBe(true)
    expect(wrapper.get('[data-ui="table"]').exists()).toBe(true)
    expect(wrapper.get('[data-ui="partial-collection"]').text()).toContain('50%')
    expect(wrapper.get('input#firewall-rule-search').attributes('name')).toBe('firewallRuleSearch')
    expect(wrapper.get('#firewall-engine-filter').attributes('role')).toBe('combobox')
    expect(wrapper.get('input[name="firewallEngineFilter"]').exists()).toBe(true)
    expect(wrapper.text()).not.toContain('删除')
    expect(wrapper.text()).not.toContain('编辑')
    expect(windowRuntime.updateWindowRuntimeState).toHaveBeenCalledWith(
      'firewall-window',
      expect.objectContaining({ allowsNodeSwitch: true, busy: false }),
    )
    wrapper.unmount()
  })

  it('成功读取零条规则时展示真实空状态', async () => {
    api.listRules.mockResolvedValue(
      response({ ...rulesPage, entries: [], availableTotal: 0, total: 0 }),
    )
    const wrapper = mountView()
    await flushPromises()

    expect(wrapper.get('[data-ui="rules-empty"]').text()).toContain('没有可读取')
    expect(wrapper.find('[data-ui="initial-error"]').exists()).toBe(false)
    wrapper.unmount()
  })

  it('首次读取失败只展示本地化标题和重试操作', async () => {
    api.listRules.mockResolvedValue({
      success: false,
      code: 503,
      message: 'firewall rules could not be read from any source',
      errorCode: 'FIREWALL_UNAVAILABLE',
      data: null,
    })
    const wrapper = mountView()
    await flushPromises()

    const errorState = wrapper.get('[data-ui="initial-error"]')
    expect(errorState.get('[data-slot="error-empty"]').exists()).toBe(true)
    expect(errorState.text()).toContain('无法读取防火墙规则')
    expect(errorState.text()).not.toContain('could not be read')
    expect(errorState.find('.error-message').exists()).toBe(false)
    expect(errorState.find('.error-hint').exists()).toBe(false)
    expect(errorState.find('.vl-alert').exists()).toBe(false)
    wrapper.unmount()
  })

  it('首次读取文案使用单行截断容器', async () => {
    api.listRules.mockImplementationOnce(() => new Promise(() => undefined))
    const wrapper = mountView()
    await flushPromises()

    const loadingState = wrapper.get('[data-ui="initial-loading"]')
    expect(loadingState.classes()).toContain('initial-loading-state')
    expect(loadingState.get('.initial-loading').exists()).toBe(true)
    expect(loadingState.get('.vl-loading-text').text()).toBe(
      '正在读取目标节点实际生效的防火墙规则...',
    )
    wrapper.unmount()
  })

  it('刷新按钮使用不可收缩的单行状态文案', async () => {
    const wrapper = mountView()
    await flushPromises()

    const refreshButton = wrapper.get('[data-slot="refresh"]')
    expect(refreshButton.classes()).toContain('refresh-button')
    expect(refreshButton.get('.refresh-label').text()).toBe('刷新规则')
    expect(refreshButton.get('.refresh-label').classes()).toContain('refresh-label')

    api.listRules.mockImplementationOnce(() => new Promise(() => undefined))
    await refreshButton.trigger('click')
    await flushPromises()
    expect(refreshButton.get('.refresh-label').text()).toBe('正在刷新')
    wrapper.unmount()
  })
})
