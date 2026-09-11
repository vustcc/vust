import { createI18n } from 'vue-i18n'
import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { operationLogApi } from '@/api/modules/operation-log'
import OperationLogView from '@/apps/views/OperationLogView.vue'
import { VustDialog } from '@/components/ui'
import en from '@/locales/en'
import zh from '@/locales/zh'

vi.mock('@/api/modules/operation-log', () => ({
  operationLogApi: { query: vi.fn(), detail: vi.fn() },
}))
vi.mock('@/stores/node', () => ({
  useNodeStore: () => ({
    nodes: [
      { id: 'local', name: '本地节点' },
      { id: 'node-81', name: '节点 81' },
    ],
    refreshNodes: vi.fn().mockResolvedValue(undefined),
  }),
}))

const response = (eventId: string) => ({
  success: true,
  code: 200,
  message: '',
  data: {
    total: 1,
    page: 1,
    pageSize: 20,
    items: [
      {
        eventId,
        occurredAt: '2026-07-17T00:00:00Z',
        module: 'nodes',
        eventCode: 'node_update',
        actor: { kind: 'user', displayName: eventId },
        clientIp: '::ffff:10.121.7.7',
        origin: { kind: 'master' },
        target: { kind: 'node', id: eventId },
        outcome: 'success',
        impact: 'info',
        traceId: 'trace',
        capabilities: { canViewDetails: true },
      },
    ],
  },
})

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

enableAutoUnmount(afterEach)
afterEach(() => {
  document.body.innerHTML = ''
})

describe('OperationLogView', () => {
  it('使用标准中英文名称展示节点离线与恢复事件', async () => {
    const result = response('node-offline-event')
    result.data.total = 2
    result.data.items = [
      { ...result.data.items[0], eventId: 'node-offline-event', eventCode: 'node_offline' },
      { ...result.data.items[0], eventId: 'node-recovered-event', eventCode: 'node_recovered' },
    ]
    vi.mocked(operationLogApi.query).mockReset().mockResolvedValue(result)
    const zhWrapper = mount(OperationLogView, {
      global: { plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })] },
    })
    await flushPromises()

    expect(zhWrapper.find('[data-ui="table"]').text()).toContain('节点离线')
    expect(zhWrapper.find('[data-ui="table"]').text()).toContain('节点恢复')
    zhWrapper.unmount()

    vi.mocked(operationLogApi.query).mockReset().mockResolvedValue(result)
    const enWrapper = mount(OperationLogView, {
      global: { plugins: [createI18n({ legacy: false, locale: 'en', messages: { zh, en } })] },
    })
    await flushPromises()

    expect(enWrapper.find('[data-ui="table"]').text()).toContain('Node offline')
    expect(enWrapper.find('[data-ui="table"]').text()).toContain('Node recovered')
    enWrapper.unmount()
  })

  it('展示子节点来源并把节点筛选传给查询接口', async () => {
    const result = response('remote-docker-event')
    result.data.items[0] = {
      ...result.data.items[0],
      module: 'docker',
      origin: { kind: 'agent', nodeId: 'node-81', nodeName: '节点 81' },
    }
    vi.mocked(operationLogApi.query).mockReset().mockResolvedValue(result)
    const wrapper = mount(OperationLogView, {
      global: { plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })] },
    })
    await flushPromises()

    expect(wrapper.find('[data-ui="origin-node"]').text()).toBe('节点 81')
    await wrapper.find('[data-ui="toolbar-actions"] button').trigger('click')
    const vm = wrapper.vm as unknown as {
      filters: { nodeId: string }
      applyFilters: () => void
    }
    vm.filters.nodeId = 'node-81'
    vm.applyFilters()
    await flushPromises()

    expect(vi.mocked(operationLogApi.query).mock.calls.at(-1)?.[0].nodeIds).toEqual(['node-81'])
  })

  it('只允许最新请求更新数据并提供稳定控件标识', async () => {
    const first = deferred<ReturnType<typeof response>>()
    const second = deferred<ReturnType<typeof response>>()
    vi.mocked(operationLogApi.query)
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise)
    const wrapper = mount(OperationLogView, {
      global: { plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })] },
    })
    const vm = wrapper.vm as unknown as { load: () => Promise<void> }
    const latest = vm.load()
    second.resolve(response('latest'))
    await latest
    first.resolve(response('stale'))
    await flushPromises()
    expect(wrapper.text()).toContain('latest (::ffff:10.121.7.7)')
    expect(wrapper.text()).not.toContain('stale')
    expect(wrapper.text()).not.toContain('Master')
    expect(wrapper.text()).not.toContain('年')
    expect(wrapper.find('[data-ui="table"]').text()).toMatch(/2026-\d{2}-\d{2} \d{2}:\d{2}:\d{2}/)
    expect(wrapper.attributes('data-page')).toBe('operation-log')
    expect(wrapper.find('#operation-log-keyword').attributes('name')).toBe('operationLogKeyword')
    expect(wrapper.findAll('.filter-select')).toHaveLength(2)
    expect(wrapper.find('[data-ui="toolbar-actions"]').exists()).toBe(true)
    expect(vi.mocked(operationLogApi.query).mock.calls[0]?.[1]?.aborted).toBe(true)
    wrapper.unmount()
  })

  it('Docker 嵌入模式将时间与结果收进更多筛选并保持主工具栏紧凑', async () => {
    const result = response('docker-event')
    result.data.items[0] = {
      ...result.data.items[0],
      module: 'docker',
      eventCode: 'docker_compose_project_redeploy',
    }
    result.data.items.push({
      ...result.data.items[0],
      eventId: 'docker-image-transfer',
      eventCode: 'docker_image_transferred_from_controller',
    })
    vi.mocked(operationLogApi.query).mockReset().mockResolvedValue(result)
    const wrapper = mount(OperationLogView, {
      props: { embedded: true, module: 'docker' },
      global: { plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })] },
    })
    await flushPromises()

    const toolbar = wrapper.find('[data-ui="toolbar"]')
    expect(toolbar.find('#operation-log-keyword').exists()).toBe(true)
    expect(toolbar.find('#operation-log-range').exists()).toBe(false)
    expect(toolbar.find('#operation-log-outcome').exists()).toBe(false)
    const moreFilters = toolbar.find('[data-ui="toolbar-actions"] button')
    expect(moreFilters.attributes('aria-expanded')).toBe('false')
    expect(moreFilters.attributes('aria-controls')).toBe('operation-log-advanced-filters')

    await moreFilters.trigger('click')

    const advancedFilters = wrapper.find('#operation-log-advanced-filters')
    expect(advancedFilters.find('#operation-log-range').exists()).toBe(true)
    expect(advancedFilters.find('#operation-log-outcome').exists()).toBe(true)
    expect(advancedFilters.find('#operation-log-impact').exists()).toBe(true)
    expect(advancedFilters.find('#operation-log-module').exists()).toBe(false)
    expect(moreFilters.attributes('aria-expanded')).toBe('true')
    expect(wrapper.find('[data-ui="table"]').text()).toContain('重新部署 Compose 项目')
    expect(wrapper.find('[data-ui="table"]').text()).toContain('从主控分发镜像')
    expect(wrapper.find('[data-ui="table"]').text()).not.toContain(
      'docker compose project redeploy',
    )
    wrapper.unmount()
  })

  it('刷新失败时保留已经加载的数据', async () => {
    vi.mocked(operationLogApi.query)
      .mockReset()
      .mockResolvedValueOnce(response('kept'))
      .mockResolvedValueOnce({ success: false, code: 500, message: 'refresh failed' })
    const wrapper = mount(OperationLogView, {
      global: { plugins: [createI18n({ legacy: false, locale: 'en', messages: { zh, en } })] },
    })
    await flushPromises()
    await (wrapper.vm as unknown as { load: () => Promise<void> }).load()
    await flushPromises()
    expect(wrapper.text()).toContain('kept')
    expect(wrapper.find('[data-ui="refresh-warning"]').exists()).toBe(true)
    wrapper.unmount()
  })

  it('匿名认证事件使用可信操作者名称且不显示目标', async () => {
    const result = response('auth-failure')
    result.data.items[0] = {
      ...result.data.items[0],
      module: 'auth',
      eventCode: 'user_login',
      actor: { kind: 'anonymous', displayName: 'anonymous' },
      target: undefined,
      outcome: 'failure',
      impact: 'error',
    }
    vi.mocked(operationLogApi.query).mockReset().mockResolvedValue(result)
    const wrapper = mount(OperationLogView, {
      global: { plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })] },
    })
    await flushPromises()
    expect(wrapper.text()).toContain('匿名访问者')
    wrapper.unmount()
  })

  it('动态套件事件优先使用当前语言的名称快照', async () => {
    const result = response('suite-event')
    result.data.items[0] = {
      ...result.data.items[0],
      module: 'suites',
      eventCode: 'suite_packet_lab_capture_started',
      eventLabel: { zhCn: '开始抓包', enUs: 'Start capture' },
      actor: { kind: 'suite', displayName: 'packet-lab/instance-1' },
    }
    vi.mocked(operationLogApi.query).mockReset().mockResolvedValue(result)
    const wrapper = mount(OperationLogView, {
      global: { plugins: [createI18n({ legacy: false, locale: 'en', messages: { zh, en } })] },
    })
    await flushPromises()
    expect(wrapper.text()).toContain('Start capture')
    expect(wrapper.text()).not.toContain('suite packet lab capture started')
    wrapper.unmount()
  })

  it.each([
    ['suite_install', '安装套件'],
    ['suite_enable', '启用套件'],
  ])('内置套件事件 %s 使用当前语言翻译', async (eventCode, translatedLabel) => {
    const result = response(eventCode)
    result.data.items[0] = {
      ...result.data.items[0],
      module: 'suites',
      eventCode,
      actor: { kind: 'user', displayName: 'admin' },
      origin: { kind: 'agent', nodeId: 'local', nodeName: '本地节点' },
    }
    vi.mocked(operationLogApi.query).mockReset().mockResolvedValue(result)
    const wrapper = mount(OperationLogView, {
      global: { plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })] },
    })
    await flushPromises()

    expect(wrapper.text()).toContain(translatedLabel)
    expect(wrapper.text()).not.toContain(eventCode)
    wrapper.unmount()
  })

  it('主控镜像分发使用可读动作与系统操作者', async () => {
    const result = response('controller-image-transfer')
    result.data.items[0] = {
      ...result.data.items[0],
      module: 'docker',
      eventCode: 'docker_image_transferred_from_controller',
      actor: { kind: 'system', displayName: 'image-acquisition' },
      origin: { kind: 'agent', nodeId: 'node-81', nodeName: '节点 81' },
      target: {
        kind: 'image',
        id: 'guowenju/fluxterm-pulse-server:latest',
        displayName: 'guowenju/fluxterm-pulse-server:latest',
      },
    }
    vi.mocked(operationLogApi.query).mockReset().mockResolvedValue(result)
    vi.mocked(operationLogApi.detail)
      .mockReset()
      .mockResolvedValue({
        success: true,
        code: 200,
        message: '',
        data: {
          ...result.data.items[0],
          parameters: { imageRef: 'guowenju/fluxterm-pulse-server:latest' },
          items: [],
        },
      })
    const wrapper = mount(OperationLogView, {
      global: { plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })] },
    })
    await flushPromises()

    const table = wrapper.find('[data-ui="table"]').text()
    expect(table).toContain('从主控分发镜像')
    expect(table).toContain('系统')
    expect(table).not.toContain('image-acquisition')
    await wrapper.find('[data-ui="table"] button').trigger('click')
    await flushPromises()
    expect(document.body.textContent).toContain('guowenju/fluxterm-pulse-server:latest')
    expect(document.body.textContent).not.toContain('image-acquisition')
    wrapper.unmount()
  })

  it('用户列不混入来源信息并由独立节点列展示 Agent 来源', async () => {
    const result = response('agent-event')
    result.data.items[0] = {
      ...result.data.items[0],
      actor: { kind: 'user', displayName: 'admin' },
      clientIp: '::ffff:10.0.0.41',
      origin: { kind: 'agent', nodeId: 'node-a', nodeName: '节点 A' },
    }
    vi.mocked(operationLogApi.query).mockReset().mockResolvedValue(result)
    const wrapper = mount(OperationLogView, {
      global: { plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })] },
    })
    await flushPromises()

    const table = wrapper.find('[data-ui="table"]')
    expect(table.text()).toContain('用户')
    expect(table.text()).toContain('admin (::ffff:10.0.0.41)')
    expect(table.text()).not.toContain('Agent')
    expect(table.text()).toContain('节点 A')
    expect(table.text()).not.toContain('用户 / 来源')
    wrapper.unmount()
  })

  it('英文用户列使用 User 标题', async () => {
    vi.mocked(operationLogApi.query).mockReset().mockResolvedValue(response('english-user'))
    const wrapper = mount(OperationLogView, {
      global: { plugins: [createI18n({ legacy: false, locale: 'en', messages: { zh, en } })] },
    })
    await flushPromises()

    expect(wrapper.find('[data-ui="table"]').text()).toContain('User')
    expect(wrapper.find('[data-ui="table"]').text()).not.toContain('Actor / origin')
    wrapper.unmount()
  })

  it('表格隐藏目标，详情宽模态框展示领域名称与目标 ID', async () => {
    const result = response('named-target')
    result.data.items[0].target = {
      kind: 'node',
      id: '019f6f64-cc8d-7a30-9812-845e0f56f185',
      displayName: '边缘节点',
    }
    vi.mocked(operationLogApi.query).mockReset().mockResolvedValue(result)
    vi.mocked(operationLogApi.detail)
      .mockReset()
      .mockResolvedValue({
        success: true,
        code: 200,
        message: '',
        data: {
          ...result.data.items[0],
          parameters: {},
          items: [],
        },
      })
    const wrapper = mount(OperationLogView, {
      global: { plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })] },
    })
    await flushPromises()
    expect(wrapper.find('[data-ui="table"]').text()).not.toContain('边缘节点')
    expect(wrapper.text()).not.toContain('019f6f64-cc8d-7a30-9812-845e0f56f185')
    await wrapper.find('[data-ui="table"] button').trigger('click')
    await flushPromises()
    const dialog = document.body.querySelector<HTMLElement>('[data-ui="detail"]')
    expect(dialog).not.toBeNull()
    expect(dialog?.classList.contains('vl-dialog-overlay')).toBe(true)
    expect(wrapper.findComponent(VustDialog).props('width')).toBe('min(1120px, 92vw)')
    expect(dialog?.querySelector('.vl-dialog-card')).not.toBeNull()
    expect(document.body.querySelector('.vl-drawer-overlay')).toBeNull()
    expect(document.body.textContent).toContain('边缘节点')
    expect(document.body.textContent).toContain('019f6f64-cc8d-7a30-9812-845e0f56f185')
    wrapper.unmount()
  })

  it('通过通知载荷直接打开操作事件并展示完整逐项结果', async () => {
    vi.mocked(operationLogApi.query).mockReset().mockResolvedValue(response('listed-event'))
    vi.mocked(operationLogApi.detail)
      .mockReset()
      .mockResolvedValue({
        success: true,
        code: 200,
        message: '',
        data: {
          ...response('audit-event').data.items[0],
          module: 'files',
          eventCode: 'file_task_succeeded',
          target: undefined,
          taskId: 'file-task-1',
          parameters: {
            operation: 'remove',
            totalItemCount: 2,
            completedItemCount: 2,
            failedItemCount: 0,
          },
          items: [
            {
              sequence: 1,
              source: { kind: 'file', id: '/root/cc', displayName: '/root/cc' },
              status: 'succeeded',
            },
            {
              sequence: 2,
              source: { kind: 'file', id: '/root/bb', displayName: '/root/bb' },
              status: 'succeeded',
            },
          ],
        },
      })
    const wrapper = mount(OperationLogView, {
      props: { payload: { eventId: 'audit-event' } },
      global: { plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })] },
    })
    await flushPromises()

    expect(operationLogApi.detail).toHaveBeenCalledWith('audit-event', expect.any(AbortSignal))
    expect(document.body.textContent).toContain('/root/cc')
    expect(document.body.textContent).toContain('/root/bb')
    expect(
      document.body
        .querySelector('[data-ui="operation-log-items"]')
        ?.classList.contains('operation-items-table'),
    ).toBe(true)
    const completedItemCountLabel = Array.from(
      document.body.querySelectorAll<HTMLElement>('[data-ui="parameters"] .vl-descriptions-label'),
    ).find((element) => element.textContent === 'completedItemCount')
    expect(completedItemCountLabel).toBeDefined()
    expect(
      document.body
        .querySelector('[data-ui="parameters"]')
        ?.classList.contains('parameter-descriptions'),
    ).toBe(true)
    wrapper.unmount()
  })

  it('详情将节点参数归一为名称，并在名称缺失时回退节点 ID', async () => {
    const base = response('node-detail').data.items[0]
    vi.mocked(operationLogApi.query).mockReset().mockResolvedValue(response('listed-event'))
    vi.mocked(operationLogApi.detail)
      .mockReset()
      .mockResolvedValueOnce({
        success: true,
        code: 200,
        message: '',
        data: {
          ...base,
          origin: { kind: 'agent', nodeId: 'local' },
          parameters: { nodeId: 'local', nodeName: 'local' },
          items: [],
        },
      })
      .mockResolvedValueOnce({
        success: true,
        code: 200,
        message: '',
        data: {
          ...base,
          origin: { kind: 'agent', nodeId: 'node-a', nodeName: '来源节点名称' },
          parameters: { nodeId: 'node-a', nodeName: '参数节点名称' },
          items: [],
        },
      })
      .mockResolvedValueOnce({
        success: true,
        code: 200,
        message: '',
        data: {
          ...base,
          origin: { kind: 'master' },
          target: undefined,
          parameters: { nodeId: 'deleted-node-id' },
          items: [],
        },
      })
    const wrapper = mount(OperationLogView, {
      props: { payload: { eventId: 'local-event' } },
      global: { plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })] },
    })
    await flushPromises()

    const parameterText = () =>
      document.body.querySelector('[data-ui="parameters"]')?.textContent ?? ''
    expect(parameterText()).toContain('节点')
    expect(parameterText()).toContain('本地节点')
    expect(parameterText()).not.toContain('nodeId')

    await wrapper.setProps({ payload: { eventId: 'child-event' } })
    await flushPromises()
    expect(parameterText()).toContain('参数节点名称')
    expect(parameterText()).not.toContain('来源节点名称')
    expect(parameterText()).not.toContain('node-a')
    expect(parameterText().match(/参数节点名称/g)).toHaveLength(1)
    expect(parameterText()).not.toContain('nodeName')

    await wrapper.setProps({ payload: { eventId: 'deleted-event' } })
    await flushPromises()
    expect(parameterText()).toContain('deleted-node-id')
    expect(parameterText()).not.toContain('nodeId')
    wrapper.unmount()
  })

  it('关闭加载中的详情时取消请求并支持按钮、遮罩与 Escape 关闭', async () => {
    vi.mocked(operationLogApi.query).mockReset().mockResolvedValue(response('pending-event'))
    const pending = deferred<never>()
    vi.mocked(operationLogApi.detail).mockReset().mockReturnValue(pending.promise)
    const wrapper = mount(OperationLogView, {
      global: { plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })] },
    })
    await flushPromises()

    await wrapper.find('[data-ui="table"] button').trigger('click')
    await flushPromises()
    const firstSignal = vi.mocked(operationLogApi.detail).mock.calls[0]?.[1]
    const overlay = document.body.querySelector<HTMLElement>('[data-ui="detail"]')
    overlay?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await flushPromises()

    expect(firstSignal?.aborted).toBe(true)
    expect(document.body.querySelector('[data-ui="detail"]')).toBeNull()

    await wrapper.find('[data-ui="table"] button').trigger('click')
    await flushPromises()
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    await flushPromises()

    const secondSignal = vi.mocked(operationLogApi.detail).mock.calls[1]?.[1]
    expect(secondSignal?.aborted).toBe(true)
    expect(document.body.querySelector('[data-ui="detail"]')).toBeNull()

    await wrapper.find('[data-ui="table"] button').trigger('click')
    await flushPromises()
    document.body.querySelector<HTMLElement>('.vl-dialog-close-btn')?.click()
    await flushPromises()

    const thirdSignal = vi.mocked(operationLogApi.detail).mock.calls[2]?.[1]
    expect(thirdSignal?.aborted).toBe(true)
    expect(document.body.querySelector('[data-ui="detail"]')).toBeNull()
    wrapper.unmount()
  })
})
