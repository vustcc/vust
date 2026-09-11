import { defineStore } from 'pinia'
import { ref } from 'vue'

export interface ConfirmationModalData {
  title: string
  message: string
  confirmText: string
  cancelText: string
  type: 'primary' | 'danger'
  isVisible: boolean
  resolve: ((confirmed: boolean) => void) | null // 用于解决 Promise 的函数
}

export const useConfirmationModalStore = defineStore('confirmationModal', () => {
  const modalData = ref<ConfirmationModalData>({
    title: '',
    message: '',
    confirmText: '确认',
    cancelText: '取消',
    type: 'primary',
    isVisible: false,
    resolve: null,
  })

  /**
   * @description 显示确认弹窗，并返回一个 Promise，用于等待用户操作。
   * @param message 弹窗的主要消息内容。
   * @param title 弹窗标题 (可选, 默认 '确认操作')。
   * @param confirmText 确认按钮文本 (可选, 默认 '确认')。
   * @param cancelText 取消按钮文本 (可选, 默认 '取消')。
   * @param type 确认操作类型 (可选, 默认 'primary')。
   * @returns Promise<boolean> 如果用户点击确认，返回 true；否则返回 false。
   */
  function showConfirmation(
    message: string,
    title: string = '确认操作',
    confirmText: string = '确认',
    cancelText: string = '取消',
    type: 'primary' | 'danger' = 'primary',
  ): Promise<boolean> {
    return new Promise((resolve) => {
      modalData.value = {
        title,
        message,
        confirmText,
        cancelText,
        type,
        isVisible: true,
        resolve: resolve as (confirmed: boolean) => void,
      }
    })
  }

  /**
   * @description 关闭弹窗并处理 Promise 结果。
   * @param confirmed 用户是否确认。
   */
  function handleModalResponse(confirmed: boolean) {
    if (modalData.value.resolve) {
      modalData.value.resolve(confirmed)
    }
    // 重置状态
    modalData.value.isVisible = false
    modalData.value.resolve = null
  }

  return {
    modalData,
    showConfirmation,
    handleModalResponse,
  }
})
