import axios from 'axios'
import {
  type AxiosInstance,
  type AxiosError,
  type AxiosRequestConfig,
  type AxiosResponse,
  HttpStatusCode,
} from 'axios'
import type { ApiResponse } from '@/api/interface'
import { handleAxiosError } from './error'
import { getLoginEntryPath } from '@/utils/login-entry'

const config = {
  baseURL: '/api/v1',
  timeout: 30000,
  withCredentials: true,
}

/**
 * @description HTTP 请求服务类
 */
class HttpClient {
  service: AxiosInstance
  private redirectingToLogin = false
  public constructor(config: AxiosRequestConfig) {
    this.service = axios.create(config)

    this.service.interceptors.request.use(
      (config) => {
        // 后端根据 Accept-Language 解析套件清单文案，前端仍只维护 zh/en 两个状态。
        const locale = localStorage.getItem('vust_locale') === 'en' ? 'en-US' : 'zh-CN'
        config.headers.set('Accept-Language', locale)
        if (config.url) {
          if (
            config.url.includes('/nodes/deploy') ||
            config.url.includes('/nodes/precheck') ||
            config.url.includes('/upgrade') ||
            config.url.includes('/repair')
          ) {
            config.timeout = 300000
          }
        }
        return config
      },
      (error) => {
        return Promise.reject(error)
      },
    )

    this.service.interceptors.response.use(
      (response: AxiosResponse) => {
        // 返回 ApiResponse
        return response.data
      },
      (error: AxiosError) => {
        if (axios.isAxiosError(error)) {
          const { response, config } = error

          if (response && response.status === HttpStatusCode.Unauthorized && config) {
            const requestUrl = config.url || ''
            const isAuthEndpoint =
              requestUrl.includes('/auth/login') || requestUrl.includes('/auth/me')
            if (!isAuthEndpoint) {
              this.handleUnauthorized()
            }
            return handleAxiosError(error)
          }

          if (response && response.status === 423) {
            if (window.location.pathname !== '/upgrade-progress') {
              const planId = window.sessionStorage.getItem('vust.activeUpgradePlanId')
              const target = planId
                ? `/upgrade-progress?planId=${encodeURIComponent(planId)}`
                : '/upgrade-progress'
              window.location.assign(target)
            }
            return response.data
          }

          if (response) {
            return handleAxiosError(error)
          }
        }

        // 返回一个拒绝的 Promise，以便在调用处捕获错误
        return Promise.reject(error)
      },
    )
  }

  private handleUnauthorized() {
    const loginPath = getLoginEntryPath()
    if (this.redirectingToLogin || window.location.pathname === loginPath) return
    this.redirectingToLogin = true
    window.location.assign(loginPath)
  }

  get<T>(url: string, params?: object, _object = {}): Promise<ApiResponse<T>> {
    return this.service.get(url, { params, ..._object })
  }
  post<T>(url: string, params?: object, config?: AxiosRequestConfig): Promise<ApiResponse<T>> {
    return this.service.post(url, params, config)
  }
  put<T>(url: string, params?: object, _object = {}): Promise<ApiResponse<T>> {
    return this.service.put(url, params, _object)
  }
  patch<T>(url: string, params?: object, _object = {}): Promise<ApiResponse<T>> {
    return this.service.patch(url, params, _object)
  }
  delete<T>(url: string, params?: object, _object = {}): Promise<ApiResponse<T>> {
    return this.service.delete(url, { params, ..._object })
  }
  download(url: string, params?: object, _object = {}): Promise<Blob> {
    return this.service.post<Blob, Blob>(url, params, _object)
  }
  upload<T>(
    url: string,
    params: object = {},
    config?: AxiosRequestConfig,
  ): Promise<ApiResponse<T>> {
    return this.service.post(url, params, config)
  }
}

export default new HttpClient(config)
