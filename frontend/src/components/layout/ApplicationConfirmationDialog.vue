<script setup lang="ts">
import ApplicationDialog from './ApplicationDialog.vue'
import { VustButton } from '@/components/ui'

/**
 * 基于 `ApplicationDialog` 的窗口级确认框。
 *
 * 适合应用内部的删除、覆盖等需要用户确认的操作。默认先聚焦取消按钮，危险操作可通过
 * `type="danger"` 呈现。通常与 `useApplicationConfirmation` 配合使用。
 *
 * @example
 * ```vue
 * <ApplicationConfirmationDialog
 *   :visible="confirmationState.visible"
 *   :title="confirmationState.title"
 *   :message="confirmationState.message"
 *   :confirm-text="confirmationState.confirmText"
 *   :cancel-text="confirmationState.cancelText"
 *   :type="confirmationState.type"
 *   @confirm="handleConfirmationResponse(true)"
 *   @cancel="handleConfirmationResponse(false)"
 * />
 * ```
 */
defineOptions({ name: 'ApplicationConfirmationDialog' })

interface Props {
  /** 是否显示确认框。 */
  visible: boolean
  /** 确认框标题。 */
  title: string
  /** 需要用户确认的正文信息。 */
  message: string
  /** 确认按钮文案。 */
  confirmText: string
  /** 取消按钮文案。 */
  cancelText: string
  /** 确认按钮类型，危险操作使用 `danger`。 */
  type?: 'primary' | 'danger'
  /** 关闭后用于恢复焦点的宿主内容区元素选择器。 */
  returnFocusSelector?: string
  /** 透传给内部对话框的稳定 `data-ui` 标记。 */
  dialogUi?: string
}

withDefaults(defineProps<Props>(), {
  type: 'primary',
  returnFocusSelector: '',
  dialogUi: 'application-confirmation-dialog',
})

const emit = defineEmits<{
  /** 用户确认操作。 */
  confirm: []
  /** 用户取消或关闭确认框。 */
  cancel: []
}>()
</script>

<template>
  <ApplicationDialog
    :visible="visible"
    :title="title"
    width="440px"
    :return-focus-selector="returnFocusSelector"
    initial-focus-selector="[data-ui='application-confirmation-cancel']"
    :data-ui="dialogUi"
    @close="emit('cancel')"
  >
    <p class="application-confirmation-dialog__message">{{ message }}</p>

    <template #footer>
      <VustButton
        type="secondary"
        data-ui="application-confirmation-cancel"
        @click="emit('cancel')"
        >{{ cancelText }}</VustButton
      >
      <VustButton :type="type" @click="emit('confirm')">{{ confirmText }}</VustButton>
    </template>
  </ApplicationDialog>
</template>

<style scoped>
.application-confirmation-dialog__message {
  margin: 0;
  color: var(--vdl-text-secondary);
  line-height: 1.6;
  white-space: pre-line;
}
</style>
