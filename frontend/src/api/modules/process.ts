/**
 * @file process.ts
 * @description 固定节点的进程与网络语义 API。
 */

import http from '@/api'
import type {
  NetworkConnectionListPage,
  ProcessActionRequest,
  ProcessForceKillConfirmation,
  ProcessListPage,
  ProcessSignalResult,
} from '@/api/generated'

export type ProcessSummary = ProcessListPage['entries'][number]
export type NetworkConnectionSummary = NetworkConnectionListPage['entries'][number]
export type ProcessSortBy =
  | 'pid'
  | 'name'
  | 'cpuPercent'
  | 'memoryPercent'
  | 'connectionCount'
  | 'startedAt'
export type NetworkSortBy =
  | 'protocol'
  | 'localEndpoint'
  | 'remoteEndpoint'
  | 'state'
  | 'processName'
export type SortOrder = 'asc' | 'desc'

export interface ProcessListParams {
  query?: string
  status?: ProcessSummary['state']
  page: number
  pageSize: number
  sortBy: ProcessSortBy
  sortOrder: SortOrder
}

export interface NetworkConnectionListParams {
  query?: string
  state?: NetworkConnectionSummary['state']
  protocol?: NetworkConnectionSummary['protocol']
  page: number
  pageSize: number
  sortBy: NetworkSortBy
  sortOrder: SortOrder
}

const nodePath = (nodeId: string, suffix: string) =>
  `/node/${encodeURIComponent(nodeId)}/${suffix.replace(/^\/+/, '')}`

/** 为窗口创建固定节点 API，禁止随后跟随全局节点变化。 */
const createScopedProcessApi = (nodeId: string) => ({
  listProcesses: (params: ProcessListParams) =>
    http.get<ProcessListPage>(nodePath(nodeId, 'processes/list'), params),
  listNetworkConnections: (params: NetworkConnectionListParams) =>
    http.get<NetworkConnectionListPage>(nodePath(nodeId, 'network-connections/list'), params),
  terminate: (processId: string, payload: ProcessActionRequest) =>
    http.post<ProcessSignalResult>(
      nodePath(nodeId, `process/${encodeURIComponent(processId)}/terminate`),
      payload,
    ),
  createForceKillConfirmation: (processId: string) =>
    http.post<ProcessForceKillConfirmation>(
      nodePath(nodeId, `process/${encodeURIComponent(processId)}/force-kill-confirmation`),
    ),
  forceKill: (processId: string, payload: ProcessActionRequest) =>
    http.post<ProcessSignalResult>(
      nodePath(nodeId, `process/${encodeURIComponent(processId)}/force-kill`),
      payload,
    ),
})

export const processApi = {
  forNode: createScopedProcessApi,
}
