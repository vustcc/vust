import http from '@/api'
import type { OperationLogDetail, OperationLogPage, OperationLogQuery } from '@/api/generated'

export const operationLogApi = {
  query(payload: OperationLogQuery, signal?: AbortSignal) {
    return http.post<OperationLogPage>('/operation-logs/query', payload, { signal })
  },
  detail(eventId: string, signal?: AbortSignal) {
    return http.get<OperationLogDetail>(
      `/operation-logs/${encodeURIComponent(eventId)}`,
      undefined,
      {
        signal,
      },
    )
  },
}
