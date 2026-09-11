import type { AxiosError } from 'axios'
import type { ApiResponse } from './interface'
import i18n from '@/i18n'

export function handleAxiosError(error: AxiosError): ApiResponse<null> {
  const status = error.response?.status ?? -1
  const responseData = error.response?.data as Partial<ApiResponse<unknown>> | undefined
  let message: string | undefined
  const errorCode = responseData?.errorCode
  const messageKey = responseData?.messageKey

  if (messageKey && typeof messageKey === 'string' && i18n.global.te(messageKey)) {
    message = i18n.global.t(messageKey)
  }

  const errorCodeKey = errorCode ? `api.errors.${errorCode}` : ''
  if (!message && errorCodeKey && i18n.global.te(errorCodeKey)) {
    message = i18n.global.t(errorCodeKey)
  }

  if (!message && responseData) {
    if (typeof responseData.message === 'string') {
      message = responseData.message
    } else if (typeof responseData.data === 'string') {
      message = responseData.data
    }
  }

  if (message && i18n.global.te(message)) {
    message = i18n.global.t(message)
  }

  if (!message) {
    switch (status) {
      case 500:
        message = i18n.global.t('api.errors.controlServiceUnavailable')
        break
      default:
        message = error.message ?? i18n.global.t('common.unknownError')
        break
    }
  }
  return {
    success: false,
    code: status,
    message,
    messageKey,
    errorCode,
    data: (responseData?.data ?? null) as null,
  }
}
