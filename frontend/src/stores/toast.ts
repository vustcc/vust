import { defineStore } from 'pinia'
import { ref } from 'vue'

/** 瞬时反馈类型；不会写入个人通知中心。 */
export type ToastType = 'success' | 'error' | 'warning' | 'info'

/** 当前正在显示的瞬时反馈。 */
export interface ToastMessage {
  id: number
  type: ToastType
  title?: string
  message: string
  timestamp: number
  duration: number
}

const DEFAULT_DURATION = 4000

/** 纯内存 Toast 状态，只负责当前页面的即时反馈。 */
export const useToastStore = defineStore('toast', () => {
  const activeNotifications = ref<ToastMessage[]>([])
  const nextId = ref(1)

  /** 添加一条瞬时反馈，并在持续时间结束后移除。 */
  function addNotification(
    type: ToastType,
    message: string,
    duration: number = DEFAULT_DURATION,
    title?: string,
  ) {
    const toast: ToastMessage = {
      id: nextId.value++,
      type,
      title,
      message,
      timestamp: Date.now(),
      duration,
    }
    activeNotifications.value.push(toast)
    window.setTimeout(() => removeNotification(toast.id), duration)
  }

  /** 移除指定瞬时反馈。 */
  function removeNotification(id: number) {
    activeNotifications.value = activeNotifications.value.filter((item) => item.id !== id)
  }

  const success = (message: string, duration?: number) =>
    addNotification('success', message, duration)
  const error = (message: string, duration?: number) => addNotification('error', message, duration)
  const warning = (message: string, duration?: number) =>
    addNotification('warning', message, duration)
  const info = (message: string, duration?: number) => addNotification('info', message, duration)
  const showToast = (type: ToastType, message: string, duration?: number, title?: string) =>
    addNotification(type, message, duration, title)

  return {
    activeNotifications,
    addNotification,
    removeNotification,
    success,
    error,
    warning,
    info,
    showToast,
  }
})
