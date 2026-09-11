import { createPinia } from 'pinia'
import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import NetworkSettings from '../NetworkSettings.vue'
import zh from '@/locales/zh'

const api = vi.hoisted(() => ({
  fetchInterfaces: vi.fn(),
  fetchNetwork: vi.fn(),
  updateNetwork: vi.fn(),
}))

vi.mock('@/api/modules/vust', () => ({ vustApi: api }))

const success = <T>(data: T) => ({ success: true, code: 200, message: '', data })
const mountSettings = () =>
  mount(NetworkSettings, {
    global: {
      plugins: [createPinia(), createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
    },
  })

describe('NetworkSettings', () => {
  beforeEach(() => {
    api.fetchInterfaces.mockResolvedValue(
      success([{ name: 'eth0', ip: '192.0.2.10', label: 'eth0' }]),
    )
    api.fetchNetwork.mockResolvedValue(success({ host: '::', port: 8443, publicHost: 'vust.test' }))
  })

  it('挂载时并行加载网卡和网络配置', async () => {
    const wrapper = mountSettings()
    await flushPromises()

    expect(api.fetchInterfaces).toHaveBeenCalledTimes(1)
    expect(api.fetchNetwork).toHaveBeenCalledTimes(1)
    expect(wrapper.find('label[for="settings-network-host"]').exists()).toBe(true)
    expect(wrapper.find('label[for="settings-network-port"]').exists()).toBe(true)
  })

  it('端口越界时显示字段错误且不提交', async () => {
    const wrapper = mountSettings()
    await flushPromises()

    await wrapper.get('#settings-network-port').setValue('70000')
    await wrapper.get('[data-ui="network-save"]').trigger('click')

    expect(wrapper.text()).toContain('端口必须在 1-65535 范围内')
    expect(api.updateNetwork).not.toHaveBeenCalled()
  })
})
