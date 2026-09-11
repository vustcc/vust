import { defineComponent, h } from 'vue'
import { createI18n } from 'vue-i18n'
import { createPinia } from 'pinia'
import { mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { DockerContainerSummary } from '@/api/interface/docker'
import en from '@/locales/en'
import zh from '@/locales/zh'
import { useConfirmationModalStore } from '@/stores/confirmation-modal'
import { useDockerStore } from '@/stores/docker'

const api = vi.hoisted(() => ({
  removeContainer: vi.fn(),
}))

vi.mock('@/api/modules/docker', () => ({
  dockerApi: {
    forNode: () => api,
  },
}))

const managedContainer: DockerContainerSummary = {
  id: 'compose-container',
  name: 'compose-web',
  imageRef: 'nginx:latest',
  imageId: 'sha256:image',
  command: 'nginx',
  createdAt: 1,
  state: 'exited',
  statusText: 'Exited',
  ports: [],
  management: { kind: 'compose', ownerName: 'demo' },
  capabilities: {
    canStart: true,
    canStop: false,
    canRestart: true,
    canPause: false,
    canUnpause: false,
    canKill: false,
    canRemove: true,
    canExec: false,
  },
}

describe('DockerStore 托管容器操作确认', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('删除托管容器时提示编排风险，取消后不发送请求', async () => {
    const pinia = createPinia()
    let store!: ReturnType<typeof useDockerStore>
    const wrapper = mount(
      defineComponent({
        setup() {
          store = useDockerStore()
          return () => h('div')
        },
      }),
      {
        global: {
          plugins: [pinia, createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
        },
      },
    )
    store.containers = [managedContainer]

    const operation = store.handleContainerAction(
      managedContainer.id,
      managedContainer.name,
      'remove',
    )
    const modalStore = useConfirmationModalStore(pinia)
    expect(modalStore.modalData.message).toContain('Compose 或套件容器')
    expect(modalStore.modalData.message).toContain('后续部署时被重新创建')

    modalStore.handleModalResponse(false)
    await expect(operation).resolves.toBe(false)
    expect(api.removeContainer).not.toHaveBeenCalled()
    wrapper.unmount()
  })
})
