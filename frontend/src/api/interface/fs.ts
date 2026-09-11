export type { FileDocument, FileSaveResult, UpdateFileContentRequest } from '@/api/generated'

/** 文件条目类型。 */
export type FileEntryKind = 'file' | 'directory' | 'symlink' | 'other'

/** 文件路径的管理归属，仅用于事实提示。 */
export interface FileManagement {
  kind: 'custom' | 'compose' | 'suite' | 'system'
  ownerName?: string
  manageVia?: string
}

/** 最终执行端计算的文件能力。 */
export interface FileCapabilities {
  canOpen: boolean
  canRead: boolean
  canWrite: boolean
  canCreateChild: boolean
  canRename: boolean
  canCopy: boolean
  canRemove: boolean
  canUpload: boolean
  canDownload: boolean
}

/** 文件列表摘要。 */
export interface FsEntry {
  name: string
  path: string
  kind: FileEntryKind
  sizeBytes?: number
  modifiedAt?: string
  createdAt?: string
  revision: string
  management: FileManagement
  capabilities: FileCapabilities
}

export interface FileEntryCounts {
  fileCount: number
  directoryCount: number
  symlinkCount: number
  otherCount: number
}

export interface FileListPage {
  path: string
  entries: FsEntry[]
  page: number
  pageSize: number
  total: number
  counts: FileEntryCounts
  loadedAt: string
}

export interface HomeResponse {
  path: string
}

export type FileOperation = 'copy' | 'move' | 'remove'
export type FileTaskStatus =
  | 'queued'
  | 'running'
  | 'cancelling'
  | 'succeeded'
  | 'failed'
  | 'cancelled'

export interface FileOperationItemRequest {
  path: string
  expectedRevision?: string
  targetPath?: string
}

export interface CreateFileOperationTaskRequest {
  operation: FileOperation
  items: FileOperationItemRequest[]
  targetDirectory?: string
  recursive: boolean
  overwrite: false
  idempotencyKey: string
}

export interface FileOperationTask {
  taskId: string
  nodeId: string
  operation: FileOperation
  status: FileTaskStatus
  stage: string
  progressPercent: number
  totalItemCount: number
  completedItemCount: number
  failedItemCount: number
  totalBytes: number
  processedBytes: number
  items: Array<{
    path: string
    targetPath?: string
    status: 'pending' | 'running' | 'succeeded' | 'failed' | 'cancelled'
    errorCode?: string
    errorSummary?: string
  }>
  errorSummary?: string
  cleanupWarning?: string
  createdAt: string
  startedAt?: string
  finishedAt?: string
}

export type FileTransferDirection = 'upload' | 'download'
export type FileTransferStatus =
  | 'created'
  | 'receiving'
  | 'ready'
  | 'streaming'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'expired'

export interface CreateFileTransferRequest {
  direction: FileTransferDirection
  path: string
  sizeBytes?: number
  sha256?: string
  expectedRevision?: string
  overwrite: false
}

export interface FileTransfer {
  transferId: string
  nodeId: string
  direction: FileTransferDirection
  status: FileTransferStatus
  path: string
  sizeBytes: number
  transferredBytes: number
  revision?: string
  errorSummary?: string
  createdAt: string
  updatedAt: string
  expiresAt: string
}
