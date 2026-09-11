import { defineComponent, h, nextTick, reactive } from 'vue'
import { createI18n } from 'vue-i18n'
import { mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { DockerProjectTask } from '@/api/interface/docker'
import en from '@/locales/en'
import zh from '@/locales/zh'
import DockerProjects from '../DockerProjects.vue'

const state = vi.hoisted(() => ({
  docker: {} as Record<string, unknown>,
  node: { currentNodeId: 'local' },
}))

vi.mock('@/stores/docker', () => ({ useDockerStore: () => state.docker }))
vi.mock('@/stores/node', () => ({ useNodeStore: () => state.node }))
vi.mock('@/stores/confirmation-modal', () => ({
  useConfirmationModalStore: () => ({ showConfirmation: vi.fn() }),
}))

const activeTask = (): DockerProjectTask => ({
  id: 'task-1',
  projectName: 'demo-project',
  operation: 'redeploy',
  status: 'running',
  stage: 'pulling',
  progressPercent: 39,
  progressMode: 'structured',
  progressItems: [
    {
      id: 'pulling:Image nginx',
      phase: 'pulling',
      status: 'working',
      label: 'Image nginx',
      action: 'Pulling',
      currentBytes: null,
      totalBytes: null,
      percent: null,
    },
    {
      id: 'pulling:sha256:abc',
      parentId: 'pulling:Image nginx',
      phase: 'pulling',
      status: 'working',
      label: 'sha256:abc',
      action: 'Downloading',
      currentBytes: 512,
      totalBytes: 1024,
      percent: 50,
    },
  ],
  pullImages: true,
  createdAt: 1,
})

const setupStore = (task: DockerProjectTask | null = null) => {
  state.docker = reactive({
    composeProjects: [],
    projectTotal: 0,
    projectPage: 1,
    projectPageSize: 20,
    projectListLoading: false,
    projectListError: '',
    projectMutationLoading: false,
    projectConfiguration: null,
    projectConfigurationError: '',
    projectConfigurationLoading: false,
    projectDeploymentProgress: task,
    projectDeploymentProgressVisible: Boolean(task),
    projectDeploymentProgressError: null,
    fetchComposeProjects: vi.fn().mockResolvedValue(true),
    recoverActiveComposeDeployment: vi.fn().mockResolvedValue(true),
    stopProjectOperationPolling: vi.fn(),
    openProjectDeploymentProgress: vi.fn(),
    closeProjectDeploymentProgress: vi.fn(),
    clearComposeProjectDetail: vi.fn(),
    clearComposeProjectConfiguration: vi.fn(),
    redeployComposeProject: vi.fn().mockResolvedValue(true),
  })
}

const mountView = () =>
  mount(DockerProjects, {
    global: {
      plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
      stubs: {
        DockerProjectDetailDrawer: true,
        MonacoEditor: defineComponent({
          name: 'MonacoEditor',
          props: { modelValue: { type: String, default: '' } },
          emits: ['update:modelValue'],
          setup(_props, { emit }) {
            return () =>
              h('textarea', {
                'data-ui': 'monaco-editor-stub',
                onInput: (event: Event) => {
                  const value = (event.target as HTMLTextAreaElement).value
                  emit('update:modelValue', value)
                },
              })
          },
        }),
      },
    },
  })

describe('DockerProjects deployment dialogs', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
    setupStore()
  })

  it('重新部署选项展示可点击的拉取镜像文案', async () => {
    const wrapper = mountView()
    ;(wrapper.vm as unknown as { openDeployment: (name: string) => void }).openDeployment(
      'demo-project',
    )
    await nextTick()

    const dialog = document.body.querySelector('[data-ui="project-deployment-dialog"]')
    expect(dialog?.querySelector('.vl-checkbox-label')?.textContent).toContain('部署前拉取最新镜像')
    expect(dialog?.querySelector('input[type="checkbox"]')?.getAttribute('aria-label')).toBe(
      '部署前拉取最新镜像',
    )
    wrapper.unmount()
  })

  it('创建项目时实时校验 YAML 语法并阻止提交', async () => {
    const wrapper = mountView()
    const vm = wrapper.vm as unknown as {
      openCreate: () => void
    }
    vm.openCreate()
    await nextTick()
    const editor = document.body.querySelector(
      '[data-ui="project-create-dialog"] [data-ui="monaco-editor-stub"]',
    ) as HTMLTextAreaElement
    const invalidYaml =
      'services:\n  app:\n    image: nginx:latest\n    name: test\n    user: admin\n    asdsdas'
    editor.value = invalidYaml
    editor.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()

    const dialog = document.body.querySelector('[data-ui="project-create-dialog"]')
    const syntaxErrorElement = dialog?.querySelector('[data-ui="compose-yaml-error"]')
    const syntaxError = syntaxErrorElement?.textContent ?? ''
    expect(syntaxErrorElement?.tagName).toBe('H4')
    expect(syntaxErrorElement?.querySelector('.vl-alert-icon')).toBeNull()
    expect(syntaxError).toContain('YAML syntax error at line 6, column 12:')
    expect(syntaxError).not.toContain('\n 3 |')
    expect(
      (dialog?.querySelector('.vl-dialog-footer button:last-child') as HTMLButtonElement).disabled,
    ).toBe(true)

    const validYaml = 'services:\n  app:\n    image: nginx:latest\n'
    editor.value = validYaml
    editor.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()

    expect(dialog?.querySelector('[data-ui="compose-yaml-error"]')).toBeNull()
    expect(
      (dialog?.querySelector('.vl-dialog-footer button:last-child') as HTMLButtonElement).disabled,
    ).toBe(false)
    wrapper.unmount()
  })

  it('修改配置时实时校验 YAML 语法并阻止保存', async () => {
    setupStore()
    state.docker.projectConfiguration = {
      composeYaml: 'services: {}',
      revision: 1,
      state: 'pending',
    }
    const wrapper = mountView()
    const vm = wrapper.vm as unknown as {
      configurationVisible: boolean
    }
    vm.configurationVisible = true
    await nextTick()
    const editor = document.body.querySelector(
      '[data-ui="project-configuration-drawer"] [data-ui="monaco-editor-stub"]',
    ) as HTMLTextAreaElement
    const invalidYaml = 'services:\n  app: ['
    editor.value = invalidYaml
    editor.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()

    const drawer = document.body.querySelector('[data-ui="project-configuration-drawer"]')
    const syntaxErrorElement = drawer?.querySelector('[data-ui="compose-yaml-error"]')
    expect(syntaxErrorElement?.tagName).toBe('H4')
    expect(syntaxErrorElement?.querySelector('.vl-alert-icon')).toBeNull()
    expect(syntaxErrorElement?.textContent).toBeTruthy()
    expect(
      (drawer?.querySelector('.vl-drawer-footer button:last-child') as HTMLButtonElement).disabled,
    ).toBe(true)

    const validYaml = 'services:\n  app:\n    image: nginx:latest\n'
    editor.value = validYaml
    editor.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()

    expect(drawer?.querySelector('[data-ui="compose-yaml-error"]')).toBeNull()
    expect(
      (drawer?.querySelector('.vl-drawer-footer button:last-child') as HTMLButtonElement).disabled,
    ).toBe(false)
    wrapper.unmount()
  })

  it('展示结构化镜像进度并自动展开分层任务', async () => {
    const task = activeTask()
    task.stage = 'applying'
    task.progressItems.forEach((item) => {
      item.status = 'done'
    })
    setupStore(task)
    const wrapper = mountView()
    await nextTick()

    expect(document.body.querySelector('[data-ui="deployment-summary"]')?.textContent).toContain(
      'demo-project',
    )
    const identity = document.body.querySelector('[data-ui="deployment-identity"]')
    expect(identity?.textContent).not.toContain('项目')
    expect(identity?.textContent).toContain('重新部署')
    const item = document.body.querySelector('[data-ui="deployment-progress-item"]')
    expect(item?.textContent).toContain('Image nginx')
    expect(item?.querySelector('.deployment-item-state .deployment-bytes')).toBeNull()
    expect(item?.querySelector('.deployment-item-identity span')).toBeNull()
    expect(item?.querySelector('[data-ui="deployment-progress-item-percent"]')?.textContent).toBe(
      '50%',
    )
    expect(item?.textContent).not.toContain('null%')
    expect(
      item?.querySelector('[data-ui="deployment-progress-item-label"]')?.hasAttribute('title'),
    ).toBe(false)

    const layer = document.body.querySelector('[data-ui="deployment-progress-layer"]')
    expect(layer?.textContent).toContain('sha256:abc')
    expect(layer?.textContent).toContain('512 B / 1.0 KiB')
    expect(layer?.querySelector('[data-ui="deployment-progress-layer-percent"]')?.textContent).toBe(
      '50%',
    )
    expect(
      layer?.querySelector('[data-ui="deployment-progress-layer-label"]')?.hasAttribute('title'),
    ).toBe(false)
    expect(
      document.body.querySelectorAll('[data-ui="deployment-stage-track"] .deployment-stage-step')[1]
        ?.classList,
    ).toContain('is-done')
    expect(
      document.body.querySelector('[data-ui="deployment-phase-title"]')?.textContent?.trim(),
    ).toBe('镜像拉取')
    wrapper.unmount()
  })

  it('完成后只保留一个状态文案和一处总体进度', async () => {
    const task = activeTask()
    task.status = 'succeeded'
    task.stage = 'completed'
    task.progressPercent = 100
    task.progressItems.forEach((item) => {
      item.status = 'done'
    })
    setupStore(task)
    const wrapper = mountView()
    await nextTick()

    const meta = document.body.querySelector('.deployment-meta')
    expect(meta?.textContent?.match(/已完成/g)).toHaveLength(1)
    expect(document.body.querySelector('.deployment-percent')).toBeNull()
    expect(
      document.body.querySelector('[data-ui="deployment-overall-progress"]')?.textContent,
    ).toContain('100%')
    expect(document.body.textContent).not.toContain('null%')
    wrapper.unmount()
  })

  it('活动部署允许转入后台并提供恢复进度入口', async () => {
    setupStore(activeTask())
    const wrapper = mountView()
    await nextTick()

    const footerButton = document.body.querySelector(
      '[data-ui="project-deployment-progress-dialog"] .vl-dialog-footer button',
    ) as HTMLButtonElement | null
    expect(footerButton?.disabled).toBe(false)
    expect(footerButton?.textContent).toContain('后台运行')
    footerButton?.click()
    expect(state.docker.closeProjectDeploymentProgress).toHaveBeenCalledOnce()

    state.docker.projectDeploymentProgressVisible = false
    await nextTick()
    const restore = wrapper.find('[data-ui="restore-deployment-progress"]')
    expect(restore.exists()).toBe(true)
    expect(restore.text()).toContain('39%')
    await restore.trigger('click')
    expect(state.docker.openProjectDeploymentProgress).toHaveBeenCalledOnce()
    wrapper.unmount()
  })
})
