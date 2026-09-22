import { createI18n } from 'vue-i18n'
import { defineComponent, nextTick, ref } from 'vue'
import { mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import en from '@/locales/en'
import zh from '@/locales/zh'
import SuiteImportWorkspace from '@/apps/views/suite-center/SuiteImportWorkspace.vue'

function mountWorkspace(props: { busy?: boolean } = {}) {
  return mount(SuiteImportWorkspace, {
    props: {
      busy: props.busy ?? false,
    },
    global: {
      plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
    },
  })
}

function selectFile(wrapper: ReturnType<typeof mountWorkspace>, file: File) {
  const input = wrapper.get<HTMLInputElement>('#suite-package-file')
  Object.defineProperty(input.element, 'files', { configurable: true, value: [file] })
  return input.trigger('change')
}

describe('SuiteImportWorkspace', () => {
  beforeEach(() => {
    vi.restoreAllMocks()
  })

  it('使用 VUST 按钮触发隐藏的原生文件选择器', async () => {
    const wrapper = mountWorkspace()
    const nativeInput = wrapper.get<HTMLInputElement>('#suite-package-file')
    const click = vi.spyOn(nativeInput.element, 'click')

    expect(nativeInput.classes()).toContain('import-workspace__native-input')
    await wrapper.get('[data-ui="suite-file-select"]').trigger('click')
    expect(click).toHaveBeenCalledTimes(1)
    wrapper.unmount()
  })

  it('选择合法套件包后显示名称并允许提交', async () => {
    const wrapper = mountWorkspace()
    const file = new File(['suite'], 'security-tools.vsp')
    await selectFile(wrapper, file)

    expect(wrapper.get('[data-slot="selected-package"]').text()).toBe('security-tools.vsp')
    await wrapper.get('[data-ui="suite-confirm-import"]').trigger('click')
    expect(wrapper.emitted('submit')).toEqual([[file]])
    wrapper.unmount()
  })

  it('拒绝错误扩展名和超过 50 MiB 的套件包', async () => {
    const wrapper = mountWorkspace()
    await selectFile(wrapper, new File(['invalid'], 'security-tools.zip'))
    expect(wrapper.get('[role="alert"]').text()).toContain('.vsp')

    const oversized = new File(['large'], 'security-tools.vsp')
    Object.defineProperty(oversized, 'size', { value: 50 * 1024 * 1024 + 1 })
    await selectFile(wrapper, oversized)
    expect(wrapper.get('[role="alert"]').text()).toContain('50 MiB')
    wrapper.unmount()
  })

  it('上传期间禁用选择、返回和重复提交', async () => {
    const wrapper = mountWorkspace({ busy: true })

    expect(wrapper.get('[data-ui="suite-file-select"]').attributes('disabled')).toBeDefined()
    expect(wrapper.get('[data-ui="suite-import-header-back"]').attributes('disabled')).toBeDefined()
    expect(wrapper.get('[data-ui="suite-confirm-import"]').attributes('disabled')).toBeDefined()
    expect(wrapper.find('[data-ui="suite-import-back"]').exists()).toBe(false)
    wrapper.unmount()
  })

  it('进入时聚焦标题，返回后恢复入口焦点并清理文件', async () => {
    const Harness = defineComponent({
      components: { SuiteImportWorkspace },
      setup() {
        const visible = ref(false)
        return { visible }
      },
      template: `
        <button v-if="!visible" data-ui="suite-import" @click="visible = true">导入</button>
        <SuiteImportWorkspace v-else :busy="false" @back="visible = false" />
      `,
    })
    const wrapper = mount(Harness, {
      attachTo: document.body,
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
      },
    })
    const trigger = wrapper.get('[data-ui="suite-import"]')
    ;(trigger.element as HTMLButtonElement).focus()
    await trigger.trigger('click')
    await nextTick()
    expect(document.activeElement).toBe(wrapper.get('h2').element)

    const input = wrapper.get<HTMLInputElement>('#suite-package-file')
    Object.defineProperty(input.element, 'files', {
      configurable: true,
      value: [new File(['suite'], 'temporary.vsp')],
    })
    await input.trigger('change')
    await wrapper.get('[data-ui="suite-import-header-back"]').trigger('click')
    await nextTick()
    expect(document.activeElement).toBe(wrapper.get('[data-ui="suite-import"]').element)

    await wrapper.get('[data-ui="suite-import"]').trigger('click')
    expect(wrapper.get('[data-slot="selected-package"]').text()).toBe('尚未选择套件包')
    wrapper.unmount()
  })
})
