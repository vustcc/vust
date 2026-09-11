import { createI18n } from 'vue-i18n'
import { createPinia } from 'pinia'
import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import FileEditorView from '../FileEditorView.vue'
import zh from '@/locales/zh'
import type { EditorTab } from '@/composables/useFileEditor'
import { VustTooltip } from '@/components/ui'

const editor = vi.hoisted(() => ({
  loadFileContent: vi.fn(),
  saveFileContent: vi.fn(),
  nodeId: '',
  updateWindowRuntimeState: vi.fn(),
}))

vi.mock('@/stores/node', () => ({ useNodeStore: () => ({ currentNodeId: 'global-node' }) }))
vi.mock('@/stores/window-manager', () => ({
  useWindowManagerStore: () => ({ updateWindowRuntimeState: editor.updateWindowRuntimeState }),
}))
vi.mock('@/composables/useFileEditor', () => ({
  useFileEditor: (nodeId: { value: string }) => {
    editor.nodeId = nodeId.value
    return {
      loadFileContent: editor.loadFileContent,
      saveFileContent: editor.saveFileContent,
    }
  },
}))

vi.mock('@/components/editor/MonacoEditor.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      name: 'MonacoEditor',
      inheritAttrs: false,
      props: {
        modelValue: { type: String, default: '' },
        documentKey: { type: String, default: '' },
        id: { type: String, default: '' },
        name: { type: String, default: '' },
        ariaLabel: { type: String, default: '' },
        wordWrap: { type: Boolean, default: false },
        minimap: { type: Boolean, default: true },
        stickyScroll: { type: Boolean, default: true },
        highlightAmbiguousUnicode: { type: Boolean, default: true },
        renderWhitespace: { type: String, default: 'selection' },
        fontSize: { type: Number, default: 14 },
      },
      emits: ['change', 'save'],
      setup(props, { expose }) {
        expose({ disposeDocument: vi.fn() })
        return () => h('div', { 'data-ui': 'monaco-stub', 'data-document-key': props.documentKey })
      },
    }),
  }
})

const writableCapabilities = {
  canOpen: true,
  canRead: true,
  canWrite: true,
  canCreateChild: false,
  canRename: true,
  canCopy: true,
  canRemove: true,
  canUpload: false,
  canDownload: true,
}

const mountView = () =>
  mount(FileEditorView, {
    attachTo: document.body,
    props: {
      windowId: 'window-a',
      payload: { nodeId: 'node-a', path: '/tmp/file.txt' },
    },
    global: {
      plugins: [createPinia(), createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
    },
  })

describe('FileEditorView', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    document.body.innerHTML = ''
    localStorage.clear()
    editor.loadFileContent.mockImplementation(async (tab: EditorTab) => {
      tab.content = 'disk content'
      tab.originalContent = 'disk content'
      tab.revision = 'revision-1'
      tab.fileSize = 12
      tab.loadedAt = '2026-07-16T00:00:00.000Z'
      tab.capabilities = writableCapabilities
      tab.loadState = 'ready'
      tab.saveState = 'idle'
      tab.isDirty = false
      return true
    })
    editor.saveFileContent.mockResolvedValue(true)
  })

  it('固定使用窗口节点并提供稳定的 tab 与编辑器语义', async () => {
    const wrapper = mountView()
    await flushPromises()

    expect(editor.nodeId).toBe('node-a')
    expect(wrapper.get('[data-page="file-editor"]').attributes('data-node-id')).toBe('node-a')
    expect(wrapper.get('[role="tablist"]').exists()).toBe(true)
    expect(wrapper.get('[role="tab"]').attributes('aria-selected')).toBe('true')
    const monaco = wrapper.getComponent({ name: 'MonacoEditor' })
    expect(monaco.props('documentKey')).toBe('node-a:/tmp/file.txt')
    expect(monaco.props('id')).toBe('file-editor-window-a-content')
    expect(monaco.props('name')).toBe('fileContent')
    const menuBar = wrapper.get('[data-ui="editor-menu-bar"]')
    expect(menuBar.findAll('[role="menuitem"]')).toHaveLength(2)
    expect(menuBar.text()).toBe('文件视图')
    const tabs = wrapper.get('[data-ui="editor-tabs"]')
    const editorArea = wrapper.get('[data-ui="editor-area"]')
    const statusBar = wrapper.get('[data-ui="editor-status-bar"]')
    expect(
      menuBar.element.compareDocumentPosition(tabs.element) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBe(Node.DOCUMENT_POSITION_FOLLOWING)
    expect(
      editorArea.element.compareDocumentPosition(statusBar.element) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBe(Node.DOCUMENT_POSITION_FOLLOWING)
    expect(statusBar.text()).toContain('/tmp/file.txt')
    expect(statusBar.text()).toContain('已保存')
    wrapper.unmount()
  })

  it('仅为非激活标签提供组件库路径提示', async () => {
    const wrapper = mountView()
    await flushPromises()

    await wrapper.setProps({
      payload: { nodeId: 'node-a', path: '/tmp/other.txt' },
    })
    await flushPromises()

    const tabNames = wrapper.findAll('.tab-name')
    expect(tabNames).toHaveLength(2)
    expect(tabNames.every((tabName) => tabName.attributes('title') === undefined)).toBe(true)

    const tooltips = wrapper.findAllComponents(VustTooltip)
    expect(tooltips).toHaveLength(2)
    expect(tooltips[0].props('text')).toBe('/tmp/file.txt')
    expect(tooltips[0].props('disabled')).toBe(false)
    expect(tooltips[1].props('text')).toBe('/tmp/other.txt')
    expect(tooltips[1].props('disabled')).toBe(true)
    wrapper.unmount()
  })

  it('通过文件菜单保存当前脏文件', async () => {
    const wrapper = mountView()
    await flushPromises()
    const monaco = wrapper.getComponent({ name: 'MonacoEditor' })
    monaco.vm.$emit('change', 'local content', 'node-a:/tmp/file.txt')
    await wrapper.vm.$nextTick()

    await wrapper.get('[data-ui="editor-file-menu-trigger"]').trigger('click')
    const saveItem = document.querySelector<HTMLButtonElement>('[data-ui="editor-save"]')
    expect(saveItem?.hasAttribute('disabled')).toBe(false)
    saveItem?.click()
    await flushPromises()

    expect(editor.saveFileContent).toHaveBeenCalledTimes(1)
    expect(editor.saveFileContent.mock.calls[0][0].path).toBe('/tmp/file.txt')
    wrapper.unmount()
  })

  it('脏内容重新加载前必须确认', async () => {
    const wrapper = mountView()
    await flushPromises()
    const monaco = wrapper.getComponent({ name: 'MonacoEditor' })
    monaco.vm.$emit('change', 'local content', 'node-a:/tmp/file.txt')
    await wrapper.vm.$nextTick()

    await wrapper.get('[data-ui="editor-file-menu-trigger"]').trigger('click')
    document.querySelector<HTMLButtonElement>('[data-ui="editor-reload"]')?.click()
    await wrapper.vm.$nextTick()
    expect(editor.loadFileContent).toHaveBeenCalledTimes(1)
    expect(document.body.textContent).toContain('重新加载将丢失当前未保存内容')

    const confirmButton = document.querySelector<HTMLButtonElement>(
      '.confirmation-actions button:last-child',
    )
    expect(confirmButton).not.toBeNull()
    confirmButton?.click()
    await flushPromises()
    expect(editor.loadFileContent).toHaveBeenCalledTimes(2)
    expect(editor.loadFileContent.mock.calls[1][1]).toEqual({ discardLocalChanges: true })
    wrapper.unmount()
  })

  it('冲突标签禁用保存但允许确认重载', async () => {
    const wrapper = mountView()
    await flushPromises()
    const monaco = wrapper.getComponent({ name: 'MonacoEditor' })
    monaco.vm.$emit('change', 'local content', 'node-a:/tmp/file.txt')
    const exposed = wrapper.vm as unknown as { activeTab: EditorTab }
    exposed.activeTab.saveState = 'conflict'
    await wrapper.vm.$nextTick()

    await wrapper.get('[data-ui="editor-file-menu-trigger"]').trigger('click')
    expect(
      document
        .querySelector<HTMLButtonElement>('[data-ui="editor-save"]')
        ?.hasAttribute('disabled'),
    ).toBe(true)
    expect(wrapper.text()).toContain('内容冲突')
    wrapper.unmount()
  })

  it('通过视图菜单同步并持久化全部高级配置', async () => {
    const wrapper = mountView()
    await flushPromises()
    const monaco = wrapper.getComponent({ name: 'MonacoEditor' })
    const openViewMenu = async () => {
      await wrapper.get('[data-ui="editor-view-menu-trigger"]').trigger('click')
      await wrapper.vm.$nextTick()
    }

    expect(monaco.props()).toMatchObject({
      wordWrap: false,
      minimap: true,
      stickyScroll: true,
      highlightAmbiguousUnicode: true,
      renderWhitespace: 'selection',
      fontSize: 14,
    })

    await openViewMenu()
    const wrapItem = document.querySelector<HTMLButtonElement>('[data-ui="editor-word-wrap"]')
    expect(wrapItem?.getAttribute('aria-checked')).toBe('false')
    wrapItem?.click()
    await wrapper.vm.$nextTick()
    expect(monaco.props('wordWrap')).toBe(true)

    await openViewMenu()
    document.querySelector<HTMLButtonElement>('[data-ui="editor-minimap"]')?.click()
    await wrapper.vm.$nextTick()
    expect(monaco.props('minimap')).toBe(false)

    await openViewMenu()
    document.querySelector<HTMLButtonElement>('[data-ui="editor-sticky-scroll"]')?.click()
    await wrapper.vm.$nextTick()
    expect(monaco.props('stickyScroll')).toBe(false)

    await openViewMenu()
    const unicodeHighlightItem = document.querySelector<HTMLButtonElement>(
      '[data-ui="editor-ambiguous-unicode-highlight"]',
    )
    expect(unicodeHighlightItem?.getAttribute('aria-checked')).toBe('true')
    unicodeHighlightItem?.click()
    await wrapper.vm.$nextTick()
    expect(monaco.props('highlightAmbiguousUnicode')).toBe(false)

    await openViewMenu()
    document.querySelector<HTMLButtonElement>('[data-ui="editor-whitespace-menu-trigger"]')?.click()
    await wrapper.vm.$nextTick()
    const whitespaceItems = document.querySelectorAll('[role="menuitemradio"]')
    expect(whitespaceItems).toHaveLength(3)
    expect(
      document
        .querySelector('[data-ui="editor-whitespace-selection"]')
        ?.getAttribute('aria-checked'),
    ).toBe('true')
    document.querySelector<HTMLButtonElement>('[data-ui="editor-whitespace-all"]')?.click()
    await wrapper.vm.$nextTick()
    expect(monaco.props('renderWhitespace')).toBe('all')

    await openViewMenu()
    const fontTrigger = document.querySelector<HTMLButtonElement>(
      '[data-ui="editor-font-size-menu-trigger"]',
    )
    expect(fontTrigger?.textContent).toContain('14px')
    fontTrigger?.click()
    await wrapper.vm.$nextTick()
    document.querySelector<HTMLButtonElement>('[data-ui="editor-font-increase"]')?.click()
    await wrapper.vm.$nextTick()
    expect(monaco.props('fontSize')).toBe(15)
    expect(JSON.parse(localStorage.getItem('vust_file_editor_preferences') ?? '{}')).toMatchObject({
      version: 1,
      wordWrap: true,
      fontSize: 15,
      highlightAmbiguousUnicode: false,
      minimap: false,
      stickyScroll: false,
      renderWhitespace: 'all',
    })

    await openViewMenu()
    document.querySelector<HTMLButtonElement>('[data-ui="editor-font-size-menu-trigger"]')?.click()
    await wrapper.vm.$nextTick()
    document.querySelector<HTMLButtonElement>('[data-ui="editor-font-reset"]')?.click()
    await wrapper.vm.$nextTick()
    expect(monaco.props('fontSize')).toBe(14)

    const exposed = wrapper.vm as unknown as { fontSize: number }
    exposed.fontSize = 24
    await wrapper.vm.$nextTick()
    await openViewMenu()
    document.querySelector<HTMLButtonElement>('[data-ui="editor-font-size-menu-trigger"]')?.click()
    await wrapper.vm.$nextTick()
    expect(
      document
        .querySelector<HTMLButtonElement>('[data-ui="editor-font-increase"]')
        ?.hasAttribute('disabled'),
    ).toBe(true)

    await wrapper.get('[data-ui="editor-view-menu-trigger"]').trigger('click')
    exposed.fontSize = 10
    await wrapper.vm.$nextTick()
    await openViewMenu()
    document.querySelector<HTMLButtonElement>('[data-ui="editor-font-size-menu-trigger"]')?.click()
    await wrapper.vm.$nextTick()
    expect(
      document
        .querySelector<HTMLButtonElement>('[data-ui="editor-font-decrease"]')
        ?.hasAttribute('disabled'),
    ).toBe(true)

    await wrapper.get('[data-ui="editor-view-menu-trigger"]').trigger('click')
    await openViewMenu()
    document.querySelector<HTMLButtonElement>('[data-ui="editor-reset-view-preferences"]')?.click()
    await wrapper.vm.$nextTick()
    expect(monaco.props()).toMatchObject({
      wordWrap: false,
      minimap: true,
      stickyScroll: true,
      highlightAmbiguousUnicode: true,
      renderWhitespace: 'selection',
      fontSize: 14,
    })
    wrapper.unmount()
  })

  it('支持菜单方向键、二级菜单和 Esc 返回焦点', async () => {
    const wrapper = mountView()
    await flushPromises()
    const viewTrigger = wrapper.get<HTMLButtonElement>('[data-ui="editor-view-menu-trigger"]')

    await viewTrigger.trigger('keydown', { key: 'ArrowDown' })
    await wrapper.vm.$nextTick()
    expect(document.activeElement?.getAttribute('data-ui')).toBe('editor-word-wrap')

    document.activeElement?.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }),
    )
    expect(document.activeElement?.getAttribute('data-ui')).toBe('editor-minimap')

    document.activeElement?.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }),
    )
    expect(document.activeElement?.getAttribute('data-ui')).toBe('editor-sticky-scroll')

    document.activeElement?.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }),
    )
    expect(document.activeElement?.getAttribute('data-ui')).toBe(
      'editor-ambiguous-unicode-highlight',
    )

    document.activeElement?.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }),
    )
    expect(document.activeElement?.getAttribute('data-ui')).toBe('editor-whitespace-menu-trigger')

    document.activeElement?.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }),
    )
    await wrapper.vm.$nextTick()
    expect(document.activeElement?.getAttribute('data-ui')).toBe('editor-whitespace-none')

    document.activeElement?.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true }),
    )
    await wrapper.vm.$nextTick()
    expect(document.activeElement?.getAttribute('data-ui')).toBe('editor-whitespace-menu-trigger')

    document.activeElement?.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }),
    )
    expect(document.activeElement?.getAttribute('data-ui')).toBe('editor-font-size-menu-trigger')

    document.activeElement?.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }),
    )
    await wrapper.vm.$nextTick()
    expect(document.activeElement?.getAttribute('data-ui')).toBe('editor-font-increase')

    document.activeElement?.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }),
    )
    await wrapper.vm.$nextTick()
    expect(document.querySelector('[data-ui="editor-view-menu"]')).toBeNull()
    expect(document.activeElement).toBe(viewTrigger.element)
    wrapper.unmount()
  })

  it('点击菜单外部后关闭浮层', async () => {
    const wrapper = mountView()
    await flushPromises()

    await wrapper.get('[data-ui="editor-file-menu-trigger"]').trigger('click')
    expect(document.querySelector('[data-ui="editor-file-menu"]')).not.toBeNull()
    document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }))
    await wrapper.vm.$nextTick()
    expect(document.querySelector('[data-ui="editor-file-menu"]')).toBeNull()
    wrapper.unmount()
  })
})
