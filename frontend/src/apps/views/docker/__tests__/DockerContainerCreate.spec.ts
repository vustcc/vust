import { defineComponent, h, reactive } from 'vue'
import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import en from '@/locales/en'
import zh from '@/locales/zh'
import DockerAllContainers from '../DockerAllContainers.vue'
import DockerContainerCreateWizard from '../DockerContainerCreateWizard.vue'

const state = vi.hoisted(() => ({
  docker: {} as Record<string, unknown>,
}))

vi.mock('@/stores/docker', () => ({ useDockerStore: () => state.docker }))

const mountOptions = {
  global: {
    plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
  },
}

describe('Docker 容器内容区创建流程', () => {
  beforeEach(() => {
    state.docker = reactive({
      isContainerCreateActive: false,
      isContainerDetailActive: false,
      containerCreateLoading: false,
      containerCreateError: null,
      selectedImageId: null,
      imagesList: [{ id: 'sha256:image', tags: ['nginx:latest'] }],
      networks: [],
      containerForm: {
        name: '',
        command: '',
        environment: [],
        ports: [],
        mounts: [],
        restartPolicy: 'no',
        maximumRetryCount: null,
        networkId: '',
        autoRemove: false,
      },
      cancelContainerCreate: vi.fn(() => {
        state.docker.isContainerCreateActive = false
      }),
      submitContainerConfig: vi.fn(),
    })
  })

  it('在列表、创建和详情之间互斥切换', async () => {
    const wrapper = mount(DockerAllContainers, {
      props: { containers: [], nodeId: 'local' },
      global: {
        stubs: {
          DockerContainerList: defineComponent(
            () => () => h('div', { 'data-ui': 'container-list-stub' }),
          ),
          DockerContainerCreateWizard: defineComponent(
            () => () => h('div', { 'data-ui': 'container-create-stub' }),
          ),
          DockerContainerDetail: defineComponent(
            () => () => h('div', { 'data-ui': 'container-detail-stub' }),
          ),
        },
      },
    })
    await flushPromises()
    expect(wrapper.find('[data-ui="container-list-stub"]').exists()).toBe(true)

    state.docker.isContainerCreateActive = true
    await flushPromises()
    expect(wrapper.find('[data-ui="container-create-stub"]').exists()).toBe(true)
    expect(wrapper.find('[data-ui="container-list-stub"]').exists()).toBe(false)
    wrapper.unmount()
  })

  it('创建表单在内容区展示并通过返回恢复列表状态', async () => {
    state.docker.isContainerCreateActive = true
    const wrapper = mount(DockerContainerCreateWizard, mountOptions)

    expect(wrapper.find('[data-ui="container-create-view"]').exists()).toBe(true)
    expect(document.body.querySelector('[data-ui="container-create-dialog"]')).toBeNull()
    expect(wrapper.find('[data-ui="workspace-header-back"]').text()).toBe('返回')
    expect(wrapper.findAll('[data-slot="footer"] button')).toHaveLength(1)
    await wrapper.find('[data-ui="workspace-header-back"]').trigger('click')
    expect(state.docker.cancelContainerCreate).toHaveBeenCalledOnce()
    expect(state.docker.isContainerCreateActive).toBe(false)
    wrapper.unmount()
  })
})
