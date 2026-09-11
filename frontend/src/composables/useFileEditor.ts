/**
 * @file useFileEditor.ts
 * @description 固定节点文件编辑器的加载、刷新、冲突保护和可靠保存状态机。
 */
import type { Ref } from 'vue'
import { fsApi } from '@/api/modules/fs'
import { useToastStore } from '@/stores/toast'
import { useI18n } from 'vue-i18n'
import type { FileDocument } from '@/api/generated'

export type EditorLoadState =
  | 'idle'
  | 'initialLoading'
  | 'ready'
  | 'refreshing'
  | 'initialError'
  | 'stale'

export type EditorSaveState = 'idle' | 'saving' | 'reconciling' | 'conflict' | 'failed'

export interface EditorTab {
  path: string
  name: string
  documentKey: string
  content: string
  originalContent: string
  revision: string
  isDirty: boolean
  fileSize: number
  loadedAt?: string
  capabilities?: FileDocument['capabilities']
  loadState: EditorLoadState
  saveState: EditorSaveState
  durability?: 'durable' | 'uncertain'
  loadSequence: number
  saveSequence: number
  loadError?: string
  refreshWarning?: string
  saveError?: string
}

interface LoadOptions {
  discardLocalChanges?: boolean
}

/** 提供单个固定节点上的文件编辑状态操作。 */
export function useFileEditor(nodeId: Readonly<Ref<string>>) {
  const toastStore = useToastStore()
  const { t } = useI18n()

  /** 将服务端文档事实写入标签，同时保留保存期间产生的新输入。 */
  const applyDocument = (tab: EditorTab, document: FileDocument, savedSnapshot?: string) => {
    const baseline = savedSnapshot ?? document.content
    if (savedSnapshot === undefined) {
      tab.content = document.content
    }
    tab.originalContent = baseline
    tab.revision = document.revision
    tab.fileSize = document.sizeBytes
    tab.loadedAt = document.loadedAt
    tab.capabilities = document.capabilities
    tab.isDirty = tab.content !== baseline
    tab.loadState = 'ready'
    tab.loadError = undefined
    tab.refreshWarning = undefined
  }

  /** 加载或刷新文件；刷新失败时保留已加载内容。 */
  const loadFileContent = async (tab: EditorTab, options: LoadOptions = {}) => {
    if (tab.saveState === 'saving' || tab.saveState === 'reconciling') return false
    if (tab.isDirty && !options.discardLocalChanges) return false

    const wasLoaded = ['ready', 'refreshing', 'stale'].includes(tab.loadState)
    const requestSequence = ++tab.loadSequence
    const requestNodeId = nodeId.value
    const requestPath = tab.path
    tab.loadState = wasLoaded ? 'refreshing' : 'initialLoading'
    tab.loadError = undefined
    tab.refreshWarning = undefined

    try {
      const response = await fsApi.forNode(requestNodeId).readFile(requestPath)
      if (
        requestSequence !== tab.loadSequence ||
        requestNodeId !== nodeId.value ||
        requestPath !== tab.path
      ) {
        return false
      }
      if (!response.success || !response.data) {
        const message = response.message || t('app.fileEditor.loadError')
        if (wasLoaded) {
          tab.loadState = 'stale'
          tab.refreshWarning = message
          toastStore.warning(t('app.fileEditor.refreshFailed', { message }))
        } else {
          tab.loadState = 'initialError'
          tab.loadError = message
        }
        return false
      }
      applyDocument(tab, response.data)
      tab.saveState = 'idle'
      tab.saveError = undefined
      tab.durability = undefined
      return true
    } catch (error) {
      if (
        requestSequence !== tab.loadSequence ||
        requestNodeId !== nodeId.value ||
        requestPath !== tab.path
      ) {
        return false
      }
      const message = error instanceof Error ? error.message : t('app.fileEditor.loadError')
      if (wasLoaded) {
        tab.loadState = 'stale'
        tab.refreshWarning = message
        toastStore.warning(t('app.fileEditor.refreshFailed', { message }))
      } else {
        tab.loadState = 'initialError'
        tab.loadError = message
      }
      return false
    }
  }

  /** 核对结果不明确的保存请求，避免已提交保存被误报为失败。 */
  const reconcileSave = async (
    tab: EditorTab,
    requestNodeId: string,
    requestPath: string,
    requestSequence: number,
    savedSnapshot: string,
  ) => {
    tab.saveState = 'reconciling'
    try {
      const response = await fsApi.forNode(requestNodeId).readFile(requestPath)
      if (
        requestSequence !== tab.saveSequence ||
        requestNodeId !== nodeId.value ||
        requestPath !== tab.path
      ) {
        return false
      }
      if (response.success && response.data?.content === savedSnapshot) {
        applyDocument(tab, response.data, savedSnapshot)
        tab.saveState = 'idle'
        tab.saveError = undefined
        toastStore.success(t('app.fileEditor.saveReconciled'))
        return true
      }
    } catch {
      // 核对失败保持本地缓冲和旧 revision，后续重试仍受乐观锁保护。
    }
    if (requestSequence === tab.saveSequence) {
      tab.saveState = 'failed'
      tab.saveError = t('app.fileEditor.saveOutcomeUnknown')
      toastStore.warning(tab.saveError)
    }
    return false
  }

  /** 判断错误是否可能发生在服务端已经提交保存之后。 */
  const isAmbiguousFailure = (code: number, errorCode?: string) =>
    code < 0 ||
    code >= 500 ||
    ['AGENT_TIMEOUT', 'AGENT_UNAVAILABLE', 'AGENT_REQUEST_FAILED'].includes(errorCode ?? '')

  /** 使用内容快照和 expectedRevision 原子保存文件。 */
  const saveFileContent = async (tab: EditorTab) => {
    if (
      !tab.isDirty ||
      tab.loadState === 'initialLoading' ||
      tab.loadState === 'refreshing' ||
      tab.saveState === 'saving' ||
      tab.saveState === 'reconciling' ||
      tab.saveState === 'conflict' ||
      tab.capabilities?.canWrite === false
    ) {
      return false
    }

    const savedSnapshot = tab.content
    const requestRevision = tab.revision
    const requestNodeId = nodeId.value
    const requestPath = tab.path
    const requestSequence = ++tab.saveSequence
    tab.saveState = 'saving'
    tab.saveError = undefined

    try {
      const response = await fsApi.forNode(requestNodeId).writeFile({
        path: requestPath,
        content: savedSnapshot,
        expectedRevision: requestRevision,
      })
      if (
        requestSequence !== tab.saveSequence ||
        requestNodeId !== nodeId.value ||
        requestPath !== tab.path
      ) {
        return false
      }
      if (!response.success || !response.data) {
        const message = response.message || t('app.common.unknownError')
        if (response.errorCode === 'FILE_CHANGED') {
          tab.saveState = 'conflict'
          tab.saveError = message
          toastStore.warning(t('app.fileEditor.conflictDetected'))
          return false
        }
        if (isAmbiguousFailure(response.code, response.errorCode)) {
          return reconcileSave(tab, requestNodeId, requestPath, requestSequence, savedSnapshot)
        }
        tab.saveState = 'failed'
        tab.saveError = message
        toastStore.error(t('app.fileEditor.saveFailed', { message }))
        return false
      }

      applyDocument(tab, response.data.document, savedSnapshot)
      tab.saveState = 'idle'
      tab.saveError = undefined
      tab.durability = response.data.durability
      if (response.data.durability === 'uncertain') {
        toastStore.warning(t('app.fileEditor.durabilityUncertain'))
      } else {
        toastStore.success(t('app.fileEditor.saveSuccess'))
      }
      return true
    } catch {
      return reconcileSave(tab, requestNodeId, requestPath, requestSequence, savedSnapshot)
    }
  }

  return {
    loadFileContent,
    saveFileContent,
  }
}
