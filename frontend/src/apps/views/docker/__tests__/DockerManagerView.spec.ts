import { nextTick, reactive } from 'vue'
import { createI18n } from 'vue-i18n'
import { flushPromises, shallowMount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import en from '@/locales/en'
import zh from '@/locales/zh'
import DockerAllContainers from '../DockerAllContainers.vue'
import DockerManagerView from '../DockerManagerView.vue'

const state = vi.hoisted(() => ({
  docker: {} as Record<string, unknown>,
  node: { currentNodeId: 'local' },
  window: { updateWindowRuntimeState: vi.fn() },
}))

vi.mock('@/stores/docker', () => ({ useDockerStore: () => state.docker }))
vi.mock('@/stores/node', () => ({ useNodeStore: () => state.node }))
vi.mock('@/stores/window-manager', () => ({ useWindowManagerStore: () => state.window }))

function setupStore() {
  state.docker = reactive({
    dockerAvailable: true,
    dockerStatusCode: 'available',
    isLoading: false,
    containers: [],
    containerResourceStats: {},
    isContainerCreateActive: false,
    isContainerDetailActive: false,
    containerStep: 'selectImage',
    selectedImageId: null,
    selectedImageLabel: '',
    containerForm: {},
    initialLoad: vi.fn().mockResolvedValue(undefined),
    refreshOverviewDataIfNeeded: vi.fn().mockResolvedValue('fresh'),
    fetchOverviewHistoryAll: vi.fn().mockResolvedValue(undefined),
    fetchContainers: vi.fn().mockResolvedValue(undefined),
    fetchDockerAvailability: vi.fn().mockResolvedValue(undefined),
    fetchContainerResourceStats: vi.fn().mockResolvedValue(undefined),
    startContainerCreateFlow: vi.fn().mockResolvedValue(undefined),
    handleContainerAction: vi.fn(),
    cancelContainerCreate: vi.fn(),
    submitContainerConfig: vi.fn(),
    startOverviewPolling: vi.fn(),
    stopOverviewPolling: vi.fn(),
    startHistoryPolling: vi.fn(),
    stopHistoryPolling: vi.fn(),
    startContainerStatsPolling: vi.fn(),
    stopContainerStatsPolling: vi.fn(),
    stopAllPolling: vi.fn(),
  })
}

describe('DockerManagerView', () => {
  beforeEach(() => {
    setupStore()
    state.window.updateWindowRuntimeState.mockReset()
  })

  it('仅保留资源管理入口并继续传播容器实时日志活跃状态', async () => {
    const wrapper = shallowMount(DockerManagerView, {
      props: { windowId: 'docker-window' },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
      },
    })
    await flushPromises()

    const menuLabels = wrapper.findAll('.nav-item-label').map((item) => item.text())
    expect(menuLabels).toEqual(['概览', '项目', '容器', '镜像', '卷', '网络'])

    await wrapper.findAll('.nav-item')[2]?.trigger('click')
    await nextTick()
    wrapper.findComponent(DockerAllContainers).vm.$emit('logsActiveChange', true)
    await nextTick()

    expect(state.window.updateWindowRuntimeState).toHaveBeenLastCalledWith(
      'docker-window',
      expect.objectContaining({ activeSession: true, blockLevel: 'active' }),
    )
  })
})
