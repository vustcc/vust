import { createPinia } from 'pinia'
import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { notificationsApi } from '@/api/modules/notifications'
import type { NotificationDetail, NotificationSummary } from '@/api/generated'
import NotificationCenterView from '@/apps/views/NotificationCenterView.vue'
import en from '@/locales/en'
import zh from '@/locales/zh'

const openWindowWithPayloadMock = vi.hoisted(() => vi.fn())

vi.mock('@/api/modules/notifications', () => ({
  notificationsApi: {
    query: vi.fn(),
    unreadSummary: vi.fn(),
    detail: vi.fn(),
    updateReadState: vi.fn(),
    readAll: vi.fn(),
    updateArchiveState: vi.fn(),
    updateBatchArchiveState: vi.fn(),
  },
}))

vi.mock('@/stores/window-manager', () => ({
  useWindowManagerStore: () => ({
    openWindowWithPayload: openWindowWithPayloadMock,
  }),
}))

const response = (notificationId: string) => ({
  success: true,
  code: 200,
  message: '',
  data: {
    total: 1,
    page: 1,
    pageSize: 20,
    items: [
      {
        notificationId,
        createdAt: '2026-07-18T00:00:00Z',
        code: 'scriptRunFinished' as const,
        category: 'task' as const,
        attentionLevel: 'info' as const,
        outcome: 'success' as const,
        source: { module: 'scripts' as const, nodeName: 'Node A' },
        subject: { kind: 'script', id: 'script-1', displayName: notificationId },
        operationEventId: `event-${notificationId}`,
        parameters: {},
        capabilities: {
          canViewDetails: true,
          canMarkRead: true,
          canMarkUnread: false,
          canArchive: true,
          canRestore: false,
          canOpenTarget: false,
        },
      },
    ],
  },
})

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

function mountView() {
  return mount(NotificationCenterView, {
    global: {
      plugins: [createPinia(), createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
    },
  })
}

describe('NotificationCenterView', () => {
  it('只允许最新列表请求更新状态并提供稳定控件标识', async () => {
    const first = deferred<ReturnType<typeof response>>()
    const second = deferred<ReturnType<typeof response>>()
    vi.mocked(notificationsApi.query)
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise)
    const wrapper = mountView()
    const latest = (
      wrapper.vm as unknown as { loadNotifications: () => Promise<void> }
    ).loadNotifications()
    second.resolve(response('latest'))
    await latest
    first.resolve(response('stale'))
    await flushPromises()
    expect(wrapper.text()).toContain('latest')
    expect(wrapper.text()).not.toContain('stale')
    expect(wrapper.attributes('data-page')).toBe('notification-center')
    expect(wrapper.find('#notification-keyword').attributes('name')).toBe('notificationKeyword')
    expect(vi.mocked(notificationsApi.query).mock.calls[0]?.[1]?.aborted).toBe(true)
    wrapper.unmount()
  })

  it('刷新失败时保留已加载通知并展示非阻塞警告', async () => {
    vi.mocked(notificationsApi.query)
      .mockReset()
      .mockResolvedValueOnce(response('kept'))
      .mockResolvedValueOnce({ success: false, code: 500, message: 'refresh failed' })
    const wrapper = mountView()
    await flushPromises()
    await (
      wrapper.vm as unknown as { loadNotifications: (silent: boolean) => Promise<void> }
    ).loadNotifications(true)
    await flushPromises()
    expect(wrapper.text()).toContain('kept')
    expect(wrapper.find('[data-slot="refresh-warning"]').exists()).toBe(true)
    wrapper.unmount()
  })

  it('使用表格当前页全选并通过选择栏清除选择', async () => {
    vi.mocked(notificationsApi.query).mockReset().mockResolvedValue(response('selectable'))
    const wrapper = mountView()
    await flushPromises()
    expect(wrapper.text()).toContain('关注级别')
    expect(wrapper.text()).toContain('提示')
    expect(wrapper.text()).toContain('结果')
    expect(wrapper.text()).toContain('成功')
    expect(wrapper.text()).toContain('目标')
    expect(wrapper.get('[data-ui="notification-table"]').text()).toContain('selectable')
    expect(wrapper.get('[data-ui="notification-table"]').text()).not.toContain('Node A')

    await wrapper.get('[data-ui="table-select-all"] input').setValue(true)
    await flushPromises()
    expect(wrapper.get('[data-ui="selection-bar"] .vl-selection-count').text()).toBe('1')
    expect(wrapper.get('[data-ui="table-row-selection"] input').element.checked).toBe(true)
    expect(wrapper.findAll('tbody .vl-table-row.is-selected')).toHaveLength(1)

    await wrapper.get('[data-ui="clear-selection"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-ui="selection-bar"]').exists()).toBe(false)
    expect(wrapper.get('[data-ui="table-row-selection"] input').element.checked).toBe(false)
    expect(wrapper.findAll('tbody .vl-table-row.is-selected')).toHaveLength(0)
    wrapper.unmount()
  })

  it('无业务结果时在列表和详情中显示占位符', async () => {
    const notification = {
      notificationId: 'node-offline',
      createdAt: '2026-08-09T08:31:58Z',
      code: 'nodeOffline',
      category: 'system',
      attentionLevel: 'warning',
      outcome: null,
      source: { module: 'nodes' },
      subject: { kind: 'node', id: 'node-1', displayName: 'Node 1' },
      operationEventId: 'event-node-offline',
      parameters: { reason: 'leaseExpired' },
      readAt: '2026-08-09T08:32:00Z',
      capabilities: {
        canViewDetails: true,
        canMarkRead: false,
        canMarkUnread: true,
        canArchive: true,
        canRestore: false,
        canOpenTarget: false,
      },
    } as unknown as NotificationSummary
    vi.mocked(notificationsApi.query)
      .mockReset()
      .mockResolvedValue({
        success: true,
        code: 200,
        message: '',
        data: { total: 1, page: 1, pageSize: 20, items: [notification] },
      })
    vi.mocked(notificationsApi.detail).mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: {
        ...notification,
        outcome: undefined,
        traceId: 'trace-node-offline',
      } as NotificationDetail,
    })
    const wrapper = mountView()
    await flushPromises()

    expect(wrapper.get('[data-slot="outcome-empty"]').text()).toBe('—')
    expect(wrapper.get('[data-ui="notification-table"]').text()).not.toContain('成功')

    await (
      wrapper.vm as unknown as {
        openDetail: (item: NotificationSummary) => Promise<void>
      }
    ).openDetail(notification)
    const detailItems = (
      wrapper.vm as unknown as {
        detailItems: Array<{ label: string; value: string }>
      }
    ).detailItems

    expect(detailItems.find((item) => item.label === '结果')).toMatchObject({ value: '—' })
    wrapper.unmount()
  })

  it('通知详情中的任务 ID 占用抽屉整行', async () => {
    vi.mocked(notificationsApi.query).mockReset().mockResolvedValue(response('with-task'))
    vi.mocked(notificationsApi.detail).mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: {
        notificationId: 'with-task',
        createdAt: '2026-07-18T00:00:00Z',
        code: 'scriptRunFinished',
        category: 'task',
        attentionLevel: 'info',
        outcome: 'success',
        source: { module: 'scripts', nodeName: 'Node A' },
        subject: { kind: 'script', id: 'script-1', displayName: 'Script 1' },
        taskId: '019fa1b7-cab3-7083-9aea-581e8c05d6c4',
        operationEventId: '019fa1b7-d3ae-71a0-88f0-32e79c2e6044',
        parameters: {},
        readAt: '2026-07-18T00:01:00Z',
        action: null,
        capabilities: {
          canViewDetails: true,
          canMarkRead: false,
          canMarkUnread: true,
          canArchive: true,
          canRestore: false,
          canOpenTarget: false,
        },
        errorCode: null,
        errorSummary: null,
        traceId: '019fa1b7-cab4-7910-8f64-7150af7c5dc6',
      },
    })
    const wrapper = mountView()
    await flushPromises()
    const item = response('with-task').data.items[0]

    await (
      wrapper.vm as unknown as {
        openDetail: (notification: typeof item) => Promise<void>
      }
    ).openDetail(item)

    const detailItems = (
      wrapper.vm as unknown as {
        detailItems: Array<{ label: string; value: string; span?: number }>
      }
    ).detailItems
    expect(detailItems.find((detailItem) => detailItem.label === '任务 ID')).toMatchObject({
      value: '019fa1b7-cab3-7083-9aea-581e8c05d6c4',
      span: 2,
    })
    wrapper.unmount()
  })

  it('文件任务通知只按总条目数展示范围', async () => {
    const result = response('file-task')
    result.data.items[0] = {
      ...result.data.items[0],
      code: 'fileTaskFinished',
      source: { module: 'files', nodeName: 'Local Node' },
      subject: undefined,
      parameters: {
        operation: 'remove',
        totalItemCount: 2,
        completedItemCount: 2,
        failedItemCount: 0,
      },
    }
    vi.mocked(notificationsApi.query).mockReset().mockResolvedValue(result)
    const wrapper = mountView()
    await flushPromises()

    expect(wrapper.get('[data-ui="notification-table"]').text()).toContain('2 个条目')
    expect(wrapper.get('[data-ui="notification-table"]').text()).not.toContain('/root/cc')
    expect(wrapper.get('[data-ui="notification-table"]').text()).not.toContain('/root/bb')
    wrapper.unmount()
  })
})
