import { nextTick, ref } from 'vue'
import { createPinia } from 'pinia'
import { createI18n } from 'vue-i18n'
import { mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  SuiteCatalogItem,
  SuiteInstallTaskResponse,
  SuiteInstanceSummary,
} from '@/api/interface/suites'
import en from '@/locales/en'
import zh from '@/locales/zh'
import SuiteCenterView from '@/apps/views/SuiteCenterView.vue'

const state = vi.hoisted(() => ({
  value: {} as Record<string, unknown>,
}))

vi.mock('@/apps/views/suite-center/useSuiteCenter', () => ({
  useSuiteCenter: () => state.value,
  validateSuitePackage: vi.fn(() => ({ valid: true })),
}))

const suite = (suiteId: string, category = 'other'): SuiteCatalogItem => ({
  suiteId,
  version: '1.0.0',
  name: suiteId === 'scanner' ? '主机扫描器' : '协议实验室',
  summary: suiteId === 'scanner' ? '扫描节点漏洞' : '学习网络协议',
  icon: 'package',
  status: 'available',
  checksum: 'checksum',
  createdAt: '',
  updatedAt: '',
  instanceCount: suiteId === 'protocol' ? 1 : 0,
  category,
})

const installed: SuiteInstanceSummary = {
  instanceId: 'instance-protocol',
  suiteId: 'protocol',
  version: '1.0.0',
  nodeId: 'local',
  composeProjectName: 'protocol',
  status: 'enabled',
  createdAt: '',
  updatedAt: '',
}

const installingTask: SuiteInstallTaskResponse = {
  taskId: 'task-scanner',
  instanceId: 'instance-scanner',
  suiteId: 'scanner',
  nodeId: 'local',
  progressPercent: 48,
  status: 'running',
  currentStep: 'pull_registry_image',
  isFinished: false,
  cancelRequested: false,
  createdAt: '',
  updatedAt: '',
}

function setupState() {
  state.value = {
    activeOperations: ref({}),
    cancelInstall: vi.fn(),
    catalog: ref([suite('scanner', 'tools'), suite('protocol')]),
    currentNodeId: ref('local'),
    currentNodeUnavailable: ref(false),
    deleteSuite: vi.fn(),
    importSuite: vi.fn(),
    installSuite: vi.fn(),
    instances: ref([installed]),
    isOperating: vi.fn(() => false),
    loadError: ref(''),
    phase: ref('ready'),
    pollingErrors: ref({}),
    refreshSuites: vi.fn(),
    refreshing: ref(false),
    retryInstallPolling: vi.fn(),
    runInstanceAction: vi.fn(),
    tasksById: ref({}),
  }
}

function mountView() {
  const pinia = createPinia()
  const i18n = createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })
  return mount(SuiteCenterView, { global: { plugins: [pinia, i18n] } })
}

describe('SuiteCenterView', () => {
  beforeEach(() => {
    localStorage.clear()
    setupState()
  })

  it('分类按钮提供选中语义并筛选重复卡片', async () => {
    const wrapper = mountView()
    const toolsButton = wrapper
      .findAll('[data-ui="suite-category-list"] button')
      .find((button) => button.text().includes('工具'))
    expect(toolsButton?.attributes('aria-pressed')).toBe('false')

    await toolsButton?.trigger('click')
    expect(toolsButton?.attributes('aria-pressed')).toBe('true')
    expect(wrapper.find('[data-slot="scanner"]').exists()).toBe(true)
    expect(wrapper.find('[data-slot="protocol"]').exists()).toBe(false)
  })

  it('名称、套件 ID 和摘要均可用于搜索', async () => {
    const wrapper = mountView()
    const search = wrapper.find('[data-slot="suite-search"] input')
    await search.setValue('漏洞')
    expect(wrapper.find('[data-slot="scanner"]').exists()).toBe(true)
    expect(wrapper.find('[data-slot="protocol"]').exists()).toBe(false)

    await search.setValue('protocol')
    expect(wrapper.find('[data-slot="protocol"]').exists()).toBe(true)
  })

  it('明确区分加载失败和空目录状态', async () => {
    const wrapper = mountView()
    const phase = state.value.phase as ReturnType<typeof ref<string>>
    const loadError = state.value.loadError as ReturnType<typeof ref<string>>

    loadError.value = '服务暂不可用'
    phase.value = 'error'
    await nextTick()
    expect(wrapper.find('[data-ui="suite-load-error"]').text()).toContain('服务暂不可用')

    phase.value = 'empty'
    await nextTick()
    const emptyState = wrapper.find('[data-ui="suite-empty-catalog"]')
    expect(emptyState.classes()).toContain('suite-state')
    expect(emptyState.find('.vl-empty-description').exists()).toBe(true)
  })

  it('刷新时由整个结果区域显示加载状态', async () => {
    const wrapper = mountView()
    const refreshing = state.value.refreshing as ReturnType<typeof ref<boolean>>

    refreshing.value = true
    await nextTick()

    const results = wrapper.find('[data-slot="suite-results"]')
    expect(results.attributes('aria-busy')).toBe('true')
    expect(results.find('[data-ui="suite-results-loading"]').classes()).toContain(
      'suite-state--loading',
    )
    expect(results.find('.suite-grid').exists()).toBe(false)
    expect(wrapper.find('[data-ui="suite-refresh"]').attributes('disabled')).toBeDefined()
  })

  it('安装任务仅在详情抽屉展示进度', async () => {
    state.value.tasksById = ref({ [installingTask.taskId]: installingTask })
    const wrapper = mountView()

    expect(wrapper.find('[data-slot="scanner"] [data-ui="suite-install-task"]').exists()).toBe(
      false,
    )

    await wrapper.find('[data-slot="scanner"] .suite-card__main').trigger('click')
    expect(document.body.querySelector('[data-ui="suite-install-task"]')).not.toBeNull()
  })
})
