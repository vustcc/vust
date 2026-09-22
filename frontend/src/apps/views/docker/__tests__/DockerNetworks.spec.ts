import { reactive } from 'vue'
import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { DockerNetworkDetail, DockerNetworkSummary } from '@/api/interface/docker'
import { VustActionMenu } from '@/components/ui'
import en from '@/locales/en'
import zh from '@/locales/zh'
import DockerNetworks from '../DockerNetworks.vue'

const state = vi.hoisted(() => ({
  docker: {} as Record<string, unknown>,
  node: {} as Record<string, unknown>,
  confirmation: { showConfirmation: vi.fn() },
}))

vi.mock('@/stores/docker', () => ({ useDockerStore: () => state.docker }))
vi.mock('@/stores/node', () => ({ useNodeStore: () => state.node }))
vi.mock('@/stores/confirmation-modal', () => ({
  useConfirmationModalStore: () => state.confirmation,
}))

const network = (
  id: string,
  kind: DockerNetworkSummary['management']['kind'],
  mutable = true,
): DockerNetworkSummary => ({
  id,
  name: id,
  createdAt: 1,
  driver: 'bridge',
  scope: 'local',
  enableIpv4: true,
  enableIpv6: false,
  internal: false,
  attachable: false,
  ingress: kind === 'system',
  configOnly: false,
  subnets: [],
  management: { kind, ownerName: `${kind}-owner` },
  capabilities: { canRemove: mutable, canManageConnections: mutable },
})

const mountView = () =>
  mount(DockerNetworks, {
    global: {
      plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
    },
  })

type RowAction = {
  label: string
  disabled?: boolean
  tooltip?: string
  handler: () => void
}

describe('DockerNetworks 托管网络操作', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
    state.confirmation.showConfirmation.mockReset()
    const networks = [
      network('compose_default', 'compose'),
      network('suite-network', 'suite'),
      network('custom-network', 'custom'),
      network('ingress', 'system', false),
    ]
    state.docker = reactive({
      networks,
      networkDetail: null as DockerNetworkDetail | null,
      containers: [{ id: 'candidate', name: 'candidate', state: 'running' }],
      networkListLoading: false,
      networkListError: '',
      networkCreateLoading: false,
      networkDeleteLoadingId: null,
      networkDetailLoading: false,
      networkDetailError: '',
      networkConnectLoading: false,
      networkDisconnectLoading: {},
      fetchNetworks: vi.fn(),
      fetchContainers: vi.fn(),
      fetchNetworkDetail: vi.fn(async (id: string) => {
        const summary = networks.find((item) => item.id === id)!
        state.docker.networkDetail = {
          summary,
          ipamConfigs: [],
          options: {},
          labels: {},
          containers: [
            {
              id: 'connected',
              name: 'connected',
              ipv4Address: '172.20.0.2/16',
            },
          ],
          peers: [],
        }
      }),
      clearNetworkDetail: vi.fn(),
      createNetwork: vi.fn().mockResolvedValue(false),
      removeNetwork: vi.fn().mockResolvedValue(true),
      connectNetwork: vi.fn(),
      disconnectNetwork: vi.fn(),
    })
    state.node = reactive({ currentNodeId: 'local' })
  })

  it('开放托管网络操作并继续保护系统网络', () => {
    const wrapper = mountView()
    const deleteActions = wrapper.findAllComponents(VustActionMenu).map((menu) => {
      const actions = menu.props('actions') as RowAction[]
      return actions.find((action) => action.label === '删除')
    })

    expect(deleteActions.slice(0, 3).every((action) => action?.disabled === false)).toBe(true)
    expect(deleteActions[3]?.disabled).toBe(true)
    expect(deleteActions[3]?.tooltip).toContain('系统网络由 Docker 管理')
    wrapper.unmount()
  })

  it('托管网络使用风险提示，取消时不发送删除请求', async () => {
    state.confirmation.showConfirmation.mockResolvedValue(false)
    const wrapper = mountView()
    const actions = wrapper.findAllComponents(VustActionMenu)[0]!.props('actions') as RowAction[]
    actions.find((action) => action.label === '删除')?.handler()
    await flushPromises()

    expect(state.confirmation.showConfirmation.mock.calls[0]?.[0]).toContain(
      '所属项目或套件状态可能不一致',
    )
    expect(state.docker.removeNetwork).not.toHaveBeenCalled()
    wrapper.unmount()
  })

  it('托管网络详情允许连接和断开容器', async () => {
    const wrapper = mountView()
    await wrapper.findAll('.network-name')[0]!.trigger('click')
    await flushPromises()

    expect(document.body.querySelector('[data-slot="connect-container"]')).not.toBeNull()
    const connectionActions = wrapper.findAllComponents(VustActionMenu).some((menu) => {
      const actions = menu.props('actions') as RowAction[]
      return actions.some((action) => action.label === '断开连接')
    })
    expect(connectionActions).toBe(true)
    wrapper.unmount()
  })

  it('在内容区切换创建视图，返回后恢复列表并清理草稿', async () => {
    const wrapper = mountView()
    expect(wrapper.find('[data-ui="network-create-button"]').text()).toBe('创建网络')

    await wrapper.find('[data-ui="network-create-button"]').trigger('click')
    expect(wrapper.find('[data-ui="network-create-view"]').exists()).toBe(true)
    expect(wrapper.find('[data-ui="toolbar"]').exists()).toBe(false)
    expect(document.body.querySelector('[data-ui="network-create-dialog"]')).toBeNull()

    await wrapper.find('#docker-network-name').setValue('temporary-network')
    expect(wrapper.find('[data-ui="workspace-header-back"]').text()).toBe('返回')
    expect(wrapper.find('[data-ui="network-create-back"]').exists()).toBe(false)
    await wrapper.find('[data-ui="workspace-header-back"]').trigger('click')
    expect(wrapper.find('[data-ui="toolbar"]').exists()).toBe(true)

    await wrapper.find('[data-ui="network-create-button"]').trigger('click')
    expect((wrapper.find('#docker-network-name').element as HTMLInputElement).value).toBe('')
    wrapper.unmount()
  })

  it('创建失败保留表单，成功后返回列表', async () => {
    const wrapper = mountView()
    await wrapper.find('[data-ui="network-create-button"]').trigger('click')
    await wrapper.find('#docker-network-name').setValue('isolated-network')
    await wrapper.find('[data-ui="network-create-submit"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-ui="network-create-view"]').exists()).toBe(true)
    ;(state.docker.createNetwork as ReturnType<typeof vi.fn>).mockResolvedValueOnce(true)
    await wrapper.find('[data-ui="network-create-submit"]').trigger('click')
    await flushPromises()
    expect(state.docker.createNetwork).toHaveBeenLastCalledWith(
      expect.objectContaining({
        name: 'isolated-network',
        internal: false,
        enableIpv6: false,
      }),
    )
    expect(wrapper.find('[data-ui="network-create-view"]').exists()).toBe(false)
    expect(wrapper.find('[data-ui="toolbar"]').exists()).toBe(true)
    wrapper.unmount()
  })
})
