<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue'
import { VustGlassSurface } from '@/components/ui'

// 负责渲染桌面应用的右键菜单容器，并处理点击外部关闭。
const props = withDefaults(
  defineProps<{
    /**
     * 菜单是否可见
     */
    visible: boolean
    /**
     * 菜单左上角 X 坐标
     */
    x: number
    /**
     * 菜单左上角 Y 坐标
     */
    y: number
    /**
     * 菜单层级
     */
    zIndex?: number
  }>(),
  {
    zIndex: 21020,
  },
)

const emit = defineEmits<{ close: [] }>()

const menuRef = ref<HTMLElement | null>(null)

function handleOutside(event: MouseEvent) {
  const target = event.target as Node | null
  if (!props.visible) return
  if (target && menuRef.value && !menuRef.value.contains(target)) {
    emit('close')
  }
}

onMounted(() => {
  window.addEventListener('mousedown', handleOutside)
})

onUnmounted(() => {
  window.removeEventListener('mousedown', handleOutside)
})
</script>

<template>
  <Teleport to="body">
    <div
      v-if="visible"
      ref="menuRef"
      class="app-context-menu"
      :style="{ top: `${y}px`, left: `${x}px`, zIndex }"
      @click.stop
    >
      <VustGlassSurface class="app-context-menu-inner" data-ui="app-context-menu">
        <slot />
      </VustGlassSurface>
    </div>
  </Teleport>
</template>

<style scoped>
.app-context-menu {
  position: fixed;
}

.app-context-menu-inner {
  border-color: var(--vdl-glass-border);
  border-radius: var(--vdl-radius-md);
  padding: var(--vdl-space-2);
  box-shadow: var(--vdl-glass-shadow);
  max-width: 260px;
  width: max-content;
  display: inline-flex;
  flex-direction: column;
  gap: var(--vdl-space-1);
}

:deep(.context-menu-btn) {
  width: 100%;
  display: block;
  background: none;
  border: none;
  color: var(--vdl-text-primary);
  padding: var(--vdl-space-2) var(--vdl-space-3);
  text-align: left;
  font-size: var(--vdl-font-body-sm);
  border-radius: var(--vdl-radius-xs);
  cursor: pointer;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  transition: background-color 0.2s;
}

:deep(.context-menu-btn:disabled) {
  opacity: 0.4;
  cursor: not-allowed;
}

:deep(.context-menu-btn:not(:disabled):hover) {
  background-color: var(--vdl-glass-control-tint);
}
</style>
