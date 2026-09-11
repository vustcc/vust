import { defineComponent, h } from 'vue'
import { createI18n } from 'vue-i18n'
import { createPinia } from 'pinia'
import { mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { DockerProjectTask, DockerProjectTaskProgressUpdate } from '@/api/interface/docker'
import en from '@/locales/en'
import zh from '@/locales/zh'
import { useDockerStore } from '@/stores/docker'

const api = vi.hoisted(() => ({
  createComposeProject: vi.fn(),
  composeProjectOperationEventsUrl: vi.fn(
    (operationId: string) =>
      `/api/v1/agent/docker/compose/project-operations/${operationId}/events`,
  ),
}))

vi.mock('@/api/modules/docker', () => ({
  dockerApi: {
    forNode: () => api,
  },
}))

class EventSourceMock {
  static instances: EventSourceMock[] = []
  readonly url: string
  readonly withCredentials: boolean
  onopen: ((event: Event) => void) | null = null
  onerror: ((event: Event) => void) | null = null
  closed = false
  private readonly listeners = new Map<string, Set<EventListener>>()

  constructor(url: string | URL, init?: EventSourceInit) {
    this.url = String(url)
    this.withCredentials = init?.withCredentials ?? false
    EventSourceMock.instances.push(this)
  }

  addEventListener(type: string, listener: EventListener) {
    const listeners = this.listeners.get(type) ?? new Set<EventListener>()
    listeners.add(listener)
    this.listeners.set(type, listeners)
  }

  emit(type: string, data: unknown) {
    const event = new MessageEvent(type, { data: JSON.stringify(data) })
    this.listeners.get(type)?.forEach((listener) => listener(event))
  }

  close() {
    this.closed = true
  }
}

const activeTask = (): DockerProjectTask => ({
  id: 'task-live',
  projectName: 'demo',
  operation: 'create',
  status: 'running',
  stage: 'pulling',
  progressPercent: 25,
  progressMode: 'structured',
  progressItems: [],
  pullImages: true,
  createdAt: 1,
})

describe('DockerStore Compose deployment SSE', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    EventSourceMock.instances = []
    vi.stubGlobal('EventSource', EventSourceMock)
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('实时合并工作中的分层事件并在停止跟踪时关闭连接', async () => {
    const task = activeTask()
    api.createComposeProject.mockResolvedValue({
      success: true,
      code: 202,
      message: '',
      data: task,
    })
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
          plugins: [
            createPinia(),
            createI18n({ legacy: false, locale: 'zh', messages: { zh, en } }),
          ],
        },
      },
    )

    expect(await store.createComposeProject({ name: 'demo', composeYaml: 'services: {}' })).toBe(
      true,
    )
    const source = EventSourceMock.instances[0]
    expect(source.url).toBe('/api/v1/agent/docker/compose/project-operations/task-live/events')
    expect(source.withCredentials).toBe(true)

    const update: DockerProjectTaskProgressUpdate = {
      progressMode: 'structured',
      progressPercent: 37,
      item: {
        id: 'pulling:sha256:abc',
        parentId: 'pulling:Image demo',
        phase: 'pulling',
        status: 'working',
        label: 'sha256:abc',
        action: 'Downloading',
        currentBytes: 25,
        totalBytes: 100,
        percent: 25,
      },
    }
    source.emit('progress', update)

    expect(store.projectDeploymentProgress?.progressPercent).toBe(37)
    expect(store.projectDeploymentProgress?.progressItems).toEqual([update.item])
    expect(store.projectDeploymentProgressVisible).toBe(true)
    store.closeProjectDeploymentProgress()
    expect(store.projectDeploymentProgressVisible).toBe(false)
    expect(store.projectDeploymentProgress?.id).toBe(task.id)
    expect(source.closed).toBe(false)
    store.openProjectDeploymentProgress()
    expect(store.projectDeploymentProgressVisible).toBe(true)
    store.stopProjectOperationPolling()
    expect(source.closed).toBe(true)
    wrapper.unmount()
  })

  it('原位更新任务与条目并忽略延迟事件造成的进度回退', async () => {
    const task = activeTask()
    api.createComposeProject.mockResolvedValue({
      success: true,
      code: 202,
      message: '',
      data: task,
    })
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
          plugins: [
            createPinia(),
            createI18n({ legacy: false, locale: 'zh', messages: { zh, en } }),
          ],
        },
      },
    )
    await store.createComposeProject({ name: 'demo', composeYaml: 'services: {}' })
    const source = EventSourceMock.instances[0]
    source.emit('progress', {
      progressMode: 'structured',
      progressPercent: 45,
      item: {
        id: 'pulling:layer',
        parentId: 'pulling:image',
        phase: 'pulling',
        status: 'working',
        label: 'layer',
        action: 'Downloading',
        currentBytes: 75,
        totalBytes: 100,
        percent: 75,
      },
    } satisfies DockerProjectTaskProgressUpdate)
    const operation = store.projectDeploymentProgress
    const item = operation?.progressItems[0]

    source.emit('snapshot', {
      ...activeTask(),
      progressPercent: 35,
      progressMode: 'unavailable',
      progressItems: [
        {
          ...item!,
          action: 'Waiting',
          currentBytes: 25,
          percent: 25,
        },
      ],
    } satisfies DockerProjectTask)

    expect(store.projectDeploymentProgress).toBe(operation)
    expect(store.projectDeploymentProgress?.progressItems[0]).toBe(item)
    expect(store.projectDeploymentProgress?.progressPercent).toBe(45)
    expect(store.projectDeploymentProgress?.progressItems[0].currentBytes).toBe(75)
    expect(store.projectDeploymentProgress?.progressItems[0].percent).toBe(75)
    expect(store.projectDeploymentProgress?.progressItems[0].action).toBe('Downloading')
    expect(store.projectDeploymentProgress?.progressMode).toBe('structured')
    wrapper.unmount()
  })

  it('限制实时条目数量并用终态快照移除后端已裁剪的旧条目', async () => {
    api.createComposeProject.mockResolvedValue({
      success: true,
      code: 202,
      message: '',
      data: activeTask(),
    })
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
          plugins: [
            createPinia(),
            createI18n({ legacy: false, locale: 'zh', messages: { zh, en } }),
          ],
        },
      },
    )
    await store.createComposeProject({ name: 'demo', composeYaml: 'services: {}' })
    const source = EventSourceMock.instances[0]
    for (let index = 0; index <= 200; index += 1) {
      source.emit('progress', {
        progressMode: 'structured',
        progressPercent: 50,
        item: {
          id: `pulling:layer-${index}`,
          parentId: 'pulling:image',
          phase: 'pulling',
          status: 'working',
          label: `layer-${index}`,
          action: 'Downloading',
        },
      } satisfies DockerProjectTaskProgressUpdate)
    }
    expect(store.projectDeploymentProgress?.progressItems).toHaveLength(200)
    expect(store.projectDeploymentProgress?.progressItems[0].id).toBe('pulling:layer-1')
    const retainedItem = store.projectDeploymentProgress?.progressItems.at(-1)

    source.emit('snapshot', {
      ...activeTask(),
      status: 'succeeded',
      stage: 'completed',
      progressPercent: 100,
      progressItems: [
        {
          ...retainedItem!,
          status: 'done',
          action: 'Pull complete',
          percent: 100,
        },
      ],
    } satisfies DockerProjectTask)

    expect(store.projectDeploymentProgress?.progressItems).toHaveLength(1)
    expect(store.projectDeploymentProgress?.progressItems[0]).toBe(retainedItem)
    expect(store.projectDeploymentProgress?.progressItems[0].status).toBe('done')
    wrapper.unmount()
  })
})
