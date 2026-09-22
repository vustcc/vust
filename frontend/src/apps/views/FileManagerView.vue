<script setup lang="ts">
import { computed, ref, watch, onMounted, onUnmounted } from 'vue'
import { fsApi } from '@/api/modules/fs'
import type { FsEntry } from '@/api/interface/fs'
import { useToastStore } from '@/stores/toast'
import { useNodeStore } from '@/stores/node'
import { useWindowManagerStore } from '@/stores/window-manager'
import { useI18n } from 'vue-i18n'
import VustTable, { type VustTableColumn } from '@/components/ui/VustTable.vue'
import {
  VustButton,
  VustActionMenu,
  VustTag,
  VustBreadcrumb,
  VustBreadcrumbItem,
  VustInput,
  VustSelect,
  VustPagination,
  VustSwitch,
  VustEmpty,
  VustLoading,
  VustCheckbox,
} from '@/components/ui'
import VustIcon from '@/components/icons/VustIcon.vue'
import AppContextMenu from '@/components/AppContextMenu.vue'
import ApplicationDialog from '@/components/layout/ApplicationDialog.vue'
import ApplicationConfirmationDialog from '@/components/layout/ApplicationConfirmationDialog.vue'
import { formatBytes } from '@/utils/units'
import { formatDateTime } from '@/utils/time'
import { useFileOperations } from '@/composables/useFileOperations'
import { useApplicationConfirmation } from '@/composables/useApplicationConfirmation'

const props = defineProps<{
  isMaximized?: boolean
  windowId?: string
  payload?: Record<string, unknown>
}>()

const { t } = useI18n()
const toastStore = useToastStore()
const nodeStore = useNodeStore()
const windowStore = useWindowManagerStore()
const { confirmationState, showConfirmation, handleConfirmationResponse } =
  useApplicationConfirmation()
const targetNodeId = ref(
  typeof props.payload?.nodeId === 'string' ? props.payload.nodeId : nodeStore.currentNodeId,
)
const {
  createFile,
  mkdir,
  removePath,
  renamePath,
  runPathTask,
  downloadFile,
  uploadTask,
  uploadActive,
  startUpload,
  cancelUpload,
  dismissUpload,
  resumeActiveTasks,
  resumeActiveTransfers,
} = useFileOperations(targetNodeId)

const currentPath = ref('')
const pathInput = ref('')
const showHidden = ref(false)
const entries = ref<FsEntry[]>([])
const selectedEntry = ref<FsEntry | null>(null)
const selectedPaths = ref<Set<string>>(new Set())
const listLoading = ref(false)
const listLoaded = ref(false)
const listLoadedAt = ref('')
const listRequestSequence = ref(0)
const fileOperationCount = ref(0)
const listError = ref('')
const fileTableRef = ref<HTMLElement | null>(null)
const totalEntries = ref(0)
const entryCounts = ref({ fileCount: 0, directoryCount: 0, symlinkCount: 0, otherCount: 0 })

/** 每页条数选项 */
const pageSizeOptions = computed(() => [
  { label: t('common.pagination.perPage', { n: 50 }), value: 50 },
  { label: t('common.pagination.perPage', { n: 100 }), value: 100 },
  { label: t('common.pagination.perPage', { n: 200 }), value: 200 },
  { label: t('common.pagination.perPage', { n: 500 }), value: 500 },
])
const pageSize = ref(50)
const currentPage = ref(1)

const columns = computed<VustTableColumn[]>(() => [
  { label: '', width: 40, slot: 'selection', headerSlot: 'selectionHeader', align: 'center' },
  { label: t('app.fileManager.name'), width: 220, minWidth: 180, slot: 'name' },
  { label: t('app.fileManager.type'), width: 50, slot: 'type', align: 'center' },
  {
    label: t('app.fileManager.lastModified'),
    width: 110,
    minWidth: 100,
    slot: 'mtime',
    align: 'center',
  },
  { label: t('app.fileManager.size'), width: 80, slot: 'size', align: 'center' },
])
const sortState = ref<{
  prop: 'name' | 'modifiedAt' | 'sizeBytes'
  order: 'ascending' | 'descending'
}>({
  prop: 'name',
  order: 'ascending',
})

const currentDisplayPath = computed(() => currentPath.value || '/')
const parentPath = computed(() => {
  const path = currentDisplayPath.value
  if (path === '/') return '/'
  const segments = path.split('/').filter(Boolean)
  segments.pop()
  return segments.length ? `/${segments.join('/')}` : '/'
})
const breadcrumbItems = computed(() => {
  const path = currentDisplayPath.value
  if (path === '/') {
    return [{ label: '/', path: '/' }]
  }
  const segments = path.split('/').filter(Boolean)
  return [
    { label: '/', path: '/' },
    ...segments.map((segment, index) => ({
      label: segment,
      path: `/${segments.slice(0, index + 1).join('/')}`,
    })),
  ]
})

const sortedEntries = computed(() => entries.value)

const totalPages = computed(() => {
  return Math.max(1, Math.ceil(totalEntries.value / pageSize.value))
})

const pagedEntries = computed(() => sortedEntries.value)

const currentPageStats = computed(() => ({
  dirs: entryCounts.value.directoryCount,
  files: entryCounts.value.fileCount,
}))

const fileManagerBusy = computed(
  () => listLoading.value || fileOperationCount.value > 0 || uploadActive.value,
)

const entryKindLabel = (kind: FsEntry['kind']) => {
  if (kind === 'directory') return t('app.fileManager.dir')
  if (kind === 'symlink') return t('app.fileManager.symlink')
  if (kind === 'other') return t('app.fileManager.other')
  return t('app.fileManager.file')
}

const runFileOperation = async <T,>(operation: () => Promise<T>) => {
  if (fileOperationCount.value > 0) return undefined
  fileOperationCount.value += 1
  try {
    return await operation()
  } finally {
    fileOperationCount.value = Math.max(0, fileOperationCount.value - 1)
  }
}

watch(
  [fileManagerBusy, uploadActive],
  ([busy, uploadBusy]) => {
    if (!props.windowId) return
    windowStore.updateWindowRuntimeState(props.windowId, {
      busy,
      allowsNodeSwitch: false,
      blockLevel: busy ? 'busy' : 'open',
      blockReason: uploadBusy
        ? t('app.fileManager.guardUpload')
        : busy
          ? t('app.fileManager.guardBusy')
          : t('app.fileManager.guardOpen'),
    })
  },
  { immediate: true },
)

const setPath = async (path: string) => {
  const normalized = path.trim() || '/'
  const previousPath = currentPath.value
  currentPath.value = normalized
  pathInput.value = normalized
  selectedEntry.value = null
  currentPage.value = 1
  const loaded = await loadEntries(normalized)
  if (loaded === false && currentPath.value === normalized) {
    currentPath.value = previousPath
    pathInput.value = previousPath || '/'
  }
}

const loadEntries = async (path: string) => {
  const requestSequence = ++listRequestSequence.value
  const requestNodeId = targetNodeId.value
  listLoading.value = true
  listError.value = ''
  try {
    const res = await fsApi.forNode(requestNodeId).listEntries({
      path,
      page: currentPage.value,
      pageSize: pageSize.value,
      sortBy: sortState.value.prop,
      sortOrder: sortState.value.order,
      showHidden: showHidden.value,
    })
    if (requestSequence !== listRequestSequence.value || requestNodeId !== targetNodeId.value)
      return undefined
    if (!res.success) {
      listError.value = res.message || t('app.fileManager.fetchError')
      toastStore.error(`${t('app.fileManager.loadError')}: ${listError.value}`)
      return false
    }
    if (!res.data) return false
    entries.value = res.data.entries
    totalEntries.value = res.data.total
    entryCounts.value = res.data.counts
    listLoadedAt.value = res.data.loadedAt
    listLoaded.value = true
    selectedPaths.value.clear()
    return true
  } catch (error) {
    if (requestSequence !== listRequestSequence.value || requestNodeId !== targetNodeId.value)
      return undefined
    listError.value = error instanceof Error ? error.message : t('app.fileManager.fetchError')
    toastStore.error(`${t('app.fileManager.loadError')}: ${listError.value}`)
    return false
  } finally {
    if (requestSequence === listRequestSequence.value && requestNodeId === targetNodeId.value) {
      listLoading.value = false
    }
  }
}

const selectEntry = (entry: FsEntry, e?: MouseEvent) => {
  selectedEntry.value = entry
  if (e && (e.ctrlKey || e.metaKey)) {
    if (selectedPaths.value.has(entry.path)) {
      selectedPaths.value.delete(entry.path)
    } else {
      selectedPaths.value.add(entry.path)
    }
  } else if (e && e.shiftKey) {
    selectedPaths.value.add(entry.path)
  }
}

const openEntry = async (entry: FsEntry) => {
  if (!entry.capabilities.canOpen) return
  if (entry.kind === 'directory') {
    await setPath(entry.path)
    return
  }
  windowStore.openWindowWithPayload(
    'file-editor',
    { path: entry.path, nodeId: targetNodeId.value },
    {
      title: t('app.fileEditor.appName'),
      i18nTitleKey: 'app.fileEditor.appName',
    },
  )
}

const openSelected = async () => {
  if (!selectedEntry.value) return
  await openEntry(selectedEntry.value)
}

const goParent = async () => {
  await setPath(parentPath.value)
}

const copyCurrentPath = async () => {
  const path = selectedEntry.value?.path || currentDisplayPath.value
  if (!navigator.clipboard) {
    toastStore.error(t('app.fileManager.copyPathUnsupported'))
    return
  }
  try {
    await navigator.clipboard.writeText(path)
    toastStore.success(t('app.fileManager.copyPathSuccess'))
  } catch (error) {
    toastStore.error(
      `${t('app.fileManager.copyPathFailed')}: ${error instanceof Error ? error.message : String(error)}`,
    )
  }
}

const refresh = async () => {
  if (!currentPath.value) return
  await loadEntries(currentPath.value)
}

const applyPathInput = async () => {
  await setPath(pathInput.value)
}

const initHome = async () => {
  await runFileOperation(async () => {
    const requestNodeId = targetNodeId.value
    const res = await fsApi.forNode(requestNodeId).home()
    if (requestNodeId !== targetNodeId.value) return
    if (res.success && res.data?.path) {
      await setPath(res.data.path)
    } else {
      await setPath('/home')
    }
  })
}

watch(showHidden, () => {
  currentPage.value = 1
  void refresh()
})
watch(pageSize, () => {
  if (currentPage.value === 1) {
    void refresh()
    return
  }
  currentPage.value = 1
})
watch(currentPage, () => void refresh())

void initHome().then(async () => {
  const resumed = await runFileOperation(resumeActiveTasks)
  if (resumed) await refresh()
  await runFileOperation(resumeActiveTransfers)
})

// Selection features
const isSelected = (path: string) => selectedPaths.value.has(path)
const toggleSelection = (path: string, checked: boolean) => {
  if (checked) selectedPaths.value.add(path)
  else selectedPaths.value.delete(path)
}
const toggleAll = (checked: boolean) => {
  if (checked) {
    pagedEntries.value.forEach((e) => selectedPaths.value.add(e.path))
  } else {
    pagedEntries.value.forEach((e) => selectedPaths.value.delete(e.path))
  }
}
const isAllSelected = computed(() => {
  if (pagedEntries.value.length === 0) return false
  return pagedEntries.value.every((e) => selectedPaths.value.has(e.path))
})

const handleBatchDelete = async () => {
  if (selectedPaths.value.size === 0) return
  const pathSnapshot = Array.from(selectedPaths.value)
  const confirmed = await showConfirmation(
    t('app.fileManager.batchDeleteWarning', { count: pathSnapshot.length }),
    t('app.fileManager.batchDeleteTitle'),
    t('app.fileManager.confirmDelete'),
    t('app.fileManager.cancel'),
    'danger',
  )
  if (!confirmed) return

  await runFileOperation(async () => {
    const success = await runPathTask(
      'remove',
      pathSnapshot.map((path) => ({ path })),
      true,
    )
    if (success) {
      pathSnapshot.forEach((path) => selectedPaths.value.delete(path))
      await refresh()
    }
  })
}

// Dialog features
const dialogState = ref({
  visible: false,
  type: 'dir' as 'dir' | 'file' | 'rename' | 'batchMove' | 'batchCopy',
  inputName: '',
  targetEntry: null as FsEntry | null,
})
const dialogReturnFocusSelector = ref('')
const dialogTitle = computed(() => {
  if (dialogState.value.type === 'dir') return t('app.fileManager.newDir')
  if (dialogState.value.type === 'file') return t('app.fileManager.newFile')
  if (dialogState.value.type === 'batchMove') return t('app.fileManager.batchMove')
  if (dialogState.value.type === 'batchCopy') return t('app.fileManager.batchCopy')
  return t('app.fileManager.rename')
})

const openDialog = (
  type: 'dir' | 'file' | 'rename' | 'batchMove' | 'batchCopy',
  entry: FsEntry | null = null,
  returnFocusSelector = '[data-ui="file-table"]',
) => {
  dialogState.value.type = type
  dialogState.value.targetEntry = entry
  dialogState.value.inputName = entry ? entry.name : ''
  dialogReturnFocusSelector.value = returnFocusSelector
  dialogState.value.visible = true
}

const createActions = computed(() => [
  {
    label: t('app.fileManager.newFile'),
    icon: 'file',
    handler: () => openDialog('file', null, '[data-ui="file-create-actions"] button'),
  },
  {
    label: t('app.fileManager.newDir'),
    icon: 'folder',
    handler: () => openDialog('dir', null, '[data-ui="file-create-actions"] button'),
  },
])

const confirmDialog = async () => {
  const { type, inputName, targetEntry } = dialogState.value
  if (!inputName.trim()) return

  await runFileOperation(async () => {
    let success = false
    if (type === 'dir') {
      const path = `${currentDisplayPath.value}/${inputName}`.replace(/\/\//g, '/')
      success = await mkdir(path)
    } else if (type === 'file') {
      const path = `${currentDisplayPath.value}/${inputName}`.replace(/\/\//g, '/')
      success = await createFile(path, '')
    } else if (type === 'batchMove' || type === 'batchCopy') {
      const targetDir = inputName.endsWith('/') ? inputName.slice(0, -1) : inputName
      success = await runPathTask(
        type === 'batchMove' ? 'move' : 'copy',
        Array.from(selectedPaths.value).map((path) => ({ path })),
        true,
        targetDir,
      )
      if (success) {
        selectedPaths.value.clear()
      }
    } else if (type === 'rename' && targetEntry) {
      const parent = targetEntry.path.substring(0, targetEntry.path.lastIndexOf('/'))
      const newPath = `${parent}/${inputName}`.replace(/\/\//g, '/')
      success = await renamePath(targetEntry.path, newPath, targetEntry.revision)
    }

    if (success) {
      dialogState.value.visible = false
      await refresh()
    }
  })
}

// Upload/Download features
const fileInput = ref<HTMLInputElement | null>(null)
const folderInput = ref<HTMLInputElement | null>(null)
const uploadActions = computed(() => [
  {
    label: t('app.fileManager.uploadFile'),
    icon: 'file',
    handler: () => fileInput.value?.click(),
  },
  {
    label: t('app.fileManager.uploadFolder'),
    icon: 'folder',
    handler: () => folderInput.value?.click(),
  },
])
const uploadProcessedFiles = computed(() => uploadTask.completedFiles + uploadTask.failedFiles)
const uploadStatusText = computed(() => t(`app.fileManager.uploadTask.${uploadTask.status}`))
const uploadStatusType = computed(() => {
  if (uploadTask.status === 'completed') return 'success'
  if (uploadTask.status === 'partial' || uploadTask.status === 'cancelling') return 'warning'
  if (uploadTask.status === 'failed') return 'danger'
  if (uploadTask.status === 'cancelled') return 'default'
  return 'primary'
})

/** 上传进行中保持进度模态框可见，终态关闭时清理任务展示。 */
const handleUploadDialogClose = () => {
  if (uploadActive.value) return
  dismissUpload()
}

/** 将浏览器选择结果转换为固定目标目录上的后台上传任务。 */
const runSelectedUpload = async (input: HTMLInputElement, kind: 'files' | 'folder') => {
  const files = Array.from(input.files || [])
  if (files.length === 0) {
    toastStore.error(t('app.fileManager.uploadTask.noFiles'))
    input.value = ''
    return
  }
  if (kind === 'folder' && files.some((file) => !file.webkitRelativePath)) {
    toastStore.error(t('app.fileManager.uploadTask.folderUnsupported'))
    input.value = ''
    return
  }
  const relativePaths = files.map((file) =>
    kind === 'folder' ? file.webkitRelativePath || file.name : file.name,
  )
  const displayName =
    kind === 'folder'
      ? relativePaths[0]?.split('/')[0] || files[0]!.name
      : files.length === 1
        ? files[0]!.name
        : t('app.fileManager.uploadTask.fileSelection', { count: files.length })
  const targetDirectory = currentDisplayPath.value
  const changed = await startUpload(targetDirectory, {
    kind,
    displayName,
    files: files.map((file, index) => ({ file, relativePath: relativePaths[index]! })),
  })
  input.value = ''
  if (changed && currentDisplayPath.value === targetDirectory) await refresh()
}

/** 接收普通文件选择结果。 */
const onFileChange = async (e: Event) => {
  const target = e.target as HTMLInputElement
  await runSelectedUpload(target, 'files')
}
/** 接收文件夹选择结果并保留浏览器提供的相对路径。 */
const onFolderChange = async (e: Event) => {
  const target = e.target as HTMLInputElement
  await runSelectedUpload(target, 'folder')
}
const handleDownload = async (entry: FsEntry) => {
  if (!entry.capabilities.canDownload) return
  await runFileOperation(() => downloadFile(entry.path, entry.name, entry.revision))
}
const handleDelete = async (entry: FsEntry) => {
  if (!entry.capabilities.canRemove) return
  const confirmed = await showConfirmation(
    entry.kind === 'directory'
      ? t('app.fileManager.deleteDirectoryWarning', { path: entry.path })
      : t('app.fileManager.deleteItemWarning', {
          path: entry.path,
          type: entryKindLabel(entry.kind),
        }),
    t('app.fileManager.deleteTitle'),
    t('app.fileManager.confirmDelete'),
    t('app.fileManager.cancel'),
    'danger',
  )
  if (!confirmed) return

  await runFileOperation(async () => {
    const success = await removePath(entry.path, true, entry.revision)
    if (success) {
      selectedPaths.value.delete(entry.path)
      await refresh()
    }
  })
}

const copyEntryPath = async (entry: FsEntry) => {
  if (!navigator.clipboard) return toastStore.error(t('app.fileManager.copyPathUnsupported'))
  await navigator.clipboard.writeText(entry.path)
  toastStore.success(t('app.fileManager.copyPathSuccess'))
}

// Context menu
const contextMenu = ref({ visible: false, x: 0, y: 0, entry: null as FsEntry | null })
const showContextMenu = (entry: FsEntry, event: MouseEvent) => {
  event.preventDefault()
  selectEntry(entry)
  contextMenu.value = { visible: true, x: event.clientX, y: event.clientY, entry }
}
const hideContextMenu = () => {
  contextMenu.value.visible = false
}
onMounted(() => document.addEventListener('click', hideContextMenu))
onUnmounted(() => document.removeEventListener('click', hideContextMenu))
</script>

<template>
  <section
    class="file-manager"
    data-page="file-manager"
    data-vust-app="file-manager"
    :data-node-id="targetNodeId"
    :data-loaded-at="listLoadedAt"
  >
    <div class="toolbar" data-slot="header" data-ui="file-manager-toolbar">
      <div class="path-bar">
        <div class="vl-input-group">
          <VustInput
            id="file-manager-path"
            name="fileManagerPath"
            data-ui="file-path-input"
            v-model="pathInput"
            class="path-input"
            :placeholder="t('app.fileManager.path')"
            @keyup.enter="applyPathInput"
          />
          <VustButton type="secondary" class="path-btn" @click="applyPathInput">
            {{ t('app.fileManager.open') }}
          </VustButton>
        </div>
      </div>
      <div class="options-bar">
        <VustSwitch v-model="showHidden" :active-text="t('app.fileManager.showHidden')" />
      </div>
      <div class="nav-actions">
        <VustActionMenu
          :label="t('common.new')"
          :actions="createActions"
          :disabled="fileOperationCount > 0"
          data-ui="file-create-actions"
        />
        <VustActionMenu
          :label="t('app.fileManager.upload')"
          :actions="uploadActions"
          :disabled="uploadActive"
          data-ui="file-upload-actions"
        />
        <input
          id="file-manager-upload-files"
          ref="fileInput"
          class="upload-input"
          name="fileManagerUploadFiles"
          type="file"
          multiple
          @change="onFileChange"
        />
        <input
          id="file-manager-upload-folder"
          ref="folderInput"
          class="upload-input"
          name="fileManagerUploadFolder"
          type="file"
          multiple
          webkitdirectory=""
          @change="onFolderChange"
        />
        <VustButton type="primary" data-ui="file-refresh" @click="refresh">{{
          t('app.fileManager.refresh')
        }}</VustButton>
      </div>
    </div>

    <section class="list-container">
      <div class="breadcrumb-wrap" data-ui="breadcrumb-bar">
        <VustBreadcrumb separator="/">
          <VustBreadcrumbItem v-for="item in breadcrumbItems" :key="item.path">
            <button type="button" class="link-btn" @click="setPath(item.path)">
              {{ item.label }}
            </button>
          </VustBreadcrumbItem>
        </VustBreadcrumb>
        <div class="list-actions">
          <VustButton size="small" :disabled="currentDisplayPath === '/'" @click="goParent">
            {{ t('app.fileManager.upDir') }}
          </VustButton>
          <VustButton size="small" :disabled="!selectedEntry" @click="openSelected">
            {{ t('app.fileManager.openSelected') }}
          </VustButton>
          <VustButton size="small" @click="copyCurrentPath">
            {{ t('app.fileManager.copyPath') }}
          </VustButton>
          <VustButton
            size="small"
            type="danger"
            :disabled="selectedPaths.size === 0"
            @click="handleBatchDelete"
          >
            {{ t('app.fileManager.batchDelete') }}
          </VustButton>
        </div>
      </div>

      <div v-if="listError" class="list-state error">{{ listError }}</div>

      <div class="table-wrapper">
        <VustTable
          ref="fileTableRef"
          class="file-table"
          data-ui="file-table"
          tabindex="-1"
          :data="pagedEntries"
          :columns="columns"
          border
          @row-contextmenu="showContextMenu"
        >
          <template #selectionHeader>
            <VustCheckbox :modelValue="isAllSelected" @update:modelValue="toggleAll" />
          </template>
          <template #selection="{ row }: { row: any }">
            <VustCheckbox
              :modelValue="isSelected(row.path)"
              @update:modelValue="(val) => toggleSelection(row.path, val)"
            />
          </template>

          <template #name="{ row }: { row: any }">
            <div
              class="entry-name"
              :class="{ 'is-selected': selectedEntry?.path === row.path }"
              @click="selectEntry(row, $event)"
              @dblclick="openEntry(row)"
            >
              <VustIcon
                class="entry-icon"
                :name="row.kind === 'directory' ? 'folder' : 'file'"
                :size="18"
              />
              <span class="entry-name-text">{{ row.name }}</span>
            </div>
          </template>

          <template #type="{ row }: { row: any }">
            <VustTag :type="row.kind === 'directory' ? 'success' : 'info'">
              {{ entryKindLabel(row.kind) }}
            </VustTag>
          </template>

          <template #mtime="{ row }: { row: any }">
            <span class="vl-text-secondary">{{ formatDateTime(row.modifiedAt) }}</span>
          </template>

          <template #size="{ row }: { row: any }">
            <span class="vl-text-secondary">
              {{ row.kind === 'directory' ? '-' : formatBytes(row.sizeBytes || 0) }}
            </span>
          </template>

          <template #empty>
            <VustEmpty :description="t('app.fileManager.empty')" />
          </template>
        </VustTable>
      </div>

      <div class="pagination-bar">
        <div class="page-info">
          {{
            t('app.fileManager.pageStats', {
              currentPage: currentPage,
              totalPages: totalPages,
              total: totalEntries,
            })
          }}
        </div>
        <div class="page-size-selector">
          <VustSelect
            v-model="pageSize"
            size="small"
            class="page-size-select"
            data-ui="file-page-size-select"
            :options="pageSizeOptions"
          />
        </div>

        <VustPagination
          :current-page="currentPage"
          :total-pages="totalPages"
          @page-change="(p) => (currentPage = p)"
        />

        <div class="page-stats">
          {{
            t('app.fileManager.currentDirStats', {
              dirs: currentPageStats.dirs,
              files: currentPageStats.files,
            })
          }}
        </div>
      </div>
    </section>

    <!-- Context Menu -->
    <AppContextMenu
      :visible="contextMenu.visible"
      :x="contextMenu.x"
      :y="contextMenu.y"
      @close="hideContextMenu"
    >
      <button
        type="button"
        class="context-menu-btn"
        data-ui="file-open-btn"
        :disabled="!contextMenu.entry?.capabilities.canOpen"
        @click="
          () => {
            contextMenu.entry && openEntry(contextMenu.entry)
            hideContextMenu()
          }
        "
      >
        {{ t('app.fileManager.open') }}
      </button>
      <button
        type="button"
        class="context-menu-btn"
        data-ui="file-rename-btn"
        :disabled="!contextMenu.entry?.capabilities.canRename"
        @click="
          () => {
            contextMenu.entry && openDialog('rename', contextMenu.entry)
            hideContextMenu()
          }
        "
      >
        {{ t('app.fileManager.rename') }}
      </button>
      <button
        v-if="contextMenu.entry?.capabilities.canDownload"
        type="button"
        class="context-menu-btn"
        data-ui="file-download-btn"
        @click="
          () => {
            contextMenu.entry && handleDownload(contextMenu.entry)
            hideContextMenu()
          }
        "
      >
        {{ t('app.fileManager.download') }}
      </button>
      <button
        type="button"
        class="context-menu-btn"
        data-ui="file-copy-path-btn"
        @click="
          () => {
            contextMenu.entry && copyEntryPath(contextMenu.entry)
            hideContextMenu()
          }
        "
      >
        {{ t('app.fileManager.copyPath') }}
      </button>
      <button
        type="button"
        class="context-menu-btn file-context-danger"
        data-ui="file-delete-btn"
        :disabled="!contextMenu.entry?.capabilities.canRemove"
        @click="
          () => {
            contextMenu.entry && handleDelete(contextMenu.entry)
            hideContextMenu()
          }
        "
      >
        {{ t('app.fileManager.delete') }}
      </button>
    </AppContextMenu>

    <ApplicationDialog
      :visible="dialogState.visible"
      :title="dialogTitle"
      :close-disabled="fileOperationCount > 0"
      initial-focus-selector="#file-manager-dialog-input"
      :return-focus-selector="dialogReturnFocusSelector"
      data-ui="file-operation-dialog"
      @close="dialogState.visible = false"
    >
      <div style="padding-top: 10px">
        <VustInput
          id="file-manager-dialog-input"
          name="fileManagerDialogInput"
          v-model="dialogState.inputName"
          :placeholder="
            dialogState.type.startsWith('batch')
              ? t('app.fileManager.targetPath')
              : t('app.fileManager.inputName')
          "
          @keyup.enter="confirmDialog"
        />
      </div>
      <template #footer>
        <VustButton :disabled="fileOperationCount > 0" @click="dialogState.visible = false">{{
          t('app.fileManager.cancel')
        }}</VustButton>
        <VustButton
          type="primary"
          :loading="fileOperationCount > 0"
          :disabled="fileOperationCount > 0 || !dialogState.inputName.trim()"
          @click="confirmDialog"
          >{{ t('app.fileManager.confirm') }}</VustButton
        >
      </template>
    </ApplicationDialog>

    <ApplicationDialog
      :visible="uploadTask.visible"
      :title="t('app.fileManager.upload')"
      width="36rem"
      :close-on-click-overlay="!uploadActive"
      :close-disabled="uploadActive"
      return-focus-selector="[data-ui='file-upload-actions'] button"
      data-ui="file-upload-dialog"
      :data-status="uploadTask.status"
      @close="handleUploadDialogClose"
    >
      <div class="upload-task" data-slot="upload-progress">
        <div class="upload-task-header">
          <div class="upload-task-title-wrap">
            <VustIcon name="upload" :size="18" />
            <div class="upload-task-title-content">
              <strong class="upload-task-title">{{ uploadTask.displayName }}</strong>
              <span class="upload-task-target" :title="uploadTask.targetDirectory">
                {{ t('app.fileManager.uploadTask.target', { path: uploadTask.targetDirectory }) }}
              </span>
            </div>
          </div>
          <VustTag :type="uploadStatusType" effect="plain">{{ uploadStatusText }}</VustTag>
        </div>

        <div class="upload-task-progress-meta" aria-live="polite">
          <span
            v-if="uploadTask.currentFile"
            class="upload-task-current"
            :title="uploadTask.currentFile"
          >
            {{ t('app.fileManager.uploadTask.current', { path: uploadTask.currentFile }) }}
          </span>
          <span v-else class="upload-task-current">{{ uploadStatusText }}</span>
          <span class="upload-task-percent">{{ uploadTask.progressPercent }}%</span>
        </div>
        <div
          class="upload-task-track"
          role="progressbar"
          data-ui="file-upload-progress"
          :aria-label="
            t('app.fileManager.uploadTask.progressAria', { name: uploadTask.displayName })
          "
          aria-valuemin="0"
          aria-valuemax="100"
          :aria-valuenow="
            uploadTask.status === 'preparing' ? undefined : uploadTask.progressPercent
          "
          :aria-busy="uploadTask.status === 'preparing'"
        >
          <div class="upload-task-value" :style="{ width: `${uploadTask.progressPercent}%` }" />
        </div>

        <div class="upload-task-summary">
          <span>
            {{
              t('app.fileManager.uploadTask.fileProgress', {
                processed: uploadProcessedFiles,
                total: uploadTask.totalFiles,
              })
            }}
          </span>
          <span>
            {{
              t('app.fileManager.uploadTask.byteProgress', {
                transferred: formatBytes(uploadTask.transferredBytes),
                total: formatBytes(uploadTask.totalBytes),
              })
            }}
          </span>
        </div>

        <div v-if="uploadTask.errorSummary" class="upload-task-error" role="alert">
          {{ uploadTask.errorSummary }}
        </div>
        <ul v-if="uploadTask.failures.length" class="upload-task-failures" role="list">
          <li v-for="failure in uploadTask.failures.slice(0, 3)" :key="failure.path">
            <span :title="failure.path">{{ failure.path }}</span>
            <span>{{ failure.message }}</span>
          </li>
          <li v-if="uploadTask.failures.length > 3">
            {{
              t('app.fileManager.uploadTask.moreFailures', {
                count: uploadTask.failures.length - 3,
              })
            }}
          </li>
        </ul>
      </div>
      <template #footer>
        <VustButton
          v-if="uploadActive"
          type="secondary"
          :disabled="uploadTask.status === 'cancelling'"
          data-ui="file-upload-cancel"
          @click="cancelUpload"
        >
          {{
            uploadTask.status === 'cancelling'
              ? t('app.fileManager.uploadTask.cancelling')
              : t('app.fileManager.uploadTask.cancel')
          }}
        </VustButton>
        <VustButton
          v-else
          type="secondary"
          data-ui="file-upload-dismiss"
          @click="handleUploadDialogClose"
        >
          {{ t('app.fileManager.uploadTask.dismiss') }}
        </VustButton>
      </template>
    </ApplicationDialog>

    <ApplicationConfirmationDialog
      :visible="confirmationState.visible"
      :title="confirmationState.title"
      :message="confirmationState.message"
      :confirm-text="confirmationState.confirmText"
      :cancel-text="confirmationState.cancelText"
      :type="confirmationState.type"
      return-focus-selector="[data-ui='file-table']"
      dialog-ui="file-confirmation-dialog"
      @confirm="handleConfirmationResponse(true)"
      @cancel="handleConfirmationResponse(false)"
    />

    <VustLoading :loading="listLoading && !listLoaded" :text="t('app.fileManager.loading')" cover />
  </section>
</template>

<style scoped>
.file-manager {
  font-family: var(--vdl-font-family);
  color: var(--vdl-text-primary);
  height: 100%;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-4);
  padding: var(--vdl-space-4);
  background: var(--vdl-bg-canvas);
  border-radius: var(--vdl-radius-lg);
  overflow: hidden;
  box-sizing: border-box;
  min-height: 0;
  position: relative;
}

.toolbar {
  padding: var(--vdl-space-2) var(--vdl-space-3);
  display: flex;
  flex-wrap: wrap;
  justify-content: space-between;
  gap: var(--vdl-space-3);
  align-items: center;
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-lg);
  background: var(--vdl-bg-panel);
}

.path-bar {
  display: flex;
  align-items: center;
  flex: 1;
  min-width: 300px;
}

.vl-input-group {
  display: flex;
  width: 100%;
}

.path-input {
  flex: 1;
}

.path-input :deep(.vl-input-inner-wrapper) {
  border-top-right-radius: 0;
  border-bottom-right-radius: 0;
  border-right: none;
}

.path-btn {
  height: auto;
  align-self: stretch;
  border-top-left-radius: 0;
  border-bottom-left-radius: 0;
}

.nav-actions {
  display: flex;
  gap: var(--vdl-space-2);
  align-items: center;
  flex-wrap: wrap;
}

.upload-input {
  display: none;
}

.upload-task {
  display: grid;
  gap: var(--vdl-space-3);
}

.upload-task-header,
.upload-task-progress-meta,
.upload-task-title-wrap,
.upload-task-summary {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-3);
}

.upload-task-header,
.upload-task-progress-meta {
  justify-content: space-between;
}

.upload-task-title-wrap,
.upload-task-title-content,
.upload-task-current {
  min-width: 0;
}

.upload-task-title-content {
  display: grid;
  gap: var(--vdl-space-1);
}

.upload-task-title,
.upload-task-target,
.upload-task-current,
.upload-task-failures span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.upload-task-title {
  font: var(--vdl-font-body-sm);
}

.upload-task-target,
.upload-task-progress-meta,
.upload-task-summary,
.upload-task-failures,
.upload-task-error {
  color: var(--vdl-text-muted);
  font: var(--vdl-font-caption);
}

.upload-task-target,
.upload-task-current,
.upload-task-failures li span:first-child {
  font-family: var(--vdl-font-mono);
}

.upload-task-current {
  flex: 1;
}

.upload-task-percent {
  color: var(--vdl-text-primary);
  font-family: var(--vdl-font-mono);
}

.upload-task-track {
  overflow: hidden;
  height: var(--vdl-space-2);
  border-radius: var(--vdl-radius-pill);
  background: var(--vdl-border-default);
}

.upload-task-value {
  height: 100%;
  border-radius: inherit;
  background: var(--vdl-primary);
  transition: width 180ms ease;
}

.upload-task-summary {
  flex-wrap: wrap;
}

.upload-task-error {
  color: var(--vdl-danger);
}

.upload-task-failures {
  display: grid;
  gap: var(--vdl-space-1);
  margin: 0;
  padding: var(--vdl-space-2) 0 0 var(--vdl-space-5);
  border-top: 1px solid var(--vdl-border-subtle);
}

.upload-task-failures li {
  display: grid;
  grid-template-columns: minmax(8rem, 1fr) minmax(8rem, 2fr);
  gap: var(--vdl-space-3);
}

.options-bar {
  display: flex;
  align-items: center;
}

.list-container {
  background: var(--vdl-bg-panel);
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-lg);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  flex: 1;
  min-height: 0;
  position: relative;
}

.breadcrumb-wrap {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-3) var(--vdl-space-4);
  border-bottom: 1px solid var(--vdl-border-default);
  background: var(--vdl-bg-muted);
}

.list-actions {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
  flex-wrap: wrap;
}

.link-btn {
  border: none;
  background: transparent;
  color: var(--vdl-primary);
  padding: 0;
  font-size: var(--vdl-font-body-sm);
  cursor: pointer;
}

.link-btn:hover {
  text-decoration: underline;
}

.list-state {
  padding: var(--vdl-space-3) var(--vdl-space-4) 0;
  font-size: var(--vdl-font-caption);
  color: var(--vdl-text-muted);
}

.list-state.error {
  color: var(--vdl-danger);
}

.table-wrapper {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}

.file-table {
  height: 100%;
}

.entry-name {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
  padding: var(--vdl-space-1) 0;
  cursor: pointer;
  border-radius: var(--vdl-radius-sm);
  transition: all 0.2s;
}

.entry-name.is-selected {
  background-color: var(--vdl-bg-active);
  box-shadow: 0 0 0 4px var(--vdl-bg-active);
}

.entry-name-text {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.entry-icon {
  color: var(--vdl-text-secondary);
}

.pagination-bar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-3) var(--vdl-space-4);
  border-top: 1px solid var(--vdl-border-default);
  font-size: var(--vdl-font-caption);
  color: var(--vdl-text-muted);
  flex-wrap: wrap;
}

.page-size-select {
  width: 110px;
}

.page-stats {
  white-space: nowrap;
}

.file-context-danger {
  color: var(--vdl-danger);
}

.file-context-danger:hover {
  background: var(--vdl-danger-muted);
}

@media (max-width: 1100px) {
  .toolbar {
    flex-direction: column;
    align-items: stretch;
  }
  .options-bar {
    align-items: flex-start;
  }
  .pagination-bar {
    flex-direction: column;
    align-items: flex-start;
    gap: var(--vdl-space-4);
  }
  .breadcrumb-wrap {
    flex-direction: column;
    align-items: flex-start;
  }
  .upload-task-summary {
    display: grid;
  }
}
</style>
