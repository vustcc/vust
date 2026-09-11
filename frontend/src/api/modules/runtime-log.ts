import http from '@/api'
import type { RuntimeLogFileList, RuntimeLogQuery, RuntimeLogQueryResult } from '@/api/generated'

export const runtimeLogApi = {
  files(params: { service: string; nodeId?: string }, signal?: AbortSignal) {
    return http.get<RuntimeLogFileList>('/runtime-logs/files', params, { signal })
  },
  query(payload: RuntimeLogQuery, signal?: AbortSignal) {
    return http.post<RuntimeLogQueryResult>('/runtime-logs/query', payload, { signal })
  },
}
