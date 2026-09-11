import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { notificationsApi } from '@/api/modules/notifications'
import { useNotificationCenterStore } from '@/stores/notification-center'

vi.mock('@/api/modules/notifications', () => ({
  notificationsApi: { unreadSummary: vi.fn() },
}))

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

class EventSourceMock {
  static instances: EventSourceMock[] = []
  readonly url: string
  readonly withCredentials: boolean
  onmessage: ((event: MessageEvent) => void) | null = null
  closed = false

  constructor(url: string | URL, init?: EventSourceInit) {
    this.url = String(url)
    this.withCredentials = init?.withCredentials ?? false
    EventSourceMock.instances.push(this)
  }

  emitChanged() {
    this.onmessage?.(new MessageEvent('message', { data: 'changed' }))
  }

  close() {
    this.closed = true
  }
}

describe('NotificationCenterStore', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    EventSourceMock.instances = []
  })

  afterEach(() => {
    vi.useRealTimers()
    vi.unstubAllGlobals()
  })

  it('未读摘要请求不重叠，失败时保留旧计数', async () => {
    const pending = deferred<{
      success: boolean
      code: number
      message: string
      data: { unreadCount: number; latestCreatedAt: string; version: string }
    }>()
    vi.mocked(notificationsApi.unreadSummary).mockReturnValueOnce(pending.promise)
    const store = useNotificationCenterStore()
    const first = store.refreshUnreadSummary()
    expect(await store.refreshUnreadSummary()).toBe(false)
    expect(notificationsApi.unreadSummary).toHaveBeenCalledTimes(1)
    pending.resolve({
      success: true,
      code: 200,
      message: '',
      data: {
        unreadCount: 8,
        latestCreatedAt: '2026-07-18T00:00:00Z',
        version: 'v1',
      },
    })
    await first
    expect(store.unreadCount).toBe(8)

    vi.mocked(notificationsApi.unreadSummary).mockRejectedValueOnce(new Error('offline'))
    await store.refreshUnreadSummary()
    expect(store.unreadCount).toBe(8)
    expect(store.error).toBe('offline')
  })

  it('收到 SSE 变化信号后刷新权威摘要并在停止时关闭连接', async () => {
    vi.stubGlobal('EventSource', EventSourceMock)
    vi.mocked(notificationsApi.unreadSummary)
      .mockResolvedValueOnce({
        success: true,
        code: 200,
        message: '',
        data: { unreadCount: 1, latestCreatedAt: '2026-07-18T00:00:00Z', version: 'v1' },
      })
      .mockResolvedValueOnce({
        success: true,
        code: 200,
        message: '',
        data: { unreadCount: 2, latestCreatedAt: '2026-07-18T00:01:00Z', version: 'v2' },
      })
    const store = useNotificationCenterStore()

    store.startPolling()
    await vi.waitFor(() => expect(store.unreadCount).toBe(1))
    const eventSource = EventSourceMock.instances[0]
    expect(eventSource.url).toBe('/api/v1/notifications/events')
    expect(eventSource.withCredentials).toBe(true)

    eventSource.emitChanged()
    await vi.waitFor(() => expect(store.unreadCount).toBe(2))

    store.stopPolling(true)
    expect(eventSource.closed).toBe(true)
    expect(store.unreadCount).toBe(0)
  })

  it('SSE 不可用时仍通过页面可见状态下的 30 秒轮询校准', async () => {
    vi.useFakeTimers()
    vi.stubGlobal('EventSource', undefined)
    vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible')
    vi.mocked(notificationsApi.unreadSummary).mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: { unreadCount: 3, latestCreatedAt: '2026-07-18T00:00:00Z', version: 'v3' },
    })
    const store = useNotificationCenterStore()

    store.startPolling()
    await vi.advanceTimersByTimeAsync(0)
    expect(notificationsApi.unreadSummary).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(30_000)
    expect(notificationsApi.unreadSummary).toHaveBeenCalledTimes(2)

    store.stopPolling()
  })
})
