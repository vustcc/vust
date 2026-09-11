import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { nodesApi } from '@/api/modules/nodes'
import { runtimeLogApi } from '@/api/modules/runtime-log'
import RuntimeLogView from '@/apps/views/RuntimeLogView.vue'
import { VustTooltip } from '@/components/ui'
import en from '@/locales/en'
import zh from '@/locales/zh'

vi.mock('@/api/modules/nodes', () => ({ nodesApi: { list: vi.fn() } }))
vi.mock('@/api/modules/runtime-log', () => ({
  runtimeLogApi: { files: vi.fn(), query: vi.fn() },
}))

function mountView() {
  return mount(RuntimeLogView, {
    global: { plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })] },
  })
}

describe('RuntimeLogView', () => {
  it('默认折叠低频筛选并将三个功能按钮收敛到独立动作区', async () => {
    vi.mocked(nodesApi.list).mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: [],
    })
    vi.mocked(runtimeLogApi.files).mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: { availability: 'available', files: [] },
    })

    const wrapper = mountView()
    await flushPromises()

    const actions = wrapper.find('[data-ui="toolbar-actions"]')
    expect(actions.findAll('button')).toHaveLength(3)
    expect(wrapper.find('[data-ui="filters"]').exists()).toBe(false)
    expect(wrapper.find('#runtime-log-level').exists()).toBe(false)
    expect(wrapper.find('#runtime-log-target').exists()).toBe(false)
    expect(wrapper.find('#runtime-log-keyword').exists()).toBe(false)

    await actions.find('button').trigger('click')

    expect(wrapper.find('[data-ui="filters"]').exists()).toBe(true)
    expect(wrapper.find('#runtime-log-level').exists()).toBe(true)
    expect(wrapper.find('#runtime-log-target').attributes('name')).toBe('runtimeLogTarget')
    expect(wrapper.find('#runtime-log-keyword').attributes('name')).toBe('runtimeLogKeyword')
    expect(actions.find('button').attributes('aria-expanded')).toBe('true')
    wrapper.unmount()
  })

  it('保持日志单行布局并仅为被截断的字段启用完整内容气泡', async () => {
    const target = 'vust_agent::api::docker::containers'
    const message = '这是一条宽度不足时必须保持单行并省略显示的长日志消息'
    vi.mocked(nodesApi.list).mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: [],
    })
    vi.mocked(runtimeLogApi.files).mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: {
        availability: 'available',
        files: [
          {
            service: 'master',
            fileId: 'master-current',
            fileVersion: '1',
            fileName: 'vust.log',
            sizeBytes: 1024,
          },
        ],
      },
    })
    vi.mocked(runtimeLogApi.query).mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: {
        lines: [
          {
            offset: 1,
            timestamp: '2026-08-04T03:02:01',
            level: 'INFO',
            target,
            message,
            parseError: false,
          },
        ],
        hasMore: false,
        scanTruncated: false,
        fileVersion: '1',
      },
    })

    const wrapper = mountView()
    await flushPromises()
    await wrapper.findAll('[data-ui="toolbar-actions"] button')[1].trigger('click')
    await flushPromises()

    const fields = wrapper.findAll<HTMLElement>('[data-slot="log-field"]')
    Object.defineProperties(fields[0].element, {
      clientWidth: { configurable: true, value: 160 },
      scrollWidth: { configurable: true, value: 320 },
    })
    Object.defineProperties(fields[1].element, {
      clientWidth: { configurable: true, value: 260 },
      scrollWidth: { configurable: true, value: 520 },
    })
    await fields[0].trigger('pointerover')
    await fields[1].trigger('pointerover')

    const tooltips = wrapper.findAllComponents(VustTooltip)
    expect(tooltips.map((tooltip) => tooltip.props('text'))).toEqual([target, message])
    expect(tooltips.every((tooltip) => tooltip.props('disabled') === false)).toBe(true)
    expect(wrapper.find('time').text()).toBe('2026-08-04 03:02:01')
    expect(wrapper.find('[data-slot="log-level"]').exists()).toBe(true)
    expect(fields[0].text()).toBe(target)
    expect(fields[1].text()).toBe(message)
    wrapper.unmount()
  })
})
