<script setup lang="ts">
import { RouterView } from 'vue-router'
import { useI18n } from 'vue-i18n'
import VustToast from './components/ui/VustToast.vue'
import VustModal from './components/ui/VustModal.vue'
import { VustGlassProvider } from '@vustcc/vue'
import { useToastStore } from './stores/toast'
import { useConfirmationModalStore } from './stores/confirmation-modal'
import { useThemeStore } from './stores/theme'

const { t } = useI18n()
const toastStore = useToastStore()
const modalStore = useConfirmationModalStore()
const themeStore = useThemeStore()
</script>

<template>
  <VustGlassProvider :glass="themeStore.glassEnabled">
    <Suspense>
      <template #default>
        <RouterView />
      </template>
      <template #fallback>
        <div class="app-loading">Loading...</div>
      </template>
    </Suspense>

    <!-- 全局 UI 组件 -->
    <VustToast
      :toasts="
        toastStore.activeNotifications.map((n) => ({
          id: String(n.id),
          type: n.type,
          title: n.title || t(`notification.title.${n.type}`),
          message: n.message,
        }))
      "
      @close="(id) => toastStore.removeNotification(Number(id))"
    />
    <VustModal
      :visible="modalStore.modalData.isVisible"
      :title="modalStore.modalData.title"
      :message="modalStore.modalData.message"
      :confirm-text="modalStore.modalData.confirmText"
      :cancel-text="modalStore.modalData.cancelText"
      :type="modalStore.modalData.type"
      data-ui="global-confirmation-modal"
      style="z-index: calc(var(--vdl-z-index-modal) + 10)"
      @confirm="modalStore.handleModalResponse(true)"
      @cancel="modalStore.handleModalResponse(false)"
    />
  </VustGlassProvider>
</template>

<style>
html,
body {
  margin: 0;
  padding: 0;
  width: 100%;
  height: 100%;
  overflow: hidden; /* 隐藏 body 级别的滚动条，交给子组件控制 */
  box-sizing: border-box; /* 使用 border-box 确保 padding 不会增加尺寸 */
}
#app {
  width: 100%;
  height: 100%;
}
.app-loading {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--vdl-text-primary);
  font-size: var(--vdl-font-body);
}
</style>
