import { reactive } from 'vue'
import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { DockerVolumeSummary } from '@/api/interface/docker'
import { VustActionMenu } from '@/components/ui'
import en from '@/locales/en'
import zh from '@/locales/zh'
import DockerVolumes from '../DockerVolumes.vue'

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

const volume = (
  name: string,
  kind: DockerVolumeSummary['management']['kind'],
): DockerVolumeSummary => ({
  name,
  createdAt: 1,
  management: { kind, ownerName: `${kind}-owner` },
  capabilities: { canRemove: true },
})

const mountView = () =>
  mount(DockerVolumes, {
    global: {
      plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
    },
  })

type RowAction = {
  label: string
  disabled?: boolean
  handler: () => void
}

describe('DockerVolumes 托管卷操作', () => {
  beforeEach(() => {
    state.confirmation.showConfirmation.mockReset()
    state.docker = reactive({
      volumes: [
        volume('compose-data', 'compose'),
        volume('suite-data', 'suite'),
        volume('custom-data', 'custom'),
      ],
      volumeWarnings: [],
      volumeDetail: null,
      volumeListLoading: false,
      volumeListError: '',
      volumeCreateLoading: false,
      volumeCreateError: null,
      volumeDeleteLoadingName: null,
      volumeDetailLoading: false,
      volumeDetailError: '',
      fetchVolumes: vi.fn(),
      fetchVolumeDetail: vi.fn(),
      clearVolumeDetail: vi.fn(),
      createVolume: vi.fn(),
      removeVolume: vi.fn().mockResolvedValue(true),
    })
    state.node = reactive({ currentNodeId: 'local' })
  })

  it('允许删除 Compose、套件和自定义卷', () => {
    const wrapper = mountView()
    const deleteActions = wrapper.findAllComponents(VustActionMenu).map((menu) => {
      const actions = menu.props('actions') as RowAction[]
      return actions.find((action) => action.label === '删除')
    })

    expect(deleteActions).toHaveLength(3)
    expect(deleteActions.every((action) => action?.disabled === false)).toBe(true)
    wrapper.unmount()
  })

  it('托管卷使用风险提示，取消时不发送删除请求', async () => {
    state.confirmation.showConfirmation.mockResolvedValue(false)
    const wrapper = mountView()
    const actions = wrapper.findAllComponents(VustActionMenu)[0]!.props('actions') as RowAction[]
    actions.find((action) => action.label === '删除')?.handler()
    await flushPromises()

    expect(state.confirmation.showConfirmation).toHaveBeenCalledOnce()
    expect(state.confirmation.showConfirmation.mock.calls[0]?.[0]).toContain(
      '所属项目或套件状态可能不一致',
    )
    expect(state.docker.removeVolume).not.toHaveBeenCalled()
    wrapper.unmount()
  })

  it('自定义卷沿用普通确认并在确认后删除', async () => {
    state.confirmation.showConfirmation.mockResolvedValue(true)
    const wrapper = mountView()
    const actions = wrapper.findAllComponents(VustActionMenu)[2]!.props('actions') as RowAction[]
    actions.find((action) => action.label === '删除')?.handler()
    await flushPromises()

    expect(state.confirmation.showConfirmation.mock.calls[0]?.[0]).toContain('此操作不可撤销')
    expect(state.confirmation.showConfirmation.mock.calls[0]?.[0]).not.toContain('所属项目或套件')
    expect(state.docker.removeVolume).toHaveBeenCalledWith(
      expect.objectContaining({ name: 'custom-data' }),
    )
    wrapper.unmount()
  })
})
