/**
 * Docker 状态管理 Store
 *
 * 统一管理 Docker 全部领域数据、数据获取、轮询定时器和业务操作。
 * 替代原 DockerManagerView.vue 中散落的 30+ ref 状态和 20+ 业务函数。
 */

import { ref, computed } from 'vue'
import { defineStore } from 'pinia'
import { useI18n } from 'vue-i18n'
import { dockerApi } from '@/api/modules/docker'
import type * as dockerType from '@/api/interface/docker'
import { useToastStore } from '@/stores/toast'
import { useConfirmationModalStore } from '@/stores/confirmation-modal'
import { useNodeStore } from '@/stores/node'
import {
  buildDockerOverviewContainerReferences,
  MAX_OVERVIEW_CONTAINERS,
  readDockerOverviewSelection,
  resolveDockerOverviewContainerIds,
  writeDockerOverviewSelection,
  type DockerOverviewContainerReference,
} from '@/stores/docker-overview-preferences'

export { MAX_OVERVIEW_CONTAINERS } from '@/stores/docker-overview-preferences'

/** 容器资源统计缓存条目 */
export interface ContainerStatsEntry {
  data: dockerType.ContainerResourceUsageSummary
  fetchedAt: number
}

/** 概览数据缓存 */
interface OverviewCacheEntry {
  timestamp: number
}

/** 容器资源统计缓存 TTL (毫秒) */
const CONTAINER_STATS_TTL = 60_000
/** 概览缓存 TTL (毫秒) */
const OVERVIEW_CACHE_TTL = 30_000
/** 与后端任务快照一致的部署进度项上限 */
const MAX_PROJECT_PROGRESS_ITEMS = 200

/** 选择概览趋势容器：优先保留有效选择，否则仅默认选择运行中的容器。 */
export function selectOverviewContainerIds(
  items: dockerType.TrendContainerItem[],
  selectedIds: string[],
) {
  const availableIds = new Set(items.map((item) => item.id))
  const retained = selectedIds.filter((id) => availableIds.has(id))
  if (retained.length > 0) return retained.slice(0, MAX_OVERVIEW_CONTAINERS)

  return items
    .filter((item) => item.state.toLowerCase() === 'running')
    .slice(0, MAX_OVERVIEW_CONTAINERS)
    .map((item) => item.id)
}

const emptyContainerStates = (): dockerType.ContainerStateCounts => ({
  total: 0,
  running: 0,
  paused: 0,
  restarting: 0,
  exited: 0,
  other: 0,
})

const emptyProjectStates = (): dockerType.ProjectStateCounts => ({
  total: 0,
  healthy: 0,
  partial: 0,
  stopped: 0,
  unknown: 0,
})

export const useDockerStore = defineStore('docker', () => {
  const { t } = useI18n()
  const toastStore = useToastStore()
  const modalStore = useConfirmationModalStore()
  const nodeStore = useNodeStore()

  // ─── Docker 客户端 (响应节点切换) ───
  const dockerClient = computed(() => dockerApi.forNode(nodeStore.currentNodeId))

  // ─── 核心状态 ───
  const dockerAvailable = ref<boolean>(true)
  const dockerStatus = ref<boolean>(false)
  const dockerStatusCode = ref<string>('unknown')
  const isLoading = ref<boolean>(true)

  // ─── 概览统计 ───
  const overviewCollectedAt = ref<number | null>(null)
  const overviewError = ref<string | null>(null)
  const containerStates = ref<dockerType.ContainerStateCounts>(emptyContainerStates())
  const projectStates = ref<dockerType.ProjectStateCounts>(emptyProjectStates())
  const imageCounts = ref<dockerType.ImageCounts>({ total: 0, dangling: 0 })
  const resourceUsage = ref<dockerType.HostResourceUsageSummary | null>(null)

  // ─── 概览图表数据 ───
  const overviewContainers = ref<dockerType.TrendContainerItem[]>([])
  const overviewSelectedContainerIds = ref<string[]>([])
  const overviewHistoryMap = ref<Record<string, dockerType.ContainerResourceUsageHistory>>({})
  const overviewHistoryLatestMap = ref<Record<string, number | null>>({})
  const overviewHistoryHours = ref<1 | 6 | 12>(1)
  const overviewHistoryError = ref<string | null>(null)

  // ─── 容器列表 ───
  const containers = ref<dockerType.DockerContainerSummary[]>([])
  const containerListLoading = ref(false)
  const containerListError = ref<string | null>(null)
  const containerListLoadedAt = ref<number | null>(null)
  const containerActionLoadingIds = ref<string[]>([])
  const containerStatsLoading = ref(false)
  const containerStatsError = ref<string | null>(null)
  const containerResourceStats = ref<Record<string, ContainerStatsEntry>>({})
  const isContainerDetailActive = ref(false)

  // ─── Compose 项目 ───
  const composeProjects = ref<dockerType.DockerProjectSummary[]>([])
  const projectTotal = ref(0)
  const projectPage = ref(1)
  const projectPageSize = ref(20)
  const projectListLoading = ref(false)
  const projectListError = ref<string | null>(null)
  const projectListLoadedAt = ref<number | null>(null)
  const projectDetail = ref<dockerType.DockerProjectDetail | null>(null)
  const projectDetailLoading = ref(false)
  const projectDetailError = ref<string | null>(null)
  const projectDetailLoadedAt = ref<number | null>(null)
  const projectConfiguration = ref<dockerType.DockerProjectConfiguration | null>(null)
  const projectConfigurationLoading = ref(false)
  const projectConfigurationError = ref<string | null>(null)
  const projectConfigurationLoadedAt = ref<number | null>(null)
  const projectDeploymentProgress = ref<dockerType.DockerProjectTask | null>(null)
  const projectDeploymentProgressVisible = ref(false)
  const projectDeploymentProgressError = ref<string | null>(null)
  const projectMutationLoading = ref(false)

  // ─── 镜像 / 网络 / 卷 / 系统 ───
  const imagesList = ref<dockerType.DockerImageSummary[]>([])
  const imageListLoading = ref(false)
  const imageListError = ref<string | null>(null)
  const imageListLoadedAt = ref<number | null>(null)
  const imageDeleteLoadingId = ref<string | null>(null)
  const controllerImages = ref<dockerType.DockerImageSummary[]>([])
  const controllerImageLoading = ref(false)
  const controllerImageError = ref<string | null>(null)
  const controllerImageLoadedAt = ref<number | null>(null)
  const imageDistributionTask = ref<dockerType.DockerImageDistributionTask | null>(null)
  const imageDistributionLoading = ref(false)
  const imageDistributionError = ref<string | null>(null)
  const imageDistributionStarting = ref(false)
  const imageDistributionCanceling = ref(false)
  const networks = ref<dockerType.DockerNetworkSummary[]>([])
  const networkListLoading = ref(false)
  const networkListError = ref<string | null>(null)
  const networkListLoadedAt = ref<number | null>(null)
  const networkDetail = ref<dockerType.DockerNetworkDetail | null>(null)
  const networkDetailLoading = ref(false)
  const networkDetailError = ref<string | null>(null)
  const networkCreateLoading = ref(false)
  const networkDeleteLoadingId = ref<string | null>(null)
  const networkConnectLoading = ref(false)
  const networkDisconnectLoading = ref<Record<string, boolean>>({})
  const volumes = ref<dockerType.DockerVolumeSummary[]>([])
  const volumeWarnings = ref<string[]>([])
  const volumeListLoading = ref(false)
  const volumeListError = ref<string | null>(null)
  const volumeDetail = ref<dockerType.DockerVolumeDetail | null>(null)
  const volumeDetailLoading = ref(false)
  const volumeDetailError = ref<string | null>(null)
  const volumeCreateLoading = ref(false)
  const volumeCreateError = ref<string | null>(null)
  const volumeDeleteLoadingName = ref<string | null>(null)
  const systemInfo = ref<dockerType.DockerSystemInfo | null>(null)
  const systemInfoLoading = ref(false)
  const systemInfoError = ref<string | null>(null)
  const systemInfoLoadedAt = ref<number | null>(null)
  const diskUsage = ref<dockerType.DockerDiskUsageSummary | null>(null)
  const diskUsageLoading = ref(false)
  const diskUsageError = ref<string | null>(null)
  const pruneLoading = ref(false)

  // ─── 容器创建表单 ───
  const isContainerCreateActive = ref(false)
  const containerStep = ref<'selectImage' | 'config'>('selectImage')
  const selectedImageId = ref<string | null>(null)
  const containerCreateLoading = ref(false)
  const containerCreateError = ref<string | null>(null)
  const containerForm = ref<{
    name: string
    command: string
    environment: Array<{ name: string; value: string }>
    ports: dockerType.DockerContainerCreatePort[]
    mounts: dockerType.DockerContainerCreateMount[]
    restartPolicy: dockerType.DockerContainerCreateRestartPolicy
    maximumRetryCount: number | null
    networkId: string
    autoRemove: boolean
  }>({
    name: '',
    command: '',
    environment: [],
    ports: [],
    mounts: [],
    restartPolicy: 'no',
    maximumRetryCount: null,
    networkId: '',
    autoRemove: false,
  })

  // ─── 缓存与防并发 ───
  const overviewCache = ref<OverviewCacheEntry | null>(null)
  const overviewRefreshInProgress = ref(false)
  const historyRefreshInProgress = ref(false)
  let historyRefreshQueued = false
  let imageListRequestSequence = 0
  let imageNodeId: string | null = null
  let controllerImageRequestSequence = 0
  let imageDistributionRequestSequence = 0
  let networkListRequestSequence = 0
  let networkDetailRequestSequence = 0
  let networkNodeId: string | null = null
  let volumeListRequestSequence = 0
  let volumeDetailRequestSequence = 0
  let volumeNodeId: string | null = null
  let containerListRequestSequence = 0
  let containerStatsRequestSequence = 0
  let containerNodeId: string | null = null
  let projectListRequestSequence = 0
  let projectDetailRequestSequence = 0
  let projectConfigurationRequestSequence = 0
  let projectDeploymentRequestSequence = 0
  let projectNodeId: string | null = null
  let projectOperationsNodeId: string | null = null

  // ─── 轮询定时器 ───
  let resourceUsageTimer: number | null = null
  let resourceHistoryTimer: number | null = null
  let containerStatsPollingTimer: number | null = null
  let projectOperationPollingTimer: number | null = null
  let projectOperationPollingInFlight = false
  let projectDeploymentEventSource: EventSource | null = null
  let projectDeploymentEventSourceTaskId: string | null = null
  let projectDeploymentEventSourceNodeId: string | null = null
  let projectDeploymentEventSourceHealthy = false
  const trackedProjectOperations = new Map<string, dockerType.DockerProjectTask>()
  let overviewPreferenceNodeId: string | null = null
  let pendingOverviewSelection: DockerOverviewContainerReference[] | null | undefined
  let overviewSelectionExplicitlyEmpty = false

  // ─── 容器统计批量队列 ───
  let containerStatsTimer: number | null = null
  const pendingContainerStatIds = new Set<string>()

  // ─── 选中镜像标签 (计算属性) ───
  const selectedImageLabel = computed(() => {
    const img = imagesList.value.find((i) => i.id === selectedImageId.value)
    if (!img) return ''
    return img.tags[0] || img.id.replace(/^sha256:/, '').substring(0, 12)
  })

  /** 从当前节点的浏览器偏好中重新准备资源趋势选择。 */
  const restoreOverviewSelectionPreference = () => {
    const nodeId = nodeStore.currentNodeId || 'local'
    overviewPreferenceNodeId = nodeId
    pendingOverviewSelection = readDockerOverviewSelection(nodeId)
    overviewSelectionExplicitlyEmpty = pendingOverviewSelection?.length === 0
    overviewSelectedContainerIds.value = []
  }

  /** 节点变化时切换到对应的资源趋势选择偏好。 */
  const ensureOverviewSelectionPreference = () => {
    const nodeId = nodeStore.currentNodeId || 'local'
    if (overviewPreferenceNodeId === nodeId) return
    restoreOverviewSelectionPreference()
  }

  /** 保存当前节点的有效容器选择，不写入趋势数据或时间范围。 */
  const persistOverviewSelection = (ids: string[]) => {
    ensureOverviewSelectionPreference()
    const nodeId = overviewPreferenceNodeId || nodeStore.currentNodeId || 'local'
    const references = buildDockerOverviewContainerReferences(overviewContainers.value, ids)
    writeDockerOverviewSelection(nodeId, references)
  }

  // ========================================
  // 数据重置
  // ========================================

  /** 重置所有 Docker 数据到初始状态 */
  const resetAll = () => {
    dockerStatus.value = false
    overviewCollectedAt.value = null
    overviewError.value = null
    containerStates.value = emptyContainerStates()
    projectStates.value = emptyProjectStates()
    imageCounts.value = { total: 0, dangling: 0 }
    resourceUsage.value = null
    overviewContainers.value = []
    restoreOverviewSelectionPreference()
    overviewHistoryMap.value = {}
    overviewHistoryLatestMap.value = {}
    overviewHistoryHours.value = 1
    overviewHistoryError.value = null
    containers.value = []
    containerListLoading.value = false
    containerListError.value = null
    containerListLoadedAt.value = null
    containerActionLoadingIds.value = []
    containerStatsLoading.value = false
    containerStatsError.value = null
    containerResourceStats.value = {}
    containerNodeId = null
    containerListRequestSequence += 1
    containerStatsRequestSequence += 1
    composeProjects.value = []
    projectTotal.value = 0
    projectPage.value = 1
    projectListLoading.value = false
    projectListError.value = null
    projectListLoadedAt.value = null
    projectDetail.value = null
    projectDetailLoading.value = false
    projectDetailError.value = null
    projectDetailLoadedAt.value = null
    projectConfiguration.value = null
    projectConfigurationLoading.value = false
    projectConfigurationError.value = null
    projectConfigurationLoadedAt.value = null
    projectDeploymentProgress.value = null
    projectDeploymentProgressVisible.value = false
    projectDeploymentProgressError.value = null
    projectMutationLoading.value = false
    projectNodeId = null
    projectOperationsNodeId = null
    trackedProjectOperations.clear()
    stopProjectOperationPolling()
    projectListRequestSequence += 1
    projectDetailRequestSequence += 1
    projectConfigurationRequestSequence += 1
    projectDeploymentRequestSequence += 1
    imagesList.value = []
    imageListLoading.value = false
    imageListError.value = null
    imageListLoadedAt.value = null
    imageDeleteLoadingId.value = null
    imageNodeId = null
    imageListRequestSequence += 1
    controllerImages.value = []
    controllerImageLoading.value = false
    controllerImageError.value = null
    controllerImageLoadedAt.value = null
    controllerImageRequestSequence += 1
    imageDistributionTask.value = null
    imageDistributionLoading.value = false
    imageDistributionError.value = null
    imageDistributionStarting.value = false
    imageDistributionCanceling.value = false
    imageDistributionRequestSequence += 1
    networks.value = []
    networkListLoading.value = false
    networkListError.value = null
    networkListLoadedAt.value = null
    networkDetail.value = null
    networkDetailLoading.value = false
    networkDetailError.value = null
    networkCreateLoading.value = false
    networkDeleteLoadingId.value = null
    networkConnectLoading.value = false
    networkDisconnectLoading.value = {}
    networkNodeId = null
    networkListRequestSequence += 1
    networkDetailRequestSequence += 1
    volumes.value = []
    volumeWarnings.value = []
    volumeListLoading.value = false
    volumeListError.value = null
    volumeDetail.value = null
    volumeDetailLoading.value = false
    volumeDetailError.value = null
    volumeCreateLoading.value = false
    volumeCreateError.value = null
    volumeDeleteLoadingName.value = null
    volumeNodeId = null
    volumeListRequestSequence += 1
    volumeDetailRequestSequence += 1
    systemInfo.value = null
    systemInfoLoading.value = false
    systemInfoError.value = null
    systemInfoLoadedAt.value = null
    diskUsage.value = null
    diskUsageLoading.value = false
    diskUsageError.value = null
    pruneLoading.value = false
    isContainerCreateActive.value = false
    containerCreateLoading.value = false
    containerCreateError.value = null
    overviewCache.value = null
  }

  // ========================================
  // 数据获取
  // ========================================

  /** 检测 Docker 可用性 */
  const fetchDockerAvailability = async (): Promise<boolean> => {
    const res = await dockerClient.value.fetchStatus()
    if (res.success && res.data) {
      const payload = res.data as { dockerAvailable?: boolean; dockerStatus?: string }
      dockerAvailable.value = Boolean(payload.dockerAvailable)
      dockerStatus.value = dockerAvailable.value
      dockerStatusCode.value = payload.dockerStatus || 'unknown'
      return dockerAvailable.value
    }
    dockerAvailable.value = false
    dockerStatus.value = false
    dockerStatusCode.value = 'unknown'
    return false
  }

  /** 获取概览数据并更新状态 */
  const fetchOverviewData = async (): Promise<dockerType.OverviewRealtimeResponse | null> => {
    if (!dockerAvailable.value) return null
    const res = await dockerClient.value.fetchOverviewRealtime()
    if (!res.success || !res.data) {
      overviewError.value = t('app.docker.messages.overviewFailed', {
        message: res.message || t('common.unknownError'),
      })
      resourceUsage.value = resourceUsage.value
        ? { ...resourceUsage.value, status: 'unavailable' }
        : null
      return null
    }
    const payload = res.data
    overviewError.value = null
    overviewCollectedAt.value = payload.collectedAt
    containerStates.value = payload.containerStates
    projectStates.value = payload.projectStates
    imageCounts.value = payload.images
    resourceUsage.value = payload.resourceUsage
    updateOverviewContainerState(payload.trendContainers || [])
    return payload
  }

  /** 获取当前选中容器的历史资源数据。 */
  const fetchOverviewHistoryAll = async (hours = overviewHistoryHours.value) => {
    if (!dockerAvailable.value) return
    if (historyRefreshInProgress.value) {
      historyRefreshQueued = true
      return
    }
    const ids = overviewSelectedContainerIds.value.slice(0, MAX_OVERVIEW_CONTAINERS)
    const requestKey = `${hours}:${ids.join(',')}`
    if (ids.length === 0) {
      overviewHistoryMap.value = {}
      overviewHistoryLatestMap.value = {}
      overviewHistoryError.value = null
      return
    }
    historyRefreshInProgress.value = true
    try {
      const res = await dockerClient.value.fetchContainerResourceUsageHistoryAll({ ids, hours })
      if (!res.success || !res.data) {
        overviewHistoryError.value = res.message || t('common.unknownError')
        return
      }
      overviewHistoryError.value = null
      const nextItems: Record<string, dockerType.ContainerResourceUsageHistory> = {}
      const nextLatestMap: Record<string, number | null> = {}
      for (const item of res.data.containers || []) {
        nextItems[item.id] = { points: item.points }
        const points = item.points || []
        nextLatestMap[item.id] = points.length ? points[points.length - 1]!.timestamp : null
      }
      const currentKey = `${overviewHistoryHours.value}:${overviewSelectedContainerIds.value
        .slice(0, MAX_OVERVIEW_CONTAINERS)
        .join(',')}`
      if (requestKey !== currentKey) {
        historyRefreshQueued = true
        return
      }
      overviewHistoryLatestMap.value = nextLatestMap
      overviewHistoryMap.value = nextItems
    } finally {
      historyRefreshInProgress.value = false
      if (historyRefreshQueued) {
        historyRefreshQueued = false
        void fetchOverviewHistoryAll()
      }
    }
  }

  /** 更新概览趋势容器，并恢复当前节点的浏览器选择偏好。 */
  const updateOverviewContainerState = (items: dockerType.TrendContainerItem[]) => {
    ensureOverviewSelectionPreference()
    overviewContainers.value = items

    if (items.length === 0) {
      const selectionChanged = overviewSelectedContainerIds.value.length > 0
      restoreOverviewSelectionPreference()
      if (selectionChanged) void fetchOverviewHistoryAll()
      return
    }

    let nextSelected: string[]
    let shouldPersist = false
    if (pendingOverviewSelection !== undefined) {
      const savedSelection = pendingOverviewSelection
      pendingOverviewSelection = undefined
      if (savedSelection === null) {
        nextSelected = selectOverviewContainerIds(items, [])
        overviewSelectionExplicitlyEmpty = false
        shouldPersist = nextSelected.length > 0
      } else if (savedSelection.length === 0) {
        nextSelected = []
        overviewSelectionExplicitlyEmpty = true
        shouldPersist = true
      } else {
        const restored = resolveDockerOverviewContainerIds(items, savedSelection)
        nextSelected = restored.length > 0 ? restored : selectOverviewContainerIds(items, [])
        overviewSelectionExplicitlyEmpty = false
        shouldPersist = nextSelected.length > 0
      }
    } else if (overviewSelectionExplicitlyEmpty) {
      nextSelected = []
    } else {
      nextSelected = selectOverviewContainerIds(items, overviewSelectedContainerIds.value)
      shouldPersist = nextSelected.length > 0
    }

    const selectionChanged = nextSelected.join(',') !== overviewSelectedContainerIds.value.join(',')
    overviewSelectedContainerIds.value = nextSelected
    if (shouldPersist) persistOverviewSelection(nextSelected)
    if (selectionChanged) void fetchOverviewHistoryAll()
  }

  /** 更新概览选中的容器列表。 */
  const updateOverviewSelectedContainers = (ids: string[]) => {
    ensureOverviewSelectionPreference()
    const unique = Array.from(new Set(ids))
    let nextIds = unique
    if (unique.length > MAX_OVERVIEW_CONTAINERS) {
      toastStore.error(
        t('app.docker.messages.maxOverviewContainers', { count: MAX_OVERVIEW_CONTAINERS }),
      )
      nextIds = unique.slice(0, MAX_OVERVIEW_CONTAINERS)
    }
    const availableIds = new Set(overviewContainers.value.map((item) => item.id))
    nextIds = nextIds.filter((id) => availableIds.has(id))
    overviewSelectedContainerIds.value = nextIds
    pendingOverviewSelection = undefined
    overviewSelectionExplicitlyEmpty = nextIds.length === 0
    persistOverviewSelection(nextIds)
    void fetchOverviewHistoryAll()
  }

  /** 更新资源趋势时间范围。 */
  const setOverviewHistoryHours = (hours: 1 | 6 | 12) => {
    if (overviewHistoryHours.value === hours) return
    overviewHistoryHours.value = hours
    void fetchOverviewHistoryAll(hours)
  }

  /** 将概览数据写入缓存 */
  const cacheOverviewState = () => {
    overviewCache.value = { timestamp: Date.now() }
  }

  /** 统一加载概览数据 */
  const loadOverviewData = async ({ showLoading }: { showLoading: boolean }) => {
    if (overviewRefreshInProgress.value) return
    if (showLoading) isLoading.value = true
    overviewRefreshInProgress.value = true
    try {
      const overviewResult = await fetchOverviewData()
      if (overviewResult) cacheOverviewState()
    } finally {
      overviewRefreshInProgress.value = false
      if (showLoading) isLoading.value = false
    }
  }

  /** 按需刷新概览数据 (缓存未过期则跳过) */
  const refreshOverviewDataIfNeeded = async (): Promise<'loaded' | 'cached' | 'skipped'> => {
    if (!dockerAvailable.value || overviewRefreshInProgress.value) return 'skipped'
    const cacheValid =
      overviewCache.value && Date.now() - overviewCache.value.timestamp < OVERVIEW_CACHE_TTL
    if (cacheValid) return 'cached'
    await loadOverviewData({ showLoading: true })
    return 'loaded'
  }

  /** 获取所有容器列表 */
  const fetchContainers = async (): Promise<boolean> => {
    if (!dockerAvailable.value) return false
    const nodeId = nodeStore.currentNodeId || 'local'
    if (containerNodeId !== nodeId) {
      containers.value = []
      containerResourceStats.value = {}
      containerListLoadedAt.value = null
      containerStatsError.value = null
      containerStatsRequestSequence += 1
      containerNodeId = nodeId
    }
    const sequence = ++containerListRequestSequence
    containerListLoading.value = true
    containerListError.value = null
    const res = await dockerClient.value.listContainers()
    if (
      sequence !== containerListRequestSequence ||
      nodeId !== (nodeStore.currentNodeId || 'local')
    ) {
      return false
    }
    containerListLoading.value = false
    if (!res.success || !res.data) {
      containerListError.value = t('app.docker.messages.listContainersFailed', {
        message: res.message || t('common.unknownError'),
      })
      return false
    }
    containers.value = res.data
    containerListLoadedAt.value = Date.now()
    const runningIds = (res.data || [])
      .filter((container) => container.state === 'running')
      .map((container) => container.id)
    if (runningIds.length > 0 && !isContainerDetailActive.value) {
      void queueContainerStatsFetch(runningIds)
    }
    return true
  }

  /** 获取 Compose 项目列表，并阻止旧节点响应覆盖当前状态。 */
  const fetchComposeProjects = async (
    query: dockerType.DockerProjectListQuery = {},
  ): Promise<boolean> => {
    if (!dockerAvailable.value) return false
    const nodeId = nodeStore.currentNodeId || 'local'
    if (projectNodeId !== nodeId) {
      composeProjects.value = []
      projectTotal.value = 0
      projectListLoadedAt.value = null
      projectDetail.value = null
      projectDetailLoading.value = false
      projectDetailError.value = null
      projectDetailLoadedAt.value = null
      projectConfiguration.value = null
      projectConfigurationLoading.value = false
      projectConfigurationError.value = null
      projectConfigurationLoadedAt.value = null
      projectNodeId = nodeId
      projectListRequestSequence += 1
      projectDetailRequestSequence += 1
      projectConfigurationRequestSequence += 1
      projectDeploymentRequestSequence += 1
      projectOperationsNodeId = nodeId
      trackedProjectOperations.clear()
      projectDeploymentProgress.value = null
      projectDeploymentProgressVisible.value = false
      projectDeploymentProgressError.value = null
      stopProjectOperationPolling()
    }
    const sequence = ++projectListRequestSequence
    projectListLoading.value = true
    projectListError.value = null
    const res = await dockerClient.value.listComposeProjects({
      ...query,
      page: query.page ?? projectPage.value,
      pageSize: query.pageSize ?? projectPageSize.value,
    })
    if (
      sequence !== projectListRequestSequence ||
      nodeId !== (nodeStore.currentNodeId || 'local')
    ) {
      return false
    }
    projectListLoading.value = false
    if (!res.success || !res.data) {
      projectListError.value = res.message || t('common.unknownError')
      return false
    }
    composeProjects.value = res.data.items
    projectTotal.value = res.data.total
    projectPage.value = res.data.page
    projectPageSize.value = res.data.pageSize
    projectListLoadedAt.value = Date.now()
    return true
  }

  /** 按需加载项目详情。 */
  const fetchComposeProjectDetail = async (name: string): Promise<boolean> => {
    const nodeId = nodeStore.currentNodeId || 'local'
    const sequence = ++projectDetailRequestSequence
    projectDetailLoading.value = true
    projectDetailError.value = null
    const res = await dockerClient.value.fetchComposeProject(name)
    if (
      sequence !== projectDetailRequestSequence ||
      nodeId !== (nodeStore.currentNodeId || 'local')
    ) {
      return false
    }
    projectDetailLoading.value = false
    if (!res.success || !res.data) {
      projectDetailError.value = res.message || t('common.unknownError')
      return false
    }
    projectDetail.value = res.data
    projectDetailLoadedAt.value = Date.now()
    return true
  }

  /** 清理详情并使进行中的请求失效。 */
  const clearComposeProjectDetail = () => {
    projectDetailRequestSequence += 1
    projectDetail.value = null
    projectDetailLoading.value = false
    projectDetailError.value = null
    projectDetailLoadedAt.value = null
  }

  /** 加载用户项目配置。 */
  const fetchComposeProjectConfiguration = async (name: string): Promise<boolean> => {
    const nodeId = nodeStore.currentNodeId || 'local'
    const sequence = ++projectConfigurationRequestSequence
    projectConfigurationLoading.value = true
    projectConfigurationError.value = null
    const res = await dockerClient.value.fetchComposeProjectConfiguration(name)
    if (
      sequence !== projectConfigurationRequestSequence ||
      nodeId !== (nodeStore.currentNodeId || 'local')
    ) {
      return false
    }
    projectConfigurationLoading.value = false
    if (!res.success || !res.data) {
      projectConfigurationError.value = res.message || t('common.unknownError')
      return false
    }
    projectConfiguration.value = res.data
    projectConfigurationLoadedAt.value = Date.now()
    return true
  }

  /** 清理配置状态并使进行中的请求失效。 */
  const clearComposeProjectConfiguration = () => {
    projectConfigurationRequestSequence += 1
    projectConfiguration.value = null
    projectConfigurationLoading.value = false
    projectConfigurationError.value = null
    projectConfigurationLoadedAt.value = null
  }

  /** 获取当前节点镜像列表，并阻止旧节点响应覆盖当前数据。 */
  const fetchImagesList = async (): Promise<boolean> => {
    if (!dockerAvailable.value) return false
    const nodeId = nodeStore.currentNodeId || 'local'
    if (imageListLoading.value && imageNodeId === nodeId) return false
    if (imageNodeId !== nodeId) {
      imagesList.value = []
      imageListLoadedAt.value = null
      imageNodeId = nodeId
    }
    const sequence = ++imageListRequestSequence
    imageListLoading.value = true
    imageListError.value = null
    try {
      const res = await dockerClient.value.listImages()
      if (
        sequence !== imageListRequestSequence ||
        nodeId !== (nodeStore.currentNodeId || 'local')
      ) {
        return false
      }
      if (!res.success) {
        imageListError.value = res.message || t('common.unknownError')
        return false
      }
      imagesList.value = res.data || []
      imageListLoadedAt.value = Date.now()
      return true
    } catch (error) {
      if (sequence === imageListRequestSequence) {
        imageListError.value = error instanceof Error ? error.message : String(error)
      }
      return false
    } finally {
      if (sequence === imageListRequestSequence) imageListLoading.value = false
    }
  }

  /** 获取主控镜像库。 */
  const fetchControllerImages = async (): Promise<boolean> => {
    if (controllerImageLoading.value) return false
    const sequence = ++controllerImageRequestSequence
    controllerImageLoading.value = true
    controllerImageError.value = null
    try {
      const res = await dockerApi.fetchControllerImages()
      if (sequence !== controllerImageRequestSequence) return false
      if (!res.success) {
        controllerImageError.value = res.message || t('common.unknownError')
        return false
      }
      controllerImages.value = res.data || []
      controllerImageLoadedAt.value = Date.now()
      return true
    } catch (error) {
      if (sequence === controllerImageRequestSequence) {
        controllerImageError.value = error instanceof Error ? error.message : String(error)
      }
      return false
    } finally {
      if (sequence === controllerImageRequestSequence) controllerImageLoading.value = false
    }
  }

  /** 恢复保留期内最近的未结束分发任务。 */
  const fetchRecentImageDistributionTask = async (): Promise<boolean> => {
    if (imageDistributionLoading.value) return false
    const sequence = ++imageDistributionRequestSequence
    imageDistributionLoading.value = true
    imageDistributionError.value = null
    try {
      const res = await dockerApi.fetchRecentImageDistributionTasks()
      if (sequence !== imageDistributionRequestSequence) return false
      if (!res.success) {
        imageDistributionError.value = res.message || t('common.unknownError')
        return false
      }
      const tasks = res.data || []
      imageDistributionTask.value =
        tasks.find((task) => ['pending', 'running'].includes(task.status)) || null
      return true
    } catch (error) {
      if (sequence === imageDistributionRequestSequence) {
        imageDistributionError.value = error instanceof Error ? error.message : String(error)
      }
      return false
    } finally {
      if (sequence === imageDistributionRequestSequence) imageDistributionLoading.value = false
    }
  }

  /** 创建主控镜像批量分发任务。 */
  const startImageDistribution = async (
    payload: dockerType.DockerImageDistributionCreateRequest,
  ): Promise<boolean> => {
    if (imageDistributionStarting.value) return false
    const sequence = ++imageDistributionRequestSequence
    imageDistributionLoading.value = false
    imageDistributionStarting.value = true
    imageDistributionError.value = null
    try {
      const res = await dockerApi.startImageDistributionTask(payload)
      if (sequence !== imageDistributionRequestSequence) return false
      if (!res.success || !res.data) {
        imageDistributionError.value = res.message || t('common.unknownError')
        return false
      }
      imageDistributionTask.value = res.data
      return true
    } catch (error) {
      if (sequence === imageDistributionRequestSequence) {
        imageDistributionError.value = error instanceof Error ? error.message : String(error)
      }
      return false
    } finally {
      if (sequence === imageDistributionRequestSequence) imageDistributionStarting.value = false
    }
  }

  /** 刷新当前批量分发任务，禁止轮询请求重叠。 */
  const fetchImageDistributionTask = async (): Promise<boolean> => {
    const taskId = imageDistributionTask.value?.taskId
    if (
      !taskId ||
      imageDistributionLoading.value ||
      imageDistributionStarting.value ||
      imageDistributionCanceling.value
    ) {
      return false
    }
    const sequence = ++imageDistributionRequestSequence
    imageDistributionLoading.value = true
    try {
      const res = await dockerApi.fetchImageDistributionTask(taskId)
      if (sequence !== imageDistributionRequestSequence) return false
      if (!res.success || !res.data) {
        imageDistributionError.value = res.message || t('common.unknownError')
        return false
      }
      imageDistributionTask.value = res.data
      imageDistributionError.value = null
      return true
    } catch (error) {
      if (sequence === imageDistributionRequestSequence) {
        imageDistributionError.value = error instanceof Error ? error.message : String(error)
      }
      return false
    } finally {
      if (sequence === imageDistributionRequestSequence) imageDistributionLoading.value = false
    }
  }

  /** 取消当前批量分发任务。 */
  const cancelImageDistribution = async (): Promise<boolean> => {
    const taskId = imageDistributionTask.value?.taskId
    if (!taskId || imageDistributionCanceling.value) return false
    const sequence = ++imageDistributionRequestSequence
    imageDistributionLoading.value = false
    imageDistributionCanceling.value = true
    try {
      const res = await dockerApi.cancelImageDistributionTask(taskId)
      if (sequence !== imageDistributionRequestSequence) return false
      if (!res.success || !res.data) {
        imageDistributionError.value = res.message || t('common.unknownError')
        return false
      }
      imageDistributionTask.value = res.data
      return true
    } catch (error) {
      if (sequence === imageDistributionRequestSequence) {
        imageDistributionError.value = error instanceof Error ? error.message : String(error)
      }
      return false
    } finally {
      if (sequence === imageDistributionRequestSequence) imageDistributionCanceling.value = false
    }
  }

  /** 离开镜像分发页面时废弃在途请求，并清除页面级终态与错误。 */
  const leaveImageDistributionView = () => {
    imageDistributionRequestSequence += 1
    imageDistributionLoading.value = false
    imageDistributionError.value = null
    imageDistributionStarting.value = false
    imageDistributionCanceling.value = false
    if (
      imageDistributionTask.value &&
      ['success', 'failed', 'cancelled'].includes(imageDistributionTask.value.status)
    ) {
      imageDistributionTask.value = null
    }
  }

  /** 获取网络摘要列表，并阻止旧节点响应覆盖当前状态。 */
  const fetchNetworks = async (): Promise<boolean> => {
    if (!dockerAvailable.value) return false
    const nodeId = nodeStore.currentNodeId || 'local'
    if (networkNodeId !== nodeId) {
      networks.value = []
      networkListLoadedAt.value = null
      networkDetail.value = null
      networkDetailLoading.value = false
      networkDetailError.value = null
      networkDetailRequestSequence += 1
      networkNodeId = nodeId
    }
    const sequence = ++networkListRequestSequence
    networkListLoading.value = true
    networkListError.value = null
    const res = await dockerClient.value.listNetworks()
    if (
      sequence !== networkListRequestSequence ||
      nodeId !== (nodeStore.currentNodeId || 'local')
    ) {
      return false
    }
    networkListLoading.value = false
    if (res.success && res.data) {
      networks.value = res.data
      networkListLoadedAt.value = Date.now()
      return true
    }
    networkListError.value = t('app.docker.messages.listNetworksFailed', {
      message: res.message || t('common.unknownError'),
    })
    return false
  }

  /** 获取卷摘要列表，并阻止旧节点响应覆盖当前状态。 */
  const fetchVolumes = async (): Promise<boolean> => {
    if (!dockerAvailable.value) return false
    const nodeId = nodeStore.currentNodeId || 'local'
    if (volumeNodeId !== nodeId) {
      volumes.value = []
      volumeWarnings.value = []
      volumeDetail.value = null
      volumeDetailLoading.value = false
      volumeDetailError.value = null
      volumeDetailRequestSequence += 1
      volumeNodeId = nodeId
    }
    const sequence = ++volumeListRequestSequence
    volumeListLoading.value = true
    volumeListError.value = null
    const res = await dockerClient.value.listVolumes()
    if (sequence !== volumeListRequestSequence || nodeId !== (nodeStore.currentNodeId || 'local')) {
      return false
    }
    volumeListLoading.value = false
    if (res.success && res.data) {
      volumes.value = res.data.items
      volumeWarnings.value = res.data.warnings
      return true
    }
    volumeListError.value = t('app.docker.messages.listVolumesFailed', {
      message: res.message || t('common.unknownError'),
    })
    return false
  }

  /** 按需获取卷详情和引用容器。 */
  const fetchVolumeDetail = async (name: string): Promise<boolean> => {
    const nodeId = nodeStore.currentNodeId || 'local'
    const sequence = ++volumeDetailRequestSequence
    volumeDetailLoading.value = true
    volumeDetailError.value = null
    const res = await dockerClient.value.inspectVolume(name)
    if (
      sequence !== volumeDetailRequestSequence ||
      nodeId !== (nodeStore.currentNodeId || 'local')
    ) {
      return false
    }
    volumeDetailLoading.value = false
    if (res.success && res.data) {
      volumeDetail.value = res.data
      return true
    }
    volumeDetailError.value = res.message || t('common.unknownError')
    return false
  }

  /** 清除卷详情并使尚未返回的详情请求失效。 */
  const clearVolumeDetail = (): void => {
    volumeDetailRequestSequence += 1
    volumeDetail.value = null
    volumeDetailLoading.value = false
    volumeDetailError.value = null
  }

  /** 获取 Docker 系统信息 */
  const fetchDockerInfo = async () => {
    if (!dockerAvailable.value) return
    systemInfoLoading.value = true
    systemInfoError.value = null
    try {
      const res = await dockerClient.value.fetchInfo()
      if (res.success && res.data) {
        systemInfo.value = res.data as dockerType.DockerSystemInfo
        systemInfoLoadedAt.value = Math.floor(Date.now() / 1000)
      } else {
        systemInfo.value = null
        systemInfoLoadedAt.value = null
        systemInfoError.value = res.message || t('common.unknownError')
      }
    } finally {
      systemInfoLoading.value = false
    }
  }

  /** 获取 Docker 磁盘使用与可回收空间。 */
  const fetchDockerDiskUsage = async () => {
    if (!dockerAvailable.value) return
    diskUsageLoading.value = true
    diskUsageError.value = null
    try {
      const res = await dockerClient.value.dfSystem()
      if (res.success && res.data) {
        diskUsage.value = res.data
      } else {
        diskUsage.value = null
        diskUsageError.value = res.message || t('common.unknownError')
      }
    } finally {
      diskUsageLoading.value = false
    }
  }

  /** 单容器资源统计 (带 TTL 缓存) */
  const fetchContainerResourceStats = async (id: string) => {
    if (!dockerAvailable.value) return
    const now = Date.now()
    const existing = containerResourceStats.value[id]
    if (existing && now - existing.fetchedAt < CONTAINER_STATS_TTL) return
    void queueContainerStatsFetch([id])
  }

  /** 批量容器资源统计 (防抖队列) */
  const queueContainerStatsFetch = async (ids: string[]) => {
    if (!dockerAvailable.value) return
    ids.forEach((id) => pendingContainerStatIds.add(id))
    if (containerStatsTimer !== null) return
    containerStatsTimer = window.setTimeout(async () => {
      const batchIds = Array.from(pendingContainerStatIds)
      pendingContainerStatIds.clear()
      containerStatsTimer = null
      if (batchIds.length === 0) return
      if (containerStatsLoading.value) {
        batchIds.forEach((id) => pendingContainerStatIds.add(id))
        return
      }
      const nodeId = nodeStore.currentNodeId || 'local'
      const sequence = ++containerStatsRequestSequence
      containerStatsLoading.value = true
      try {
        const res = await dockerClient.value.fetchContainerResourceUsageSummaries({ ids: batchIds })
        if (
          sequence !== containerStatsRequestSequence ||
          nodeId !== (nodeStore.currentNodeId || 'local')
        ) {
          return
        }
        if (res.success && res.data) {
          const now = Date.now()
          const updates = { ...containerResourceStats.value }
          for (const [id, summary] of Object.entries(res.data.summaries || {})) {
            updates[id] = { data: summary, fetchedAt: now }
          }
          containerResourceStats.value = updates
          containerStatsError.value = null
        } else {
          containerStatsError.value = res.message || t('common.unknownError')
        }
      } finally {
        if (sequence === containerStatsRequestSequence) containerStatsLoading.value = false
        if (pendingContainerStatIds.size > 0) void queueContainerStatsFetch([])
      }
    }, 150)
  }

  // ========================================
  // 轮询管理
  // ========================================

  /** 启动概览数据轮询 */
  const startOverviewPolling = () => {
    if (!dockerAvailable.value || resourceUsageTimer !== null) return
    resourceUsageTimer = window.setInterval(() => {
      void loadOverviewData({ showLoading: false })
    }, 30_000)
  }

  /** 停止概览数据轮询 */
  const stopOverviewPolling = () => {
    if (resourceUsageTimer === null) return
    window.clearInterval(resourceUsageTimer)
    resourceUsageTimer = null
  }

  /** 启动历史数据轮询 */
  const startHistoryPolling = () => {
    if (!dockerAvailable.value || resourceHistoryTimer !== null) return
    resourceHistoryTimer = window.setInterval(() => {
      void fetchOverviewHistoryAll()
    }, 60_000)
  }

  /** 停止历史数据轮询 */
  const stopHistoryPolling = () => {
    if (resourceHistoryTimer === null) return
    window.clearInterval(resourceHistoryTimer)
    resourceHistoryTimer = null
  }

  /** 启动容器统计轮询 */
  const startContainerStatsPolling = (activeMenuGetter: () => string) => {
    if (!dockerAvailable.value || containerStatsPollingTimer !== null) return
    containerStatsPollingTimer = window.setInterval(() => {
      if (activeMenuGetter() !== 'containers') return
      if (isContainerDetailActive.value) return
      const runningIds = containers.value
        .filter((container) => container.state === 'running')
        .map((container) => container.id)
      if (runningIds.length > 0) void queueContainerStatsFetch(runningIds)
    }, CONTAINER_STATS_TTL)
  }

  /** 停止容器统计轮询 */
  const stopContainerStatsPolling = () => {
    if (containerStatsPollingTimer === null) return
    window.clearInterval(containerStatsPollingTimer)
    containerStatsPollingTimer = null
  }

  /** 停止所有轮询 */
  const stopAllPolling = () => {
    stopOverviewPolling()
    stopHistoryPolling()
    stopContainerStatsPolling()
    stopProjectOperationPolling()
  }

  // ========================================
  // 初始加载
  // ========================================

  /** 初始加载流程 */
  const initialLoad = async () => {
    restoreOverviewSelectionPreference()
    const available = await fetchDockerAvailability()
    if (!available) {
      resetAll()
      isLoading.value = false
      return
    }
    await loadOverviewData({ showLoading: true })
  }

  // ========================================
  // 业务操作
  // ========================================

  /** 容器操作 (支持批量) */
  const handleContainerAction = async (
    idOrIds: string | null | (string | null)[],
    nameOrNames: string | null | (string | null)[],
    action: dockerType.DockerContainerAction,
  ): Promise<boolean> => {
    const ids = Array.isArray(idOrIds) ? idOrIds : [idOrIds]
    const names = Array.isArray(nameOrNames) ? nameOrNames : [nameOrNames]
    const validTargets = ids
      .map((id, index) => ({ id: id || '', name: names[index] || id || '' }))
      .filter((item) => item.id)
    if (validTargets.length === 0) {
      toastStore.error(t('app.docker.messages.invalidContainerNameOrId'))
      return false
    }
    if (validTargets.some(({ id }) => containerActionLoadingIds.value.includes(id))) {
      return false
    }
    const targetIds = validTargets.map(({ id }) => id)
    let confirmed = true
    if (action === 'remove') {
      const targetNames = validTargets.map((item) => item.name).join(', ')
      const promptName =
        validTargets.length > 1
          ? t('app.docker.messages.containerCount', { count: validTargets.length })
          : `"${targetNames}"`
      const containsManagedContainer = containers.value.some(
        (container) => targetIds.includes(container.id) && container.management.kind !== 'custom',
      )
      confirmed = await modalStore.showConfirmation(
        t(
          containsManagedContainer
            ? 'app.docker.messages.deleteManagedContainersConfirm'
            : 'app.docker.messages.deleteContainersConfirm',
          { name: promptName },
        ),
        t('app.docker.messages.deleteConfirmTitle'),
        t('app.docker.messages.deleteAction'),
        t('confirmation.cancel'),
      )
    }
    if (!confirmed) return false

    containerActionLoadingIds.value = [...containerActionLoadingIds.value, ...targetIds]
    let succeeded = false
    try {
      if (validTargets.length > 1) {
        const res = await dockerClient.value.batchContainerAction({ ids: targetIds, action })
        if (!res.success || !res.data) {
          toastStore.error(
            t('app.docker.messages.containerActionFailed', {
              name: t('app.docker.messages.containerCount', { count: validTargets.length }),
              action,
              message: res.message || t('common.unknownError'),
            }),
          )
          return false
        }
        const failures = res.data.items.filter((item) => !item.success)
        failures.forEach((item) => {
          toastStore.error(
            t('app.docker.messages.containerActionFailed', {
              name: item.name,
              action,
              message: item.errorMessage || t('common.unknownError'),
            }),
          )
        })
        succeeded = failures.length === 0
      } else {
        const [{ id, name }] = validTargets
        const client = dockerClient.value
        const res =
          action === 'start'
            ? await client.startContainer(id)
            : action === 'stop'
              ? await client.stopContainer(id)
              : action === 'restart'
                ? await client.restartContainer(id)
                : action === 'pause'
                  ? await client.pauseContainer(id)
                  : action === 'unpause'
                    ? await client.unpauseContainer(id)
                    : action === 'kill'
                      ? await client.killContainer(id)
                      : await client.removeContainer(id)
        if (!res.success) {
          toastStore.error(
            t('app.docker.messages.containerActionFailed', {
              name,
              action,
              message: res.message || t('common.unknownError'),
            }),
          )
          return false
        }
        succeeded = true
      }
      if (succeeded) {
        toastStore.success(
          t('app.docker.messages.containerActionSuccess', {
            count: validTargets.length,
            action,
          }),
        )
      }
      await Promise.all([fetchContainers(), fetchComposeProjects(), fetchOverviewData()])
      return succeeded
    } finally {
      containerActionLoadingIds.value = containerActionLoadingIds.value.filter(
        (id) => !targetIds.includes(id),
      )
    }
  }

  /** 判断项目后台操作是否仍在执行。 */
  const isProjectOperationActive = (operation: dockerType.DockerProjectTask) =>
    operation.status === 'queued' || operation.status === 'running'

  /** 合并同一进度项，避免延迟事件让百分比或终态发生回退。 */
  const mergeProjectProgressItem = (
    current: dockerType.DockerProjectTaskProgressItem,
    incoming: dockerType.DockerProjectTaskProgressItem,
  ) => {
    const terminalStatuses = new Set<dockerType.DockerProjectProgressStatus>([
      'done',
      'warning',
      'error',
    ])
    const terminalRegression = terminalStatuses.has(current.status) && incoming.status === 'working'
    const status = terminalRegression ? current.status : incoming.status
    const numericRegression =
      (typeof current.percent === 'number' &&
        typeof incoming.percent === 'number' &&
        incoming.percent < current.percent) ||
      (typeof current.currentBytes === 'number' &&
        typeof incoming.currentBytes === 'number' &&
        current.totalBytes === incoming.totalBytes &&
        incoming.currentBytes < current.currentBytes)
    const percent =
      typeof current.percent === 'number' && typeof incoming.percent === 'number'
        ? Math.max(current.percent, incoming.percent)
        : (incoming.percent ?? current.percent)
    const currentBytes =
      typeof current.currentBytes === 'number' &&
      typeof incoming.currentBytes === 'number' &&
      current.totalBytes === incoming.totalBytes
        ? Math.max(current.currentBytes, incoming.currentBytes)
        : (incoming.currentBytes ?? current.currentBytes)
    Object.assign(current, incoming, {
      status,
      action: terminalRegression || numericRegression ? current.action : incoming.action,
      details: terminalRegression || numericRegression ? current.details : incoming.details,
      percent,
      currentBytes,
    })
  }

  /** 原位合并任务快照；权威快照同时移除后端已裁剪的旧条目。 */
  const mergeProjectDeploymentSnapshot = (
    incoming: dockerType.DockerProjectTask,
    authoritative = !isProjectOperationActive(incoming),
  ) => {
    const current = projectDeploymentProgress.value
    if (!current || current.id !== incoming.id) {
      projectDeploymentProgress.value = incoming
      return incoming
    }
    const currentItems = current.progressItems
    const currentItemsById = new Map(currentItems.map((item) => [item.id, item]))
    const reconciledItems = incoming.progressItems.map((item) => {
      const existing = currentItemsById.get(item.id)
      if (existing) mergeProjectProgressItem(existing, item)
      return existing ?? item
    })
    if (authoritative) {
      currentItems.splice(0, currentItems.length, ...reconciledItems)
    } else {
      for (const item of reconciledItems) {
        if (!currentItemsById.has(item.id)) currentItems.push(item)
      }
      if (currentItems.length > MAX_PROJECT_PROGRESS_ITEMS) {
        currentItems.splice(0, currentItems.length - MAX_PROJECT_PROGRESS_ITEMS)
      }
    }
    const active = isProjectOperationActive(current) && isProjectOperationActive(incoming)
    const progressModeOrder: dockerType.DockerProjectProgressMode[] = [
      'unavailable',
      'text',
      'structured',
    ]
    const progressMode =
      active &&
      progressModeOrder.indexOf(incoming.progressMode) <
        progressModeOrder.indexOf(current.progressMode)
        ? current.progressMode
        : incoming.progressMode
    const activeStageOrder: dockerType.DockerProjectTaskStage[] = [
      'validating',
      'preparing',
      'pulling',
      'applying',
      'verifying',
    ]
    const currentStageIndex = activeStageOrder.indexOf(current.stage)
    const incomingStageIndex = activeStageOrder.indexOf(incoming.stage)
    const stage =
      active &&
      currentStageIndex >= 0 &&
      incomingStageIndex >= 0 &&
      incomingStageIndex < currentStageIndex
        ? current.stage
        : incoming.stage
    const progressPercent = active
      ? Math.max(current.progressPercent, incoming.progressPercent)
      : incoming.progressPercent
    Object.assign(current, incoming, {
      stage,
      progressMode,
      progressPercent,
      progressItems: currentItems,
    })
    return current
  }

  /** 将实时单项进度合并到当前部署快照。 */
  const applyProjectProgressUpdate = (
    operationId: string,
    update: dockerType.DockerProjectTaskProgressUpdate,
  ) => {
    const operation = projectDeploymentProgress.value
    if (!operation || operation.id !== operationId) return
    const existing = operation.progressItems.find((item) => item.id === update.item.id)
    if (existing) mergeProjectProgressItem(existing, update.item)
    else operation.progressItems.push(update.item)
    if (operation.progressItems.length > MAX_PROJECT_PROGRESS_ITEMS) {
      operation.progressItems.splice(0, operation.progressItems.length - MAX_PROJECT_PROGRESS_ITEMS)
    }
    operation.progressMode = update.progressMode
    operation.progressPercent = Math.max(operation.progressPercent, update.progressPercent)
    trackedProjectOperations.set(operationId, operation)
    projectDeploymentProgressError.value = null
  }

  /** 处理项目操作终态，确保通知和事实数据只刷新一次。 */
  const finishTrackedProjectOperation = async (operation: dockerType.DockerProjectTask) => {
    const tracked = trackedProjectOperations.delete(operation.id)
    if (projectDeploymentProgress.value?.id === operation.id) {
      mergeProjectDeploymentSnapshot(operation)
      projectDeploymentProgressError.value = null
      stopProjectDeploymentEventStream(operation.id)
    }
    if (!tracked) return
    notifyProjectOperationResult(operation)
    await Promise.all([fetchComposeProjects(), fetchOverviewData()])
  }

  /** 关闭指定部署任务的 SSE 连接。 */
  function stopProjectDeploymentEventStream(operationId?: string) {
    if (operationId && projectDeploymentEventSourceTaskId !== operationId) return
    projectDeploymentEventSource?.close()
    projectDeploymentEventSource = null
    projectDeploymentEventSourceTaskId = null
    projectDeploymentEventSourceNodeId = null
    projectDeploymentEventSourceHealthy = false
  }

  /** 为创建或重新部署任务建立实时进度事件流。 */
  function startProjectDeploymentEventStream(operation: dockerType.DockerProjectTask) {
    if (
      typeof EventSource === 'undefined' ||
      !isProjectOperationActive(operation) ||
      (operation.operation !== 'create' && operation.operation !== 'redeploy')
    ) {
      return false
    }
    const nodeId = nodeStore.currentNodeId || 'local'
    if (
      projectDeploymentEventSource &&
      projectDeploymentEventSourceTaskId === operation.id &&
      projectDeploymentEventSourceNodeId === nodeId
    ) {
      return true
    }
    stopProjectDeploymentEventStream()
    const source = new EventSource(
      dockerClient.value.composeProjectOperationEventsUrl(operation.id),
      { withCredentials: true },
    )
    projectDeploymentEventSource = source
    projectDeploymentEventSourceTaskId = operation.id
    projectDeploymentEventSourceNodeId = nodeId

    const isCurrentStream = () =>
      projectDeploymentEventSource === source &&
      projectDeploymentEventSourceTaskId === operation.id &&
      projectDeploymentEventSourceNodeId === (nodeStore.currentNodeId || 'local')
    const parseEvent = <T>(event: Event): T | null => {
      try {
        return JSON.parse((event as MessageEvent<string>).data) as T
      } catch {
        return null
      }
    }
    source.addEventListener('snapshot', (event) => {
      if (!isCurrentStream()) return
      const snapshot = parseEvent<dockerType.DockerProjectTask>(event)
      if (!snapshot || snapshot.id !== operation.id) return
      projectDeploymentEventSourceHealthy = true
      const merged = mergeProjectDeploymentSnapshot(snapshot)
      trackedProjectOperations.set(snapshot.id, merged)
      projectDeploymentProgressError.value = null
    })
    source.addEventListener('progress', (event) => {
      if (!isCurrentStream()) return
      const update = parseEvent<dockerType.DockerProjectTaskProgressUpdate>(event)
      if (!update) return
      projectDeploymentEventSourceHealthy = true
      applyProjectProgressUpdate(operation.id, update)
    })
    source.addEventListener('terminal', (event) => {
      if (!isCurrentStream()) return
      const terminal = parseEvent<dockerType.DockerProjectTask>(event)
      if (!terminal || terminal.id !== operation.id) return
      void finishTrackedProjectOperation(terminal)
    })
    source.addEventListener('resync', () => {
      if (!isCurrentStream()) return
      projectDeploymentEventSourceHealthy = false
      void pollProjectOperations()
    })
    source.onopen = () => {
      if (!isCurrentStream()) return
      projectDeploymentEventSourceHealthy = true
      projectDeploymentProgressError.value = null
    }
    source.onerror = () => {
      if (!isCurrentStream()) return
      projectDeploymentEventSourceHealthy = false
      startProjectOperationPolling()
    }
    return true
  }

  /** 跟踪新提交的后台操作；创建和重新部署同时打开部署进度。 */
  const acceptProjectOperation = (operation: dockerType.DockerProjectTask) => {
    const nodeId = nodeStore.currentNodeId || 'local'
    if (projectOperationsNodeId !== nodeId) {
      trackedProjectOperations.clear()
      projectOperationsNodeId = nodeId
    }
    trackedProjectOperations.set(operation.id, operation)
    if (operation.operation === 'create' || operation.operation === 'redeploy') {
      projectDeploymentProgress.value = operation
      projectDeploymentProgressVisible.value = true
      projectDeploymentProgressError.value = null
      startProjectDeploymentEventStream(operation)
    }
    startProjectOperationPolling()
  }

  /** 提交项目创建任务。 */
  const createComposeProject = async (
    payload: dockerType.DockerProjectCreateRequest,
  ): Promise<boolean> => {
    if (projectMutationLoading.value) return false
    projectMutationLoading.value = true
    try {
      const res = await dockerClient.value.createComposeProject(payload)
      if (!res.success || !res.data) {
        toastStore.error(res.message || t('common.unknownError'))
        return false
      }
      acceptProjectOperation(res.data)
      return true
    } finally {
      projectMutationLoading.value = false
    }
  }

  /** 提交项目启停或重启任务。 */
  const runComposeProjectLifecycle = async (
    name: string,
    operation: 'start' | 'stop' | 'restart',
  ): Promise<boolean> => {
    if (projectMutationLoading.value) return false
    projectMutationLoading.value = true
    try {
      const res = await (operation === 'start'
        ? dockerClient.value.startComposeProject(name)
        : operation === 'stop'
          ? dockerClient.value.stopComposeProject(name)
          : dockerClient.value.restartComposeProject(name))
      if (!res.success || !res.data) {
        toastStore.error(res.message || t('common.unknownError'))
        return false
      }
      acceptProjectOperation(res.data)
      return true
    } finally {
      projectMutationLoading.value = false
    }
  }

  /** 提交重新部署任务。 */
  const redeployComposeProject = async (name: string, pullImages: boolean): Promise<boolean> => {
    if (projectMutationLoading.value) return false
    projectMutationLoading.value = true
    try {
      const res = await dockerClient.value.redeployComposeProject(name, { pullImages })
      if (!res.success || !res.data) {
        toastStore.error(res.message || t('common.unknownError'))
        return false
      }
      acceptProjectOperation(res.data)
      return true
    } finally {
      projectMutationLoading.value = false
    }
  }

  /** 提交服务伸缩任务。 */
  const scaleComposeProjectService = async (
    name: string,
    service: string,
    replicas: number,
  ): Promise<boolean> => {
    if (projectMutationLoading.value) return false
    projectMutationLoading.value = true
    try {
      const res = await dockerClient.value.scaleComposeProject(name, service, { replicas })
      if (!res.success || !res.data) {
        toastStore.error(res.message || t('common.unknownError'))
        return false
      }
      acceptProjectOperation(res.data)
      return true
    } finally {
      projectMutationLoading.value = false
    }
  }

  /** 提交项目删除任务。 */
  const removeComposeProject = async (name: string): Promise<boolean> => {
    if (projectMutationLoading.value) return false
    projectMutationLoading.value = true
    try {
      const res = await dockerClient.value.deleteComposeProject(name)
      if (!res.success || !res.data) {
        toastStore.error(res.message || t('common.unknownError'))
        return false
      }
      acceptProjectOperation(res.data)
      return true
    } finally {
      projectMutationLoading.value = false
    }
  }

  /** 保存配置但不触发部署。 */
  const saveComposeProjectConfiguration = async (
    name: string,
    composeYaml: string,
    expectedRevision: number,
  ): Promise<boolean> => {
    if (projectMutationLoading.value) return false
    projectMutationLoading.value = true
    projectConfigurationError.value = null
    try {
      const res = await dockerClient.value.updateComposeProjectConfiguration(name, {
        composeYaml,
        expectedRevision,
      })
      if (!res.success || !res.data) {
        projectConfigurationError.value = res.message || t('common.unknownError')
        return false
      }
      projectConfiguration.value = res.data
      return true
    } finally {
      projectMutationLoading.value = false
    }
  }

  /** 仅使用 Agent Compose 语义校验配置，不执行本地降级。 */
  const validateComposeYaml = async (
    composeYaml: string,
  ): Promise<dockerType.DockerProjectConfigurationValidateResponse | null> => {
    projectConfigurationError.value = null
    const res = await dockerClient.value.validateComposeYaml({ composeYaml })
    if (!res.success || !res.data) {
      projectConfigurationError.value = res.message || t('common.unknownError')
      return null
    }
    return res.data
  }

  /** 恢复当前节点仍在执行的创建或重新部署进度，不加载历史记录。 */
  const recoverActiveComposeDeployment = async (): Promise<boolean> => {
    const nodeId = nodeStore.currentNodeId || 'local'
    const sequence = ++projectDeploymentRequestSequence
    const res = await dockerClient.value.fetchActiveComposeDeployment()
    if (
      sequence !== projectDeploymentRequestSequence ||
      nodeId !== (nodeStore.currentNodeId || 'local')
    ) {
      return false
    }
    projectOperationsNodeId = nodeId
    if (!res.success) {
      projectDeploymentProgressError.value = res.message || t('common.unknownError')
      return false
    }
    projectDeploymentProgressError.value = null
    if (!res.data) return true
    const alreadyTracked = trackedProjectOperations.has(res.data.id)
    projectDeploymentProgress.value = res.data
    if (!alreadyTracked) projectDeploymentProgressVisible.value = true
    trackedProjectOperations.set(res.data.id, res.data)
    startProjectDeploymentEventStream(res.data)
    startProjectOperationPolling()
    return true
  }

  /** 提示后台项目操作的最终结果。 */
  const notifyProjectOperationResult = (operation: dockerType.DockerProjectTask) => {
    const params = {
      name: operation.projectName,
      operation: t(`app.docker.projects.deploymentProgress.operations.${operation.operation}`),
    }
    if (operation.status === 'succeeded') {
      toastStore.success(t('app.docker.projects.operationResult.succeeded', params))
    } else if (operation.status === 'cancelled') {
      toastStore.warning(t('app.docker.projects.operationResult.cancelled', params))
    } else {
      toastStore.error(
        operation.errorSummary || t('app.docker.projects.operationResult.failed', params),
      )
    }
  }

  /** 轮询已由当前页面提交或恢复的操作，终态后刷新事实数据。 */
  const pollProjectOperations = async () => {
    if (projectOperationPollingInFlight || !trackedProjectOperations.size) return
    const nodeId = nodeStore.currentNodeId || 'local'
    if (projectOperationsNodeId !== nodeId) return
    projectOperationPollingInFlight = true
    try {
      const operationIds = [...trackedProjectOperations.keys()].filter(
        (operationId) =>
          !(
            projectDeploymentEventSourceHealthy &&
            projectDeploymentEventSourceTaskId === operationId
          ),
      )
      if (!operationIds.length) return
      const responses = await Promise.all(
        operationIds.map(async (operationId) => ({
          operationId,
          response: await dockerClient.value.fetchComposeProjectOperation(operationId),
        })),
      )
      if (nodeId !== (nodeStore.currentNodeId || 'local')) return
      for (const { operationId, response } of responses) {
        if (!response.success || !response.data) {
          if (projectDeploymentProgress.value?.id === operationId) {
            projectDeploymentProgressError.value = response.message || t('common.unknownError')
          }
          continue
        }
        const operation = response.data
        trackedProjectOperations.set(operationId, operation)
        if (projectDeploymentProgress.value?.id === operationId) {
          const merged = mergeProjectDeploymentSnapshot(operation, true)
          trackedProjectOperations.set(operationId, merged)
          projectDeploymentProgressError.value = null
        }
        if (!isProjectOperationActive(operation)) {
          await finishTrackedProjectOperation(operation)
        }
      }
    } finally {
      projectOperationPollingInFlight = false
    }
  }

  /** 启动无重叠的项目操作轮询。 */
  function startProjectOperationPolling() {
    if (projectOperationPollingTimer !== null || !trackedProjectOperations.size) return
    const poll = async () => {
      projectOperationPollingTimer = null
      if (document.hidden) return
      await pollProjectOperations()
      if (trackedProjectOperations.size) {
        projectOperationPollingTimer = window.setTimeout(poll, 1000)
      }
    }
    projectOperationPollingTimer = window.setTimeout(poll, 500)
  }

  /** 停止项目操作轮询。 */
  function stopProjectOperationPolling() {
    if (projectOperationPollingTimer !== null) {
      window.clearTimeout(projectOperationPollingTimer)
      projectOperationPollingTimer = null
    }
    stopProjectDeploymentEventStream()
  }

  /** 关闭部署进度视图；活动任务继续在后台接收 SSE 或轮询更新。 */
  const closeProjectDeploymentProgress = () => {
    projectDeploymentProgressVisible.value = false
    if (
      projectDeploymentProgress.value &&
      !isProjectOperationActive(projectDeploymentProgress.value)
    ) {
      projectDeploymentProgress.value = null
      projectDeploymentProgressError.value = null
    }
  }

  /** 重新打开当前节点正在跟踪的部署进度。 */
  const openProjectDeploymentProgress = () => {
    if (projectDeploymentProgress.value) projectDeploymentProgressVisible.value = true
  }

  /** 删除镜像 */
  const handleDeleteImage = async (id: string): Promise<boolean> => {
    if (!id) {
      toastStore.error(t('app.docker.messages.invalidImageId'))
      return false
    }
    if (imageDeleteLoadingId.value) return false
    const image = imagesList.value.find((item) => item.id === id)
    const displayName = image?.tags[0] || id.replace(/^sha256:/, '').substring(0, 12) || id
    const confirmed = await modalStore.showConfirmation(
      t('app.docker.messages.deleteImageConfirmAllTags', {
        name: displayName,
        count: image?.tags.length || 0,
      }),
      t('app.docker.messages.deleteConfirmTitle'),
      t('app.docker.messages.deleteAction'),
      t('confirmation.cancel'),
    )
    if (!confirmed) return false
    imageDeleteLoadingId.value = id
    try {
      const res = await dockerClient.value.removeImage(id)
      if (!res.success) {
        toastStore.error(
          t('app.docker.messages.deleteImageFailed', {
            name: displayName,
            message: res.message || t('common.unknownError'),
          }),
        )
        return false
      }
      toastStore.success(t('app.docker.messages.deleteImageSuccess', { name: displayName }))
      await Promise.all([fetchImagesList(), fetchOverviewData()])
      return true
    } finally {
      imageDeleteLoadingId.value = null
    }
  }

  /** 按需获取网络详情，并阻止过期详情覆盖当前选择。 */
  const fetchNetworkDetail = async (id: string): Promise<dockerType.DockerNetworkDetail | null> => {
    if (!id) return null
    const nodeId = nodeStore.currentNodeId || 'local'
    const sequence = ++networkDetailRequestSequence
    if (networkDetail.value?.summary.id !== id) networkDetail.value = null
    networkDetailLoading.value = true
    networkDetailError.value = null
    const res = await dockerClient.value.inspectNetwork(id)
    if (
      sequence !== networkDetailRequestSequence ||
      nodeId !== (nodeStore.currentNodeId || 'local')
    ) {
      return null
    }
    networkDetailLoading.value = false
    if (res.success && res.data) {
      networkDetail.value = res.data
      return res.data
    }
    networkDetailError.value = t('app.docker.messages.inspectNetworkFailed', {
      message: res.message || t('common.unknownError'),
    })
    return null
  }

  /** 清理当前网络详情并使在途请求失效。 */
  const clearNetworkDetail = () => {
    networkDetailRequestSequence += 1
    networkDetail.value = null
    networkDetailLoading.value = false
    networkDetailError.value = null
  }

  /** 创建 Bridge 网络。 */
  const createNetwork = async (
    payload: dockerType.DockerNetworkCreateRequest,
  ): Promise<boolean> => {
    if (networkCreateLoading.value) return false
    networkCreateLoading.value = true
    try {
      const res = await dockerClient.value.createNetwork(payload)
      if (!res.success) {
        toastStore.error(
          t('app.docker.messages.createNetworkFailed', {
            message: res.message || t('common.unknownError'),
          }),
        )
        return false
      }
      toastStore.success(t('app.docker.messages.createNetworkSuccess', { name: payload.name }))
      if (res.data?.warning) toastStore.info(res.data.warning)
      await fetchNetworks()
      return true
    } finally {
      networkCreateLoading.value = false
    }
  }

  /** 删除允许管理的网络。 */
  const removeNetwork = async (network: dockerType.DockerNetworkSummary): Promise<boolean> => {
    if (networkDeleteLoadingId.value) return false
    networkDeleteLoadingId.value = network.id
    try {
      const res = await dockerClient.value.removeNetwork(network.id)
      if (!res.success) {
        toastStore.error(
          t('app.docker.messages.deleteNetworkFailed', {
            message: res.message || t('common.unknownError'),
          }),
        )
        return false
      }
      toastStore.success(t('app.docker.messages.deleteNetworkSuccess'))
      if (networkDetail.value?.summary.id === network.id) clearNetworkDetail()
      await fetchNetworks()
      return true
    } finally {
      networkDeleteLoadingId.value = null
    }
  }

  /** 将运行中的容器连接到自定义网络。 */
  const connectNetwork = async (networkId: string, containerId: string): Promise<boolean> => {
    if (!networkId || !containerId || networkConnectLoading.value) return false
    networkConnectLoading.value = true
    try {
      const res = await dockerClient.value.connectNetwork(networkId, { container: containerId })
      if (!res.success) {
        toastStore.error(
          t('app.docker.messages.connectNetworkFailed', {
            message: res.message || t('common.unknownError'),
          }),
        )
        return false
      }
      toastStore.success(t('app.docker.messages.connectNetworkSuccess'))
      await fetchNetworkDetail(networkId)
      return true
    } finally {
      networkConnectLoading.value = false
    }
  }

  /** 从自定义网络断开容器。 */
  const disconnectNetwork = async (
    networkId: string,
    containerId: string,
    force: boolean,
  ): Promise<boolean> => {
    if (!networkId || !containerId || networkDisconnectLoading.value[containerId]) return false
    networkDisconnectLoading.value[containerId] = true
    try {
      const res = await dockerClient.value.disconnectNetwork(networkId, {
        container: containerId,
        force,
      })
      if (!res.success) {
        toastStore.error(
          t('app.docker.messages.disconnectNetworkFailed', {
            message: res.message || t('common.unknownError'),
          }),
        )
        return false
      }
      toastStore.success(t('app.docker.messages.disconnectNetworkSuccess'))
      await fetchNetworkDetail(networkId)
      return true
    } finally {
      networkDisconnectLoading.value[containerId] = false
    }
  }

  // ─── 容器创建流程 ───

  const resetContainerForm = () => {
    containerForm.value = {
      name: '',
      command: '',
      environment: [],
      ports: [],
      mounts: [],
      restartPolicy: 'no',
      maximumRetryCount: null,
      networkId: '',
      autoRemove: false,
    }
    containerCreateError.value = null
  }

  /** 启动容器创建流程 */
  const startContainerCreateFlow = async () => {
    await fetchImagesList()
    if (networks.value.length === 0) await fetchNetworks()
    selectedImageId.value = null
    containerStep.value = 'selectImage'
    resetContainerForm()
    isContainerCreateActive.value = true
  }

  /** 取消容器创建 */
  const cancelContainerCreate = () => {
    if (containerCreateLoading.value) return
    isContainerCreateActive.value = false
    selectedImageId.value = null
    containerStep.value = 'selectImage'
    resetContainerForm()
  }

  /** 提交容器创建 */
  const submitContainerConfig = async () => {
    if (containerCreateLoading.value) return
    if (!selectedImageId.value) {
      toastStore.error(t('app.docker.messages.imageRequired'))
      return
    }
    if (!containerForm.value.name.trim()) {
      toastStore.error(t('app.docker.messages.containerNameRequired'))
      return
    }
    containerCreateLoading.value = true
    containerCreateError.value = null
    try {
      const res = await dockerClient.value.createContainer({
        name: containerForm.value.name.trim(),
        imageRef: selectedImageId.value,
        command: containerForm.value.command ? ['/bin/sh', '-c', containerForm.value.command] : [],
        environment: containerForm.value.environment,
        ports: containerForm.value.ports,
        mounts: containerForm.value.mounts,
        restartPolicy: containerForm.value.restartPolicy,
        maximumRetryCount: containerForm.value.maximumRetryCount ?? undefined,
        networkId: containerForm.value.networkId || undefined,
        autoRemove: containerForm.value.autoRemove,
        autoStart: true,
      })
      if (res.success) {
        toastStore.success(
          t('app.docker.messages.createContainerSuccess', { name: containerForm.value.name }),
        )
        isContainerCreateActive.value = false
        resetContainerForm()
        selectedImageId.value = null
        await Promise.all([fetchContainers(), fetchComposeProjects(), fetchOverviewData()])
      } else {
        containerCreateError.value = t('app.docker.messages.createContainerFailed', {
          message: res.message || t('common.unknownError'),
        })
      }
    } finally {
      containerCreateLoading.value = false
    }
  }

  /** 创建本地数据卷，仅成功时刷新列表。 */
  const createVolume = async (payload: dockerType.DockerVolumeCreateRequest): Promise<boolean> => {
    if (volumeCreateLoading.value) return false
    volumeCreateLoading.value = true
    volumeCreateError.value = null
    try {
      const res = await dockerClient.value.createVolume(payload)
      if (!res.success) {
        volumeCreateError.value = t('app.docker.messages.createVolumeFailed', {
          message: res.message || t('common.unknownError'),
        })
        return false
      }
      toastStore.success(t('app.docker.messages.createVolumeSuccess', { name: payload.name }))
      await fetchVolumes()
      return true
    } finally {
      volumeCreateLoading.value = false
    }
  }

  /** 删除允许管理的数据卷，并保留失败后的现有列表。 */
  const removeVolume = async (volume: dockerType.DockerVolumeSummary): Promise<boolean> => {
    if (!volume.capabilities.canRemove || volumeDeleteLoadingName.value) return false
    volumeDeleteLoadingName.value = volume.name
    try {
      const res = await dockerClient.value.removeVolume(volume.name)
      if (!res.success) {
        toastStore.error(
          t('app.docker.messages.deleteVolumeFailed', {
            message: res.message || t('common.unknownError'),
          }),
        )
        return false
      }
      toastStore.success(t('app.docker.messages.deleteVolumeSuccess'))
      await fetchVolumes()
      return true
    } finally {
      volumeDeleteLoadingName.value = null
    }
  }

  /** 清理 Docker 系统垃圾 */
  const handlePruneSystem = async () => {
    if (pruneLoading.value) return
    const confirmed = await modalStore.showConfirmation(
      t('app.docker.messages.pruneConfirm'),
      t('app.docker.messages.pruneConfirmTitle'),
      t('app.docker.messages.pruneAction'),
      t('confirmation.cancel'),
    )
    if (!confirmed) return
    pruneLoading.value = true
    toastStore.info(t('app.docker.messages.pruneStarted'))
    try {
      const res = await dockerClient.value.pruneSystem()
      if (res.success) {
        toastStore.success(t('app.docker.messages.pruneSuccess'))
        await Promise.all([
          fetchContainers(),
          fetchImagesList(),
          fetchNetworks(),
          fetchVolumes(),
          fetchOverviewData(),
          fetchDockerDiskUsage(),
        ])
      } else {
        toastStore.error(
          t('app.docker.messages.pruneFailed', {
            message: res.message || t('common.unknownError'),
          }),
        )
      }
    } catch (error) {
      toastStore.error(
        t('app.docker.messages.pruneFailed', {
          message: error instanceof Error ? error.message : t('common.unknownError'),
        }),
      )
    } finally {
      pruneLoading.value = false
    }
  }

  // ========================================
  // 导出
  // ========================================

  return {
    // 核心状态
    dockerAvailable,
    dockerStatus,
    dockerStatusCode,
    isLoading,

    // 概览统计
    overviewCollectedAt,
    overviewError,
    containerStates,
    projectStates,
    imageCounts,
    resourceUsage,

    // 概览图表
    overviewContainers,
    overviewSelectedContainerIds,
    overviewHistoryMap,
    overviewHistoryHours,
    overviewHistoryError,

    // 容器
    containers,
    containerListLoading,
    containerListError,
    containerListLoadedAt,
    containerActionLoadingIds,
    containerStatsLoading,
    containerStatsError,
    containerResourceStats,
    isContainerDetailActive,

    // Compose 项目
    composeProjects,
    projectTotal,
    projectPage,
    projectPageSize,
    projectListLoading,
    projectListError,
    projectListLoadedAt,
    projectDetail,
    projectDetailLoading,
    projectDetailError,
    projectDetailLoadedAt,
    projectConfiguration,
    projectConfigurationLoading,
    projectConfigurationError,
    projectConfigurationLoadedAt,
    projectDeploymentProgress,
    projectDeploymentProgressVisible,
    projectDeploymentProgressError,
    projectMutationLoading,

    // 镜像 / 网络 / 卷 / 系统
    imagesList,
    imageListLoading,
    imageListError,
    imageListLoadedAt,
    imageDeleteLoadingId,
    controllerImages,
    controllerImageLoading,
    controllerImageError,
    controllerImageLoadedAt,
    imageDistributionTask,
    imageDistributionLoading,
    imageDistributionError,
    imageDistributionStarting,
    imageDistributionCanceling,
    networks,
    networkListLoading,
    networkListError,
    networkListLoadedAt,
    networkDetail,
    networkDetailLoading,
    networkDetailError,
    networkCreateLoading,
    networkDeleteLoadingId,
    networkConnectLoading,
    networkDisconnectLoading,
    volumes,
    volumeWarnings,
    volumeListLoading,
    volumeListError,
    volumeDetail,
    volumeDetailLoading,
    volumeDetailError,
    volumeCreateLoading,
    volumeCreateError,
    volumeDeleteLoadingName,
    systemInfo,
    systemInfoLoading,
    systemInfoError,
    systemInfoLoadedAt,
    diskUsage,
    diskUsageLoading,
    diskUsageError,
    pruneLoading,

    // 容器创建
    isContainerCreateActive,
    containerCreateLoading,
    containerCreateError,
    containerStep,
    selectedImageId,
    selectedImageLabel,
    containerForm,

    // 数据获取
    fetchDockerAvailability,
    fetchOverviewData,
    fetchOverviewHistoryAll,
    fetchContainers,
    fetchComposeProjects,
    fetchComposeProjectDetail,
    clearComposeProjectDetail,
    fetchComposeProjectConfiguration,
    clearComposeProjectConfiguration,
    recoverActiveComposeDeployment,
    fetchImagesList,
    fetchControllerImages,
    fetchRecentImageDistributionTask,
    startImageDistribution,
    fetchImageDistributionTask,
    cancelImageDistribution,
    leaveImageDistributionView,
    fetchNetworks,
    fetchVolumes,
    fetchVolumeDetail,
    clearVolumeDetail,
    fetchDockerInfo,
    fetchDockerDiskUsage,
    fetchContainerResourceStats,
    loadOverviewData,
    refreshOverviewDataIfNeeded,
    updateOverviewSelectedContainers,
    setOverviewHistoryHours,
    initialLoad,

    // 轮询
    startOverviewPolling,
    stopOverviewPolling,
    startHistoryPolling,
    stopHistoryPolling,
    startContainerStatsPolling,
    stopContainerStatsPolling,
    stopAllPolling,

    // 业务操作
    handleContainerAction,
    createComposeProject,
    runComposeProjectLifecycle,
    redeployComposeProject,
    scaleComposeProjectService,
    removeComposeProject,
    saveComposeProjectConfiguration,
    validateComposeYaml,
    openProjectDeploymentProgress,
    closeProjectDeploymentProgress,
    handleDeleteImage,
    fetchNetworkDetail,
    clearNetworkDetail,
    createNetwork,
    removeNetwork,
    connectNetwork,
    disconnectNetwork,

    // 容器创建
    startContainerCreateFlow,
    cancelContainerCreate,
    submitContainerConfig,
    resetContainerForm,

    // 编排与系统操作
    startProjectOperationPolling,
    stopProjectOperationPolling,
    createVolume,
    removeVolume,
    handlePruneSystem,

    // 工具
    resetAll,
  }
})
