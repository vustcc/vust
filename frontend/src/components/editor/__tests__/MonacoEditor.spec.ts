import { mount } from '@vue/test-utils'
import { createI18n } from 'vue-i18n'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import MonacoEditor from '../MonacoEditor.vue'
import zh from '@/locales/zh'

const monacoMock = vi.hoisted(() => ({
  models: [] as Array<{
    value: string
    language: string
    uri: unknown
    disposed: boolean
    getValue: () => string
    setValue: (value: string) => void
    dispose: () => void
  }>,
  createModel: vi.fn(),
  setModel: vi.fn(),
  saveViewState: vi.fn(() => ({ cursorState: [], viewState: {} })),
  restoreViewState: vi.fn(),
  setModelLanguage: vi.fn(),
  updateOptions: vi.fn(),
  setPosition: vi.fn(),
  mouseDownHandler: null as ((event: { target: { position: unknown } }) => void) | null,
  createOptions: null as Record<string, unknown> | null,
  editorRoot: null as HTMLDivElement | null,
}))

vi.mock('../vdl-monaco-theme', () => ({
  VDL_THEME_DARK: 'vdl-dark',
  VDL_THEME_LIGHT: 'vdl-light',
  createSdlDarkTheme: () => ({}),
  createSdlLightTheme: () => ({}),
}))

vi.mock('../monaco-workers', () => {
  let activeModel: ReturnType<typeof monacoMock.createModel> | null = null
  const root = document.createElement('div')
  monacoMock.editorRoot = root
  const textarea = document.createElement('textarea')
  textarea.className = 'inputarea'
  root.appendChild(textarea)

  monacoMock.createModel.mockImplementation((value: string, language: string, uri: unknown) => {
    const model = {
      value,
      language,
      uri,
      disposed: false,
      getValue: () => model.value,
      setValue: (nextValue: string) => {
        model.value = nextValue
      },
      dispose: () => {
        model.disposed = true
      },
    }
    monacoMock.models.push(model)
    return model
  })

  const editor = {
    createModel: monacoMock.createModel,
    create: vi.fn((_container: HTMLElement, options: { model: typeof activeModel }) => {
      activeModel = options.model
      monacoMock.createOptions = options
      return {
        addCommand: vi.fn(),
        onDidChangeModelContent: vi.fn(),
        onMouseDown: vi.fn((handler) => {
          monacoMock.mouseDownHandler = handler
        }),
        getValue: () => activeModel?.getValue() ?? '',
        setValue: (value: string) => activeModel?.setValue(value),
        getModel: () => activeModel,
        setModel: (model: typeof activeModel) => {
          activeModel = model
          monacoMock.setModel(model)
        },
        saveViewState: monacoMock.saveViewState,
        restoreViewState: monacoMock.restoreViewState,
        setPosition: monacoMock.setPosition,
        getDomNode: () => root,
        updateOptions: monacoMock.updateOptions,
        layout: vi.fn(),
        focus: vi.fn(),
        dispose: vi.fn(),
      }
    }),
    setModelLanguage: (model: (typeof monacoMock.models)[number], language: string) => {
      model.language = language
      monacoMock.setModelLanguage(model, language)
    },
    defineTheme: vi.fn(),
    setTheme: vi.fn(),
  }

  return {
    setupMonacoWorkers: vi.fn(),
    monaco: {
      editor,
      Uri: { from: (value: unknown) => value },
      KeyMod: { CtrlCmd: 1 },
      KeyCode: { KeyS: 2 },
    },
  }
})

describe('MonacoEditor 多文档模型', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    monacoMock.models.length = 0
    monacoMock.createOptions = null
    monacoMock.mouseDownHandler = null
  })

  it('初始化并动态同步高级视图配置', async () => {
    const wrapper = mount(MonacoEditor, {
      props: {
        modelValue: '中文，content',
        documentKey: 'node-a:/unicode.txt',
      },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })

    expect(monacoMock.createOptions?.unicodeHighlight).toEqual({
      ambiguousCharacters: true,
      invisibleCharacters: true,
    })
    expect(monacoMock.createOptions?.minimap).toEqual({ enabled: true })
    expect(monacoMock.createOptions?.stickyScroll).toEqual({ enabled: true })
    expect(monacoMock.createOptions?.renderWhitespace).toBe('selection')
    expect(monacoMock.createOptions?.fixedOverflowWidgets).toBe(true)

    await wrapper.setProps({
      highlightAmbiguousUnicode: false,
      minimap: false,
      stickyScroll: false,
      renderWhitespace: 'all',
    })
    expect(monacoMock.updateOptions).toHaveBeenCalledWith({
      unicodeHighlight: {
        ambiguousCharacters: false,
        invisibleCharacters: true,
      },
    })
    expect(monacoMock.updateOptions).toHaveBeenCalledWith({ minimap: { enabled: false } })
    expect(monacoMock.updateOptions).toHaveBeenCalledWith({ stickyScroll: { enabled: false } })
    expect(monacoMock.updateOptions).toHaveBeenCalledWith({ renderWhitespace: 'all' })
    wrapper.unmount()
  })

  it('按 documentKey 隔离模型并恢复视图状态', async () => {
    const wrapper = mount(MonacoEditor, {
      props: {
        modelValue: 'content-a',
        documentKey: 'node-a:/a.txt',
        filePath: '/a.txt',
      },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })
    const firstModel = monacoMock.models[0]

    await wrapper.setProps({
      modelValue: 'content-b',
      documentKey: 'node-a:/b.txt',
      filePath: '/b.txt',
    })
    const secondModel = monacoMock.models[1]
    expect(monacoMock.models).toHaveLength(2)
    expect(monacoMock.setModel).toHaveBeenLastCalledWith(secondModel)

    await wrapper.setProps({
      modelValue: 'content-a',
      documentKey: 'node-a:/a.txt',
      filePath: '/a.txt',
    })
    expect(monacoMock.models).toHaveLength(2)
    expect(monacoMock.setModel).toHaveBeenLastCalledWith(firstModel)
    expect(monacoMock.restoreViewState).toHaveBeenCalled()
    wrapper.unmount()
  })

  it('点击后接管滚轮，并将只读光标同步到点击位置', () => {
    const wrapper = mount(MonacoEditor, {
      props: {
        modelValue: 'readonly content',
        documentKey: 'readonly:/script.sh',
        readOnly: true,
        wheelFocusOnClick: true,
        fixedOverflowWidgets: false,
      },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })

    expect(monacoMock.createOptions?.scrollbar).toMatchObject({
      handleMouseWheel: false,
      alwaysConsumeMouseWheel: false,
    })
    expect(monacoMock.createOptions?.fixedOverflowWidgets).toBe(false)

    const position = { lineNumber: 2, column: 4 }
    monacoMock.mouseDownHandler?.({ target: { position } })
    expect(monacoMock.setPosition).toHaveBeenCalledWith(position, 'mouse')
    expect(monacoMock.updateOptions).toHaveBeenLastCalledWith({
      scrollbar: { handleMouseWheel: true },
    })

    document.body.dispatchEvent(new Event('pointerdown', { bubbles: true, composed: true }))
    expect(monacoMock.updateOptions).toHaveBeenLastCalledWith({
      scrollbar: { handleMouseWheel: false },
    })
    wrapper.unmount()
  })

  it('把 id/name 落到 Monaco 原生输入并释放关闭文档', () => {
    const wrapper = mount(MonacoEditor, {
      props: {
        id: 'file-editor-content',
        name: 'fileContent',
        modelValue: 'content',
        documentKey: 'node-a:/a.txt',
      },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })

    const textarea = monacoMock.editorRoot?.querySelector<HTMLTextAreaElement>('textarea.inputarea')
    expect(textarea?.id).toBe('file-editor-content')
    expect(textarea?.name).toBe('fileContent')

    const model = monacoMock.models[0]
    ;(wrapper.vm as unknown as { disposeDocument: (key: string) => void }).disposeDocument(
      'node-a:/a.txt',
    )
    expect(model.disposed).toBe(true)
    wrapper.unmount()
  })
})
