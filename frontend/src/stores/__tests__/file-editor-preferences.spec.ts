import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it } from 'vitest'
import {
  DEFAULT_FILE_EDITOR_PREFERENCES,
  FILE_EDITOR_PREFERENCES_STORAGE_KEY,
  useFileEditorPreferencesStore,
} from '@/stores/file-editor-preferences'

describe('FileEditorPreferencesStore', () => {
  beforeEach(() => {
    localStorage.clear()
    setActivePinia(createPinia())
  })

  it('使用默认值并在同一 Pinia 中共享和持久化变更', () => {
    const first = useFileEditorPreferencesStore()
    const second = useFileEditorPreferencesStore()
    expect(first).toBe(second)
    expect(first.$state).toEqual(DEFAULT_FILE_EDITOR_PREFERENCES)

    first.wordWrap = true
    first.fontSize = 18
    first.renderWhitespace = 'all'

    expect(second.wordWrap).toBe(true)
    expect(JSON.parse(localStorage.getItem(FILE_EDITOR_PREFERENCES_STORAGE_KEY) || '{}')).toEqual({
      version: 1,
      wordWrap: true,
      fontSize: 18,
      highlightAmbiguousUnicode: true,
      minimap: true,
      stickyScroll: true,
      renderWhitespace: 'all',
    })
  })

  it('恢复合法字段、限制字号并为缺失字段使用默认值', () => {
    localStorage.setItem(
      FILE_EDITOR_PREFERENCES_STORAGE_KEY,
      JSON.stringify({
        version: 1,
        wordWrap: true,
        fontSize: 99,
        minimap: false,
        renderWhitespace: 'none',
      }),
    )

    const store = useFileEditorPreferencesStore()
    expect(store.wordWrap).toBe(true)
    expect(store.fontSize).toBe(24)
    expect(store.minimap).toBe(false)
    expect(store.renderWhitespace).toBe('none')
    expect(store.highlightAmbiguousUnicode).toBe(true)
    expect(store.stickyScroll).toBe(true)
  })

  it('损坏 JSON、未知版本和非法字段回退到默认值', () => {
    localStorage.setItem(FILE_EDITOR_PREFERENCES_STORAGE_KEY, '{broken')
    expect(useFileEditorPreferencesStore().$state).toEqual(DEFAULT_FILE_EDITOR_PREFERENCES)

    setActivePinia(createPinia())
    localStorage.setItem(
      FILE_EDITOR_PREFERENCES_STORAGE_KEY,
      JSON.stringify({ version: 2, wordWrap: true }),
    )
    expect(useFileEditorPreferencesStore().$state).toEqual(DEFAULT_FILE_EDITOR_PREFERENCES)

    setActivePinia(createPinia())
    localStorage.setItem(
      FILE_EDITOR_PREFERENCES_STORAGE_KEY,
      JSON.stringify({ version: 1, fontSize: 'large', renderWhitespace: 'invalid' }),
    )
    expect(useFileEditorPreferencesStore().$state).toEqual(DEFAULT_FILE_EDITOR_PREFERENCES)
  })

  it('重置全部偏好并立即覆盖本地存储', () => {
    const store = useFileEditorPreferencesStore()
    store.wordWrap = true
    store.fontSize = 20
    store.highlightAmbiguousUnicode = false
    store.minimap = false
    store.stickyScroll = false
    store.renderWhitespace = 'all'

    store.resetPreferences()

    expect(store.$state).toEqual(DEFAULT_FILE_EDITOR_PREFERENCES)
    expect(JSON.parse(localStorage.getItem(FILE_EDITOR_PREFERENCES_STORAGE_KEY) || '{}')).toEqual({
      version: 1,
      ...DEFAULT_FILE_EDITOR_PREFERENCES,
    })
  })
})
