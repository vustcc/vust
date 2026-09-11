/**
 * 类型定义参考自 Rust 后端 bollard 库：
 *
 * 注意：TS 类型是手动映射的，可能和 Rust 类型存在微小差异。
 */

interface OciPlatform {
  /** The CPU architecture, for example `amd64` or `ppc64`. */
  architecture?: string

  /** The operating system, for example `linux` or `windows`. */
  os?: string

  /** Optional field specifying the operating system version, for example on Windows `10.0.19041.1165`. */
  os_version?: string

  /** Optional field specifying an array of strings, each listing a required OS feature (for example on Windows `win32k`). */
  os_features?: string[]

  /** Optional field specifying a variant of the CPU, for example `v7` to specify ARMv7 when architecture is `arm`. */
  variant?: string
}

interface OciDescriptor {
  media_type?: string

  digest?: string

  size?: number

  urls?: string[]

  annotations?: Record<string, string>

  data?: string

  platform?: OciPlatform

  /** ArtifactType is the IANA media type of this artifact. */
  artifact_type?: string
}

enum PortTypeEnum {
  EMPTY = '',
  TCP = 'tcp',
  UDP = 'udp',
  SCTP = 'sctp',
}

enum ContainerSummaryStateEnum {
  EMPTY = '',
  CREATED = 'created',
  RUNNING = 'running',
  PAUSED = 'paused',
  RESTARTING = 'restarting',
  EXITED = 'exited',
  REMOVING = 'removing',
  DEAD = 'dead',
}

export interface Port {
  IP?: string
  PrivatePort: number
  PublicPort: number
  Type?: PortTypeEnum
}

interface ContainerSummaryHostConfig {
  /** Networking mode (`host`, `none`, `container:<id>`) or name of the primary
   *  network the container is using. This field is primarily for backward
   *  compatibility. The container can be connected to multiple networks for
   *  which information can be found in the `NetworkSettings.Networks`
   *  field, which enumerates settings per network. */
  network_mode?: string

  /** Arbitrary key-value metadata attached to the container. */
  annotations?: Record<string, string>
}

export interface EndpointIpamConfig {
  /** IPv4 地址 */
  IPv4Address?: string

  /** IPv6 地址 */
  IPv6Address?: string

  /** 本地链路 IP 列表 */
  LinkLocalIPs?: string[]
}

export interface EndpointSettings {
  /** IPAM 配置 */
  IPAMConfig?: EndpointIpamConfig

  /** 链接列表 */
  Links?: string[]

  /** MAC 地址 */
  MacAddress?: string

  /** 别名列表 */
  Aliases?: string[]

  /** 驱动参数映射 */
  DriverOpts?: Record<string, string>

  /** 网关优先级 */
  GwPriority?: number

  /** 网络唯一 ID */
  NetworkID?: string

  /** 端点唯一 ID */
  EndpointID?: string

  /** 网关地址 */
  Gateway?: string

  /** IPv4 地址 */
  IPAddress?: string

  /** IPv4 掩码长度 */
  IPPrefixLen?: number

  /** IPv6 网关地址 */
  IPv6Gateway?: string

  /** 全局 IPv6 地址 */
  GlobalIPv6Address?: string

  /** 全局 IPv6 掩码长度 */
  GlobalIPv6PrefixLen?: number

  /** DNS 名称列表（非 FQDN） */
  DNSNames?: string[]
}

export interface ContainerSummaryNetworkSettings {
  /** Summary of network-settings for each network the container is attached to. */
  Networks?: Record<string, EndpointSettings>
}

/** https://docs.rs/bollard/0.19.4/bollard/models/struct.MountPoint.html */
export interface MountPoint {
  Type?: string
  Name?: string
  Source?: string
  Destination?: string
  Driver?: string
  Mode?: string
  RW?: boolean
  Propagation?: string
}

export interface ContainerSummary {
  /** The ID of this container as a 128-bit (64-character) hexadecimal string (32 bytes). */
  Id?: string

  /** The names associated with this container. */
  Names?: string[]

  /** The name or ID of the image used to create the container. */
  Image?: string

  /** The ID (digest) of the image that this container was created from. */
  ImageID?: string

  /** OCI descriptor of the platform-specific manifest of the image the container was created from. */
  ImageManifestDescriptor?: OciDescriptor

  /** Command to run when starting the container */
  Command?: string

  /** Date and time at which the container was created as a Unix timestamp */
  Created?: number

  /** Port-mappings for the container. */
  Ports?: Port[]

  /** The size of files that have been created or changed by this container. */
  SizeRw?: number

  /** The total size of all files in the read-only layers from the image that the container uses. */
  SizeRootFs?: number

  /** User-defined key/value metadata. */
  Labels?: Record<string, string>

  /** The state of this container. */
  State?: ContainerSummaryStateEnum

  /** Additional human-readable status of this container (e.g. `Exit 0`) */
  Status?: string

  HostConfig?: ContainerSummaryHostConfig

  NetworkSettings?: ContainerSummaryNetworkSettings

  /** List of mounts used by the container. */
  Mounts?: MountPoint[]
}

export type DockerContainerState =
  | 'created'
  | 'running'
  | 'paused'
  | 'restarting'
  | 'stopping'
  | 'exited'
  | 'removing'
  | 'dead'
  | 'unknown'

export type DockerContainerManagementKind = 'suite' | 'compose' | 'custom'

/** 容器在当前模块中的管理归属。 */
export interface DockerContainerManagement {
  kind: DockerContainerManagementKind
  ownerName?: string
}

/** 容器状态允许执行的操作。 */
export interface DockerContainerCapabilities {
  canStart: boolean
  canStop: boolean
  canRestart: boolean
  canPause: boolean
  canUnpause: boolean
  canKill: boolean
  canRemove: boolean
  canExec: boolean
}

/** 容器端口映射摘要。 */
export interface DockerContainerPort {
  hostIp?: string
  hostPort?: number
  containerPort: number
  protocol: string
}

/** 容器健康检查摘要。 */
export interface DockerContainerHealth {
  status: string
  failingStreak: number
}

/** 容器列表使用的稳定领域摘要。 */
export interface DockerContainerSummary {
  id: string
  name: string
  imageRef: string
  imageId: string
  command: string
  createdAt: number
  state: DockerContainerState
  statusText: string
  health?: DockerContainerHealth
  ports: DockerContainerPort[]
  management: DockerContainerManagement
  capabilities: DockerContainerCapabilities
}

/** Docker 镜像列表使用的稳定领域摘要。 */
export interface DockerImageSummary {
  id: string
  tags: string[]
  digests: string[]
  createdAt: number
  sizeBytes: number
  containerCount: number
  dangling: boolean
}

export type DockerImageTaskStatus = 'pending' | 'running' | 'success' | 'failed' | 'cancelled'
export type DockerImageSource = 'target' | 'controller' | 'registry'
export type DockerImageStage = 'checking' | 'exporting' | 'uploading' | 'loading' | 'pulling'

/** 单个目标节点的镜像分发状态。 */
export interface DockerImageDistributionTarget {
  nodeId: string
  nodeName: string
  status: DockerImageTaskStatus
  source?: DockerImageSource
  stage: DockerImageStage
  progressPercent: number
  errorSummary?: string
}

/** 主控镜像批量分发任务。 */
export interface DockerImageDistributionTask {
  taskId: string
  imageRef: string
  status: DockerImageTaskStatus
  createdAt: number
  targets: DockerImageDistributionTarget[]
}

export interface DockerImageDistributionCreateRequest {
  imageRef: string
  targetNodeIds: string[]
}

export type DockerContainerAction =
  | 'start'
  | 'stop'
  | 'restart'
  | 'pause'
  | 'unpause'
  | 'kill'
  | 'remove'

/** 批量执行容器生命周期动作。 */
export interface DockerContainerBatchActionRequest {
  ids: string[]
  action: DockerContainerAction
}

/** 单个容器的批量动作执行结果。 */
export interface DockerContainerBatchActionItem {
  id: string
  name: string
  success: boolean
  state?: DockerContainerState
  errorCode?: string
  errorMessage?: string
}

export interface DockerContainerBatchActionResult {
  items: DockerContainerBatchActionItem[]
}

export type DockerContainerCreateMountKind = 'bind' | 'volume'
export type DockerContainerCreateRestartPolicy = 'no' | 'always' | 'unless_stopped' | 'on_failure'

export interface DockerContainerCreatePort {
  hostIp?: string
  hostPort?: number
  containerPort: number
  protocol: 'tcp' | 'udp' | 'sctp'
}

export interface DockerContainerCreateMount {
  kind: DockerContainerCreateMountKind
  source: string
  target: string
  readOnly: boolean
}

/** 创建容器的结构化请求。 */
export interface DockerContainerCreateRequest {
  name: string
  imageRef: string
  command: string[]
  environment: Array<{ name: string; value: string }>
  ports: DockerContainerCreatePort[]
  mounts: DockerContainerCreateMount[]
  restartPolicy: DockerContainerCreateRestartPolicy
  maximumRetryCount?: number
  networkId?: string
  autoRemove: boolean
  autoStart: boolean
}

export interface DockerContainerCreateResult {
  id: string
  name: string
  started: boolean
  warnings: string[]
}

/** 容器重命名请求 */
export interface ContainerRenameRequest {
  name: string
}

/** 容器 exec 请求 */
export interface ContainerExecRequest {
  command: string
}

/** 容器 exec 返回 */
export interface ContainerExecResult {
  exitCode?: number | null
  output: string
}

export interface ContainerTopResponse {
  titles?: string[]
  processes?: string[][]
}

export interface ContainerStatsBatchRequest {
  ids: string[]
}

export interface ContainerStatsBatchResponse {
  summaries: Record<string, ContainerResourceUsageSummary>
}

/** 容器详情使用的稳定领域模型。 */
export interface DockerContainerDetail {
  summary: DockerContainerSummary
  createdAt?: string
  startedAt?: string
  finishedAt?: string
  exitCode?: number
  errorMessage?: string
  oomKilled: boolean
  restartCount: number
  restartPolicy: { name: string; maximumRetryCount: number }
  entrypoint: string[]
  command: string[]
  environment: Array<{ name: string; value: string }>
  mounts: Array<{
    kind: string
    name?: string
    source: string
    target: string
    readOnly: boolean
    mode?: string
  }>
  networks: Array<{
    id: string
    name: string
    endpointId: string
    macAddress: string
    ipv4Address: string
    ipv6Address: string
    gateway: string
    aliases: string[]
  }>
  labels: Record<string, string>
  logDriver: string
}

export type DockerProjectRuntimeState = 'running' | 'partial' | 'stopped' | 'unknown'
export type DockerProjectConfigurationState = 'applied' | 'pending' | 'missing'
export type DockerProjectManagementKind = 'custom' | 'suite' | 'system'
export type DockerProjectManageVia = 'projects' | 'suite_center' | 'system'

/** Compose 项目的容器状态分布。 */
export interface DockerProjectContainerStates {
  total: number
  running: number
  paused: number
  restarting: number
  exited: number
  other: number
}

/** Compose 项目的管理归属。 */
export interface DockerProjectManagement {
  kind: DockerProjectManagementKind
  ownerName?: string
  readOnly: boolean
  manageVia: DockerProjectManageVia
}

/** Compose 项目允许执行的操作。 */
export interface DockerProjectCapabilities {
  canStart: boolean
  canStop: boolean
  canRestart: boolean
  canRedeploy: boolean
  canEditConfiguration: boolean
  canScale: boolean
  canRemove: boolean
}

/** Compose 项目列表项。 */
export interface DockerProjectSummary {
  name: string
  createdAt: number
  runtimeState: DockerProjectRuntimeState
  configurationState: DockerProjectConfigurationState
  serviceCount: number
  containerStates: DockerProjectContainerStates
  management: DockerProjectManagement
  capabilities: DockerProjectCapabilities
}

/** Compose 项目分页列表。 */
export interface DockerProjectPage {
  total: number
  page: number
  pageSize: number
  items: DockerProjectSummary[]
}

/** Compose 项目列表筛选。 */
export interface DockerProjectListQuery {
  keyword?: string
  managementKind?: DockerProjectManagementKind
  runtimeState?: DockerProjectRuntimeState
  page?: number
  pageSize?: number
}

/** Compose 服务内的容器摘要。 */
export interface DockerProjectContainerSummary {
  id: string
  name: string
  state: string
  ipAddresses: string[]
  publishedPorts: string[]
}

/** Compose 服务摘要。 */
export interface DockerProjectServiceSummary {
  name: string
  image?: string
  runtimeState: DockerProjectRuntimeState
  containerStates: DockerProjectContainerStates
  containers: DockerProjectContainerSummary[]
}

/** Compose 项目详情。 */
export interface DockerProjectDetail {
  summary: DockerProjectSummary
  services: DockerProjectServiceSummary[]
}

/** Compose 项目创建请求。 */
export interface DockerProjectCreateRequest {
  name: string
  composeYaml: string
}

/** Compose 项目配置。 */
export interface DockerProjectConfiguration {
  composeYaml: string
  revision: number
  appliedRevision?: number
  state: DockerProjectConfigurationState
}

/** Compose 配置保存请求。 */
export interface DockerProjectConfigurationUpdateRequest {
  composeYaml: string
  expectedRevision: number
}

/** Compose 配置校验结果。 */
export interface DockerProjectConfigurationValidateResponse {
  valid: boolean
  error?: string
}

export type DockerProjectTaskOperation =
  | 'create'
  | 'start'
  | 'stop'
  | 'restart'
  | 'redeploy'
  | 'scale'
  | 'remove'
export type DockerProjectTaskStatus = 'queued' | 'running' | 'succeeded' | 'failed' | 'cancelled'
export type DockerProjectTaskStage =
  | 'validating'
  | 'preparing'
  | 'pulling'
  | 'applying'
  | 'verifying'
  | 'rolling_back'
  | 'cleaning_up'
  | 'completed'
  | 'cancelled'
  | 'interrupted'
export type DockerProjectProgressMode = 'structured' | 'text' | 'unavailable'
export type DockerProjectProgressPhase = 'pulling' | 'applying'
export type DockerProjectProgressStatus = 'working' | 'done' | 'warning' | 'error'

/** Compose 命令输出中的单个资源进度快照。 */
export interface DockerProjectTaskProgressItem {
  id: string
  parentId?: string
  phase: DockerProjectProgressPhase
  status: DockerProjectProgressStatus
  label: string
  action: string
  details?: string
  currentBytes?: number | null
  totalBytes?: number | null
  percent?: number | null
}

/** Compose 项目任务事件流中的单项进度更新。 */
export interface DockerProjectTaskProgressUpdate {
  progressMode: DockerProjectProgressMode
  progressPercent: number
  item: DockerProjectTaskProgressItem
}

/** Compose 项目后台任务。 */
export interface DockerProjectTask {
  id: string
  projectName: string
  operation: DockerProjectTaskOperation
  status: DockerProjectTaskStatus
  stage: DockerProjectTaskStage
  progressPercent: number
  progressMode: DockerProjectProgressMode
  progressItems: DockerProjectTaskProgressItem[]
  serviceName?: string
  replicas?: number
  pullImages: boolean
  errorSummary?: string
  cleanupWarning?: string
  createdAt: number
  startedAt?: number
  finishedAt?: number
}

/** Docker daemon 镜像与代理设置。 */
export interface DockerDaemonSettings {
  registryMirrors: string[]
  proxy: string
  proxyEnabled: boolean
}

/** Docker 网络 IPAM 配置。 */
export interface DockerNetworkIpamConfig {
  subnet?: string
  gateway?: string
  ipRange?: string
}

export type DockerNetworkManagementKind = 'system' | 'compose' | 'suite' | 'custom'

/** Docker 网络的管理归属。 */
export interface DockerNetworkManagement {
  kind: DockerNetworkManagementKind
  ownerName?: string
}

/** Docker 网络允许执行的操作。 */
export interface DockerNetworkCapabilities {
  canRemove: boolean
  canManageConnections: boolean
}

/** Docker 网络列表项。 */
export interface DockerNetworkSummary {
  id: string
  name: string
  createdAt?: number
  driver: string
  scope: string
  enableIpv4: boolean
  enableIpv6: boolean
  internal: boolean
  attachable: boolean
  ingress: boolean
  configOnly: boolean
  subnets: string[]
  management: DockerNetworkManagement
  capabilities: DockerNetworkCapabilities
}

/** Docker 网络中的容器端点。 */
export interface DockerNetworkContainer {
  id: string
  name: string
  endpointId?: string
  macAddress?: string
  ipv4Address?: string
  ipv6Address?: string
}

/** Docker Overlay 网络的对等节点。 */
export interface DockerNetworkPeer {
  name?: string
  ip?: string
}

/** Docker 网络详情。 */
export interface DockerNetworkDetail {
  summary: DockerNetworkSummary
  ipamConfigs: DockerNetworkIpamConfig[]
  options: Record<string, string>
  labels: Record<string, string>
  containers: DockerNetworkContainer[]
  peers: DockerNetworkPeer[]
}

/** Bridge 网络创建请求。 */
export interface DockerNetworkCreateRequest {
  name: string
  internal: boolean
  enableIpv6: boolean
  ipv4?: DockerNetworkIpamConfig
  ipv6?: DockerNetworkIpamConfig
  options?: Record<string, string>
  labels?: Record<string, string>
}

/** Docker 网络创建结果。 */
export interface DockerNetworkCreateResult {
  id: string
  warning?: string
}

/** 网络连接请求 */
export interface NetworkConnectRequest {
  container: string
}

/** 网络断开请求 */
export interface NetworkDisconnectRequest {
  container: string
  force: boolean
}

/** 容器资源统计快照。 */
export interface ContainerResourceUsageSummary {
  status: ResourceSampleStatus
  collectedAt: number | null
  cpuCorePercent: number
  memoryWorkingSetBytes: number
  memoryLimitBytes: number
  memoryPercent: number
  networkRxBytesPerSecond: number | null
  networkTxBytesPerSecond: number | null
}

export type ResourceSampleStatus = 'fresh' | 'partial' | 'stale' | 'unavailable'

/** 宿主机 Docker 资源汇总。 */
export interface HostResourceUsageSummary {
  status: ResourceSampleStatus
  collectedAt: number | null
  runningContainerCount: number
  sampledContainerCount: number
  cpuHostPercent: number
  cpuCorePercent: number
  memoryWorkingSetBytes: number
  memoryLimitBytes: number
  memoryPercent: number
}

/** 容器资源趋势点。 */
export interface ContainerResourceUsagePoint {
  timestamp: number
  cpuCorePercent: number
  memoryWorkingSetBytes: number
  memoryLimitBytes: number
  memoryPercent: number
  networkRxBytesPerSecond: number | null
  networkTxBytesPerSecond: number | null
}

export interface ContainerResourceUsageHistory {
  points: ContainerResourceUsagePoint[]
}

export interface ContainerStatsHistoryAllItem {
  id: string
  name: string
  points: ContainerResourceUsagePoint[]
}

export interface ContainerStatsHistoryAllResponse {
  containers: ContainerStatsHistoryAllItem[]
}

export interface ContainerStateCounts {
  total: number
  running: number
  paused: number
  restarting: number
  exited: number
  other: number
}

export interface ProjectStateCounts {
  total: number
  healthy: number
  partial: number
  stopped: number
  unknown: number
}

export interface ImageCounts {
  total: number
  dangling: number
}

export interface TrendContainerItem {
  id: string
  name: string
  createdAt: number
  state: string
}

export interface OverviewRealtimeResponse {
  collectedAt: number
  containerStates: ContainerStateCounts
  projectStates: ProjectStateCounts
  images: ImageCounts
  resourceUsage: HostResourceUsageSummary
  trendContainers: TrendContainerItem[]
}

export interface ContainerStatsHistoryQuery {
  ids: string[]
  hours?: number
}

export interface DockerDiskUsageCategory {
  totalCount: number
  activeCount: number
  sizeBytes: number
  reclaimableBytes: number
}

export interface DockerDiskUsageSummary {
  collectedAt: number
  images: DockerDiskUsageCategory
  containers: DockerDiskUsageCategory
  volumes: DockerDiskUsageCategory
  buildCache: DockerDiskUsageCategory
}

/** 容器引用 */
export interface ContainerRef {
  id: string
  name: string
}

/** Docker 系统信息 */
export interface DockerSystemInfo {
  ServerVersion?: string
  Driver?: string
  KernelVersion?: string
  OperatingSystem?: string
  OSType?: string
  Architecture?: string
  NCPU?: number
  MemTotal?: number
  Images?: number
  Containers?: number
  ContainersRunning?: number
  ContainersPaused?: number
  ContainersStopped?: number
}

export type DockerVolumeManagementKind = 'suite' | 'compose' | 'custom'

/** Docker 卷归属信息。 */
export interface DockerVolumeManagement {
  kind: DockerVolumeManagementKind
  ownerName?: string
}

/** Docker 卷在当前模块允许执行的操作。 */
export interface DockerVolumeCapabilities {
  canRemove: boolean
}

/** Docker 卷列表摘要。 */
export interface DockerVolumeSummary {
  name: string
  createdAt?: number
  management: DockerVolumeManagement
  capabilities: DockerVolumeCapabilities
}

/** Docker 卷列表响应。 */
export interface DockerVolumeListResponse {
  items: DockerVolumeSummary[]
  warnings: string[]
}

/** 引用 Docker 卷的容器挂载信息。 */
export interface DockerVolumeContainerReference {
  id: string
  name: string
  state: string
  destination?: string
  readOnly: boolean
}

/** Docker 卷详情。 */
export interface DockerVolumeDetail {
  summary: DockerVolumeSummary
  mountpoint: string
  scope: string
  options: Record<string, string>
  labels: Record<string, string>
  referencedContainers: DockerVolumeContainerReference[]
}

/** Docker 本地卷创建请求。 */
export interface DockerVolumeCreateRequest {
  name: string
  options?: Record<string, string>
  labels?: Record<string, string>
}

/** Compose 服务容器 (通过标签筛选) */
export interface ComposeContainerSummary extends ContainerSummary {
  projectName: string
  serviceName: string
}
