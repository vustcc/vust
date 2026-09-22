import { defineComponent, nextTick, ref } from 'vue'
import { enableAutoUnmount, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { createI18n } from 'vue-i18n'
import ApplicationDialog from '../ApplicationDialog.vue'

const i18n = createI18n({
  legacy: false,
  locale: 'en',
  messages: { en: { common: { close: 'Close' } } },
})

const mountOptions = {
  attachTo: document.body,
  global: { plugins: [i18n] },
}

enableAutoUnmount(afterEach)

const Harness = defineComponent({
  components: { ApplicationDialog },
  setup() {
    const visible = ref(true)
    return { visible }
  },
  template: `
    <section class="application-window">
      <header class="window-header"><button data-ui="window-trigger">Open</button></header>
      <main class="window-content">
        <ApplicationDialog
          :visible="visible"
          title="Window dialog"
          initial-focus-selector="[data-ui='initial-focus']"
          return-focus-selector="[data-ui='return-focus']"
          @close="visible = false"
        >
          <button data-ui="initial-focus">First</button>
          <button data-ui="last-focus">Last</button>
        </ApplicationDialog>
        <button data-ui="return-focus">Return</button>
      </main>
    </section>
  `,
})

describe('ApplicationDialog', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
  })

  it('将遮罩挂载到最近的窗口内容区且不覆盖标题栏', async () => {
    mount(Harness, mountOptions)
    await nextTick()

    const content = document.querySelector('.window-content')!
    const header = document.querySelector('.window-header')!
    const overlay = document.querySelector('[data-ui="application-dialog-overlay"]')!

    expect(overlay.closest('.window-content')).toBe(content)
    expect(header.contains(overlay)).toBe(false)
    expect(Array.from(document.body.children)).not.toContain(overlay)
    expect((content as HTMLElement).style.overflow).toBe('hidden')
  })

  it('处理初始焦点、Tab 循环、Escape 和焦点恢复', async () => {
    const wrapper = mount(Harness, mountOptions)
    await nextTick()

    const first = document.querySelector<HTMLElement>('[data-ui="initial-focus"]')!
    const last = document.querySelector<HTMLElement>('[data-ui="last-focus"]')!
    expect(document.activeElement).toBe(first)

    last.focus()
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }))
    expect(document.activeElement).toBe(document.querySelector('.application-dialog__close'))

    document.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Tab', shiftKey: true, bubbles: true }),
    )
    expect(document.activeElement).toBe(last)

    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    await nextTick()

    expect(document.querySelector('[data-ui="application-dialog-overlay"]')).toBeNull()
    expect(document.activeElement).toBe(
      wrapper.get<HTMLElement>('[data-ui="return-focus"]').element,
    )
    expect(wrapper.get<HTMLElement>('.window-content').element.style.overflow).toBe('')
  })

  it('禁用关闭时忽略 Escape、遮罩点击和关闭按钮', async () => {
    const wrapper = mount(ApplicationDialog, {
      ...mountOptions,
      props: {
        visible: true,
        title: 'Busy',
        closeDisabled: true,
      },
    })
    await nextTick()

    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    await wrapper.get('[data-ui="application-dialog-overlay"]').trigger('click')
    await wrapper.get('.application-dialog__close').trigger('click')

    expect(wrapper.emitted('close')).toBeUndefined()
  })

  it('多个窗口并存时只遮罩发起操作的内容区', async () => {
    const wrapper = mount(
      defineComponent({
        components: { ApplicationDialog },
        template: `
          <div>
            <section data-window="first">
              <header class="window-header">First</header>
              <main class="window-content"><ApplicationDialog visible title="Only first" /></main>
            </section>
            <section data-window="second">
              <header class="window-header">Second</header>
              <main class="window-content"></main>
            </section>
          </div>
        `,
      }),
      mountOptions,
    )
    await nextTick()

    expect(
      wrapper
        .get('[data-window="first"] .window-content')
        .findAll('[data-ui="application-dialog-overlay"]'),
    ).toHaveLength(1)
    expect(
      wrapper
        .get('[data-window="second"] .window-content')
        .findAll('[data-ui="application-dialog-overlay"]'),
    ).toHaveLength(0)
  })

  it('同一内容区存在多个对话框时按计数恢复滚动', async () => {
    const wrapper = mount(
      defineComponent({
        components: { ApplicationDialog },
        setup() {
          const firstVisible = ref(true)
          const secondVisible = ref(true)
          return { firstVisible, secondVisible }
        },
        template: `
          <main class="window-content">
            <ApplicationDialog :visible="firstVisible" title="First" />
            <ApplicationDialog :visible="secondVisible" title="Second" />
          </main>
        `,
      }),
      mountOptions,
    )
    await nextTick()

    const content = wrapper.get<HTMLElement>('.window-content')
    expect(content.element.style.overflow).toBe('hidden')
    ;(wrapper.vm as unknown as { secondVisible: boolean }).secondVisible = false
    await nextTick()
    expect(content.element.style.overflow).toBe('hidden')
    ;(wrapper.vm as unknown as { firstVisible: boolean }).firstVisible = false
    await nextTick()
    expect(content.element.style.overflow).toBe('')
  })
})
