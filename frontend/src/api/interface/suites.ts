export type SuiteInstanceStatus =
  | 'installing'
  | 'installed'
  | 'enabling'
  | 'enabled'
  | 'disabling'
  | 'disabled'
  | 'uninstalling'
  | 'error'
  | 'unknown'

export type SuiteInstallTaskStatus =
  | 'queued'
  | 'running'
  | 'canceling'
  | 'success'
  | 'failed'
  | 'canceled'
  | 'unknown'

export type SuiteInstallStep =
  | 'queued'
  | 'prepare'
  | 'pull_registry_image'
  | 'start_services'
  | 'completed'
  | 'canceling'
  | 'canceled'
  | 'failed'
  | 'recovery_failed'
  | 'unknown'

export interface SuiteCatalogItem {
  suiteId: string
  version: string
  name: string
  summary: string
  icon: string
  status: string
  checksum: string
  createdAt: string
  updatedAt: string
  instanceCount: number
  category?: string
}

export interface SuiteInstanceSummary {
  instanceId: string
  suiteId: string
  version: string
  nodeId: string
  composeProjectName: string
  status: SuiteInstanceStatus
  lastError?: string
  createdAt: string
  updatedAt: string
}

export interface SuiteListResponse {
  catalog: SuiteCatalogItem[]
  instances: SuiteInstanceSummary[]
}

export interface SuiteInstallTaskResponse {
  taskId: string
  instanceId: string
  suiteId: string
  nodeId: string
  progressPercent: number
  status: SuiteInstallTaskStatus
  currentStep: SuiteInstallStep
  currentImage?: string
  isFinished: boolean
  error?: string
  cancelRequested: boolean
  recoveryStartedAt?: string
  createdAt: string
  updatedAt: string
  finishedAt?: string
}

export type SuiteInstallProgress = SuiteInstallTaskResponse

export interface SuiteUninstallRequest {
  removeData: boolean
}
