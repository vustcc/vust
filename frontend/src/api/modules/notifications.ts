import http from '@/api'
import type {
  NotificationArchiveStateRequest,
  NotificationBatchArchiveStateRequest,
  NotificationDetail,
  NotificationPage,
  NotificationQuery,
  NotificationReadStateRequest,
  NotificationUnreadSummary,
} from '@/api/generated'

/** 个人通知中心只读内容与个人状态接口。 */
export const notificationsApi = {
  query(payload: NotificationQuery, signal?: AbortSignal) {
    return http.post<NotificationPage>('/notifications/query', payload, { signal })
  },
  unreadSummary(signal?: AbortSignal) {
    return http.get<NotificationUnreadSummary>('/notifications/unread-summary', undefined, {
      signal,
    })
  },
  detail(notificationId: string, signal?: AbortSignal) {
    return http.get<NotificationDetail>(
      `/notifications/${encodeURIComponent(notificationId)}`,
      undefined,
      { signal },
    )
  },
  updateReadState(notificationId: string, payload: NotificationReadStateRequest) {
    return http.patch<null>(
      `/notifications/${encodeURIComponent(notificationId)}/read-state`,
      payload,
    )
  },
  readAll() {
    return http.post<null>('/notifications/read-all')
  },
  updateArchiveState(notificationId: string, payload: NotificationArchiveStateRequest) {
    return http.patch<null>(
      `/notifications/${encodeURIComponent(notificationId)}/archive-state`,
      payload,
    )
  },
  updateBatchArchiveState(payload: NotificationBatchArchiveStateRequest) {
    return http.post<null>('/notifications/archive-state', payload)
  },
}
