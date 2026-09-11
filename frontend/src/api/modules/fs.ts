import http from '@/api'
import type { AxiosProgressEvent } from 'axios'
import type {
  CreateFileOperationTaskRequest,
  CreateFileTransferRequest,
  FileDocument,
  FileListPage,
  FileOperationTask,
  FileSaveResult,
  FileTransfer,
  FsEntry,
  HomeResponse,
  UpdateFileContentRequest,
} from '../interface/fs'

/** 上传分块的传输控制选项。 */
export interface UploadChunkOptions {
  signal?: AbortSignal
  onProgress?: (loadedBytes: number) => void
}

const nodePath = (nodeId: string, path: string) =>
  `/node/${encodeURIComponent(nodeId)}/${path.replace(/^\/+/, '')}`

/** 为固定节点创建文件管理 API，避免窗口跟随全局节点切换。 */
const createScopedFsApi = (nodeId: string) => ({
  listEntries: (params: {
    path: string
    page: number
    pageSize: number
    sortBy: 'name' | 'modifiedAt' | 'sizeBytes'
    sortOrder: 'ascending' | 'descending'
    showHidden: boolean
  }) =>
    http.get<FileListPage>(nodePath(nodeId, 'files/list'), {
      ...params,
      sortOrder: params.sortOrder === 'ascending' ? 'asc' : 'desc',
    }),
  home: () => http.get<HomeResponse>(nodePath(nodeId, 'files/home')),
  detail: (path: string) => http.get<FsEntry>(nodePath(nodeId, 'file/detail'), { path }),
  readFile: (path: string) => http.get<FileDocument>(nodePath(nodeId, 'file/content'), { path }),
  createFile: (payload: { path: string; content?: string }) =>
    http.post<FsEntry>(nodePath(nodeId, 'files'), payload),
  writeFile: (payload: UpdateFileContentRequest) =>
    http.put<FileSaveResult>(nodePath(nodeId, 'file/content'), payload),
  mkdir: (payload: { path: string; recursive: boolean }) =>
    http.post<FsEntry>(nodePath(nodeId, 'directories'), payload),
  createTask: (payload: CreateFileOperationTaskRequest) =>
    http.post<FileOperationTask>(nodePath(nodeId, 'file-operation-tasks'), payload),
  taskDetail: (taskId: string) =>
    http.get<FileOperationTask>(
      nodePath(nodeId, `file-operation-task/${encodeURIComponent(taskId)}/detail`),
    ),
  activeTasks: () => http.get<FileOperationTask[]>(nodePath(nodeId, 'file-operation-tasks/active')),
  cancelTask: (taskId: string) =>
    http.post<FileOperationTask>(
      nodePath(nodeId, `file-operation-task/${encodeURIComponent(taskId)}/cancel`),
    ),
  createTransfer: (payload: CreateFileTransferRequest) =>
    http.post<FileTransfer>(nodePath(nodeId, 'file-transfers'), payload),
  transferDetail: (transferId: string) =>
    http.get<FileTransfer>(
      nodePath(nodeId, `file-transfer/${encodeURIComponent(transferId)}/detail`),
    ),
  activeTransfers: () => http.get<FileTransfer[]>(nodePath(nodeId, 'file-transfers/active')),
  uploadChunk: (
    transferId: string,
    chunk: ArrayBuffer,
    start: number,
    end: number,
    total: number,
    options: UploadChunkOptions = {},
  ) =>
    http.put<FileTransfer>(
      nodePath(nodeId, `file-transfer/${encodeURIComponent(transferId)}/chunk`),
      chunk,
      {
        signal: options.signal,
        onUploadProgress: ({ loaded }: AxiosProgressEvent) => options.onProgress?.(loaded),
        headers: {
          'Content-Type': 'application/octet-stream',
          'Content-Range': `bytes ${start}-${end}/${total}`,
        },
      },
    ),
  completeTransfer: (transferId: string) =>
    http.post<FileTransfer>(
      nodePath(nodeId, `file-transfer/${encodeURIComponent(transferId)}/complete`),
    ),
  cancelTransfer: (transferId: string) =>
    http.post<FileTransfer>(
      nodePath(nodeId, `file-transfer/${encodeURIComponent(transferId)}/cancel`),
    ),
  downloadUrl: (transferId: string) =>
    `/api/v1${nodePath(nodeId, `file-transfer/${encodeURIComponent(transferId)}/content`)}`,
})

export const fsApi = {
  forNode: createScopedFsApi,
}
