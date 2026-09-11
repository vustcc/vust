import http from '@/api'
import * as docker from '../interface/docker'
import type { AxiosProgressEvent } from 'axios'

const IMAGE_UPLOAD_TIMEOUT_MS = 24 * 60 * 60 * 1000
const CONTAINER_BATCH_ACTION_TIMEOUT_MS = 610 * 1000

export interface DockerImageLoadOptions {
  signal?: AbortSignal
  onUploadProgress?: (progressEvent: AxiosProgressEvent) => void
}

const buildDockerPath = (path: string, nodeId?: string) => {
  if (!nodeId || nodeId === 'local') {
    return path
  }
  return `/node/${encodeURIComponent(nodeId)}${path}`
}

const createScopedDockerApi = (nodeId?: string) => ({
  fetchStatus: () => {
    return http.get<{ dockerAvailable?: boolean; dockerStatus?: string }>(
      buildDockerPath('/agent/docker/status', nodeId),
    )
  },
  fetchInfo: () => {
    return http.get(buildDockerPath('/agent/docker/info', nodeId))
  },

  /** 获取概览实时数据，合并了容器和镜像统计 */
  fetchOverviewRealtime: () => {
    return http.post<docker.OverviewRealtimeResponse>(
      buildDockerPath('/agent/docker/overview/realtime', nodeId),
    )
  },
  listContainers: () => {
    return http.get<docker.DockerContainerSummary[]>(
      buildDockerPath('/agent/docker/containers', nodeId),
    )
  },
  createContainer: (payload: docker.DockerContainerCreateRequest) => {
    return http.post<docker.DockerContainerCreateResult>(
      buildDockerPath('/agent/docker/containers', nodeId),
      payload,
    )
  },
  inspectContainer: (id: string) => {
    return http.get<docker.DockerContainerDetail>(
      buildDockerPath(`/agent/docker/containers/${encodeURIComponent(id)}`, nodeId),
    )
  },
  startContainer: (id: string) => {
    return http.post(
      buildDockerPath(`/agent/docker/containers/${encodeURIComponent(id)}/start`, nodeId),
    )
  },
  stopContainer: (id: string) => {
    return http.post(
      buildDockerPath(`/agent/docker/containers/${encodeURIComponent(id)}/stop`, nodeId),
    )
  },
  restartContainer: (id: string) => {
    return http.post(
      buildDockerPath(`/agent/docker/containers/${encodeURIComponent(id)}/restart`, nodeId),
    )
  },
  renameContainer: (id: string, payload: docker.ContainerRenameRequest) => {
    return http.post(
      buildDockerPath(`/agent/docker/containers/${encodeURIComponent(id)}/rename`, nodeId),
      payload,
    )
  },
  pauseContainer: (id: string) => {
    return http.post(
      buildDockerPath(`/agent/docker/containers/${encodeURIComponent(id)}/pause`, nodeId),
    )
  },
  unpauseContainer: (id: string) => {
    return http.post(
      buildDockerPath(`/agent/docker/containers/${encodeURIComponent(id)}/unpause`, nodeId),
    )
  },
  killContainer: (id: string) => {
    return http.post(
      buildDockerPath(`/agent/docker/containers/${encodeURIComponent(id)}/kill`, nodeId),
    )
  },
  removeContainer: (id: string) => {
    return http.delete(
      buildDockerPath(`/agent/docker/containers/${encodeURIComponent(id)}`, nodeId),
    )
  },
  batchContainerAction: (payload: docker.DockerContainerBatchActionRequest) => {
    return http.post<docker.DockerContainerBatchActionResult>(
      buildDockerPath('/agent/docker/containers/actions', nodeId),
      payload,
      { timeout: CONTAINER_BATCH_ACTION_TIMEOUT_MS },
    )
  },
  execContainer: (id: string, payload: docker.ContainerExecRequest) => {
    return http.post<docker.ContainerExecResult>(
      buildDockerPath(`/agent/docker/containers/${encodeURIComponent(id)}/exec`, nodeId),
      payload,
    )
  },
  fetchContainerTop: (id: string) => {
    return http.get<docker.ContainerTopResponse>(
      buildDockerPath(`/agent/docker/containers/${encodeURIComponent(id)}/top`, nodeId),
    )
  },
  listComposeProjects: (params?: docker.DockerProjectListQuery) => {
    return http.get<docker.DockerProjectPage>(
      buildDockerPath('/agent/docker/compose/projects', nodeId),
      params,
    )
  },
  fetchComposeProject: (name: string) => {
    return http.get<docker.DockerProjectDetail>(
      buildDockerPath(`/agent/docker/compose/projects/${encodeURIComponent(name)}`, nodeId),
    )
  },
  createComposeProject: (payload: docker.DockerProjectCreateRequest) => {
    return http.post<docker.DockerProjectTask>(
      buildDockerPath('/agent/docker/compose/projects', nodeId),
      payload,
    )
  },
  startComposeProject: (name: string) => {
    return http.post<docker.DockerProjectTask>(
      buildDockerPath(`/agent/docker/compose/projects/${encodeURIComponent(name)}/start`, nodeId),
    )
  },
  stopComposeProject: (name: string) => {
    return http.post<docker.DockerProjectTask>(
      buildDockerPath(`/agent/docker/compose/projects/${encodeURIComponent(name)}/stop`, nodeId),
    )
  },
  restartComposeProject: (name: string) => {
    return http.post<docker.DockerProjectTask>(
      buildDockerPath(`/agent/docker/compose/projects/${encodeURIComponent(name)}/restart`, nodeId),
    )
  },
  redeployComposeProject: (name: string, payload: { pullImages: boolean }) => {
    return http.post<docker.DockerProjectTask>(
      buildDockerPath(
        `/agent/docker/compose/projects/${encodeURIComponent(name)}/deployments`,
        nodeId,
      ),
      payload,
    )
  },
  scaleComposeProject: (name: string, service: string, payload: { replicas: number }) => {
    return http.put<docker.DockerProjectTask>(
      buildDockerPath(
        `/agent/docker/compose/projects/${encodeURIComponent(name)}/services/${encodeURIComponent(service)}/replicas`,
        nodeId,
      ),
      payload,
    )
  },
  fetchComposeProjectConfiguration: (name: string) => {
    return http.get<docker.DockerProjectConfiguration>(
      buildDockerPath(
        `/agent/docker/compose/projects/${encodeURIComponent(name)}/configuration`,
        nodeId,
      ),
    )
  },
  updateComposeProjectConfiguration: (
    name: string,
    payload: docker.DockerProjectConfigurationUpdateRequest,
  ) => {
    return http.put<docker.DockerProjectConfiguration>(
      buildDockerPath(
        `/agent/docker/compose/projects/${encodeURIComponent(name)}/configuration`,
        nodeId,
      ),
      payload,
    )
  },
  validateComposeYaml: (payload: { composeYaml: string }) => {
    return http.post<docker.DockerProjectConfigurationValidateResponse>(
      buildDockerPath('/agent/docker/compose/configurations/validate', nodeId),
      payload,
    )
  },
  deleteComposeProject: (name: string) => {
    return http.delete<docker.DockerProjectTask>(
      buildDockerPath(`/agent/docker/compose/projects/${encodeURIComponent(name)}`, nodeId),
    )
  },
  fetchActiveComposeDeployment: () => {
    return http.get<docker.DockerProjectTask | null>(
      buildDockerPath('/agent/docker/compose/deployments/active', nodeId),
    )
  },
  fetchComposeProjectOperation: (operationId: string) => {
    return http.get<docker.DockerProjectTask>(
      buildDockerPath(
        `/agent/docker/compose/project-operations/${encodeURIComponent(operationId)}`,
        nodeId,
      ),
    )
  },
  composeProjectOperationEventsUrl: (operationId: string) => {
    return `/api/v1${buildDockerPath(
      `/agent/docker/compose/project-operations/${encodeURIComponent(operationId)}/events`,
      nodeId,
    )}`
  },
  listImages: () => {
    return http.get<docker.DockerImageSummary[]>(buildDockerPath('/agent/docker/images', nodeId))
  },
  /** 将 Docker 镜像归档上传并载入当前作用域节点。 */
  loadImage: (payload: FormData, options: DockerImageLoadOptions = {}) => {
    return http.upload<string>(buildDockerPath('/agent/docker/images/load', nodeId), payload, {
      ...options,
      timeout: IMAGE_UPLOAD_TIMEOUT_MS,
    })
  },
  fetchDaemonSettings: () => {
    return http.get<docker.DockerDaemonSettings>(
      buildDockerPath('/agent/docker/daemon/settings', nodeId),
    )
  },
  updateDaemonSettings: (payload: docker.DockerDaemonSettings) => {
    return http.put<docker.DockerDaemonSettings>(
      buildDockerPath('/agent/docker/daemon/settings', nodeId),
      payload,
      { timeout: 120000 },
    )
  },
  listVolumes: () => {
    return http.get<docker.DockerVolumeListResponse>(
      buildDockerPath('/agent/docker/volumes', nodeId),
    )
  },
  createVolume: (payload: docker.DockerVolumeCreateRequest) => {
    return http.post<docker.DockerVolumeSummary>(
      buildDockerPath('/agent/docker/volumes', nodeId),
      payload,
    )
  },
  removeVolume: (name: string) => {
    return http.delete(buildDockerPath(`/agent/docker/volumes/${encodeURIComponent(name)}`, nodeId))
  },
  inspectVolume: (name: string) => {
    return http.get<docker.DockerVolumeDetail>(
      buildDockerPath(`/agent/docker/volumes/${encodeURIComponent(name)}`, nodeId),
    )
  },
  dfSystem: () => {
    return http.get<docker.DockerDiskUsageSummary>(
      buildDockerPath('/agent/docker/system/df', nodeId),
    )
  },
  pruneSystem: () => {
    return http.post(buildDockerPath('/agent/docker/system/prune', nodeId), undefined, {
      timeout: 600000,
    })
  },
  /** 获取 Docker 网络摘要列表。 */
  listNetworks: () => {
    return http.get<docker.DockerNetworkSummary[]>(
      buildDockerPath('/agent/docker/networks', nodeId),
    )
  },
  createNetwork: (payload: docker.DockerNetworkCreateRequest) => {
    return http.post<docker.DockerNetworkCreateResult>(
      buildDockerPath('/agent/docker/networks', nodeId),
      payload,
    )
  },
  inspectNetwork: (id: string) => {
    return http.get<docker.DockerNetworkDetail>(
      buildDockerPath(`/agent/docker/networks/${encodeURIComponent(id)}`, nodeId),
    )
  },
  removeNetwork: (id: string) => {
    return http.delete(buildDockerPath(`/agent/docker/networks/${encodeURIComponent(id)}`, nodeId))
  },
  connectNetwork: (id: string, payload: docker.NetworkConnectRequest) => {
    return http.post(
      buildDockerPath(`/agent/docker/networks/${encodeURIComponent(id)}/connect`, nodeId),
      payload,
    )
  },
  disconnectNetwork: (id: string, payload: docker.NetworkDisconnectRequest) => {
    return http.post(
      buildDockerPath(`/agent/docker/networks/${encodeURIComponent(id)}/disconnect`, nodeId),
      payload,
    )
  },
  fetchResourceUsageHistory: (hours?: number) => {
    return http.post(
      buildDockerPath('/agent/docker/stats/history', nodeId),
      hours ? { hours } : undefined,
    )
  },
  fetchContainerResourceUsageSummary: (id: string) => {
    return http.get<docker.ContainerResourceUsageSummary>(
      buildDockerPath(`/agent/docker/containers/${encodeURIComponent(id)}/stats/summary`, nodeId),
    )
  },
  fetchContainerResourceUsageHistory: (id: string, hours?: number) => {
    return http.post<docker.ContainerStatsHistoryAllResponse>(
      buildDockerPath(`/agent/docker/containers/${encodeURIComponent(id)}/stats/history`, nodeId),
      hours ? { hours } : undefined,
    )
  },
  fetchContainerResourceUsageHistoryAll: (payload: docker.ContainerStatsHistoryQuery) => {
    return http.post<docker.ContainerStatsHistoryAllResponse>(
      buildDockerPath('/agent/docker/containers/stats/history', nodeId),
      payload,
    )
  },
  fetchContainerResourceUsageSummaries: (payload: docker.ContainerStatsBatchRequest) => {
    return http.post<docker.ContainerStatsBatchResponse>(
      buildDockerPath('/agent/docker/containers/stats/summary', nodeId),
      payload,
    )
  },
  fetchContainerLogs: (containerId: string, tail = 100) => {
    return http.get<string[]>(
      buildDockerPath(`/agent/docker/containers/${encodeURIComponent(containerId)}/logs`, nodeId),
      { tail },
    )
  },
  installDocker: (timeoutSecs: number = 600) => {
    return http.post<{
      exitCode: number
      stdout: string
      stderr: string
      timedOut: boolean
      startedAt: number
      finishedAt: number
    }>(
      buildDockerPath('/agent/docker/install', nodeId),
      {
        mirror: 'official',
        timeoutSecs,
      },
      {
        timeout: timeoutSecs * 1000,
      },
    )
  },
  removeImage: (id: string) => {
    return http.delete<unknown[]>(
      buildDockerPath(`/agent/docker/images/${encodeURIComponent(id)}`, nodeId),
    )
  },
  resolveImage: (imageName: string) => {
    return http.post<ImageResolveResponse>(
      buildDockerPath('/agent/docker/images/resolve', nodeId),
      {
        imageName,
      },
    )
  },
  searchImages: (payload: { keyword: string; page: number; pageSize: number }) => {
    return http.post<ImageSearchResponse>(
      buildDockerPath('/agent/docker/images/search', nodeId),
      payload,
    )
  },
  fetchImageTags: (payload: { repository: string; page: number; pageSize: number }) => {
    return http.post<ImageTagsResponse>(
      buildDockerPath('/agent/docker/images/tags', nodeId),
      payload,
    )
  },
})

export interface ImageTagInfo {
  name: string
  fullSize?: number
  lastUpdated?: string
}

export interface ImageSearchResult {
  repository: string
  displayName: string
  description?: string
  starCount?: number
  pullCount?: number
  isOfficial: boolean
  isAutomated: boolean
}

export interface ImageSearchResponse {
  page: number
  pageSize: number
  hasMore: boolean
  results: ImageSearchResult[]
}

export interface ImageTagsResponse {
  repository: string
  page: number
  pageSize: number
  hasMore: boolean
  tags: ImageTagInfo[]
}

export interface ImageResolveResponse {
  repository: string
  displayName: string
  requestedTag?: string
  defaultTag?: string
  tags: ImageTagInfo[]
}

export type ImagePullStatus = 'pending' | 'running' | 'success' | 'failed' | 'cancelled'

export interface ImagePullProgress {
  taskId: string
  nodeId: string
  imageRef: string
  status: ImagePullStatus
  source?: 'target' | 'controller' | 'registry'
  stage: 'checking' | 'exporting' | 'uploading' | 'loading' | 'pulling'
  progressPercent: number
  statusText: string
  controllerError?: string
  registryError?: string
}

export const dockerApi = Object.assign(createScopedDockerApi(), {
  forNode: createScopedDockerApi,
  startImageTask: (payload: { nodeId: string; imageRef: string; sourceMode: 'controller-first' }) =>
    http.post<ImagePullProgress>('/docker/image-tasks', payload),
  fetchImageTaskProgress: (taskId: string) =>
    http.get<ImagePullProgress>(`/docker/image-tasks/${encodeURIComponent(taskId)}/progress`),
  cancelImageTask: (taskId: string) =>
    http.delete<ImagePullProgress>(`/docker/image-tasks/${encodeURIComponent(taskId)}`),
  fetchControllerImages: () => http.get<docker.DockerImageSummary[]>('/docker/controller/images'),
  startImageDistributionTask: (payload: docker.DockerImageDistributionCreateRequest) =>
    http.post<docker.DockerImageDistributionTask>('/docker/image-distribution-tasks', payload),
  fetchImageDistributionTask: (taskId: string) =>
    http.get<docker.DockerImageDistributionTask>(
      `/docker/image-distribution-tasks/${encodeURIComponent(taskId)}`,
    ),
  fetchRecentImageDistributionTasks: () =>
    http.get<docker.DockerImageDistributionTask[]>('/docker/image-distribution-tasks/recent'),
  cancelImageDistributionTask: (taskId: string) =>
    http.delete<docker.DockerImageDistributionTask>(
      `/docker/image-distribution-tasks/${encodeURIComponent(taskId)}`,
    ),
})
