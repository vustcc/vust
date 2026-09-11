/**
 * @file useFileOperations.ts
 * @description 封装固定节点上的同步文件操作、持久任务与分块传输。
 */
import { computed, reactive, type Ref } from 'vue'
import { fsApi } from '@/api/modules/fs'
import type { FileOperation, FileOperationItemRequest, FileOperationTask } from '@/api/interface/fs'
import { useToastStore } from '@/stores/toast'
import { useI18n } from 'vue-i18n'

const UPLOAD_CHUNK_BYTES = 8 * 1024 * 1024
const TASK_POLL_INTERVAL_MS = 500
const TERMINAL_TASK_STATUSES = new Set(['succeeded', 'failed', 'cancelled'])
const TERMINAL_TRANSFER_STATUSES = new Set(['completed', 'failed', 'cancelled', 'expired'])

export type FileUploadTaskStatus =
  | 'preparing'
  | 'uploading'
  | 'cancelling'
  | 'completed'
  | 'partial'
  | 'failed'
  | 'cancelled'

export interface FileUploadSource {
  file: File
  relativePath: string
}

export interface FileUploadSelection {
  kind: 'files' | 'folder'
  displayName: string
  files: FileUploadSource[]
}

export interface FileUploadFailure {
  path: string
  message: string
}

export interface FileUploadTaskState {
  visible: boolean
  kind: FileUploadSelection['kind']
  displayName: string
  targetDirectory: string
  status: FileUploadTaskStatus
  totalFiles: number
  completedFiles: number
  failedFiles: number
  totalBytes: number
  transferredBytes: number
  progressPercent: number
  currentFile: string
  failures: FileUploadFailure[]
  errorSummary: string
}

const ACTIVE_UPLOAD_STATUSES = new Set<FileUploadTaskStatus>([
  'preparing',
  'uploading',
  'cancelling',
])

const newIdempotencyKey = () =>
  globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random().toString(36).slice(2)}`

const wait = (delayMs: number) => new Promise((resolve) => window.setTimeout(resolve, delayMs))

/** 拼接服务器绝对路径，避免重复分隔符。 */
const joinAbsolutePath = (directory: string, relativePath: string) => {
  const root = directory === '/' ? '' : directory.replace(/\/+$/, '')
  return `${root}/${relativePath}`.replace(/\/{2,}/g, '/')
}

/** 校验浏览器提供的相对路径，禁止目录逃逸和平台分隔符混用。 */
const normalizeRelativePath = (path: string) => {
  if (!path || path.includes('\\') || path.includes('\0') || path.startsWith('/')) {
    throw new Error('invalid upload relative path')
  }
  const segments = path.split('/')
  if (segments.some((segment) => !segment || segment === '.' || segment === '..')) {
    throw new Error('invalid upload relative path')
  }
  return segments.join('/')
}

export function useFileOperations(nodeId: Readonly<Ref<string>>) {
  const toastStore = useToastStore()
  const { t } = useI18n()
  const fsClient = computed(() => fsApi.forNode(nodeId.value))
  const uploadTask = reactive<FileUploadTaskState>({
    visible: false,
    kind: 'files',
    displayName: '',
    targetDirectory: '',
    status: 'completed',
    totalFiles: 0,
    completedFiles: 0,
    failedFiles: 0,
    totalBytes: 0,
    transferredBytes: 0,
    progressPercent: 0,
    currentFile: '',
    failures: [],
    errorSummary: '',
  })
  const uploadActive = computed(
    () => uploadTask.visible && ACTIVE_UPLOAD_STATUSES.has(uploadTask.status),
  )
  let uploadCancelRequested = false
  let uploadAbortController: AbortController | null = null
  let currentTransferId = ''
  let currentTransferClient: ReturnType<(typeof fsApi)['forNode']> | null = null
  let currentCancelPromise: Promise<void> | null = null

  const createFile = async (path: string, content = '') => {
    const res = await fsClient.value.createFile({ path, content })
    if (!res.success) toastStore.error(res.message || t('app.fileManager.writeFailed'))
    return res.success
  }

  const mkdir = async (path: string, recursive = false) => {
    const res = await fsClient.value.mkdir({ path, recursive })
    if (!res.success) toastStore.error(res.message || t('app.fileManager.mkdirFailed'))
    return res.success
  }

  /** 串行等待同一任务，确保轮询不会重叠。 */
  const pollTask = async (task: FileOperationTask) => {
    let current = task
    while (!TERMINAL_TASK_STATUSES.has(current.status)) {
      await wait(TASK_POLL_INTERVAL_MS)
      const res = await fsClient.value.taskDetail(current.taskId)
      if (!res.success || !res.data) {
        toastStore.error(res.message || t('app.fileManager.fetchError'))
        return false
      }
      current = res.data
    }
    if (current.status !== 'succeeded') {
      toastStore.error(current.errorSummary || t('app.fileManager.fetchError'))
      return false
    }
    return true
  }

  const runPathTask = async (
    operation: FileOperation,
    items: FileOperationItemRequest[],
    recursive = false,
    targetDirectory?: string,
  ) => {
    const res = await fsClient.value.createTask({
      operation,
      items,
      targetDirectory,
      recursive,
      overwrite: false,
      idempotencyKey: newIdempotencyKey(),
    })
    if (!res.success || !res.data) {
      toastStore.error(res.message || t('app.fileManager.fetchError'))
      return false
    }
    return pollTask(res.data)
  }

  const removePath = (path: string, recursive = false, expectedRevision?: string) =>
    runPathTask('remove', [{ path, expectedRevision }], recursive)

  const renamePath = (from: string, to: string, expectedRevision?: string) =>
    runPathTask('move', [{ path: from, targetPath: to, expectedRevision }], true)

  const copyPath = (from: string, to: string, expectedRevision?: string) =>
    runPathTask('copy', [{ path: from, targetPath: to, expectedRevision }], true)

  /** 串行跟踪单个传输，避免同一传输产生重叠轮询。 */
  const pollTransfer = async (transferId: string) => {
    while (true) {
      await wait(TASK_POLL_INTERVAL_MS)
      const res = await fsClient.value.transferDetail(transferId)
      if (!res.success || !res.data) {
        toastStore.error(res.message || t('app.fileManager.fetchError'))
        return false
      }
      if (!TERMINAL_TRANSFER_STATUSES.has(res.data.status)) continue
      if (res.data.status !== 'completed') {
        toastStore.error(res.data.errorSummary || t('app.fileManager.downloadFailed'))
        return false
      }
      return true
    }
  }

  const downloadFile = async (path: string, fileName?: string, expectedRevision?: string) => {
    const client = fsClient.value
    const res = await client.createTransfer({
      direction: 'download',
      path,
      expectedRevision,
      overwrite: false,
    })
    if (!res.success || !res.data) {
      toastStore.error(res.message || t('app.fileManager.downloadFailed'))
      return false
    }
    const link = document.createElement('a')
    link.href = client.downloadUrl(res.data.transferId)
    link.download = fileName || path.split('/').pop() || 'download'
    document.body.appendChild(link)
    link.click()
    link.remove()
    return pollTransfer(res.data.transferId)
  }

  /** 计算并写入上传任务的整体进度。 */
  const updateUploadProgress = (
    settledWorkBytes: number,
    currentWorkBytes: number,
    actualTransferredBytes: number,
  ) => {
    uploadTask.transferredBytes = Math.min(uploadTask.totalBytes, actualTransferredBytes)
    if (uploadTask.totalBytes > 0) {
      uploadTask.progressPercent = Math.min(
        100,
        Math.round(((settledWorkBytes + currentWorkBytes) / uploadTask.totalBytes) * 100),
      )
      return
    }
    const processedFiles = uploadTask.completedFiles + uploadTask.failedFiles
    uploadTask.progressPercent = uploadTask.totalFiles
      ? Math.round((processedFiles / uploadTask.totalFiles) * 100)
      : 0
  }

  /** 取消当前服务端传输；并发调用复用同一请求。 */
  const cancelCurrentTransfer = async () => {
    if (!currentTransferId || !currentTransferClient) return
    if (currentCancelPromise) return currentCancelPromise
    const transferId = currentTransferId
    const client = currentTransferClient
    currentCancelPromise = client
      .cancelTransfer(transferId)
      .then(() => undefined)
      .catch(() => undefined)
      .finally(() => {
        if (currentTransferId === transferId) currentTransferId = ''
        currentCancelPromise = null
      })
    return currentCancelPromise
  }

  /** 上传一个文件并报告本文件的实际传输字节。 */
  const uploadOneFile = async (
    client: ReturnType<(typeof fsApi)['forNode']>,
    targetPath: string,
    file: File,
    onProgress: (uploadedBytes: number) => void,
  ) => {
    const active = await client.activeTransfers()
    if (!active.success) {
      throw new Error(active.message || t('app.fileManager.uploadFailed'))
    }
    const resumable = active.data?.find(
      (transfer) =>
        transfer.direction === 'upload' &&
        transfer.path === targetPath &&
        transfer.sizeBytes === file.size &&
        !TERMINAL_TRANSFER_STATUSES.has(transfer.status),
    )
    const created = resumable
      ? { success: true, data: resumable, message: '' }
      : await client.createTransfer({
          direction: 'upload',
          path: targetPath,
          sizeBytes: file.size,
          overwrite: false,
        })
    if (!created.success || !created.data) {
      throw new Error(created.message || t('app.fileManager.uploadFailed'))
    }

    currentTransferId = created.data.transferId
    currentTransferClient = client
    let uploadedBytes = created.data.transferredBytes
    onProgress(uploadedBytes)
    if (uploadCancelRequested) {
      await cancelCurrentTransfer()
      return { completed: false, uploadedBytes }
    }

    try {
      for (let start = uploadedBytes; start < file.size; start += UPLOAD_CHUNK_BYTES) {
        if (uploadCancelRequested) break
        const endExclusive = Math.min(start + UPLOAD_CHUNK_BYTES, file.size)
        const chunk = await file.slice(start, endExclusive).arrayBuffer()
        if (uploadCancelRequested) break
        const controller = new AbortController()
        uploadAbortController = controller
        const uploaded = await client.uploadChunk(
          currentTransferId,
          chunk,
          start,
          endExclusive - 1,
          file.size,
          {
            signal: controller.signal,
            onProgress: (loadedBytes) => {
              uploadedBytes = Math.max(uploadedBytes, Math.min(endExclusive, start + loadedBytes))
              onProgress(uploadedBytes)
            },
          },
        )
        uploadAbortController = null
        if (!uploaded.success || !uploaded.data) {
          throw new Error(uploaded.message || t('app.fileManager.uploadFailed'))
        }
        uploadedBytes = uploaded.data.transferredBytes
        onProgress(uploadedBytes)
      }
      if (uploadCancelRequested) {
        await cancelCurrentTransfer()
        return { completed: false, uploadedBytes }
      }
      const completed = await client.completeTransfer(currentTransferId)
      if (!completed.success) {
        throw new Error(completed.message || t('app.fileManager.uploadFailed'))
      }
      currentTransferId = ''
      onProgress(file.size)
      return { completed: true, uploadedBytes: file.size }
    } catch (error) {
      uploadAbortController = null
      await cancelCurrentTransfer()
      if (uploadCancelRequested) return { completed: false, uploadedBytes }
      throw error
    }
  }

  /** 启动当前窗口内唯一的后台上传任务。 */
  const startUpload = async (targetDirectory: string, selection: FileUploadSelection) => {
    if (uploadActive.value || selection.files.length === 0) return false
    let files: FileUploadSource[]
    try {
      files = selection.files.map((source) => ({
        file: source.file,
        relativePath: normalizeRelativePath(source.relativePath),
      }))
    } catch {
      toastStore.error(t('app.fileManager.uploadInvalidPath'))
      return false
    }

    Object.assign(uploadTask, {
      visible: true,
      kind: selection.kind,
      displayName: selection.displayName,
      targetDirectory,
      status: 'preparing' as FileUploadTaskStatus,
      totalFiles: files.length,
      completedFiles: 0,
      failedFiles: 0,
      totalBytes: files.reduce((total, source) => total + source.file.size, 0),
      transferredBytes: 0,
      progressPercent: 0,
      currentFile: '',
      failures: [],
      errorSummary: '',
    })
    uploadCancelRequested = false
    const client = fsClient.value
    currentTransferClient = client
    let settledWorkBytes = 0
    let actualTransferredBytes = 0

    try {
      if (selection.kind === 'folder') {
        const directories = new Set<string>()
        files.forEach(({ relativePath }) => {
          const segments = relativePath.split('/')
          segments.pop()
          for (let end = 1; end <= segments.length; end += 1) {
            directories.add(segments.slice(0, end).join('/'))
          }
        })
        for (const directory of [...directories].sort(
          (left, right) => left.split('/').length - right.split('/').length,
        )) {
          if (uploadCancelRequested) break
          uploadTask.currentFile = directory
          const created = await client.mkdir({
            path: joinAbsolutePath(targetDirectory, directory),
            recursive: false,
          })
          if (!created.success) {
            throw new Error(created.message || t('app.fileManager.uploadDirectoryFailed'))
          }
        }
      }

      for (const source of files) {
        if (uploadCancelRequested) break
        uploadTask.status = 'uploading'
        uploadTask.currentFile = source.relativePath
        let currentUploadedBytes = 0
        try {
          const result = await uploadOneFile(
            client,
            joinAbsolutePath(targetDirectory, source.relativePath),
            source.file,
            (uploadedBytes) => {
              currentUploadedBytes = Math.max(currentUploadedBytes, uploadedBytes)
              updateUploadProgress(
                settledWorkBytes,
                currentUploadedBytes,
                actualTransferredBytes + currentUploadedBytes,
              )
            },
          )
          if (!result.completed) break
          uploadTask.completedFiles += 1
          actualTransferredBytes += source.file.size
        } catch (error) {
          const message = error instanceof Error ? error.message : t('app.fileManager.uploadFailed')
          uploadTask.failedFiles += 1
          actualTransferredBytes += currentUploadedBytes
          uploadTask.failures.push({ path: source.relativePath, message })
        }
        settledWorkBytes += source.file.size
        updateUploadProgress(settledWorkBytes, 0, actualTransferredBytes)
      }

      if (uploadCancelRequested) {
        uploadTask.status = 'cancelled'
      } else if (uploadTask.failedFiles === 0) {
        uploadTask.status = 'completed'
        uploadTask.progressPercent = 100
      } else if (uploadTask.completedFiles > 0) {
        uploadTask.status = 'partial'
        uploadTask.progressPercent = 100
      } else {
        uploadTask.status = 'failed'
        uploadTask.progressPercent = 100
      }
    } catch (error) {
      uploadTask.errorSummary =
        error instanceof Error ? error.message : t('app.fileManager.uploadFailed')
      uploadTask.status = uploadCancelRequested ? 'cancelled' : 'failed'
    } finally {
      uploadAbortController = null
      currentTransferId = ''
      currentTransferClient = null
      currentCancelPromise = null
      uploadTask.currentFile = ''
    }
    return uploadTask.completedFiles > 0
  }

  /** 请求取消上传，当前分块中止后由服务端清理暂存文件。 */
  const cancelUpload = async () => {
    if (!uploadActive.value || uploadTask.status === 'cancelling') return
    uploadCancelRequested = true
    uploadTask.status = 'cancelling'
    uploadAbortController?.abort()
    await cancelCurrentTransfer()
  }

  /** 关闭已结束的上传任务摘要。 */
  const dismissUpload = () => {
    if (!uploadActive.value) uploadTask.visible = false
  }

  /** 页面恢复后继续等待当前节点的活动文件任务。 */
  const resumeActiveTasks = async () => {
    const res = await fsClient.value.activeTasks()
    if (!res.success || !res.data?.length) return false
    await Promise.all(res.data.map(pollTask))
    return true
  }

  /** 页面恢复后继续跟踪已开始的下载；上传在用户重新选择同一文件后续传。 */
  const resumeActiveTransfers = async () => {
    const res = await fsClient.value.activeTransfers()
    const downloads = res.data?.filter((transfer) => transfer.direction === 'download') || []
    if (!res.success || downloads.length === 0) return false
    await Promise.all(downloads.map((transfer) => pollTransfer(transfer.transferId)))
    return true
  }

  return {
    createFile,
    mkdir,
    removePath,
    renamePath,
    copyPath,
    runPathTask,
    downloadFile,
    uploadTask,
    uploadActive,
    startUpload,
    cancelUpload,
    dismissUpload,
    resumeActiveTasks,
    resumeActiveTransfers,
  }
}
