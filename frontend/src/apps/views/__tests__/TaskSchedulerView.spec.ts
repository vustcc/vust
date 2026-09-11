import { createPinia } from 'pinia'
import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ScheduledTaskSummary } from '@/api/generated/scheduled-tasks'
import { taskApi } from '@/api/modules/task'
import { nodesApi } from '@/api/modules/nodes'
import { VustActionMenu } from '@/components/ui'
import TaskSchedulerView from '@/apps/views/TaskSchedulerView.vue'
import en from '@/locales/en'
import zh from '@/locales/zh'

vi.mock('@/api/modules/task', () => ({
  taskApi: {
    list: vi.fn(),
    detail: vi.fn(),
    create: vi.fn(),
    update: vi.fn(),
    updateState: vi.fn(),
    remove: vi.fn(),
    startRun: vi.fn(),
    listRuns: vi.fn(),
    runDetail: vi.fn(),
    runOutput: vi.fn(),
    cancelRun: vi.fn(),
    migrate: vi.fn(),
    operation: vi.fn(),
    cancelOperation: vi.fn(),
    createBatch: vi.fn(),
    batch: vi.fn(),
  },
}))
vi.mock('@/api/modules/nodes', () => ({ nodesApi: { list: vi.fn() } }))

const task: ScheduledTaskSummary = {
  taskId: 'task-readonly',
  name: 'Readonly Task',
  node: { nodeId: 'local', nodeName: 'Local' },
  schedule: { cronExpr: '*/5 * * * *', timeZone: 'Asia/Shanghai', summary: 'Every 5 minutes' },
  desiredState: 'enabled',
  deployment: { status: 'ready', revision: 1 },
  nextRun: { status: 'scheduled', at: '2026-07-16T10:00:00Z' },
  ownership: { kind: 'custom' },
  capabilities: {
    canUpdate: false,
    canChangeState: false,
    canRun: false,
    canRemove: false,
    canMigrate: false,
  },
  createdAt: '2026-07-16T00:00:00Z',
  updatedAt: '2026-07-16T00:00:00Z',
}

describe('TaskSchedulerView', () => {
  beforeEach(() => {
    sessionStorage.clear()
    vi.mocked(nodesApi.list).mockResolvedValue({ success: true, code: 200, message: '', data: [] })
    vi.mocked(taskApi.list).mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: {
        items: [task],
        page: 1,
        pageSize: 50,
        total: 1,
        loadedAt: '2026-07-16T00:00:00Z',
      },
    })
  })

  it('提供稳定页面标记和真实筛选控件标识', async () => {
    const wrapper = mount(TaskSchedulerView, {
      global: {
        plugins: [createPinia(), createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
      },
    })
    await flushPromises()

    expect(wrapper.attributes('data-page')).toBe('scheduled-tasks')
    expect(wrapper.find('[data-ui="toolbar"]').exists()).toBe(true)
    expect(wrapper.find('[data-ui="table"]').exists()).toBe(true)
    expect(wrapper.find('#scheduled-task-node-filter').exists()).toBe(true)
    expect(wrapper.find('#scheduled-task-search').attributes('name')).toBe('scheduledTaskSearch')
    wrapper.unmount()
  })

  it('能力为 false 时操作菜单不提供越权入口', async () => {
    const wrapper = mount(TaskSchedulerView, {
      global: {
        plugins: [createPinia(), createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
      },
    })
    await flushPromises()

    const menus = wrapper.findAllComponents(VustActionMenu)
    const rowMenu = menus.find((menu) =>
      menu.props('actions').some((action) => action.label === '查看详情'),
    )
    expect(rowMenu?.props('actions').map((action) => action.label)).toEqual(['查看详情'])
    wrapper.unmount()
  })
})
