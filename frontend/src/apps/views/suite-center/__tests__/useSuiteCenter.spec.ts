import { defineComponent } from 'vue'
import { createPinia } from 'pinia'
import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  SuiteCatalogItem,
  SuiteInstallTaskResponse,
  SuiteInstanceSummary,
  SuiteListResponse,
} from '@/api/interface/suites'
import { useNodeStore } from '@/stores/node'
import {
  resetSuiteCenterRuntimeForTests,
  useSuiteCenter,
  validateSuitePackage,
} from '@/apps/views/suite-center/useSuiteCenter'

const api = vi.hoisted(() => ({
  fetchSuites: vi.fn(),
  fetchInstallTasks: vi.fn(),
  fetchInstallProgress: vi.fn(),
  installSuite: vi.fn(),
  cancelInstall: vi.fn(),
  importSuite: vi.fn(),
  enableInstance: vi.fn(),
  disableInstance: vi.fn(),
  uninstallInstance: vi.fn(),
  deleteSuite: vi.fn(),
}))
const windowManager = vi.hoisted(() => ({
  registerGlobalOperation: vi.fn(),
  updateGlobalOperation: vi.fn(),
  finishGlobalOperation: vi.fn(),
  refreshDesktopState: vi.fn().mockResolvedValue(undefined),
  closeWindowsBySuiteInstanceId: vi.fn(),
}))

vi.mock('@/api/modules/suites', () => ({ suitesApi: api }))
vi.mock('@/stores/window-manager', () => ({ useWindowManagerStore: () => windowManager }))

const response = <T>(data: T) => ({ success: true, code: 200, message: '', data })
const failedResponse = { success: false, code: 503, message: 'temporary', data: null }

const suite = (suiteId: string, instanceCount = 0): SuiteCatalogItem => ({
  suiteId,
  version: '1.0.0',
  name: suiteId,
  summary: '',
  icon: 'package',
  status: 'available',
  checksum: 'checksum',
  createdAt: '',
  updatedAt: '',
  instanceCount,
})

const instance = (suiteId: string, nodeId = 'local'): SuiteInstanceSummary => ({
  instanceId: `instance-${suiteId}`,
  suiteId,
  version: '1.0.0',
  nodeId,
  composeProjectName: suiteId,
  status: 'installed',
  createdAt: '',
  updatedAt: '',
})

const task = (taskId = 'task-1', finished = false): SuiteInstallTaskResponse => ({
  taskId,
  instanceId: 'instance-suite-1',
  suiteId: 'suite-1',
  nodeId: 'local',
  progressPercent: finished ? 100 : 20,
  status: finished ? 'success' : 'running',
  currentStep: finished ? 'completed' : 'prepare',
  isFinished: finished,
  cancelRequested: false,
  createdAt: '',
  updatedAt: '',
})

const deferred = <T>() => {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise
  })
  return { promise, resolve }
}

const Harness = defineComponent({
  setup: () => useSuiteCenter(),
  template: '<div />',
})

function mountHarness() {
  const pinia = createPinia()
  const i18n = createI18n({
    legacy: false,
    locale: 'zh',
    messages: {
      zh: {
        app: {
          suiteCenter: {
            installOperationTitle: '安装 {suiteId}',
            messages: {
              loadFailed: '加载失败',
              pollFailed: '轮询失败',
              installSuccess: '安装成功',
              installFailed: '安装失败',
              cancelInstallSuccess: '取消成功',
            },
          },
        },
      },
    },
  })
  const wrapper = mount(Harness, { global: { plugins: [pinia, i18n] } })
  return { wrapper, nodeStore: useNodeStore(pinia) }
}

describe('useSuiteCenter', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    resetSuiteCenterRuntimeForTests()
    vi.clearAllMocks()
    localStorage.clear()
    api.fetchSuites.mockResolvedValue(response<SuiteListResponse>({ catalog: [], instances: [] }))
    api.fetchInstallTasks.mockResolvedValue(response([]))
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('节点快速切换时旧响应不能覆盖当前节点', async () => {
    const localRequest = deferred<ReturnType<typeof response<SuiteListResponse>>>()
    const nodeRequest = deferred<ReturnType<typeof response<SuiteListResponse>>>()
    api.fetchSuites
      .mockImplementationOnce(() => localRequest.promise)
      .mockImplementationOnce(() => nodeRequest.promise)
    const { wrapper, nodeStore } = mountHarness()
    nodeStore.currentNodeId = 'node-2'
    await flushPromises()

    nodeRequest.resolve(response({ catalog: [suite('node-suite')], instances: [] }))
    await flushPromises()
    localRequest.resolve(response({ catalog: [suite('local-suite')], instances: [] }))
    await flushPromises()

    expect(wrapper.vm.catalog[0]?.suiteId).toBe('node-suite')
    wrapper.unmount()
  })

  it('安装轮询等待当前请求结束后才安排下一次请求', async () => {
    api.fetchInstallTasks.mockResolvedValue(response([task()]))
    const progressRequest = deferred<ReturnType<typeof response<SuiteInstallTaskResponse>>>()
    api.fetchInstallProgress.mockImplementation(() => progressRequest.promise)
    const { wrapper } = mountHarness()
    await flushPromises()

    expect(windowManager.registerGlobalOperation).toHaveBeenCalledWith(
      expect.objectContaining({
        operationId: 'suite-install:task-1',
        blocksNodeSwitch: false,
        cancellable: true,
      }),
    )

    await vi.advanceTimersByTimeAsync(5000)
    expect(api.fetchInstallProgress).toHaveBeenCalledTimes(1)

    progressRequest.resolve(response(task()))
    await flushPromises()
    await vi.advanceTimersByTimeAsync(1000)
    expect(api.fetchInstallProgress).toHaveBeenCalledTimes(2)
    wrapper.unmount()
  })

  it('关闭并重开窗口复用后台轮询且恢复完成状态', async () => {
    api.fetchInstallTasks.mockResolvedValue(response([task()]))
    const progressRequest = deferred<ReturnType<typeof response<SuiteInstallTaskResponse>>>()
    api.fetchInstallProgress.mockImplementation(() => progressRequest.promise)
    const first = mountHarness().wrapper
    await flushPromises()
    first.unmount()

    const second = mountHarness().wrapper
    await flushPromises()
    expect(api.fetchInstallProgress).toHaveBeenCalledTimes(1)

    const silentRefresh = deferred<ReturnType<typeof response<SuiteListResponse>>>()
    api.fetchSuites.mockImplementationOnce(() => silentRefresh.promise)
    progressRequest.resolve(response(task('task-1', true)))
    await flushPromises()
    expect(second.vm.tasksById['task-1'].isFinished).toBe(true)
    expect(second.vm.refreshing).toBe(false)

    silentRefresh.resolve(
      response({ catalog: [suite('suite-1')], instances: [instance('suite-1')] }),
    )
    await flushPromises()
    expect(windowManager.finishGlobalOperation).toHaveBeenCalledWith('suite-install:task-1')
    second.unmount()
  })

  it('连续三次轮询失败后显示内联错误', async () => {
    api.fetchInstallTasks.mockResolvedValue(response([task()]))
    api.fetchInstallProgress.mockResolvedValue(failedResponse)
    const { wrapper } = mountHarness()
    await flushPromises()
    await vi.advanceTimersByTimeAsync(1000)
    await flushPromises()
    await vi.advanceTimersByTimeAsync(2000)
    await flushPromises()

    expect(wrapper.vm.pollingErrors['task-1']).toBe('轮询失败')
    wrapper.unmount()
  })

  it('不同套件操作状态相互独立', async () => {
    const firstAction = deferred<ReturnType<typeof response<SuiteInstanceSummary>>>()
    api.enableInstance.mockImplementationOnce(() => firstAction.promise)
    api.disableInstance.mockResolvedValue(response(instance('suite-2')))
    const { wrapper } = mountHarness()
    await flushPromises()

    const first = wrapper.vm.runInstanceAction(suite('suite-1'), instance('suite-1'), 'enable')
    const second = wrapper.vm.runInstanceAction(suite('suite-2'), instance('suite-2'), 'disable')
    await flushPromises()
    expect(wrapper.vm.isOperating('suite-1')).toBe(true)
    expect(wrapper.vm.isOperating('suite-2')).toBe(false)

    firstAction.resolve(response(instance('suite-1')))
    await first
    await second
    expect(wrapper.vm.isOperating('suite-1')).toBe(false)
    wrapper.unmount()
  })

  it('卸载期间仅更新实例状态并静默同步目录', async () => {
    const installedSuite = suite('suite-1', 1)
    const installedInstance = instance('suite-1')
    api.fetchSuites
      .mockResolvedValueOnce(
        response<SuiteListResponse>({
          catalog: [installedSuite],
          instances: [installedInstance],
        }),
      )
      .mockResolvedValueOnce(
        response<SuiteListResponse>({ catalog: [suite('suite-1')], instances: [] }),
      )
    const uninstallRequest = deferred<ReturnType<typeof response<null>>>()
    api.uninstallInstance.mockImplementationOnce(() => uninstallRequest.promise)
    const { wrapper } = mountHarness()
    await flushPromises()

    const uninstall = wrapper.vm.runInstanceAction(installedSuite, installedInstance, 'uninstall')
    await flushPromises()
    expect(wrapper.vm.instances[0]?.status).toBe('uninstalling')
    expect(wrapper.vm.refreshing).toBe(false)

    uninstallRequest.resolve(response(null))
    expect(await uninstall).toBe(true)
    expect(wrapper.vm.instances).toEqual([])
    expect(wrapper.vm.refreshing).toBe(false)
    wrapper.unmount()
  })

  it('实例操作失败后恢复原状态', async () => {
    const installedSuite = suite('suite-1', 1)
    const installedInstance = instance('suite-1')
    api.fetchSuites.mockResolvedValueOnce(
      response<SuiteListResponse>({
        catalog: [installedSuite],
        instances: [installedInstance],
      }),
    )
    api.enableInstance.mockResolvedValueOnce(failedResponse)
    const { wrapper } = mountHarness()
    await flushPromises()

    expect(await wrapper.vm.runInstanceAction(installedSuite, installedInstance, 'enable')).toBe(
      false,
    )
    expect(wrapper.vm.instances[0]?.status).toBe('installed')
    wrapper.unmount()
  })

  it('仅允许删除所有节点实例数为零的套件包', async () => {
    api.deleteSuite.mockResolvedValue(response(null))
    const { wrapper } = mountHarness()
    await flushPromises()

    expect(await wrapper.vm.deleteSuite(suite('used-suite', 1))).toBe(false)
    expect(api.deleteSuite).not.toHaveBeenCalled()
    expect(await wrapper.vm.deleteSuite(suite('empty-suite', 0))).toBe(true)
    expect(api.deleteSuite).toHaveBeenCalledWith('empty-suite')
    wrapper.unmount()
  })
})

describe('validateSuitePackage', () => {
  it('校验 .vsp 扩展名和 50 MiB 上限', () => {
    expect(validateSuitePackage(new File(['ok'], 'suite.vsp')).valid).toBe(true)
    expect(validateSuitePackage(new File(['bad'], 'suite.zip')).reason).toBe('extension')
    const oversized = new File(['x'], 'large.vsp')
    Object.defineProperty(oversized, 'size', { value: 50 * 1024 * 1024 + 1 })
    expect(validateSuitePackage(oversized).reason).toBe('size')
  })
})
