import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type {
  SuiteCatalogItem,
  SuiteInstallTaskResponse,
  SuiteInstanceStatus,
  SuiteInstanceSummary,
} from '@/api/interface/suites'
import { suitesApi } from '@/api/modules/suites'
import { useNodeStore } from '@/stores/node'
import { useToastStore } from '@/stores/toast'
import { useWindowManagerStore } from '@/stores/window-manager'

const MAX_PACKAGE_SIZE = 50 * 1024 * 1024
const POLL_BACKOFF = [1000, 2000, 5000] as const
const INSTANCE_STATUSES = new Set<SuiteInstanceStatus>([
  'installing',
  'installed',
  'enabling',
  'enabled',
  'disabling',
  'disabled',
  'uninstalling',
  'error',
])
const sharedTasksById = ref<Record<string, SuiteInstallTaskResponse>>({})
const sharedPollingErrors = ref<Record<string, string>>({})
const sharedActiveOperations = ref<Record<string, boolean>>({})
const sharedPollTimers = new Map<string, ReturnType<typeof window.setTimeout>>()
const sharedPollingTasks = new Set<string>()
const refreshSubscribers = new Set<(nodeId: string) => Promise<void>>()

export type SuiteCenterPhase = 'loading' | 'ready' | 'empty' | 'error'

export interface SuitePackageValidation {
  valid: boolean
  reason?: 'extension' | 'size'
}

interface RefreshSuitesOptions {
  clearInstances?: boolean
  showLoading?: boolean
}

/** 校验套件包扩展名与前端允许的体积上限。 */
export function validateSuitePackage(file: File): SuitePackageValidation {
  if (!file.name.toLowerCase().endsWith('.vsp')) return { valid: false, reason: 'extension' }
  if (file.size > MAX_PACKAGE_SIZE) return { valid: false, reason: 'size' }
  return { valid: true }
}

/** 将未知后端实例状态收敛为安全展示值。 */
export function normalizeSuiteInstanceStatus(status?: string): SuiteInstanceStatus {
  return status && INSTANCE_STATUSES.has(status as SuiteInstanceStatus)
    ? (status as SuiteInstanceStatus)
    : 'unknown'
}

/** 仅供测试隔离模块级后台任务运行时。 */
export function resetSuiteCenterRuntimeForTests() {
  sharedPollTimers.forEach((timer) => window.clearTimeout(timer))
  sharedPollTimers.clear()
  sharedPollingTasks.clear()
  refreshSubscribers.clear()
  sharedTasksById.value = {}
  sharedPollingErrors.value = {}
  sharedActiveOperations.value = {}
}

/** 编排套件目录、实例、后台安装任务与资源级操作状态。 */
export function useSuiteCenter() {
  const { t, locale } = useI18n()
  const nodeStore = useNodeStore()
  const toastStore = useToastStore()
  const windowStore = useWindowManagerStore()

  const catalog = ref<SuiteCatalogItem[]>([])
  const instances = ref<SuiteInstanceSummary[]>([])
  const tasksById = sharedTasksById
  const pollingErrors = sharedPollingErrors
  const activeOperations = sharedActiveOperations
  const phase = ref<SuiteCenterPhase>('loading')
  const loadError = ref('')
  const refreshing = ref(false)
  const requestSequence = ref(0)
  const visibleRefreshSequence = ref<number | null>(null)
  const pollTimers = sharedPollTimers
  const pollingTasks = sharedPollingTasks

  const currentNodeId = computed(() => nodeStore.currentNodeId || 'local')
  const currentNodeUnavailable = computed(() => nodeStore.currentNodeUnavailable)
  const currentNodeTasks = computed(() =>
    Object.values(tasksById.value).filter(
      (task) => task.nodeId === currentNodeId.value && !task.isFinished,
    ),
  )

  function operationKey(suiteId: string, nodeId = currentNodeId.value) {
    return `suite:${nodeId}:${suiteId}`
  }

  function isOperating(suiteId: string, nodeId = currentNodeId.value) {
    return activeOperations.value[operationKey(suiteId, nodeId)] === true
  }

  function setOperating(suiteId: string, nodeId: string, busy: boolean) {
    const key = operationKey(suiteId, nodeId)
    activeOperations.value = { ...activeOperations.value, [key]: busy }
    if (!busy) {
      const next = { ...activeOperations.value }
      delete next[key]
      activeOperations.value = next
    }
  }

  function normalizeInstances(items: SuiteInstanceSummary[]) {
    return items.map((item) => ({
      ...item,
      status: normalizeSuiteInstanceStatus(item.status),
    }))
  }

  /** 加载当前节点目录，忽略节点或语言切换产生的过期响应。 */
  async function refreshSuites(options: RefreshSuitesOptions = {}) {
    const sequence = ++requestSequence.value
    const targetNodeId = currentNodeId.value
    const showLoading = options.showLoading ?? true
    if (options.clearInstances) instances.value = []
    if (showLoading) {
      visibleRefreshSequence.value = sequence
      refreshing.value = true
      if (catalog.value.length === 0) phase.value = 'loading'
      loadError.value = ''
    }
    try {
      const response = await suitesApi.fetchSuites(targetNodeId)
      if (sequence !== requestSequence.value || targetNodeId !== currentNodeId.value) return
      if (!response.success || !response.data) {
        if (showLoading || catalog.value.length === 0) {
          loadError.value = response.message || t('app.suiteCenter.messages.loadFailed')
          phase.value = 'error'
        }
        return
      }
      catalog.value = response.data.catalog
      instances.value = normalizeInstances(response.data.instances)
      phase.value = catalog.value.length > 0 ? 'ready' : 'empty'
    } catch (error) {
      if (sequence !== requestSequence.value || targetNodeId !== currentNodeId.value) return
      console.error('Failed to load suites', error)
      if (showLoading || catalog.value.length === 0) {
        loadError.value = t('app.suiteCenter.messages.loadFailed')
        phase.value = 'error'
      }
    } finally {
      if (visibleRefreshSequence.value === sequence) {
        visibleRefreshSequence.value = null
        refreshing.value = false
      }
    }
  }

  function registerTask(task: SuiteInstallTaskResponse) {
    tasksById.value = { ...tasksById.value, [task.taskId]: task }
    if (!task.isFinished) {
      setOperating(task.suiteId, task.nodeId, true)
      windowStore.registerGlobalOperation({
        operationId: `suite-install:${task.taskId}`,
        nodeId: task.nodeId,
        sourceAppId: 'suite-center',
        title: t('app.suiteCenter.installOperationTitle', { suiteId: task.suiteId }),
        cancellable: !task.cancelRequested,
        blocksNodeSwitch: false,
      })
    }
  }

  /** 恢复当前节点尚未结束的持久化安装任务。 */
  async function restoreActiveTasks(nodeId = currentNodeId.value) {
    try {
      const response = await suitesApi.fetchInstallTasks(nodeId, true)
      if (!response.success || !response.data) return
      for (const task of response.data) {
        registerTask(task)
        startInstallPolling(task.taskId)
      }
    } catch (error) {
      console.error('Failed to restore suite install tasks', error)
    }
  }

  async function loadCurrentNode(clearInstances = true) {
    await Promise.all([refreshSuites({ clearInstances }), restoreActiveTasks(currentNodeId.value)])
  }

  function schedulePoll(taskId: string, delay: number, failures: number) {
    if (pollTimers.has(taskId)) window.clearTimeout(pollTimers.get(taskId))
    pollTimers.set(
      taskId,
      window.setTimeout(() => {
        pollTimers.delete(taskId)
        void pollInstallTask(taskId, failures)
      }, delay),
    )
  }

  async function finishTask(task: SuiteInstallTaskResponse) {
    setOperating(task.suiteId, task.nodeId, false)
    windowStore.finishGlobalOperation(`suite-install:${task.taskId}`)
    delete pollingErrors.value[task.taskId]
    if (task.status === 'success') {
      toastStore.success(t('app.suiteCenter.messages.installSuccess'))
    } else if (task.status === 'canceled') {
      toastStore.success(t('app.suiteCenter.messages.cancelInstallSuccess'))
    } else {
      toastStore.error(task.error || t('app.suiteCenter.messages.installFailed'))
    }
    await Promise.all([...refreshSubscribers].map((refresh) => refresh(task.nodeId)))
    await windowStore.refreshDesktopState()
  }

  async function pollInstallTask(taskId: string, failures = 0) {
    if (pollingTasks.has(taskId)) return
    pollingTasks.add(taskId)
    try {
      const response = await suitesApi.fetchInstallProgress(taskId)
      if (!response.success || !response.data) throw new Error(response.message)
      const task = response.data
      tasksById.value = { ...tasksById.value, [taskId]: task }
      delete pollingErrors.value[taskId]
      windowStore.updateGlobalOperation(`suite-install:${taskId}`, {
        cancellable: !task.cancelRequested && !task.isFinished,
      })
      if (task.isFinished) {
        await finishTask(task)
      } else {
        schedulePoll(taskId, 1000, 0)
      }
    } catch (error) {
      const nextFailures = failures + 1
      console.error('Failed to fetch suite install progress', error)
      if (nextFailures >= 3) {
        pollingErrors.value = {
          ...pollingErrors.value,
          [taskId]: t('app.suiteCenter.messages.pollFailed'),
        }
      }
      schedulePoll(taskId, POLL_BACKOFF[Math.min(nextFailures - 1, 2)], nextFailures)
    } finally {
      pollingTasks.delete(taskId)
    }
  }

  function startInstallPolling(taskId: string) {
    if (pollTimers.has(taskId) || pollingTasks.has(taskId)) return
    void pollInstallTask(taskId)
  }

  function retryInstallPolling(taskId: string) {
    const next = { ...pollingErrors.value }
    delete next[taskId]
    pollingErrors.value = next
    if (pollTimers.has(taskId)) {
      window.clearTimeout(pollTimers.get(taskId))
      pollTimers.delete(taskId)
    }
    startInstallPolling(taskId)
  }

  async function installSuite(suite: SuiteCatalogItem) {
    const nodeId = currentNodeId.value
    if (currentNodeUnavailable.value || isOperating(suite.suiteId, nodeId)) return false
    setOperating(suite.suiteId, nodeId, true)
    try {
      const response = await suitesApi.installSuite(suite.suiteId, nodeId)
      if (!response.success || !response.data) {
        toastStore.error(response.message || t('app.suiteCenter.messages.installFailed'))
        setOperating(suite.suiteId, nodeId, false)
        return false
      }
      registerTask(response.data)
      startInstallPolling(response.data.taskId)
      return true
    } catch (error) {
      console.error('Failed to install suite', error)
      toastStore.error(t('app.suiteCenter.messages.installFailed'))
      setOperating(suite.suiteId, nodeId, false)
      return false
    }
  }

  async function cancelInstall(taskId: string) {
    const task = tasksById.value[taskId]
    if (!task || task.cancelRequested || task.isFinished) return false
    try {
      const response = await suitesApi.cancelInstall(taskId)
      if (!response.success || !response.data) {
        toastStore.error(response.message || t('app.suiteCenter.messages.cancelInstallFailed'))
        return false
      }
      tasksById.value = { ...tasksById.value, [taskId]: response.data }
      if (response.data.isFinished) await finishTask(response.data)
      return true
    } catch (error) {
      console.error('Failed to cancel suite install', error)
      toastStore.error(t('app.suiteCenter.messages.cancelInstallFailed'))
      return false
    }
  }

  async function importSuite(file: File) {
    const validation = validateSuitePackage(file)
    if (!validation.valid) return validation
    activeOperations.value = { ...activeOperations.value, import: true }
    try {
      const response = await suitesApi.importSuite(file)
      if (!response.success) {
        toastStore.error(response.message || t('app.suiteCenter.messages.importFailed'))
        return { valid: false } as SuitePackageValidation
      }
      toastStore.success(t('app.suiteCenter.messages.importSuccess'))
      await refreshSuites()
      return validation
    } catch (error) {
      console.error('Failed to import suite', error)
      toastStore.error(t('app.suiteCenter.messages.importFailed'))
      return { valid: false } as SuitePackageValidation
    } finally {
      const next = { ...activeOperations.value }
      delete next.import
      activeOperations.value = next
    }
  }

  async function runInstanceAction(
    suite: SuiteCatalogItem,
    instance: SuiteInstanceSummary,
    action: 'enable' | 'disable' | 'uninstall',
    removeData = false,
  ) {
    if (isOperating(suite.suiteId, instance.nodeId)) return false
    const pendingStatus: Record<typeof action, SuiteInstanceStatus> = {
      enable: 'enabling',
      disable: 'disabling',
      uninstall: 'uninstalling',
    }
    const previousInstance =
      instances.value.find((item) => item.instanceId === instance.instanceId) ?? instance
    const restorePreviousStatus = () => {
      instances.value = instances.value.map((item) =>
        item.instanceId === previousInstance.instanceId ? previousInstance : item,
      )
    }
    instances.value = instances.value.map((item) =>
      item.instanceId === instance.instanceId ? { ...item, status: pendingStatus[action] } : item,
    )
    setOperating(suite.suiteId, instance.nodeId, true)
    try {
      const response =
        action === 'enable'
          ? await suitesApi.enableInstance(instance.instanceId)
          : action === 'disable'
            ? await suitesApi.disableInstance(instance.instanceId)
            : await suitesApi.uninstallInstance(instance.instanceId, { removeData })
      if (!response.success) {
        restorePreviousStatus()
        toastStore.error(response.message || t(`app.suiteCenter.messages.${action}Failed`))
        return false
      }
      toastStore.success(t(`app.suiteCenter.messages.${action}Success`))
      if (action !== 'enable') windowStore.closeWindowsBySuiteInstanceId(instance.instanceId)
      if (action === 'uninstall') {
        instances.value = instances.value.filter((item) => item.instanceId !== instance.instanceId)
      } else {
        const completedStatus: SuiteInstanceStatus = action === 'enable' ? 'enabled' : 'disabled'
        instances.value = instances.value.map((item) =>
          item.instanceId === instance.instanceId ? { ...item, status: completedStatus } : item,
        )
      }
      await refreshSuites({ showLoading: false })
      await windowStore.refreshDesktopState()
      return true
    } catch (error) {
      restorePreviousStatus()
      console.error(`Failed to ${action} suite instance`, error)
      toastStore.error(t(`app.suiteCenter.messages.${action}Failed`))
      return false
    } finally {
      setOperating(suite.suiteId, instance.nodeId, false)
    }
  }

  async function deleteSuite(suite: SuiteCatalogItem) {
    if (suite.instanceCount > 0 || isOperating(suite.suiteId)) return false
    setOperating(suite.suiteId, currentNodeId.value, true)
    try {
      const response = await suitesApi.deleteSuite(suite.suiteId)
      if (!response.success) {
        toastStore.error(response.message || t('app.suiteCenter.messages.deleteFailed'))
        return false
      }
      toastStore.success(t('app.suiteCenter.messages.deleteSuccess'))
      await refreshSuites()
      return true
    } catch (error) {
      console.error('Failed to delete suite package', error)
      toastStore.error(t('app.suiteCenter.messages.deleteFailed'))
      return false
    } finally {
      setOperating(suite.suiteId, currentNodeId.value, false)
    }
  }

  const refreshCompletedTaskNode = async (nodeId: string) => {
    if (nodeId === currentNodeId.value) await refreshSuites({ showLoading: false })
  }

  onMounted(() => {
    refreshSubscribers.add(refreshCompletedTaskNode)
    void loadCurrentNode()
  })
  watch(currentNodeId, () => void loadCurrentNode(true))
  watch(locale, () => void refreshSuites())
  onBeforeUnmount(() => {
    refreshSubscribers.delete(refreshCompletedTaskNode)
    requestSequence.value += 1
  })

  return {
    activeOperations,
    catalog,
    currentNodeId,
    currentNodeTasks,
    currentNodeUnavailable,
    deleteSuite,
    importSuite,
    installSuite,
    instances,
    isOperating,
    loadError,
    phase,
    pollingErrors,
    refreshSuites,
    retryInstallPolling,
    runInstanceAction,
    cancelInstall,
    tasksById,
    refreshing,
  }
}
