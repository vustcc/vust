import http from '@/api'

export interface NodeMetadata {
  resource?: {
    version: string
    cpuPercent: number
    memoryUsedBytes: number
    memoryTotalBytes: number
    memoryPercent: number
    loadAvg1: number
    loadAvg5: number
    loadAvg15: number
    diskReadBytes: number
    diskWriteBytes: number
    networkRxBytes: number
    networkTxBytes: number
    collectedAt: number
  }
  [key: string]: unknown
}

export interface NodeSummaryResponse {
  nodeId: string
  name: string
  groupName: string
  description?: string
  address?: string
  servicePort?: string
  status: string
  lifecycleStatus?: string
  runtimeStatus?: string
  healthStatus?: string
  tags: string[]
  metadata?: NodeMetadata
  lastSeenAt?: string
}

export interface NodeProvisioningResponse {
  deployMethod: string
  sshAddr?: string
  sshPort?: number
  sshUser?: string
  sshAuthMode?: string
  installDir: string
  systemdServiceName: string
  expectedListenPort?: number
  lastDeployResultStatus?: string
  lastDeployErrorSummary?: string
  lastDeployAt?: string
}

export interface NodeDeployPayload {
  listenAddr?: string
  vustUrl?: string
}

export interface NodePrecheckPayload {
  name?: string
  addr?: string
  port?: string
  user?: string
  pwd?: string
  privateKey?: string
  privateKeyPassphrase?: string
  authMode?: string
  servicePort?: string
  installDir?: string
  vustUrl?: string
}

export interface NodePrecheckDetail {
  status: 'passed' | 'warning' | 'failed' | 'skipped'
  message: string
}

export interface NodeAgentStatus {
  kind:
    | 'clean'
    | 'current_controller'
    | 'other_controller'
    | 'residual_install'
    | 'controller_conflict'
    | 'version_incompatible'
  blocking: boolean
  message: string
  requiredAction: string
  detectedAgentId?: string
  detectedVustUrl?: string
  detectedVersion?: string
  existingNodeId?: string
  installDir?: string
  nodeRole?: string
}

export interface NodeVersionCompatibility {
  controllerVersion: string
  agentVersion?: string
  compatible: boolean
  reason: string
  requiredAction: string
}

export interface NodePrecheckResponse {
  passed: boolean
  agentStatus: NodeAgentStatus
  versionCompatibility: NodeVersionCompatibility
  ssh: NodePrecheckDetail
  os: NodePrecheckDetail
  permission: NodePrecheckDetail
  service: NodePrecheckDetail
  systemd: NodePrecheckDetail
  directory: NodePrecheckDetail
  docker: NodePrecheckDetail
  port: NodePrecheckDetail
  callback: NodePrecheckDetail
}

export interface NodeUpdatePayload {
  addr?: string
  port?: string
  user?: string
  authMode?: string
  pwd?: string
  privateKey?: string
  privateKeyPassphrase?: string
  servicePort?: string
  vustUrl?: string
}

export interface NodeCheckItem {
  status: 'passed' | 'warning' | 'failed' | 'skipped'
  code?: string
  message: string
}

export interface NodeCheckResponse {
  status: string
  ssh: NodeCheckItem
  service: NodeCheckItem
  api: NodeCheckItem
}

export interface NodeObservationResponse {
  observationId: string
  source: string
  systemSnapshot?: Record<string, unknown>
  dockerSnapshot?: Record<string, unknown>
  probeResult?: {
    status?: string
    sshOk?: boolean
    serviceOk?: boolean
    apiOk?: boolean
    detail?: Record<string, unknown>
  }
  observedAt: string
}

export interface NodeSessionResponse {
  sessionId: string
  agentId: string
  advertiseAddr?: string
  listenAddr?: string
  listenPort?: number
  registeredAt: string
  leaseExpiresAt: string
  lastHeartbeatAt?: string
  lastSeenAt?: string
  status: string
  closeReason?: string
  closedAt?: string
}

export interface NodeDetailResponse {
  node: NodeSummaryResponse
  provisioning?: NodeProvisioningResponse
  session?: NodeSessionResponse
  latestObservation?: NodeObservationResponse
}

export interface NodeDeployCreateResult {
  logs: string[]
  node?: NodeSummaryResponse
}

export const nodesApi = {
  list: () => {
    return http.get<NodeSummaryResponse[]>('/nodes/list')
  },
  detail: (nodeId: string) => {
    return http.get<NodeDetailResponse>(`/node/${nodeId}/detail`)
  },
  create: (payload: Record<string, unknown>) => {
    return http.post<NodeSummaryResponse>('/nodes/create', payload)
  },
  precheck: (payload: NodePrecheckPayload) => {
    return http.post<NodePrecheckResponse>('/nodes/precheck', payload, { timeout: 180000 })
  },
  deployCreate: (payload: Record<string, unknown>) => {
    return http.post<NodeDeployCreateResult>('/nodes/deploy', payload, { timeout: 300000 })
  },
  deploy: (nodeId: string, payload: NodeDeployPayload) => {
    return http.post(`/node/${nodeId}/deploy`, payload, { timeout: 300000 })
  },
  getDeployProgress: (nodeId: string) => {
    return http.get<{
      progressPercent: number
      logs: string[]
      isFinished: boolean
      error?: string
    }>(`/node/${nodeId}/deploy-progress`)
  },
  repair: (nodeId: string, payload: NodeDeployPayload = {}) => {
    return http.post(`/node/${nodeId}/repair`, payload, { timeout: 300000 })
  },
  retire: (nodeId: string) => {
    return http.post(`/node/${nodeId}/retire`)
  },
  uninstall: (nodeId: string) => {
    return http.post(`/node/${nodeId}/uninstall`)
  },
  update: (nodeId: string, payload: NodeUpdatePayload) => {
    return http.put<NodeSummaryResponse>(`/node/${nodeId}/update`, payload)
  },
  about: (nodeId: string) => {
    return http.get<{ vustUrl?: string }>(`/node/${nodeId}/agent/system/about`)
  },
  check: (nodeId: string) => {
    return http.post<NodeCheckResponse>(`/node/${nodeId}/check`)
  },
  observations: (nodeId: string, limit = 10) => {
    return http.get<NodeObservationResponse[]>(`/node/${nodeId}/observations`, { limit })
  },
  sessions: (nodeId: string, limit = 10) => {
    return http.get<NodeSessionResponse[]>(`/node/${nodeId}/sessions`, { limit })
  },
  remove: (nodeId: string) => {
    return http.delete(`/node/${nodeId}/remove`)
  },
}
