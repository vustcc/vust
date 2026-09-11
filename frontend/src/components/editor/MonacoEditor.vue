<script setup lang="ts">
/**
 * @file MonacoEditor.vue
 * @description VUST 平台 Monaco Editor 封装组件。
 * 提供语法高亮、VDL 主题适配、语言自动检测和多文档模型隔离能力。
 * 严格遵循 VDL 设计规范。
 */

import { onMounted, onUnmounted, ref, watch, computed, shallowRef } from 'vue'
import { useI18n } from 'vue-i18n'
import { setupMonacoWorkers, monaco } from './monaco-workers'
import {
  VDL_THEME_DARK,
  VDL_THEME_LIGHT,
  createSdlDarkTheme,
  createSdlLightTheme,
} from './vdl-monaco-theme'

setupMonacoWorkers()

interface Props {
  /** 传递到 Monaco 可聚焦输入区的字段 ID */
  id?: string
  /** 传统 textarea 模式下的字段名称 */
  name?: string
  /** 编辑器内容（v-model） */
  modelValue?: string
  /** 语言标识（如 'json', 'yaml', 'typescript'），不传时自动检测 */
  language?: string
  /** 文件路径，用于自动推断语言 */
  filePath?: string
  /** 多文档模式下稳定且唯一的文档键 */
  documentKey?: string
  /** 是否只读 */
  readOnly?: boolean
  /** 字号 */
  fontSize?: number
  /** 是否自动换行 */
  wordWrap?: boolean
  /** 是否高亮易与基础 ASCII 混淆的 Unicode 字符 */
  highlightAmbiguousUnicode?: boolean
  /** 是否显示 minimap */
  minimap?: boolean
  /** 是否启用 Sticky Scroll */
  stickyScroll?: boolean
  /** 是否仅在点击编辑器后接管鼠标滚轮 */
  wheelFocusOnClick?: boolean
  /** 浮层组件是否使用固定定位 */
  fixedOverflowWidgets?: boolean
  /** 空白字符显示方式 */
  renderWhitespace?: 'none' | 'selection' | 'all'
  /** 传递到 Monaco 可聚焦编辑区的可访问名称 */
  ariaLabel?: string
  /** 传递到 Monaco 可聚焦编辑区的可访问名称元素 ID */
  ariaLabelledby?: string
}

const props = withDefaults(defineProps<Props>(), {
  id: '',
  name: '',
  modelValue: '',
  language: '',
  filePath: '',
  documentKey: '',
  readOnly: false,
  fontSize: 13,
  wordWrap: true,
  highlightAmbiguousUnicode: true,
  minimap: true,
  stickyScroll: true,
  wheelFocusOnClick: false,
  fixedOverflowWidgets: true,
  renderWhitespace: 'selection',
  ariaLabel: '',
  ariaLabelledby: '',
})

const emit = defineEmits<{
  (e: 'update:modelValue', value: string): void
  (e: 'save', documentKey: string): void
  (e: 'change', value: string, documentKey: string): void
}>()

const { t } = useI18n()
const containerRef = ref<HTMLDivElement | null>(null)
const editorInstance = shallowRef<monaco.editor.IStandaloneCodeEditor | null>(null)
let resizeObserver: ResizeObserver | null = null
let themesRegistered = false
let activeDocumentKey = ''
let standaloneModel: monaco.editor.ITextModel | null = null
let syncingModel = false
let wheelCaptureActive = false

interface ManagedDocument {
  model: monaco.editor.ITextModel
  viewState: monaco.editor.ICodeEditorViewState | null
}

const managedDocuments = new Map<string, ManagedDocument>()

/** 将字段标识同步到 Monaco 当前渲染模式下的真实交互控件。 */
function syncInteractiveAttributes() {
  const editorDom = editorInstance.value?.getDomNode()
  if (!editorDom) return

  const textarea = editorDom.querySelector<HTMLTextAreaElement>('textarea.inputarea')
  const nativeEditContext = editorDom.querySelector<HTMLElement>('.native-edit-context')
  const interactiveElement = textarea ?? nativeEditContext

  if (interactiveElement) {
    if (props.id) interactiveElement.id = props.id
    else interactiveElement.removeAttribute('id')

    if (props.ariaLabelledby) {
      interactiveElement.setAttribute('aria-labelledby', props.ariaLabelledby)
      interactiveElement.removeAttribute('aria-label')
    } else {
      interactiveElement.removeAttribute('aria-labelledby')
      interactiveElement.setAttribute('aria-label', props.ariaLabel || t('common.editor'))
    }
  }

  if (textarea) {
    if (props.name) textarea.name = props.name
    else textarea.removeAttribute('name')
  }

  const imeTextarea = editorDom.querySelector<HTMLTextAreaElement>('textarea.ime-text-area')
  if (imeTextarea) {
    if (props.id) imeTextarea.id = `${props.id}-ime`
    else imeTextarea.removeAttribute('id')
  }
}

/** 根据文件扩展名推断 Monaco 语言 */
const EXT_LANG_MAP: Record<string, string> = {
  js: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  jsx: 'javascript',
  ts: 'typescript',
  tsx: 'typescript',
  vue: 'html',
  html: 'html',
  htm: 'html',
  css: 'css',
  scss: 'scss',
  less: 'less',
  json: 'json',
  jsonc: 'json',
  yaml: 'yaml',
  yml: 'yaml',
  xml: 'xml',
  svg: 'xml',
  md: 'markdown',
  markdown: 'markdown',
  py: 'python',
  rs: 'rust',
  go: 'go',
  java: 'java',
  c: 'c',
  h: 'c',
  cpp: 'cpp',
  hpp: 'cpp',
  sh: 'shell',
  bash: 'shell',
  zsh: 'shell',
  sql: 'sql',
  dockerfile: 'dockerfile',
  toml: 'ini',
  ini: 'ini',
  conf: 'ini',
  cfg: 'ini',
  env: 'ini',
  lua: 'lua',
  rb: 'ruby',
  php: 'php',
  pl: 'perl',
  r: 'r',
  swift: 'swift',
  kt: 'kotlin',
  kts: 'kotlin',
  graphql: 'graphql',
  gql: 'graphql',
  proto: 'protobuf',
}

/**
 * 从文件路径推断语言
 */
function detectLanguage(filePath: string): string {
  if (!filePath) return 'plaintext'

  const fileName = filePath.split('/').pop() || ''
  const lowerName = fileName.toLowerCase()

  // 特殊文件名匹配
  if (lowerName === 'dockerfile' || lowerName.startsWith('dockerfile.')) return 'dockerfile'
  if (lowerName === 'makefile' || lowerName === 'gnumakefile') return 'makefile'
  if (lowerName === '.gitignore' || lowerName === '.dockerignore') return 'ignore'
  if (lowerName === 'nginx.conf') return 'nginx'

  const ext = fileName.includes('.') ? fileName.split('.').pop()?.toLowerCase() || '' : ''
  return EXT_LANG_MAP[ext] || 'plaintext'
}

const resolvedLanguage = computed(() => {
  if (props.language) return props.language
  return detectLanguage(props.filePath)
})

/** 检测当前是否为浅色主题 */
function isLightTheme(): boolean {
  return document.documentElement.getAttribute('data-theme') === 'light'
}

/** 注册 VDL 主题到 Monaco */
function ensureThemesRegistered() {
  if (themesRegistered) return
  monaco.editor.defineTheme(VDL_THEME_DARK, createSdlDarkTheme())
  monaco.editor.defineTheme(VDL_THEME_LIGHT, createSdlLightTheme())
  themesRegistered = true
}

/** 返回或创建独立 Monaco 文档模型。 */
function getOrCreateDocument(documentKey: string) {
  const existing = managedDocuments.get(documentKey)
  if (existing) return existing
  const uri = monaco.Uri.from({
    scheme: 'vust-file',
    path: `/${encodeURIComponent(documentKey)}`,
  })
  const document = {
    model: monaco.editor.createModel(props.modelValue, resolvedLanguage.value, uri),
    viewState: null,
  }
  managedDocuments.set(documentKey, document)
  return document
}

/** 切换文档时保存并恢复光标、选择区和滚动位置。 */
function switchDocument(documentKey: string) {
  const editor = editorInstance.value
  if (!editor || !documentKey || activeDocumentKey === documentKey) return

  const activeDocument = managedDocuments.get(activeDocumentKey)
  if (activeDocument) activeDocument.viewState = editor.saveViewState()

  const nextDocument = getOrCreateDocument(documentKey)
  syncingModel = true
  if (nextDocument.model.getValue() !== props.modelValue) {
    nextDocument.model.setValue(props.modelValue)
  }
  monaco.editor.setModelLanguage(nextDocument.model, resolvedLanguage.value)
  editor.setModel(nextDocument.model)
  activeDocumentKey = documentKey
  if (nextDocument.viewState) editor.restoreViewState(nextDocument.viewState)
  syncingModel = false
  syncInteractiveAttributes()
}

/** 释放已关闭标签对应的 Monaco model。 */
function disposeDocument(documentKey: string) {
  const document = managedDocuments.get(documentKey)
  if (!document) return
  if (activeDocumentKey === documentKey) {
    editorInstance.value?.setModel(null)
    activeDocumentKey = ''
  }
  document.model.dispose()
  managedDocuments.delete(documentKey)
}

/** 切换编辑器是否接管鼠标滚轮。 */
function setWheelCapture(active: boolean) {
  if (!props.wheelFocusOnClick || wheelCaptureActive === active) return
  wheelCaptureActive = active
  editorInstance.value?.updateOptions({
    scrollbar: { handleMouseWheel: active },
  })
}

/** 点击编辑器外部时把滚轮交还给外围滚动容器。 */
function handleDocumentPointerDown(event: PointerEvent) {
  if (!wheelCaptureActive) return
  const container = containerRef.value
  if (container && event.composedPath().includes(container)) return
  setWheelCapture(false)
}

/** 初始化编辑器实例 */
function createEditor() {
  if (!containerRef.value) return

  ensureThemesRegistered()

  const currentTheme = isLightTheme() ? VDL_THEME_LIGHT : VDL_THEME_DARK

  const initialDocument = props.documentKey ? getOrCreateDocument(props.documentKey) : null
  standaloneModel = initialDocument
    ? null
    : monaco.editor.createModel(props.modelValue, resolvedLanguage.value)
  activeDocumentKey = props.documentKey
  const editor = monaco.editor.create(containerRef.value, {
    model: initialDocument?.model ?? standaloneModel,
    theme: currentTheme,
    readOnly: props.readOnly,
    fontSize: props.fontSize,
    fontFamily: 'var(--vdl-font-mono)',
    lineHeight: 1.6,
    wordWrap: props.wordWrap ? 'on' : 'off',
    unicodeHighlight: {
      ambiguousCharacters: props.highlightAmbiguousUnicode,
      invisibleCharacters: true,
    },
    minimap: { enabled: props.minimap },
    stickyScroll: { enabled: props.stickyScroll },
    renderWhitespace: props.renderWhitespace,
    ariaLabel: props.ariaLabelledby ? '' : props.ariaLabel || t('common.editor'),
    automaticLayout: false,
    scrollBeyondLastLine: false,
    renderLineHighlight: 'line',
    cursorBlinking: 'smooth',
    cursorSmoothCaretAnimation: 'on',
    smoothScrolling: true,
    bracketPairColorization: { enabled: true },
    guides: { bracketPairs: true, indentation: true },
    padding: { top: 8, bottom: 8 },
    scrollbar: {
      handleMouseWheel: !props.wheelFocusOnClick,
      alwaysConsumeMouseWheel: !props.wheelFocusOnClick,
      verticalScrollbarSize: 8,
      horizontalScrollbarSize: 8,
      verticalSliderSize: 8,
      horizontalSliderSize: 8,
    },
    contextmenu: true,
    fixedOverflowWidgets: props.fixedOverflowWidgets,
  })

  // 注册 Ctrl+S 保存快捷键
  editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => {
    emit('save', activeDocumentKey)
  })

  // 内容变更回调
  editor.onDidChangeModelContent(() => {
    if (syncingModel) return
    const value = editor.getValue()
    emit('update:modelValue', value)
    emit('change', value, activeDocumentKey)
  })

  editor.onMouseDown((event) => {
    setWheelCapture(true)
    if (props.readOnly && event.target.position) {
      editor.setPosition(event.target.position, 'mouse')
    }
  })

  editorInstance.value = editor
  syncInteractiveAttributes()

  // 监听容器尺寸变化以触发 layout
  if ('ResizeObserver' in window) {
    resizeObserver = new ResizeObserver(() => {
      editor.layout()
    })
    resizeObserver.observe(containerRef.value)
  }
}

// --- 响应式 prop 同步 ---

watch(
  () => props.documentKey,
  (documentKey) => switchDocument(documentKey),
)

/** 外部 modelValue 变更 → 同步到编辑器（避免自身触发的循环） */
watch(
  () => props.modelValue,
  (newVal) => {
    const editor = editorInstance.value
    if (!editor) return
    if (editor.getValue() !== newVal) {
      syncingModel = true
      editor.setValue(newVal)
      syncingModel = false
    }
  },
)

watch(
  [() => props.id, () => props.name, () => props.ariaLabel, () => props.ariaLabelledby],
  syncInteractiveAttributes,
)

/** 语言变更 → 更新 model language */
watch(resolvedLanguage, (lang) => {
  const editor = editorInstance.value
  if (!editor) return
  const model = editor.getModel()
  if (model) {
    monaco.editor.setModelLanguage(model, lang)
  }
})

/** readOnly 变更 */
watch(
  () => props.readOnly,
  (val) => {
    editorInstance.value?.updateOptions({ readOnly: val })
  },
)

/** fontSize 变更 */
watch(
  () => props.fontSize,
  (val) => {
    editorInstance.value?.updateOptions({ fontSize: val })
  },
)

/** wordWrap 变更 */
watch(
  () => props.wordWrap,
  (val) => {
    editorInstance.value?.updateOptions({ wordWrap: val ? 'on' : 'off' })
  },
)

/** 易混淆 Unicode 字符高亮变更 */
watch(
  () => props.highlightAmbiguousUnicode,
  (val) => {
    editorInstance.value?.updateOptions({
      unicodeHighlight: {
        ambiguousCharacters: val,
        invisibleCharacters: true,
      },
    })
  },
)

/** minimap 变更 */
watch(
  () => props.minimap,
  (val) => {
    editorInstance.value?.updateOptions({ minimap: { enabled: val } })
  },
)

/** Sticky Scroll 变更 */
watch(
  () => props.stickyScroll,
  (val) => {
    editorInstance.value?.updateOptions({ stickyScroll: { enabled: val } })
  },
)

/** 空白字符显示方式变更 */
watch(
  () => props.renderWhitespace,
  (val) => {
    editorInstance.value?.updateOptions({ renderWhitespace: val })
  },
)

/** 监听 data-theme 切换以动态更新主题 */
let themeObserver: MutationObserver | null = null

function watchThemeSwitch() {
  if (typeof MutationObserver === 'undefined') return

  themeObserver = new MutationObserver(() => {
    // 主题变更时重新注册主题色并应用
    themesRegistered = false
    ensureThemesRegistered()
    const theme = isLightTheme() ? VDL_THEME_LIGHT : VDL_THEME_DARK
    monaco.editor.setTheme(theme)
  })

  themeObserver.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ['data-theme'],
  })
}

onMounted(() => {
  createEditor()
  watchThemeSwitch()
  document.addEventListener('pointerdown', handleDocumentPointerDown, true)
})

onUnmounted(() => {
  document.removeEventListener('pointerdown', handleDocumentPointerDown, true)
  themeObserver?.disconnect()
  themeObserver = null
  resizeObserver?.disconnect()
  resizeObserver = null
  editorInstance.value?.setModel(null)
  editorInstance.value?.dispose()
  editorInstance.value = null
  standaloneModel?.dispose()
  standaloneModel = null
  managedDocuments.forEach((document) => document.model.dispose())
  managedDocuments.clear()
  activeDocumentKey = ''
})

/**
 * 暴露编辑器实例供父组件命令式操作
 */
defineExpose({
  /** 获取原始 Monaco 编辑器实例 */
  getEditor: () => editorInstance.value,
  /** 手动触发 layout */
  layout: () => editorInstance.value?.layout(),
  /** 聚焦编辑器 */
  focus: () => editorInstance.value?.focus(),
  /** 释放已关闭标签对应的文档模型 */
  disposeDocument,
})
</script>

<template>
  <div
    class="monaco-editor-wrapper"
    data-ui="monaco-editor"
    data-slot="editor"
    data-native-context-menu
  >
    <div ref="containerRef" class="monaco-container" />
  </div>
</template>

<style scoped>
.monaco-editor-wrapper {
  width: 100%;
  height: 100%;
  display: flex;
  flex-direction: column;
  min-height: 0;
  position: relative;
}

.monaco-container {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}
</style>
