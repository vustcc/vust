import { defineComponent, nextTick, ref } from 'vue'
import { mount } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import DockerContentWorkspace from '../DockerContentWorkspace.vue'

const WorkspaceHarness = defineComponent({
  components: { DockerContentWorkspace },
  setup() {
    const visible = ref(false)
    return { visible }
  },
  template: `
    <button v-if="!visible" data-ui="workspace-trigger" @click="visible = true">打开</button>
    <DockerContentWorkspace
      v-else
      title="创建资源"
      back-label="返回列表"
      return-focus-selector="[data-ui='workspace-trigger']"
      @back="visible = false"
    />
  `,
})

describe('DockerContentWorkspace 键盘焦点', () => {
  it('进入时聚焦标题，返回时恢复入口焦点', async () => {
    const wrapper = mount(WorkspaceHarness, { attachTo: document.body })
    const trigger = wrapper.find('[data-ui="workspace-trigger"]')
    ;(trigger.element as HTMLButtonElement).focus()
    await trigger.trigger('click')
    await nextTick()

    expect(document.activeElement).toBe(wrapper.find('h2').element)
    await wrapper.find('[data-ui="workspace-header-back"]').trigger('click')
    await nextTick()
    expect(document.activeElement).toBe(wrapper.find('[data-ui="workspace-trigger"]').element)
    wrapper.unmount()
  })
})
