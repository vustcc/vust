import { defineComponent } from 'vue'
import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  ScheduledTaskListPage,
  ScheduledTaskOperation,
  ScheduledTaskRun,
  ScheduledTaskSummary,
} from '@/api/generated/scheduled-tasks'
import { taskApi } from '@/api/modules/task'
import { nodesApi } from '@/api/modules/nodes'
import { useTaskScheduler } from '@/composables/useTaskScheduler'
import en from '@/locales/en'
import zh from '@/locales/zh'

vi.mock('@/api/modules/task', () => ({
  taskApi: {
    list: vi.fn(),
    detail: vi.fn(),
    create: vi.fn(),
    update: vi.fn(),
    updateState: vi.fn(),
    remove: vi.fn(),
    startRun: vi.fn(),
    listRuns: vi.fn(),
    runDetail: vi.fn(),
    runOutput: vi.fn(),
    cancelRun: vi.fn(),
    migrate: vi.fn(),
    operation: vi.fn(),
    cancelOperation: vi.fn(),
    createBatch: vi.fn(),
    batch: vi.fn(),
  },
}))
vi.mock('@/api/modules/nodes', () => ({ nodesApi: { list: vi.fn() } }))

const api = vi.mocked(taskApi)
const nodeApi = vi.mocked(nodesApi)
const response = <T>(data: T) => ({ success: true, code: 200, message: '', data })
const deferred = <T>() => {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise
  })
  return { promise, resolve }
}
const summary = (taskId: string, nodeId = 'local'): ScheduledTaskSummary => ({
  taskId,
  name: `Task ${taskId}`,
  node: { nodeId, nodeName: nodeId },
  schedule: { cronExpr: '*/5 * * * *', timeZone: 'Asia/Shanghai', summary: '每 5 分钟' },
  desiredState: 'enabled',
  deployment: { status: 'ready', revision: 1 },
  nextRun: { status: 'scheduled', at: '2026-07-16T10:00:00Z' },
  ownership: { kind: 'custom' },
  capabilities: {
    canUpdate: true,
    canChangeState: true,
    canRun: true,
    canRemove: true,
    canMigrate: true,
  },
  createdAt: '2026-07-16T00:00:00Z',
  updatedAt: '2026-07-16T00:00:00Z',
})
const page = (items: ScheduledTaskSummary[]): ScheduledTaskListPage => ({
  items,
  page: 1,
  pageSize: 50,
  total: items.length,
  loadedAt: '2026-07-16T00:00:00Z',
})
const run = (): ScheduledTaskRun => ({
  runId: 'run-1',
  taskId: 'task-1',
  nodeId: 'local',
  triggerSource: 'manual',
  status: 'queued',
  queuedAt: '2026-07-16T00:00:00Z',
  output: { available: false, truncated: false, sizeBytes: 0 },
  capabilities: { canCancel: true },
})
const operation = (): ScheduledTaskOperation => ({
  operationId: 'operation-1',
  taskId: 'task-1',
  kind: 'update',
  status: 'queued',
  completedSteps: 0,
  totalSteps: 1,
  canCancel: true,
  createdAt: '2026-07-16T00:00:00Z',
  updatedAt: '2026-07-16T00:00:00Z',
})

const Harness = defineComponent({
  setup() {
    return useTaskScheduler()
  },
  template: '<div />',
})
const mountHarness = () =>
  mount(Harness, {
    global: {
      plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
    },
  })

describe('useTaskScheduler', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    sessionStorage.clear()
    nodeApi.list.mockResolvedValue(response([]))
    api.list.mockResolvedValue(response(page([])))
  })

  afterEach(() => {
    vi.useRealTimers()
    vi.clearAllMocks()
  })

  it('节点快速切换时旧响应不能覆盖新状态', async () => {
    const oldRequest = deferred<ReturnType<typeof response<ScheduledTaskListPage>>>()
    const newRequest = deferred<ReturnType<typeof response<ScheduledTaskListPage>>>()
    api.list
      .mockImplementationOnce(() => oldRequest.promise)
      .mockImplementationOnce(() => newRequest.promise)
    const wrapper = mountHarness()
    wrapper.vm.filters.nodeId = 'node-2'
    await flushPromises()

    oldRequest.resolve(response(page([summary('old', 'local')])))
    newRequest.resolve(response(page([summary('new', 'node-2')])))
    await flushPromises()

    expect(wrapper.vm.tasks.map((item) => item.taskId)).toEqual(['new'])
    wrapper.unmount()
  })

  it('刷新失败保留旧数据并显示非阻塞警告', async () => {
    api.list.mockResolvedValueOnce(response(page([summary('task-1')])))
    const wrapper = mountHarness()
    await flushPromises()
    api.list.mockRejectedValueOnce(new Error('temporary failure'))
    await wrapper.vm.refreshTasks()

    expect(wrapper.vm.tasks).toHaveLength(1)
    expect(wrapper.vm.listState.warning).toBe('temporary failure')
    expect(wrapper.vm.listState.error).toBe('')
    wrapper.unmount()
  })

  it('重复提交只创建一次运行', async () => {
    const pending = deferred<ReturnType<typeof response<ScheduledTaskRun>>>()
    api.startRun.mockImplementation(() => pending.promise)
    const wrapper = mountHarness()
    await flushPromises()

    const first = wrapper.vm.startRun('task-1')
    const second = wrapper.vm.startRun('task-1')
    expect(api.startRun).toHaveBeenCalledTimes(1)
    pending.resolve(response(run()))
    await Promise.all([first, second])
    wrapper.unmount()
  })

  it('轮询请求完成前不会安排重叠请求', async () => {
    const pending = deferred<ReturnType<typeof response<ScheduledTaskOperation>>>()
    api.operation.mockImplementation(() => pending.promise)
    const wrapper = mountHarness()
    await flushPromises()
    wrapper.vm.trackOperation(operation())

    await vi.advanceTimersByTimeAsync(4_000)
    expect(api.operation).toHaveBeenCalledTimes(1)
    pending.resolve(response({ ...operation(), status: 'succeeded' }))
    await flushPromises()
    wrapper.unmount()
  })
})
