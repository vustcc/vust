import { createI18n } from 'vue-i18n'
import { defineComponent } from 'vue'
import { mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import en from '@/locales/en'
import zh from '@/locales/zh'
import SuiteImportDialog from '@/apps/views/suite-center/SuiteImportDialog.vue'

const DialogStub = defineComponent({
  name: 'VustDialog',
  props: { visible: Boolean, title: String },
  emits: ['close'],
  template: '<div v-if="visible"><slot /><div><slot name="footer" /></div></div>',
})

function mountDialog(props: { visible?: boolean; busy?: boolean } = {}) {
  return mount(SuiteImportDialog, {
    props: {
      visible: props.visible ?? true,
      busy: props.busy ?? false,
    },
    global: {
      plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
      stubs: { VustDialog: DialogStub },
    },
  })
}

function selectFile(wrapper: ReturnType<typeof mountDialog>, file: File) {
  const input = wrapper.get<HTMLInputElement>('#suite-package-file')
  Object.defineProperty(input.element, 'files', { configurable: true, value: [file] })
  return input.trigger('change')
}

describe('SuiteImportDialog', () => {
  beforeEach(() => {
    vi.restoreAllMocks()
  })

  it('使用 VUST 按钮触发隐藏的原生文件选择器', async () => {
    const wrapper = mountDialog()
    const nativeInput = wrapper.get<HTMLInputElement>('#suite-package-file')
    const click = vi.spyOn(nativeInput.element, 'click')

    expect(nativeInput.classes()).toContain('import-dialog__native-input')
    await wrapper.get('[data-ui="suite-file-select"]').trigger('click')
    expect(click).toHaveBeenCalledTimes(1)
  })

  it('选择合法套件包后在独立文件区域显示名称并允许提交', async () => {
    const wrapper = mountDialog()
    const file = new File(['suite'], 'security-tools.vsp')
    await selectFile(wrapper, file)

    expect(wrapper.get('[data-slot="selected-package"]').text()).toBe('security-tools.vsp')
    await wrapper.get('[data-ui="suite-confirm-import"]').trigger('click')
    expect(wrapper.emitted('submit')).toEqual([[file]])
  })

  it('拒绝错误扩展名和超过 50 MiB 的套件包', async () => {
    const wrapper = mountDialog()
    await selectFile(wrapper, new File(['invalid'], 'security-tools.zip'))
    expect(wrapper.get('[role="alert"]').text()).toContain('.vsp')

    const oversized = new File(['large'], 'security-tools.vsp')
    Object.defineProperty(oversized, 'size', { value: 50 * 1024 * 1024 + 1 })
    await selectFile(wrapper, oversized)
    expect(wrapper.get('[role="alert"]').text()).toContain('50 MiB')
  })

  it('关闭弹窗后清空文件并在 busy 状态禁用选择', async () => {
    const wrapper = mountDialog()
    await selectFile(wrapper, new File(['suite'], 'security-tools.vsp'))
    await wrapper.setProps({ visible: false })
    await wrapper.setProps({ visible: true, busy: true })

    expect(wrapper.get('[data-slot="selected-package"]').text()).toBe('尚未选择套件包')
    expect(wrapper.get('[data-ui="suite-file-select"]').attributes('disabled')).toBeDefined()
    expect(wrapper.get<HTMLInputElement>('#suite-package-file').element.value).toBe('')
  })
})
