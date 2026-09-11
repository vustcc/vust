import http from '@/api'
import type * as suites from '../interface/suites'

export const suitesApi = {
  fetchSuites: (nodeId: string) => {
    return http.get<suites.SuiteListResponse>('/suites/list', { nodeId })
  },
  importSuite: (file: File) => {
    const formData = new FormData()
    formData.append('file', file)
    return http.upload<unknown>('/suites/import', formData, {
      headers: { 'Content-Type': 'multipart/form-data' },
      timeout: 300000,
    })
  },
  installSuite: (suiteId: string, nodeId: string) => {
    return http.post<suites.SuiteInstallTaskResponse>(
      `/suites/${encodeURIComponent(suiteId)}/install`,
      { nodeId },
    )
  },
  fetchInstallProgress: (taskId: string) => {
    return http.get<suites.SuiteInstallTaskResponse>(
      `/suite-install-tasks/${encodeURIComponent(taskId)}/progress`,
    )
  },
  fetchInstallTasks: (nodeId: string, activeOnly = true) => {
    return http.get<suites.SuiteInstallTaskResponse[]>('/suite-install-tasks/list', {
      nodeId,
      activeOnly,
    })
  },
  cancelInstall: (taskId: string) => {
    return http.post<suites.SuiteInstallTaskResponse>(
      `/suite-install-tasks/${encodeURIComponent(taskId)}/cancel`,
    )
  },
  deleteSuite: (suiteId: string) => {
    return http.delete<unknown>(`/suites/${encodeURIComponent(suiteId)}`)
  },
  enableInstance: (instanceId: string) => {
    return http.post<suites.SuiteInstanceSummary>(
      `/suite-instances/${encodeURIComponent(instanceId)}/enable`,
      undefined,
      { timeout: 300000 },
    )
  },
  disableInstance: (instanceId: string) => {
    return http.post<suites.SuiteInstanceSummary>(
      `/suite-instances/${encodeURIComponent(instanceId)}/disable`,
      undefined,
      { timeout: 300000 },
    )
  },
  uninstallInstance: (instanceId: string, payload: suites.SuiteUninstallRequest) => {
    return http.post<unknown>(
      `/suite-instances/${encodeURIComponent(instanceId)}/uninstall`,
      payload,
      {
        timeout: 300000,
      },
    )
  },
}
