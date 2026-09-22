import { createPinia } from 'pinia'
import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ScriptDetail, ScriptRun, ScriptSummary } from '@/api/generated/scripts'
import { nodesApi } from '@/api/modules/nodes'
import { scriptsApi } from '@/api/modules/scripts'
import ScriptManagerView from '@/apps/views/ScriptManagerView.vue'
import ApplicationDialog from '@/components/layout/ApplicationDialog.vue'
import { VustActionMenu, VustButton, VustCheckbox, VustTable, VustTooltip } from '@/components/ui'
import en from '@/locales/en'
import zh from '@/locales/zh'

vi.mock('@/components/editor/MonacoEditor.vue', () => ({
  default: {
    name: 'MonacoEditor',
    props: {
      id: String,
      name: String,
      modelValue: String,
      readOnly: Boolean,
      documentKey: String,
      wheelFocusOnClick: Boolean,
      fixedOverflowWidgets: Boolean,
    },
    template: '<textarea :id="id" :name="name" :value="modelValue" />',
  },
}))
vi.mock('@/apps/views/scripts/ScriptRunTerminal.vue', () => ({
  default: {
    name: 'ScriptRunTerminal',
    props: { runId: String },
    emits: ['control', 'disconnected'],
    methods: { close() {} },
    template: '<div data-ui="script-run-terminal" />',
  },
}))
vi.mock('@/api/modules/nodes', () => ({ nodesApi: { list: vi.fn() } }))
vi.mock('@/stores/window-manager', () => ({
  useWindowManagerStore: () => ({ updateWindowRuntimeState: vi.fn() }),
}))
vi.mock('@/api/modules/scripts', () => ({
  scriptsApi: {
    list: vi.fn(),
    detail: vi.fn(),
    create: vi.fn(),
    update: vi.fn(),
    remove: vi.fn(),
    startRun: vi.fn(),
    dismissRun: vi.fn(),
  },
}))

const response = <T>(data: T) => ({ success: true, code: 200, message: '', data })
const deferred = <T>() => {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((complete) => {
    resolve = complete
  })
  return { promise, resolve }
}
const script = (interactive = false): ScriptSummary => ({
  scriptId: interactive ? 'script-interactive' : 'script-standard',
  name: interactive ? 'Interactive' : 'Standard',
  description: 'A long script description',
  interactive,
  language: 'shell',
  revision: 1,
  ownership: { kind: 'custom' },
  capabilities: {
    canUpdate: true,
    canRemove: true,
    canClone: true,
    canRun: true,
  },
  createdAt: '2026-07-16T00:00:00Z',
  updatedAt: '2026-07-16T00:00:00Z',
  updatedBy: 'admin',
})
const detail = (value: ScriptSummary): ScriptDetail => ({
  ...value,
  source: { content: 'echo ok', sizeBytes: 7, sha256: 'a'.repeat(64) },
  executionDefaults: { timeoutSeconds: 300 },
})
const queuedRun = (): ScriptRun => ({
  runId: 'run-1',
  scriptId: 'script-standard',
  scriptName: 'Standard',
  scriptRevision: 1,
  sourceSha256: 'a'.repeat(64),
  nodeId: 'local',
  nodeName: 'Local',
  status: 'queued',
  queuedAt: '2026-07-16T00:00:00Z',
  capabilities: { canCancel: true },
})

const mountView = () => {
  const windowElement = document.createElement('section')
  windowElement.className = 'application-window'
  windowElement.innerHTML = `
    <header class="window-header">Script Library</header>
    <main class="window-content"></main>
  `
  document.body.append(windowElement)

  return mount(ScriptManagerView, {
    attachTo: windowElement.querySelector<HTMLElement>('.window-content')!,
    global: {
      plugins: [createPinia(), createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
    },
  })
}

const clickDialogButton = async (dialogUi: string, label: string) => {
  const button = Array.from(
    document.querySelectorAll<HTMLButtonElement>(`[data-ui="${dialogUi}"] button`),
  ).find((item) => item.textContent?.trim() === label)
  expect(button).toBeDefined()
  button!.click()
  await flushPromises()
}

const setDocumentInputValue = async (selector: string, value: string) => {
  const input = document.querySelector<HTMLInputElement>(selector)
  expect(input).not.toBeNull()
  input!.value = value
  input!.dispatchEvent(new Event('input', { bubbles: true }))
  await flushPromises()
}

describe('ScriptManagerView', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    localStorage.clear()
    vi.mocked(nodesApi.list).mockResolvedValue(
      response([{ nodeId: 'local', name: 'Local', status: 'online' } as never]),
    )
    vi.mocked(scriptsApi.list).mockResolvedValue(
      response({
        items: [script(), script(true)],
        page: 1,
        pageSize: 50,
        total: 2,
        loadedAt: '2026-07-16T00:00:00Z',
      }),
    )
    vi.mocked(scriptsApi.detail).mockImplementation((scriptId) => {
      const value = scriptId === 'script-interactive' ? script(true) : script()
      return Promise.resolve(response(detail(value)))
    })
    vi.mocked(scriptsApi.startRun).mockResolvedValue(response(queuedRun()))
    vi.mocked(scriptsApi.dismissRun).mockResolvedValue(response(undefined))
  })

  afterEach(() => {
    document.body.innerHTML = ''
    vi.useRealTimers()
  })

  it('使用五列表格和稳定 DOM 标记，不提供刷新与执行记录入口', async () => {
    const wrapper = mountView()
    await flushPromises()
    expect(wrapper.attributes('data-page')).toBe('script-library')
    expect(wrapper.find('[data-ui="toolbar"]').exists()).toBe(true)
    expect(wrapper.find('[data-ui="table"]').exists()).toBe(true)
    expect(wrapper.find('#script-library-search').attributes('name')).toBe('scriptLibrarySearch')
    const labels = wrapper
      .findComponent(VustTable)
      .props('columns')
      .map((column: { label: string }) => column.label)
    expect(labels).toEqual(['名称', '交互式', '描述', '时间', '操作'])
    const toolbar = wrapper.find('[data-ui="toolbar"]')
    expect(toolbar.text()).not.toContain('刷新')
    expect(toolbar.findAllComponents(VustButton)).toHaveLength(1)
    expect(toolbar.findComponent(VustButton).text()).toBe('新建')
    expect(toolbar.find('.vl-icon').exists()).toBe(false)
    expect(wrapper.text()).not.toContain('执行记录')
    expect(localStorage.length).toBe(0)
    wrapper.unmount()
  })

  it('名称点击后只读展示脚本正文', async () => {
    const wrapper = mountView()
    await flushPromises()
    await wrapper.findAll('.name-button')[0]!.trigger('click')
    await flushPromises()
    const detailDialog = wrapper
      .findAllComponents(ApplicationDialog)
      .find((item) => item.props('title') === 'Standard')
    expect(detailDialog?.props('visible')).toBe(true)
    const detailElement = document.querySelector('[data-ui="script-detail-dialog"]')!
    const detailOverlay = detailElement.closest('[data-ui="application-dialog-overlay"]')!
    expect(detailOverlay.closest('.window-content')).toBe(document.querySelector('.window-content'))
    expect(document.querySelector('.window-header')?.contains(detailOverlay)).toBe(false)
    expect(Array.from(document.body.children)).not.toContain(detailOverlay)
    const editor = wrapper
      .findAllComponents({ name: 'MonacoEditor' })
      .find((item) => item.props('id') === 'script-library-readonly-source')
    expect(editor?.props('modelValue')).toBe('echo ok')
    expect(editor?.props('readOnly')).toBe(true)
    expect(editor?.props('wheelFocusOnClick')).toBe(true)
    expect(editor?.props('fixedOverflowWidgets')).toBe(false)
    wrapper.unmount()
  })

  it('搜索框有内容时可以一键清空', async () => {
    const wrapper = mountView()
    await flushPromises()
    const searchInput = wrapper.find('#script-library-search')
    expect(wrapper.find('.search-clear').exists()).toBe(false)
    await searchInput.setValue('scan')
    expect(wrapper.find('.search-clear').attributes('aria-label')).toBe('清空脚本搜索')
    await wrapper.find('.search-clear').trigger('click')
    expect((searchInput.element as HTMLInputElement).value).toBe('')
    expect(wrapper.find('.search-clear').exists()).toBe(false)
    wrapper.unmount()
  })

  it('每行固定四项操作，交互式执行保持可用', async () => {
    const wrapper = mountView()
    await flushPromises()
    const menus = wrapper.findAllComponents(VustActionMenu)
    expect(menus).toHaveLength(2)
    expect(menus[0]!.props('actions').map((action: { label: string }) => action.label)).toEqual([
      '执行',
      '克隆',
      '编辑',
      '删除',
    ])
    const interactiveRun = menus[1]!.props('actions')[0]
    expect(interactiveRun.disabled).toBe(false)
    expect(interactiveRun.tooltip).toBeUndefined()
    wrapper.unmount()
  })

  it('新建表单展示规定的交互式提示', async () => {
    const wrapper = mountView()
    await flushPromises()
    const createButton = wrapper.findAll('button').find((item) => item.text().includes('新建'))
    await createButton!.trigger('click')
    const tooltip = wrapper
      .findAllComponents(VustTooltip)
      .find((item) => item.props('text').includes('用于标记在执行过程中需要用户输入参数'))
    expect(tooltip?.props('text')).toBe('用于标记在执行过程中需要用户输入参数或做出选择的脚本。')
    expect(
      wrapper
        .findAllComponents(VustCheckbox)
        .some((item) => item.props('id') === 'script-library-interactive'),
    ).toBe(true)
    const formElement = document.querySelector('[data-ui="script-form-dialog"] [data-ui="form"]')!
    const formOverlay = formElement.closest('[data-ui="application-dialog-overlay"]')!
    expect(formOverlay.closest('.window-content')).toBe(document.querySelector('.window-content'))
    expect(document.activeElement?.getAttribute('name')).toBe('scriptName')
    expect(
      Array.from(formElement.querySelectorAll(':scope > [data-slot]')).map((item) =>
        item.getAttribute('data-slot'),
      ),
    ).toEqual(['form-name', 'form-options', 'form-source', 'form-description'])
    expect(
      document.querySelector('[data-slot="form-options"]')?.classList.contains('form-options-row'),
    ).toBe(true)
    const optionsRow = document.querySelector('[data-slot="form-options"]')!
    expect(optionsRow.querySelector('.interactive-inline-field')?.textContent).toContain(
      '交互式脚本',
    )
    expect(optionsRow.querySelector('.timeout-inline-field label')?.textContent?.trim()).toBe(
      '超时（秒）',
    )
    expect(optionsRow.querySelector('#script-library-timeout')).not.toBeNull()
    const formEditor = wrapper
      .findAllComponents({ name: 'MonacoEditor' })
      .find((item) => item.props('id') === 'script-library-source')
    expect(formEditor?.props('wheelFocusOnClick')).toBe(true)
    wrapper.unmount()
  })

  it('新建、克隆和编辑复用窗口级表单并恢复正确数据', async () => {
    const wrapper = mountView()
    await flushPromises()

    await wrapper.get('[data-ui="script-create"]').trigger('click')
    let formDialog = wrapper
      .findAllComponents(ApplicationDialog)
      .find((item) => item.props('title') === '新建脚本')!
    expect(formDialog.props('visible')).toBe(true)
    expect(document.querySelector<HTMLInputElement>('#script-library-name')?.value).toBe('')
    formDialog.vm.$emit('close')
    await flushPromises()

    await wrapper.findAllComponents(VustActionMenu)[0]!.props('actions')[1].handler()
    await flushPromises()
    formDialog = wrapper
      .findAllComponents(ApplicationDialog)
      .find((item) => item.props('title') === '克隆脚本')!
    expect(formDialog.props('visible')).toBe(true)
    expect(document.querySelector<HTMLInputElement>('#script-library-name')?.value).toBe(
      'Standard 副本',
    )
    formDialog.vm.$emit('close')
    await flushPromises()

    await wrapper.findAllComponents(VustActionMenu)[0]!.props('actions')[2].handler()
    await flushPromises()
    formDialog = wrapper
      .findAllComponents(ApplicationDialog)
      .find((item) => item.props('title') === '编辑脚本')!
    expect(formDialog.props('visible')).toBe(true)
    expect(document.querySelector<HTMLInputElement>('#script-library-name')?.value).toBe('Standard')
    wrapper.unmount()
  })

  it('脏表单取消放弃时保留内容，确认放弃后返回新建入口', async () => {
    const wrapper = mountView()
    await flushPromises()
    await wrapper.get('[data-ui="script-create"]').trigger('click')
    await setDocumentInputValue('#script-library-name', 'Draft')

    const formDialog = wrapper
      .findAllComponents(ApplicationDialog)
      .find((item) => item.props('title') === '新建脚本')!
    formDialog.vm.$emit('close')
    await flushPromises()
    expect(document.querySelector('[data-ui="script-confirmation-dialog"]')).not.toBeNull()

    await clickDialogButton('script-confirmation-dialog', '取消')
    expect(formDialog.props('visible')).toBe(true)
    expect(document.querySelector<HTMLInputElement>('#script-library-name')?.value).toBe('Draft')
    expect(document.activeElement?.getAttribute('data-ui')).toBe('script-form-dialog')

    formDialog.vm.$emit('close')
    await flushPromises()
    await clickDialogButton('script-confirmation-dialog', '放弃修改')
    expect(formDialog.props('visible')).toBe(false)
    expect(document.activeElement?.getAttribute('data-ui')).toBe('script-create')
    wrapper.unmount()
  })

  it('保存失败保留表单，保存期间禁止关闭，成功后恢复列表', async () => {
    vi.mocked(scriptsApi.create).mockRejectedValueOnce(new Error('save failed'))
    const wrapper = mountView()
    await flushPromises()
    await wrapper.get('[data-ui="script-create"]').trigger('click')
    await setDocumentInputValue('#script-library-name', 'Deploy')

    const formDialog = wrapper
      .findAllComponents(ApplicationDialog)
      .find((item) => item.props('title') === '新建脚本')!
    const saveButton = formDialog
      .findAllComponents(VustButton)
      .find((item) => item.text() === '保存')!
    await saveButton.trigger('click')
    await flushPromises()
    expect(formDialog.props('visible')).toBe(true)
    expect(document.querySelector<HTMLInputElement>('#script-library-name')?.value).toBe('Deploy')

    const saveRequest = deferred<ReturnType<typeof response<ScriptDetail>>>()
    vi.mocked(scriptsApi.create).mockReturnValueOnce(saveRequest.promise)
    await saveButton.trigger('click')
    expect(formDialog.props('closeDisabled')).toBe(true)
    expect(
      document.querySelector<HTMLButtonElement>(
        '[data-ui="script-form-dialog"] .application-dialog__close',
      )?.disabled,
    ).toBe(true)

    saveRequest.resolve(response(detail(script())))
    await flushPromises()
    expect(formDialog.props('visible')).toBe(false)
    expect(scriptsApi.list).toHaveBeenCalledTimes(2)
    wrapper.unmount()
  })

  it('删除脚本只调用一次删除接口', async () => {
    const wrapper = mountView()
    await flushPromises()
    await wrapper.findAllComponents(VustActionMenu)[0]!.props('actions')[3].handler()
    await flushPromises()
    const confirmation = document.querySelector('[data-ui="script-confirmation-dialog"]')!
    expect(confirmation.textContent).toContain('Standard')
    expect(confirmation.closest('.window-content')).toBe(document.querySelector('.window-content'))
    await clickDialogButton('script-confirmation-dialog', '确认删除')
    expect(scriptsApi.remove).toHaveBeenCalledTimes(1)
    expect(scriptsApi.remove).toHaveBeenCalledWith('script-standard')
    wrapper.unmount()
  })

  it('普通脚本执行后立即展示统一终端', async () => {
    const wrapper = mountView()
    await flushPromises()
    await wrapper.findAllComponents(VustActionMenu)[0]!.props('actions')[0].handler()
    await flushPromises()
    const runDialog = wrapper
      .findAllComponents(ApplicationDialog)
      .find((item) => item.props('title') === '执行脚本 Standard')!
    expect(document.querySelector('[data-ui="run-dialog"]')?.closest('.window-content')).toBe(
      document.querySelector('.window-content'),
    )
    expect(document.querySelector('[data-ui="run-dialog"]')?.textContent).not.toContain(
      '脚本将以当前已保存版本',
    )
    const runButton = runDialog.findAllComponents(VustButton).find((item) => item.text() === '执行')
    await runButton!.trigger('click')
    await flushPromises()
    const activeRunDialog = document.querySelector('[data-ui="run-dialog"]')!
    expect(activeRunDialog.querySelector('[data-ui="script-run-terminal"]')).not.toBeNull()
    expect(activeRunDialog.querySelector('.output-shell')).toBeNull()
    wrapper.unmount()
  })

  it('提交执行期间禁止关闭执行 Dialog', async () => {
    const runRequest = deferred<ReturnType<typeof response<ScriptRun>>>()
    vi.mocked(scriptsApi.startRun).mockReturnValueOnce(runRequest.promise)
    const wrapper = mountView()
    await flushPromises()
    await wrapper.findAllComponents(VustActionMenu)[0]!.props('actions')[0].handler()
    await flushPromises()

    const runDialog = wrapper
      .findAllComponents(ApplicationDialog)
      .find((item) => item.props('title') === '执行脚本 Standard')!
    await runDialog
      .findAllComponents(VustButton)
      .find((item) => item.text() === '执行')!
      .trigger('click')
    expect(runDialog.props('closeDisabled')).toBe(true)
    runDialog.vm.$emit('close')
    await flushPromises()
    expect(runDialog.props('visible')).toBe(true)

    runRequest.resolve(response(queuedRun()))
    await flushPromises()
    expect(runDialog.props('closeDisabled')).toBe(false)
    wrapper.unmount()
  })

  it('交互式标记不改变手动执行终端', async () => {
    const wrapper = mountView()
    await flushPromises()
    await wrapper.findAllComponents(VustActionMenu)[1]!.props('actions')[0].handler()
    await flushPromises()
    const runDialog = wrapper
      .findAllComponents(ApplicationDialog)
      .find((item) => item.props('title') === '执行脚本 Interactive')!
    await runDialog
      .findAllComponents(VustButton)
      .find((item) => item.text() === '执行')!
      .trigger('click')
    await flushPromises()
    expect(document.querySelector('[data-ui="script-run-terminal"]')).not.toBeNull()
    wrapper.unmount()
  })

  it('活动执行关闭时确认、请求销毁并清空 Dialog', async () => {
    const wrapper = mountView()
    await flushPromises()
    const menu = wrapper.findAllComponents(VustActionMenu)[0]!
    await menu.props('actions')[0].handler()
    await flushPromises()
    const runDialog = wrapper
      .findAllComponents(ApplicationDialog)
      .find((item) => item.props('title') === '执行脚本 Standard')!
    expect(runDialog.props('visible')).toBe(true)
    const runButton = runDialog.findAllComponents(VustButton).find((item) => item.text() === '执行')
    await runButton!.trigger('click')
    await flushPromises()
    runDialog.vm.$emit('close')
    await flushPromises()
    expect(document.querySelector('[data-ui="script-confirmation-dialog"]')).not.toBeNull()
    const dismissRequest = deferred<ReturnType<typeof response<undefined>>>()
    vi.mocked(scriptsApi.dismissRun).mockReturnValueOnce(dismissRequest.promise)
    await clickDialogButton('script-confirmation-dialog', '取消并关闭')
    expect(scriptsApi.dismissRun).toHaveBeenCalledWith('run-1')
    expect(runDialog.props('closeDisabled')).toBe(true)
    expect(runDialog.props('visible')).toBe(true)

    dismissRequest.resolve(response(undefined))
    await flushPromises()
    expect(runDialog.props('visible')).toBe(false)
    wrapper.unmount()
  })
})
