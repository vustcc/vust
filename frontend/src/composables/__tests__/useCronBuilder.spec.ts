import { defineComponent } from 'vue'
import { createI18n } from 'vue-i18n'
import { mount } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import { useCronBuilder } from '@/composables/useCronBuilder'
import en from '@/locales/en'
import zh from '@/locales/zh'

const Harness = defineComponent({
  setup() {
    return useCronBuilder()
  },
  template: '<div />',
})

const mountHarness = () =>
  mount(Harness, {
    global: {
      plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
    },
  })

describe('useCronBuilder', () => {
  it('简单模式只生成五段分钟级 Cron', () => {
    const wrapper = mountHarness()
    expect(wrapper.vm.cronExprForSubmit).toBe('*/5 * * * *')

    wrapper.vm.simpleCron.type = 'daily'
    wrapper.vm.simpleCron.hour = 9
    wrapper.vm.simpleCron.minute = 30
    expect(wrapper.vm.cronExprForSubmit).toBe('30 9 * * *')
  })

  it('六段表达式不再被静默兼容', () => {
    const wrapper = mountHarness()
    wrapper.vm.loadCron('0 */5 * * * *')
    expect(wrapper.vm.cronMode).toBe('advanced')
    expect(wrapper.vm.advancedCronExpr).toBe('0 */5 * * * *')
  })

  it('可解析五段周计划并保持星期口径', () => {
    const wrapper = mountHarness()
    wrapper.vm.loadCron('15 8 * * 1')
    expect(wrapper.vm.cronMode).toBe('simple')
    expect(wrapper.vm.simpleCron.type).toBe('weekly')
    expect(wrapper.vm.simpleCron.weekday).toBe(1)
    expect(wrapper.vm.cronExprForSubmit).toBe('15 8 * * 1')
  })
})
