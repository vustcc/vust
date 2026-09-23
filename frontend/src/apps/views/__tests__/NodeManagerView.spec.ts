import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import NodeManagerView from '../NodeManagerView.vue'
import zh from '@/locales/zh'
import en from '@/locales/en'
import { nodesApi } from '@/api/modules/nodes'
import { VustActionMenu, VustButton, VustCheckbox } from '@/components/ui'
import { upgradesApi } from '@/api/modules/upgrades'

const nodeStore = vi.hoisted(() => ({
  nodes: [
    {
      id: 'local',
      name: '本地节点',
      address: '127.0.0.1',
      status: 'online',
      tags: ['local'],
    },
  ] as Array<Record<string, unknown>>,
  currentNodeId: 'local',
  refreshNodes: vi.fn().mockResolvedValue(undefined),
  startAutoRefresh: vi.fn(),
  requestSwitchCurrentNode: vi.fn(() => ({ switched: true, reason: '' })),
}))
const windowManager = vi.hoisted(() => ({
  checkBeforeNodeSwitch: vi.fn(() => ({ allowed: true, blockers: [] })),
  registerGlobalOperation: vi.fn(),
  finishGlobalOperation: vi.fn(),
  updateWindowRuntimeState: vi.fn(),
}))
const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }))

vi.mock('@/stores/node', () => ({ useNodeStore: () => nodeStore }))
vi.mock('@/stores/window-manager', () => ({ useWindowManagerStore: () => windowManager }))
vi.mock('@/stores/toast', () => ({ useToastStore: () => toast }))
vi.mock('vue-router', () => ({ useRouter: () => ({ push: vi.fn() }) }))
vi.mock('@/api/modules/upgrades', () => ({
  upgradesApi: {
    listReleases: vi.fn(),
    createPlan: vi.fn(),
    startPlan: vi.fn(),
  },
}))
vi.mock('@/api/modules/nodes', () => ({
  nodesApi: {
    list: vi.fn(),
    detail: vi.fn(),
    about: vi.fn(),
    precheck: vi.fn(),
    deployCreate: vi.fn(),
    getDeployProgress: vi.fn(),
    update: vi.fn(),
    remove: vi.fn(),
    check: vi.fn(),
    repair: vi.fn(),
    retire: vi.fn(),
    uninstall: vi.fn(),
  },
}))

const response = <T>(data: T) => ({ success: true, code: 200, message: '', data })

const precheckResult = (passed: boolean) => {
  const detail = {
    status: passed ? 'passed' : 'failed',
    code: '',
    message: passed ? '正常' : '失败',
  }
  return {
    passed,
    agentStatus: { blocking: !passed, message: passed ? '正常' : '冲突' },
    versionCompatibility: { status: 'passed', message: '正常', requiredAction: '' },
    ssh: detail,
    os: detail,
    permission: detail,
    service: detail,
    systemd: detail,
    directory: detail,
    docker: detail,
    port: detail,
    callback: detail,
  }
}

const mountView = () => {
  const windowElement = document.createElement('section')
  windowElement.className = 'application-window'
  windowElement.innerHTML = `
    <header class="window-header">节点管理</header>
    <main class="window-content"></main>
  `
  document.body.append(windowElement)
  return mount(NodeManagerView, {
    attachTo: windowElement.querySelector<HTMLElement>('.window-content')!,
    props: { windowId: 'node-window' },
    global: {
      plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
    },
  })
}

async function openCreateAndSetAddress(wrapper: ReturnType<typeof mountView>) {
  await wrapper.get('[data-ui="node-create"]').trigger('click')
  const address = wrapper.get('[data-ui="node-create-address"] input')
  await address.setValue('192.0.2.10')
  return address
}

describe('NodeManagerView', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    nodeStore.nodes = [
      {
        id: 'local',
        name: '本地节点',
        address: '127.0.0.1',
        status: 'online',
        tags: ['local'],
      },
    ]
    nodeStore.currentNodeId = 'local'
    vi.mocked(nodesApi.precheck).mockResolvedValue(response(precheckResult(true)) as never)
    vi.mocked(nodesApi.deployCreate).mockResolvedValue(
      response({
        logs: [],
        node: {
          nodeId: 'node-1',
          name: 'node-1',
          address: '192.0.2.10',
          status: 'deploying',
          tags: [],
        },
      }) as never,
    )
    vi.mocked(nodesApi.getDeployProgress).mockResolvedValue(
      response({ progressPercent: 25, logs: ['installing'], isFinished: false }) as never,
    )
    vi.mocked(nodesApi.detail).mockResolvedValue(response(undefined) as never)
    vi.mocked(nodesApi.about).mockResolvedValue(response(undefined) as never)
    vi.mocked(nodesApi.check).mockResolvedValue(
      response({
        status: 'online',
        ssh: { status: 'passed', message: 'SSH 正常' },
        service: { status: 'passed', message: '服务正常' },
        api: { status: 'passed', message: 'API 正常' },
      }) as never,
    )
    vi.mocked(upgradesApi.listReleases).mockResolvedValue(response([]) as never)
  })

  afterEach(() => {
    document.body.innerHTML = ''
    vi.useRealTimers()
  })

  it('在内容区打开创建表单，返回后恢复列表并清理草稿', async () => {
    const wrapper = mountView()
    await flushPromises()
    const address = await openCreateAndSetAddress(wrapper)

    expect(wrapper.find('[data-ui="table"]').exists()).toBe(false)
    expect(wrapper.find('[data-ui="node-create-view"]').exists()).toBe(true)
    expect(document.body.querySelector('.vl-dialog-overlay')).toBeNull()

    await wrapper.get('[data-ui="node-provision-back"]').trigger('click')
    expect(wrapper.find('[data-ui="table"]').exists()).toBe(true)

    await wrapper.get('[data-ui="node-create"]').trigger('click')
    expect(
      (wrapper.get('[data-ui="node-create-address"] input').element as HTMLInputElement).value,
    ).toBe('')
    expect((address.element as HTMLInputElement).isConnected).toBe(false)
    wrapper.unmount()
  })

  it('检查失败时保留输入并允许返回修改', async () => {
    vi.mocked(nodesApi.precheck).mockResolvedValue(response(precheckResult(false)) as never)
    const wrapper = mountView()
    await flushPromises()
    await openCreateAndSetAddress(wrapper)
    await wrapper.get('[data-ui="node-run-precheck"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-ui="node-precheck-view"]').exists()).toBe(true)
    expect(wrapper.text()).toContain('未通过')
    await wrapper.get('[data-ui="node-precheck-edit"]').trigger('click')
    expect(
      (wrapper.get('[data-ui="node-create-address"] input').element as HTMLInputElement).value,
    ).toBe('192.0.2.10')
    wrapper.unmount()
  })

  it('部署可转入后台、重新查看，并在终态后手动返回列表', async () => {
    vi.useFakeTimers()
    const wrapper = mountView()
    await flushPromises()
    await openCreateAndSetAddress(wrapper)
    await wrapper.get('[data-ui="node-run-precheck"]').trigger('click')
    await flushPromises()
    await wrapper.get('[data-ui="node-start-deploy"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-ui="node-deploy-view"]').exists()).toBe(true)
    expect(windowManager.registerGlobalOperation).toHaveBeenCalledOnce()
    expect(windowManager.updateWindowRuntimeState).toHaveBeenLastCalledWith(
      'node-window',
      expect.objectContaining({ busy: true }),
    )

    await wrapper.get('[data-ui="node-deploy-background"]').trigger('click')
    expect(wrapper.find('[data-ui="table"]').exists()).toBe(true)
    expect(wrapper.find('[data-ui="node-deploy-resume"]').exists()).toBe(true)
    await wrapper.get('[data-ui="node-deploy-resume"]').trigger('click')

    vi.mocked(nodesApi.getDeployProgress).mockResolvedValue(
      response({ progressPercent: 100, logs: ['done'], isFinished: true }) as never,
    )
    await vi.advanceTimersByTimeAsync(1500)
    await flushPromises()
    expect(windowManager.finishGlobalOperation).toHaveBeenCalledWith('node-deploy:node-1')
    expect(wrapper.find('[data-ui="node-deploy-finish"]').exists()).toBe(true)
    expect(wrapper.find('[data-ui="node-deploy-view"]').exists()).toBe(true)

    await wrapper.get('[data-ui="node-deploy-finish"]').trigger('click')
    expect(wrapper.find('[data-ui="table"]').exists()).toBe(true)
    expect(wrapper.find('[data-ui="node-deploy-resume"]').exists()).toBe(false)
    wrapper.unmount()
  })

  it('将编辑、升级、检测详情和操作确认限制在当前窗口内容区', async () => {
    nodeStore.nodes = [
      ...nodeStore.nodes,
      {
        id: 'node-2',
        name: '远程节点',
        address: '192.0.2.20:22',
        status: 'online',
        tags: [],
      },
    ]
    const wrapper = mountView()
    await flushPromises()
    const windowContent = document.querySelector('.window-content')!
    const windowHeader = document.querySelector('.window-header')!
    const actions = wrapper.findComponent(VustActionMenu).props('actions')

    await actions.find((action: { label: string }) => action.label === '编辑').handler()
    await flushPromises()
    const editDialog = document.querySelector('[data-ui="node-edit-dialog"]')!
    expect(windowContent.contains(editDialog)).toBe(true)
    expect(windowHeader.contains(editDialog)).toBe(false)
    const editCancel = Array.from(editDialog.querySelectorAll('button')).find(
      (button) => button.textContent?.trim() === '取消',
    )
    editCancel?.click()
    await flushPromises()

    await actions.find((action: { label: string }) => action.label === '检测').handler()
    await flushPromises()
    const checkDialog = document.querySelector('[data-ui="node-check-dialog"]')!
    expect(windowContent.contains(checkDialog)).toBe(true)
    expect(document.body.querySelector(':scope > .application-dialog-overlay')).toBeNull()
    checkDialog.querySelector<HTMLButtonElement>('[aria-label]')?.click()
    await flushPromises()

    const checkboxes = wrapper.findAllComponents(VustCheckbox)
    checkboxes.at(-1)?.vm.$emit('change', true)
    await flushPromises()
    await wrapper
      .findAllComponents(VustButton)
      .find((button) => button.attributes('data-ui') === 'node-upgrade-selected')
      ?.trigger('click')
    await flushPromises()
    expect(windowContent.querySelector('[data-ui="node-upgrade-dialog"]')).not.toBeNull()
    document.querySelector<HTMLElement>('[data-ui="node-upgrade-dialog"] [aria-label]')?.click()
    await flushPromises()

    const operationPromise = actions
      .find((action: { label: string }) => action.label === '退役')
      .handler()
    await flushPromises()
    const confirmation = document.querySelector('[data-ui="node-confirmation-dialog"]')!
    expect(windowContent.contains(confirmation)).toBe(true)
    const cancel = Array.from(confirmation.querySelectorAll('button')).find(
      (button) => button.textContent?.trim() === '取消',
    )
    cancel?.click()
    await operationPromise
    wrapper.unmount()
  })
})
