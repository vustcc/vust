import { createI18n } from 'vue-i18n'
import { createPinia } from 'pinia'
import { flushPromises, shallowMount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import SuiteWebAppView from '@/apps/views/SuiteWebAppView.vue'
import { useThemeStore } from '@/stores/theme'

const sdk = vi.hoisted(() => {
  const bridge = {
    sendTheme: vi.fn(),
    sendLocale: vi.fn(),
    destroy: vi.fn(),
  }
  return {
    bridge,
    createSuiteHostBridge: vi.fn(() => bridge),
  }
})

vi.mock('@vustcc/suite-sdk', () => ({
  createSuiteHostBridge: sdk.createSuiteHostBridge,
  SUITE_MESSAGE_TYPES: {
    suiteWindowFocus: 'suite:window:focus',
    suiteNotificationShow: 'suite:notification:show',
    suiteNavigationOpen: 'suite:navigation:open',
  },
}))

vi.mock('@/stores/window-manager', () => ({
  useWindowManagerStore: () => ({
    openWindows: [],
    maxZIndex: 1,
    focusWindow: vi.fn(),
    openWindowWithPayload: vi.fn(),
  }),
}))

vi.mock('@/stores/toast', () => ({
  useToastStore: () => ({ showToast: vi.fn() }),
}))

describe('SuiteWebAppView material sync', () => {
  beforeEach(() => {
    localStorage.clear()
    sdk.bridge.sendTheme.mockClear()
    sdk.bridge.sendLocale.mockClear()
    sdk.bridge.destroy.mockClear()
    sdk.createSuiteHostBridge.mockClear()
  })

  it('sends the initial preference and resends after it changes', async () => {
    const pinia = createPinia()
    shallowMount(SuiteWebAppView, {
      props: {
        windowId: 'suite-window',
        payload: { url: 'https://suite.example.test/' },
      },
      global: {
        plugins: [pinia, createI18n({ legacy: false, locale: 'zh', messages: { zh: {} } })],
      },
    })
    await flushPromises()

    const options = sdk.createSuiteHostBridge.mock.calls[0]?.[0]
    expect(options?.theme()).toEqual({
      theme: 'light',
      resolvedTheme: 'light',
      glassEnabled: true,
    })
    const initialCalls = sdk.bridge.sendTheme.mock.calls.length

    useThemeStore(pinia).setGlassEnabled(false)
    await flushPromises()
    expect(options?.theme().glassEnabled).toBe(false)
    expect(sdk.bridge.sendTheme.mock.calls.length).toBeGreaterThan(initialCalls)
  })
})
