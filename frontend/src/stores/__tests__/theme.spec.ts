import { beforeEach, describe, expect, it } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { applyGlassPreference, getStoredGlassEnabled, useThemeStore } from '@/stores/theme'

describe('Liquid Glass preference', () => {
  beforeEach(() => {
    localStorage.clear()
    document.documentElement.removeAttribute('data-glass')
    setActivePinia(createPinia())
  })

  it('defaults missing and invalid values to enabled', () => {
    expect(getStoredGlassEnabled()).toBe(true)
    localStorage.setItem('vust_glass_enabled', 'invalid')
    expect(getStoredGlassEnabled()).toBe(true)
  })

  it('persists the preference and applies it to the document root', () => {
    expect(applyGlassPreference(false)).toBe(false)
    expect(localStorage.getItem('vust_glass_enabled')).toBe('false')
    expect(document.documentElement.dataset.glass).toBe('disabled')

    const store = useThemeStore()
    store.setGlassEnabled(true)
    expect(store.glassEnabled).toBe(true)
    expect(document.documentElement.dataset.glass).toBe('enabled')
  })
})
