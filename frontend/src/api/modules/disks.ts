/**
 * @file disks.ts
 * @description 节点磁盘管理稳定领域 API。
 */

import type { AxiosRequestConfig } from 'axios'
import http from '@/api'
import type {
  CreateDiskOperationRequest,
  DiskDetail,
  DiskInventory,
  DiskOperation,
} from '@/api/generated/disks'

const config = (signal?: AbortSignal): AxiosRequestConfig => ({ signal })
const nodePath = (nodeId: string) => `/node/${encodeURIComponent(nodeId)}`

export const disksApi = {
  inventory: (nodeId: string, signal?: AbortSignal) =>
    http.get<DiskInventory>(`${nodePath(nodeId)}/disks`, undefined, config(signal)),
  detail: (nodeId: string, diskId: string, signal?: AbortSignal) =>
    http.get<DiskDetail>(
      `${nodePath(nodeId)}/disks/${encodeURIComponent(diskId)}`,
      undefined,
      config(signal),
    ),
  createOperation: (nodeId: string, request: CreateDiskOperationRequest, idempotencyKey: string) =>
    http.post<DiskOperation>(`${nodePath(nodeId)}/disk-operations`, request, {
      headers: { 'Idempotency-Key': idempotencyKey },
    }),
  operation: (nodeId: string, operationId: string, signal?: AbortSignal) =>
    http.get<DiskOperation>(
      `${nodePath(nodeId)}/disk-operations/${encodeURIComponent(operationId)}`,
      undefined,
      config(signal),
    ),
  cancelOperation: (nodeId: string, operationId: string) =>
    http.post<DiskOperation>(
      `${nodePath(nodeId)}/disk-operations/${encodeURIComponent(operationId)}/cancel`,
    ),
}

export type {
  CreateDiskOperationRequest,
  DiskDetail,
  DiskInventory,
  DiskOperation,
  DiskPartition,
  DiskSummary,
} from '@/api/generated/disks'
