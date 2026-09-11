import http from '@/api'

export interface UpgradeReleaseAsset {
  name: string
  downloadUrl: string
  size?: number
  contentType?: string
  component?: string
  targetTriple?: string
  sha256?: string
  signatureName?: string
  signatureStatus?: string
}

export interface UpgradeReleaseRecord {
  releaseId: string
  version: string
  tagName: string
  channel: string
  source: string
  releaseUrl: string
  assets: string
  checksumStatus: string
  signatureStatus: string
  supportedSuiteContractVersions: string
  upgradeEligible: boolean
  upgradeBlockReason?: string
  syncedAt: string
  publishedAt?: string
  createdAt: string
  updatedAt: string
}

export interface SuiteCompatibilityInstance {
  instanceId: string
  suiteId: string
  suiteName: string
  nodeId: string
  nodeName: string
  platformContractVersion: number
  status: 'compatible' | 'incompatible'
  reason: 'supported' | 'contractVersionNotSupported'
}

export interface SuiteCompatibilityReport {
  releaseVersion: string
  supportedSuiteContractVersions: number[]
  compatible: boolean
  summary: {
    total: number
    compatible: number
    incompatible: number
  }
  instances: SuiteCompatibilityInstance[]
}

export interface UpgradePlanRecord {
  planId: string
  targetVersion: string
  component: string
  scope: string
  strategy: string
  status: string
  requestedBy: string
  startedAt?: string
  finishedAt?: string
  createdAt: string
  updatedAt: string
}

export interface UpgradeTargetRecord {
  targetId: string
  planId: string
  targetType: string
  nodeId?: string
  currentVersion?: string
  targetVersion: string
  status: string
  subStatus?: string
  retryCount: number
  errorDetail?: string
  startedAt?: string
  finishedAt?: string
  createdAt: string
  updatedAt: string
}

export interface UpgradeEventRecord {
  eventId: string
  planId: string
  targetId?: string
  eventType: string
  message: string
  metadata: string
  createdAt: string
}

export interface UpgradePlanDetail {
  plan: UpgradePlanRecord
  targets: UpgradeTargetRecord[]
  events: UpgradeEventRecord[]
  nodeNames?: Record<string, string>
}

export interface UpgradePlanCreatePayload {
  targetVersion: string
  component?: 'cluster'
  nodeIds?: string[]
  includeOffline?: boolean
  overwriteSameVersion?: boolean
  maxConcurrency?: number
  failureThresholdPercent?: number
}

export const upgradesApi = {
  listReleases: () => http.get<UpgradeReleaseRecord[]>('/upgrades/releases/list'),
  syncReleases: () => http.post<UpgradeReleaseRecord[]>('/upgrades/releases/sync'),
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  uploadRelease: (payload: FormData, onProgress?: (progressEvent: any) => void) =>
    http.upload<UpgradeReleaseRecord>('/upgrades/releases/upload', payload, {
      headers: { 'Content-Type': 'multipart/form-data' },
      timeout: 300000,
      onUploadProgress: onProgress,
    }),
  createPlan: (payload: UpgradePlanCreatePayload) =>
    http.post<UpgradePlanDetail>('/upgrades/plan/create', payload),
  startPlan: (planId: string) => http.post<UpgradePlanDetail>(`/upgrades/plan/${planId}/start`),
  detail: (planId: string) => http.get<UpgradePlanDetail>(`/upgrades/plan/${planId}/detail`),
  latestPlan: () => http.get<UpgradePlanDetail>('/upgrades/plan/latest'),
  cancelPlan: (planId: string) => http.post<UpgradePlanDetail>(`/upgrades/plan/${planId}/cancel`),
  deleteRelease: (version: string) => http.delete<void>(`/upgrades/release/${version}/delete`),
  checkSuiteCompatibility: (version: string, nodeIds?: string[]) =>
    http.post<SuiteCompatibilityReport>(`/upgrades/release/${version}/compatibility/check`, {
      nodeIds,
    }),
}
