import { nextTick, reactive } from 'vue'
import { createI18n } from 'vue-i18n'
import { mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { DockerContainerSummary } from '@/api/interface/docker'
import { VustActionMenu, VustCheckbox } from '@/components/ui'
import en from '@/locales/en'
import zh from '@/locales/zh'
import DockerContainerList from '../DockerContainerList.vue'

const state = vi.hoisted(() => ({
  docker: {} as Record<string, unknown>,
}))

vi.mock('@/stores/docker', () => ({ useDockerStore: () => state.docker }))

const capabilities = (state: 'running' | 'exited') => ({
  canStart: state === 'exited',
  canStop: state === 'running',
  canRestart: true,
  canPause: state === 'running',
  canUnpause: false,
  canKill: state === 'running',
  canRemove: state === 'exited',
  canExec: state === 'running',
})

const container = (
  id: string,
  kind: DockerContainerSummary['management']['kind'],
  state: 'running' | 'exited',
): DockerContainerSummary => ({
  id,
  name: `${kind}-${state}`,
  imageRef: 'nginx:latest',
  imageId: 'sha256:image',
  command: 'nginx',
  createdAt: 1,
  state,
  statusText: state,
  ports: [],
  management: { kind, ownerName: `${kind}-owner` },
  capabilities: capabilities(state),
})

const mountView = () =>
  mount(DockerContainerList, {
    global: {
      plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
    },
  })

describe('DockerContainerList 托管容器操作', () => {
  beforeEach(() => {
    state.docker = reactive({
      containers: [
        container('compose-running', 'compose', 'running'),
        container('suite-exited', 'suite', 'exited'),
      ],
      containerResourceStats: {},
      containerListLoading: false,
      containerListError: '',
      containerStatsError: '',
      containerActionLoadingIds: [],
      fetchContainers: vi.fn(),
      fetchContainerResourceStats: vi.fn(),
      startContainerCreateFlow: vi.fn(),
      handleContainerAction: vi.fn().mockResolvedValue(true),
    })
  })

  it('允许选择 Compose 和套件容器并按实时状态提供操作', async () => {
    const wrapper = mountView()
    const checkboxes = wrapper.findAllComponents(VustCheckbox)
    expect(checkboxes).toHaveLength(3)
    expect(checkboxes.slice(1).every((checkbox) => !checkbox.props('disabled'))).toBe(true)

    const menus = wrapper.findAllComponents(VustActionMenu)
    expect(menus).toHaveLength(2)
    const composeActions = menus[0]!.props('actions') as Array<{
      label: string
      disabled: boolean
      handler: () => Promise<void>
    }>
    expect(composeActions.map((action) => action.label)).toEqual([
      '启动容器',
      '停止容器',
      '重启容器',
      '暂停容器',
      '恢复容器',
      '强制停止',
      '删除容器',
    ])
    expect(composeActions.find((action) => action.label === '停止容器')?.disabled).toBe(false)
    expect(composeActions.find((action) => action.label === '删除容器')?.disabled).toBe(true)

    const suiteActions = menus[1]!.props('actions') as Array<{
      label: string
      disabled: boolean
    }>
    expect(suiteActions.find((action) => action.label === '启动容器')?.disabled).toBe(false)
    expect(suiteActions.find((action) => action.label === '删除容器')?.disabled).toBe(false)
    expect(suiteActions.find((action) => action.label === '停止容器')?.disabled).toBe(true)

    await composeActions.find((action) => action.label === '停止容器')?.handler()
    expect(state.docker.handleContainerAction).toHaveBeenCalledWith(
      'compose-running',
      'compose-running',
      'stop',
    )
    wrapper.unmount()
  })

  it('全选会包含所有归属的容器并显示批量操作', async () => {
    const wrapper = mountView()
    wrapper.findComponent(VustCheckbox).vm.$emit('update:modelValue', true)
    await nextTick()

    expect(wrapper.text()).toContain('已选择 2 个')
    const batchMenu = wrapper.findAllComponents(VustActionMenu)[0]!
    const batchActions = batchMenu.props('actions') as Array<{ label: string; disabled: boolean }>
    expect(batchActions.find((action) => action.label === '批量停止')?.disabled).toBe(false)
    expect(batchActions.find((action) => action.label === '批量删除')?.disabled).toBe(false)
    wrapper.unmount()
  })
})
