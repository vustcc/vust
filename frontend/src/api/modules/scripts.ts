/**
 * @file scripts.ts
 * @description 脚本库领域 API；正文按需加载，手动执行使用一次性终端会话。
 */

import type { AxiosRequestConfig } from 'axios'
import http from '@/api'
import type {
  CreateScriptRequest,
  CreateScriptRunRequest,
  ScriptDetail,
  ScriptListPage,
  ScriptRun,
  UpdateScriptRequest,
} from '@/api/generated/scripts'

export type ScriptListQuery = {
  keyword?: string
  page?: number
  pageSize?: number
  sortBy?: 'name' | 'updatedAt'
  sortOrder?: 'asc' | 'desc'
}

const config = (signal?: AbortSignal): AxiosRequestConfig => ({ signal })

export const scriptsApi = {
  list: (query: ScriptListQuery = {}, signal?: AbortSignal) =>
    http.get<ScriptListPage>('/scripts', query, config(signal)),
  detail: (scriptId: string, signal?: AbortSignal) =>
    http.get<ScriptDetail>(`/scripts/${scriptId}`, undefined, config(signal)),
  create: (payload: CreateScriptRequest) => http.post<ScriptDetail>('/scripts', payload),
  update: (scriptId: string, payload: UpdateScriptRequest) =>
    http.patch<ScriptDetail>(`/scripts/${scriptId}`, payload),
  remove: (scriptId: string) => http.delete<void>(`/scripts/${scriptId}`),
  startRun: (scriptId: string, payload: CreateScriptRunRequest, idempotencyKey: string) =>
    http.post<ScriptRun>(`/scripts/${scriptId}/runs`, payload, {
      headers: { 'Idempotency-Key': idempotencyKey },
    }),
  dismissRun: (runId: string) => http.delete<void>(`/script-runs/${runId}`),
}

export type {
  CreateScriptRequest,
  CreateScriptRunRequest,
  ScriptDetail,
  ScriptListPage,
  ScriptRun,
  ScriptRunStatus,
  ScriptSummary,
  UpdateScriptRequest,
} from '@/api/generated/scripts'
