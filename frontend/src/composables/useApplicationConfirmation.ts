import { onBeforeUnmount, ref } from 'vue'

export type ApplicationConfirmationType = 'primary' | 'danger'

/** 窗口级确认框的完整展示状态。 */
export interface ApplicationConfirmationState {
  /** 是否显示确认框。 */
  visible: boolean
  /** 确认框标题。 */
  title: string
  /** 确认提示正文。 */
  message: string
  /** 确认按钮文案。 */
  confirmText: string
  /** 取消按钮文案。 */
  cancelText: string
  /** 确认按钮的视觉类型。 */
  type: ApplicationConfirmationType
}

const initialState = (): ApplicationConfirmationState => ({
  visible: false,
  title: '',
  message: '',
  confirmText: '',
  cancelText: '',
  type: 'primary',
})

/**
 * 提供应用窗口级的 Promise 式确认流程。
 *
 * 组合式函数只管理确认状态和 Promise，不负责渲染。调用方需要在当前应用模板中挂载
 * `ApplicationConfirmationDialog`，并把其确认、取消事件交给
 * `handleConfirmationResponse`。组件卸载时，尚未完成的确认会自动按取消处理。
 *
 * @example
 * ```ts
 * const { confirmationState, showConfirmation, handleConfirmationResponse } =
 *   useApplicationConfirmation()
 *
 * const confirmed = await showConfirmation(
 *   '删除后无法恢复，是否继续？',
 *   '确认删除',
 *   '删除',
 *   '取消',
 *   'danger',
 * )
 * if (confirmed) await removeItem()
 * ```
 */
export function useApplicationConfirmation() {
  const confirmationState = ref<ApplicationConfirmationState>(initialState())
  let resolveCurrent: ((confirmed: boolean) => void) | null = null

  /**
   * 打开确认框并等待用户选择。
   *
   * 若已有等待中的确认，会先将旧确认按取消处理，再展示新确认。
   *
   * @returns 用户确认时返回 `true`，取消、关闭或组件卸载时返回 `false`。
   */
  function showConfirmation(
    message: string,
    title: string,
    confirmText: string,
    cancelText: string,
    type: ApplicationConfirmationType = 'primary',
  ) {
    resolveCurrent?.(false)

    confirmationState.value = {
      visible: true,
      title,
      message,
      confirmText,
      cancelText,
      type,
    }

    return new Promise<boolean>((resolve) => {
      resolveCurrent = resolve
    })
  }

  /** 根据确认组件的事件结果完成当前 Promise，并清理显示状态。 */
  function handleConfirmationResponse(confirmed: boolean) {
    const resolve = resolveCurrent
    resolveCurrent = null
    confirmationState.value = initialState()
    resolve?.(confirmed)
  }

  onBeforeUnmount(() => {
    resolveCurrent?.(false)
    resolveCurrent = null
  })

  return {
    confirmationState,
    showConfirmation,
    handleConfirmationResponse,
  }
}
