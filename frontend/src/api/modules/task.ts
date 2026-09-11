/**
 * @file task.ts
 * @description 计划任务稳定领域 API；不再包含旧字段归一化与强制同步入口。
 */

import type { AxiosRequestConfig } from 'axios'
import http from '@/api'
import type {
  CreateScheduledTaskBatchRequest,
  CreateScheduledTaskMigrationRequest,
  CreateScheduledTaskRequest,
  ScheduledTaskBatch,
  ScheduledTaskDetail,
  ScheduledTaskListPage,
  ScheduledTaskOperation,
  ScheduledTaskRun,
  ScheduledTaskRunOutput,
  ScheduledTaskRunPage,
  UpdateScheduledTaskRequest,
  UpdateScheduledTaskStateRequest,
} from '@/api/generated/scheduled-tasks'
import type { ScheduledTaskListQuery, ScheduledTaskMutationResponse } from '@/api/interface/task'

const requestConfig = (signal?: AbortSignal): AxiosRequestConfig => ({ signal })

export const taskApi = {
  list: (query: ScheduledTaskListQuery = {}, signal?: AbortSignal) => {
    return http.get<ScheduledTaskListPage>('/scheduled-tasks', query, requestConfig(signal))
  },
  detail: (taskId: string, signal?: AbortSignal) => {
    return http.get<ScheduledTaskDetail>(
      `/scheduled-tasks/${taskId}`,
      undefined,
      requestConfig(signal),
    )
  },
  create: (payload: CreateScheduledTaskRequest) => {
    return http.post<ScheduledTaskMutationResponse>('/scheduled-tasks', payload)
  },
  update: (taskId: string, payload: UpdateScheduledTaskRequest) => {
    return http.patch<ScheduledTaskMutationResponse>(`/scheduled-tasks/${taskId}`, payload)
  },
  updateState: (taskId: string, payload: UpdateScheduledTaskStateRequest) => {
    return http.patch<ScheduledTaskMutationResponse>(`/scheduled-tasks/${taskId}/state`, payload)
  },
  remove: (taskId: string) => {
    return http.delete<ScheduledTaskOperation>(`/scheduled-tasks/${taskId}`)
  },
  startRun: (taskId: string) => {
    return http.post<ScheduledTaskRun>(`/scheduled-tasks/${taskId}/runs`)
  },
  listRuns: (taskId: string, page = 1, pageSize = 50, signal?: AbortSignal) => {
    return http.get<ScheduledTaskRunPage>(
      `/scheduled-tasks/${taskId}/runs`,
      { page, pageSize },
      requestConfig(signal),
    )
  },
  runDetail: (taskId: string, runId: string, signal?: AbortSignal) => {
    return http.get<ScheduledTaskRun>(
      `/scheduled-tasks/${taskId}/runs/${runId}`,
      undefined,
      requestConfig(signal),
    )
  },
  runOutput: (
    taskId: string,
    runId: string,
    offsetBytes = 0,
    limitBytes = 65_536,
    signal?: AbortSignal,
  ) => {
    return http.get<ScheduledTaskRunOutput>(
      `/scheduled-tasks/${taskId}/runs/${runId}/output`,
      { offsetBytes, limitBytes },
      requestConfig(signal),
    )
  },
  cancelRun: (taskId: string, runId: string) => {
    return http.post<ScheduledTaskRun>(`/scheduled-tasks/${taskId}/runs/${runId}/cancel`)
  },
  migrate: (taskId: string, payload: CreateScheduledTaskMigrationRequest) => {
    return http.post<ScheduledTaskOperation>(`/scheduled-tasks/${taskId}/migrations`, payload)
  },
  operation: (operationId: string, signal?: AbortSignal) => {
    return http.get<ScheduledTaskOperation>(
      `/scheduled-task-operations/${operationId}`,
      undefined,
      requestConfig(signal),
    )
  },
  cancelOperation: (operationId: string) => {
    return http.post<ScheduledTaskOperation>(`/scheduled-task-operations/${operationId}/cancel`)
  },
  createBatch: (payload: CreateScheduledTaskBatchRequest) => {
    return http.post<ScheduledTaskBatch>('/scheduled-task-batches', payload)
  },
  batch: (batchId: string, signal?: AbortSignal) => {
    return http.get<ScheduledTaskBatch>(
      `/scheduled-task-batches/${batchId}`,
      undefined,
      requestConfig(signal),
    )
  },
}
