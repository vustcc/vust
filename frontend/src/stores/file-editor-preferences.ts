import { ref, watch } from 'vue'
import { defineStore } from 'pinia'

export type FileEditorRenderWhitespace = 'none' | 'selection' | 'all'

export interface FileEditorPreferences {
  wordWrap: boolean
  fontSize: number
  highlightAmbiguousUnicode: boolean
  minimap: boolean
  stickyScroll: boolean
  renderWhitespace: FileEditorRenderWhitespace
}

interface PersistedFileEditorPreferences extends FileEditorPreferences {
  version: 1
}

export const FILE_EDITOR_PREFERENCES_STORAGE_KEY = 'vust_file_editor_preferences'

export const DEFAULT_FILE_EDITOR_PREFERENCES: Readonly<FileEditorPreferences> = Object.freeze({
  wordWrap: false,
  fontSize: 14,
  highlightAmbiguousUnicode: true,
  minimap: true,
  stickyScroll: true,
  renderWhitespace: 'selection',
})

/** 将未知数据归一化为有效的文件编辑器偏好。 */
function normalizePreferences(value: unknown): FileEditorPreferences {
  if (!value || typeof value !== 'object') {
    return { ...DEFAULT_FILE_EDITOR_PREFERENCES }
  }

  const candidate = value as Partial<PersistedFileEditorPreferences>
  if (candidate.version !== 1) {
    return { ...DEFAULT_FILE_EDITOR_PREFERENCES }
  }

  const fontSize =
    typeof candidate.fontSize === 'number' && Number.isFinite(candidate.fontSize)
      ? Math.min(24, Math.max(10, Math.round(candidate.fontSize)))
      : DEFAULT_FILE_EDITOR_PREFERENCES.fontSize
  const renderWhitespace = ['none', 'selection', 'all'].includes(candidate.renderWhitespace ?? '')
    ? (candidate.renderWhitespace as FileEditorRenderWhitespace)
    : DEFAULT_FILE_EDITOR_PREFERENCES.renderWhitespace

  return {
    wordWrap:
      typeof candidate.wordWrap === 'boolean'
        ? candidate.wordWrap
        : DEFAULT_FILE_EDITOR_PREFERENCES.wordWrap,
    fontSize,
    highlightAmbiguousUnicode:
      typeof candidate.highlightAmbiguousUnicode === 'boolean'
        ? candidate.highlightAmbiguousUnicode
        : DEFAULT_FILE_EDITOR_PREFERENCES.highlightAmbiguousUnicode,
    minimap:
      typeof candidate.minimap === 'boolean'
        ? candidate.minimap
        : DEFAULT_FILE_EDITOR_PREFERENCES.minimap,
    stickyScroll:
      typeof candidate.stickyScroll === 'boolean'
        ? candidate.stickyScroll
        : DEFAULT_FILE_EDITOR_PREFERENCES.stickyScroll,
    renderWhitespace,
  }
}

/** 从本地存储读取文件编辑器偏好，非法数据回退到默认值。 */
export function readFileEditorPreferences(): FileEditorPreferences {
  if (typeof window === 'undefined') {
    return { ...DEFAULT_FILE_EDITOR_PREFERENCES }
  }
  try {
    const raw = window.localStorage.getItem(FILE_EDITOR_PREFERENCES_STORAGE_KEY)
    return raw ? normalizePreferences(JSON.parse(raw)) : { ...DEFAULT_FILE_EDITOR_PREFERENCES }
  } catch {
    return { ...DEFAULT_FILE_EDITOR_PREFERENCES }
  }
}

export const useFileEditorPreferencesStore = defineStore('file-editor-preferences', () => {
  const initial = readFileEditorPreferences()
  const wordWrap = ref(initial.wordWrap)
  const fontSize = ref(initial.fontSize)
  const highlightAmbiguousUnicode = ref(initial.highlightAmbiguousUnicode)
  const minimap = ref(initial.minimap)
  const stickyScroll = ref(initial.stickyScroll)
  const renderWhitespace = ref<FileEditorRenderWhitespace>(initial.renderWhitespace)
  let isResetting = false

  /** 将当前偏好写入本地存储。 */
  function persistPreferences() {
    if (typeof window === 'undefined') return
    const value: PersistedFileEditorPreferences = {
      version: 1,
      wordWrap: wordWrap.value,
      fontSize: fontSize.value,
      highlightAmbiguousUnicode: highlightAmbiguousUnicode.value,
      minimap: minimap.value,
      stickyScroll: stickyScroll.value,
      renderWhitespace: renderWhitespace.value,
    }
    window.localStorage.setItem(FILE_EDITOR_PREFERENCES_STORAGE_KEY, JSON.stringify(value))
  }

  watch(
    [wordWrap, fontSize, highlightAmbiguousUnicode, minimap, stickyScroll, renderWhitespace],
    () => {
      if (!isResetting) persistPreferences()
    },
    { flush: 'sync' },
  )

  /** 恢复全部文件编辑器视图偏好。 */
  function resetPreferences() {
    isResetting = true
    wordWrap.value = DEFAULT_FILE_EDITOR_PREFERENCES.wordWrap
    fontSize.value = DEFAULT_FILE_EDITOR_PREFERENCES.fontSize
    highlightAmbiguousUnicode.value = DEFAULT_FILE_EDITOR_PREFERENCES.highlightAmbiguousUnicode
    minimap.value = DEFAULT_FILE_EDITOR_PREFERENCES.minimap
    stickyScroll.value = DEFAULT_FILE_EDITOR_PREFERENCES.stickyScroll
    renderWhitespace.value = DEFAULT_FILE_EDITOR_PREFERENCES.renderWhitespace
    isResetting = false
    persistPreferences()
  }

  return {
    wordWrap,
    fontSize,
    highlightAmbiguousUnicode,
    minimap,
    stickyScroll,
    renderWhitespace,
    resetPreferences,
  }
})
