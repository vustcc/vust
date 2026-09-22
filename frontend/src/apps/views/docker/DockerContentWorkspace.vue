<script setup lang="ts">
/** Docker 资源创建与长流程使用的窗口内容区工作台。 */
import { nextTick, onBeforeUnmount, onMounted, ref } from 'vue'
import { VustButton } from '@/components/ui'

const props = withDefaults(
  defineProps<{
    title: string
    backLabel?: string
    backDisabled?: boolean
    returnFocusSelector?: string
  }>(),
  {
    backLabel: '',
    backDisabled: false,
    returnFocusSelector: '',
  },
)

defineEmits<{
  (event: 'back'): void
}>()

const titleElement = ref<HTMLHeadingElement | null>(null)

/** 工作台进入内容区后将键盘焦点移动到标题。 */
onMounted(() => {
  titleElement.value?.focus({ preventScroll: true })
})

/** 工作台离开后将焦点恢复到重新渲染的入口操作。 */
onBeforeUnmount(() => {
  if (!props.returnFocusSelector) return
  void nextTick(() => {
    document.querySelector<HTMLElement>(props.returnFocusSelector)?.focus({ preventScroll: true })
  })
})
</script>

<template>
  <section class="docker-content-workspace" data-ui="docker-content-workspace">
    <header class="workspace-header" data-slot="header">
      <VustButton
        v-if="backLabel"
        type="secondary"
        :disabled="backDisabled"
        data-ui="workspace-header-back"
        @click="$emit('back')"
      >
        {{ backLabel }}
      </VustButton>
      <h2 ref="titleElement" tabindex="-1">{{ title }}</h2>
    </header>
    <div class="workspace-body" data-slot="body">
      <slot />
    </div>
    <footer v-if="$slots.footer" class="workspace-footer" data-slot="footer">
      <slot name="footer" />
    </footer>
  </section>
</template>

<style scoped>
.docker-content-workspace {
  display: flex;
  flex: 1;
  min-height: 0;
  flex-direction: column;
  overflow: hidden;
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-panel);
}
.workspace-header,
.workspace-footer {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-3) var(--vdl-space-4);
}
.workspace-header {
  border-bottom: 1px solid var(--vdl-border-subtle);
}
.workspace-header h2 {
  margin: 0;
  color: var(--vdl-text-primary);
  font-size: var(--vdl-font-subtitle);
}
.workspace-body {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: var(--vdl-space-4);
  scrollbar-gutter: stable;
}
.workspace-footer {
  justify-content: flex-end;
  border-top: 1px solid var(--vdl-border-subtle);
}
@media (max-width: 860px) {
  .workspace-header {
    align-items: flex-start;
    flex-direction: column;
  }
}
</style>
