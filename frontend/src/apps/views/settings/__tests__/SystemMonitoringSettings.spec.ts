import { createPinia } from 'pinia'
import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import SystemMonitoringSettings from '../SystemMonitoringSettings.vue'
import { systemMonitoringApi } from '@/api/modules/systemMonitoring'

vi.mock('@/api/modules/systemMonitoring', () => ({
  systemMonitoringApi: {
    fetchSettings: vi.fn(),
    updateSettings: vi.fn(),
    clearHistory: vi.fn(),
  },
}))

const api = vi.mocked(systemMonitoringApi)
const messages = {
  app: {
    nodes: { master: 'Local' },
    settings: {
      monitoring: {
        label: 'Monitoring',
        description: 'Description',
        loading: 'Loading',
        loadFailed: 'Load failed',
        saveFailed: 'Save failed',
        clearFailed: 'Clear failed',
        readOnly: 'Read only',
        running: 'Running',
        stopped: 'Stopped',
        collection: 'Collection',
        collectionHint: 'Collection hint',
        retention: 'Retention',
        retentionHint: 'Retention hint',
        retentionOption: '{days} days',
        sampleInterval: 'Interval',
        seconds: '{seconds} seconds',
        storedSamples: 'Samples',
        oldestSample: 'Oldest',
        newestSample: 'Newest',
        clearHistory: 'Clear',
        clearTitle: 'Clear history',
        clearConfirm: 'Confirm clear',
      },
    },
  },
  common: {
    enabled: 'Enabled',
    disabled: 'Disabled',
    save: 'Save',
    cancel: 'Cancel',
    confirm: 'Confirm',
  },
}

const settings = (canManage = true) => ({
  ownership: 'system' as const,
  historyCollectionEnabled: true,
  historySampleIntervalSeconds: 60,
  retentionDays: 7,
  storedSampleCount: 12,
  oldestSampledAt: null,
  newestSampledAt: null,
  capabilities: { canManageCollection: canManage, canClearHistory: canManage },
})

const mountSettings = () =>
  mount(SystemMonitoringSettings, {
    global: {
      plugins: [
        createPinia(),
        createI18n({ legacy: false, locale: 'en', messages: { en: messages } }),
      ],
    },
  })

describe('SystemMonitoringSettings', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('输入控件具有稳定 name 且 label 指向真实可聚焦控件', async () => {
    api.fetchSettings.mockResolvedValue({ success: true, code: 200, message: '', data: settings() })
    const wrapper = mountSettings()
    await flushPromises()

    expect(wrapper.find('label[for="system-monitoring-collection"]').exists()).toBe(true)
    expect(
      wrapper.find('#system-monitoring-collection[name="systemMonitoringCollection"]').exists(),
    ).toBe(true)
    expect(wrapper.find('label[for="system-monitoring-retention"]').exists()).toBe(true)
    expect(wrapper.find('button#system-monitoring-retention').exists()).toBe(true)
    expect(wrapper.find('input[name="systemMonitoringRetention"]').exists()).toBe(true)
  })

  it('无管理能力时不渲染可执行入口', async () => {
    api.fetchSettings.mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: settings(false),
    })
    const wrapper = mountSettings()
    await flushPromises()

    expect(wrapper.find('#system-monitoring-collection').exists()).toBe(false)
    expect(wrapper.text()).toContain('Read only')
    expect(wrapper.text()).not.toContain('Clear')
  })
})
