<script setup lang="ts">
/**
 * @file DockerImageList.vue
 * @description 当前节点 Docker 镜像列表，提供独立加载、刷新和安全删除状态。
 */

import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { dockerApi } from '@/api/modules/docker'
import { useDockerStore } from '@/stores/docker'
import { useNodeStore } from '@/stores/node'
import { useToastStore } from '@/stores/toast'
import { formatImageTags, formatBytes, formatDockerImageBytes } from '@/utils/docker-format'
import {
  VustAlert,
  VustButton,
  VustDialog,
  VustEmpty,
  VustInput,
  VustLoading,
  VustTable,
  VustTag,
} from '@/components/ui'
import type { VustTableColumn } from '@/components/ui/VustTable.vue'
import type { DockerImageSummary } from '@/api/interface/docker'

const { t, locale } = useI18n()
const store = useDockerStore()
const nodeStore = useNodeStore()
const toastStore = useToastStore()
const searchQuery = ref('')
const fileInput = ref<HTMLInputElement | null>(null)
const importVisible = ref(false)
const importFile = ref<File | null>(null)
const importError = ref('')
const importInProgress = ref(false)
const importStage = ref<'idle' | 'uploading' | 'loading'>('idle')
const uploadProgress = ref(0)
const importTargetNodeId = ref('')
const importTargetNodeName = ref('')
let importAbortController: AbortController | null = null

const MAX_IMAGE_ARCHIVE_BYTES = 10_000_000_000
const IMAGE_ARCHIVE_SUFFIXES = [
  '.tar.gz',
  '.tar.bz2',
  '.tar.xz',
  '.tar.zst',
  '.tgz',
  '.tbz',
  '.tbz2',
  '.txz',
  '.tzst',
  '.tar',
]
const IMAGE_ARCHIVE_ACCEPT = IMAGE_ARCHIVE_SUFFIXES.join(',')

const filteredImages = computed(() => {
  const query = searchQuery.value.trim().toLowerCase()
  if (!query) return store.imagesList
  return store.imagesList.filter((image) => {
    const shortId = image.id.replace(/^sha256:/, '').substring(0, 12)
    return (
      image.id.toLowerCase().includes(query) ||
      shortId.toLowerCase().includes(query) ||
      image.tags.some((tag) => tag.toLowerCase().includes(query)) ||
      image.digests.some((digest) => digest.toLowerCase().includes(query))
    )
  })
})

const columns = computed<VustTableColumn[]>(() => [
  { label: t('app.docker.images.columns.tags'), minWidth: 240, slot: 'tags' },
  { label: t('app.docker.images.columns.id'), width: 140, slot: 'id', align: 'center' },
  { label: t('app.docker.images.columns.size'), width: 120, slot: 'size', align: 'center' },
  {
    label: t('app.docker.images.columns.containers'),
    width: 100,
    align: 'center',
    slot: 'containers',
  },
  {
    label: t('app.docker.images.columns.createdAt'),
    minWidth: 180,
    slot: 'createdAt',
    align: 'center',
  },
  { label: t('app.docker.images.columns.actions'), width: 100, align: 'center', slot: 'actions' },
])

const formatCreatedAt = (createdAt: number) =>
  new Date(createdAt * 1000).toLocaleString(locale.value === 'zh' ? 'zh-CN' : 'en-US')

const shortImageId = (id: string) => id.replace(/^sha256:/, '').substring(0, 12)

const importStatusText = computed(() =>
  importStage.value === 'loading'
    ? t('app.docker.images.archiveImport.loading')
    : t('app.docker.images.archiveImport.uploading'),
)

/** 校验待导入归档的文件名和十进制 10 GB 大小边界。 */
function validateImageArchive(file: File): string {
  const normalizedName = file.name.trim().toLowerCase()
  if (
    !normalizedName ||
    !IMAGE_ARCHIVE_SUFFIXES.some((suffix) => normalizedName.endsWith(suffix))
  ) {
    return t('app.docker.images.archiveImport.invalidFormat')
  }
  if (file.size === 0) {
    return t('app.docker.images.archiveImport.emptyFile')
  }
  if (file.size > MAX_IMAGE_ARCHIVE_BYTES) {
    return t('app.docker.images.archiveImport.tooLarge')
  }
  return ''
}

/** 清理弹窗内的临时文件和上传状态。 */
function resetImportState() {
  importFile.value = null
  importError.value = ''
  importInProgress.value = false
  importStage.value = 'idle'
  uploadProgress.value = 0
  importAbortController = null
  if (fileInput.value) fileInput.value.value = ''
}

/** 打开镜像归档导入弹窗并固定本次操作的目标节点。 */
function openImportDialog() {
  resetImportState()
  const nodeId = nodeStore.currentNodeId || 'local'
  importTargetNodeId.value = nodeId
  importTargetNodeName.value = nodeStore.nodes.find((node) => node.id === nodeId)?.name || nodeId
  importVisible.value = true
}

/** 打开原生文件选择器。 */
function selectImportFile() {
  if (!importInProgress.value) fileInput.value?.click()
}

/** 接收并预检用户选择的镜像归档。 */
function handleImportFileChange(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  importError.value = ''
  importFile.value = null
  if (!file) return

  const validationError = validateImageArchive(file)
  if (validationError) {
    importError.value = validationError
    input.value = ''
    return
  }
  importFile.value = file
}

/** 中止当前上传并关闭弹窗；用户主动取消时提供中性反馈。 */
function closeImportDialog(showFeedback = true) {
  const wasInProgress = importInProgress.value
  importAbortController?.abort()
  importVisible.value = false
  resetImportState()
  if (wasInProgress && showFeedback) {
    toastStore.info(t('app.docker.images.archiveImport.cancelled'))
  }
}

/** 上传镜像归档；传输完成后保持等待，直至 Docker Engine 载入结束。 */
async function startImageImport() {
  if (!importFile.value || importInProgress.value) return
  const validationError = validateImageArchive(importFile.value)
  if (validationError) {
    importError.value = validationError
    return
  }

  const targetNodeId = importTargetNodeId.value
  const formData = new FormData()
  formData.append('file', importFile.value, importFile.value.name)
  const controller = new AbortController()
  importAbortController = controller
  importInProgress.value = true
  importStage.value = 'uploading'
  uploadProgress.value = 0
  importError.value = ''

  try {
    const response = await dockerApi.forNode(targetNodeId).loadImage(formData, {
      signal: controller.signal,
      onUploadProgress: ({ loaded, total }) => {
        if (!total || controller.signal.aborted) return
        uploadProgress.value = Math.min(100, Math.round((loaded / total) * 100))
        if (uploadProgress.value >= 100) importStage.value = 'loading'
      },
    })
    if (controller.signal.aborted) return
    if (!response.success) {
      importError.value = response.message || t('app.docker.images.archiveImport.failed')
      return
    }
    if (nodeStore.currentNodeId !== targetNodeId) return

    importVisible.value = false
    resetImportState()
    toastStore.success(t('app.docker.images.archiveImport.success'))
    await Promise.allSettled([store.fetchImagesList(), store.fetchOverviewData()])
  } catch (error) {
    if (controller.signal.aborted) return
    console.error('Failed to import Docker image archive', error)
    importError.value = t('app.docker.images.archiveImport.failed')
  } finally {
    if (importAbortController === controller) {
      importAbortController = null
      importInProgress.value = false
      importStage.value = 'idle'
    }
  }
}

watch(
  () => nodeStore.currentNodeId,
  (nodeId) => {
    searchQuery.value = ''
    if (importVisible.value && nodeId !== importTargetNodeId.value) {
      const wasInProgress = importInProgress.value
      closeImportDialog(false)
      if (wasInProgress) {
        toastStore.info(t('app.docker.images.archiveImport.nodeChangedCancelled'))
      }
    }
    void store.fetchImagesList()
  },
)

onMounted(() => {
  void store.fetchImagesList()
})

onBeforeUnmount(() => {
  importAbortController?.abort()
})
</script>

<template>
  <div class="image-list-page" data-page="docker-image-list">
    <div class="image-toolbar" data-ui="toolbar">
      <VustInput
        id="docker-local-image-search"
        v-model="searchQuery"
        name="docker-local-image-search"
        :placeholder="t('app.docker.images.searchPlaceholder')"
        clearable
        class="search-input"
        data-ui="image-search-input"
      />
      <div class="image-toolbar-actions" data-slot="actions">
        <VustButton
          type="primary"
          size="small"
          data-ui="image-import-button"
          @click="openImportDialog"
        >
          {{ t('app.docker.images.archiveImport.action') }}
        </VustButton>
        <VustButton
          size="small"
          :loading="store.imageListLoading"
          data-ui="image-refresh-button"
          @click="store.fetchImagesList"
        >
          {{ t('common.refresh') }}
        </VustButton>
      </div>
    </div>

    <VustAlert
      v-if="store.imageListError"
      type="warning"
      :title="t('app.docker.images.refreshFailed')"
      :description="store.imageListError"
      show-icon
      data-ui="image-list-error"
    />

    <div class="image-table-shell" data-ui="table">
      <VustTable v-if="filteredImages.length" :data="filteredImages" :columns="columns" border>
        <template #tags="{ row }: { row: DockerImageSummary }">
          <div class="resource-name-cell">
            <span :title="formatImageTags(row.tags)">{{ formatImageTags(row.tags) }}</span>
            <VustTag v-if="row.dangling" type="warning" size="small">
              {{ t('app.docker.images.dangling') }}
            </VustTag>
          </div>
        </template>
        <template #id="{ row }: { row: DockerImageSummary }">
          <span class="image-id" :title="row.id">{{ shortImageId(row.id) }}</span>
        </template>
        <template #size="{ row }: { row: DockerImageSummary }">
          {{ formatDockerImageBytes(row.sizeBytes) }}
        </template>
        <template #containers="{ row }: { row: DockerImageSummary }">
          <VustTag type="info">{{ row.containerCount }}</VustTag>
        </template>
        <template #createdAt="{ row }: { row: DockerImageSummary }">
          {{ formatCreatedAt(row.createdAt) }}
        </template>
        <template #actions="{ row }: { row: DockerImageSummary }">
          <VustButton
            type="danger"
            size="small"
            :loading="store.imageDeleteLoadingId === row.id"
            :disabled="row.containerCount > 0 || Boolean(store.imageDeleteLoadingId)"
            :title="
              row.containerCount > 0
                ? t('app.docker.images.actions.inUse', { count: row.containerCount })
                : undefined
            "
            data-ui="image-delete-button"
            @click="store.handleDeleteImage(row.id)"
          >
            {{ t('app.docker.images.actions.delete') }}
          </VustButton>
        </template>
      </VustTable>
      <VustEmpty
        v-else-if="!store.imageListLoading"
        :description="
          searchQuery ? t('app.docker.images.filteredEmpty') : t('app.docker.images.emptyLocal')
        "
      />
      <VustLoading :loading="store.imageListLoading && !store.imagesList.length" cover />
    </div>

    <VustDialog
      :visible="importVisible"
      :title="t('app.docker.images.archiveImport.title')"
      width="600px"
      :close-on-click-overlay="false"
      data-ui="image-import-dialog"
      @close="closeImportDialog"
    >
      <div class="image-import-body" data-slot="body">
        <VustAlert
          type="info"
          :title="t('app.docker.images.archiveImport.target', { node: importTargetNodeName })"
          :description="t('app.docker.images.archiveImport.hint')"
          show-icon
        />

        <div class="image-import-file-row">
          <input
            ref="fileInput"
            class="native-file-input"
            type="file"
            :accept="IMAGE_ARCHIVE_ACCEPT"
            :disabled="importInProgress"
            data-ui="image-import-file-input"
            @change="handleImportFileChange"
          />
          <VustInput
            id="docker-image-archive-name"
            class="image-import-file-name"
            :model-value="importFile?.name || ''"
            :placeholder="t('app.docker.images.archiveImport.filePlaceholder')"
            :aria-label="t('app.docker.images.archiveImport.filePlaceholder')"
            readonly
            data-ui="image-import-file-name"
          />
          <VustButton
            type="secondary"
            :disabled="importInProgress"
            data-ui="image-import-file-select"
            @click="selectImportFile"
          >
            {{ t('app.docker.images.archiveImport.selectFile') }}
          </VustButton>
        </div>

        <div v-if="importFile" class="image-import-file-meta" data-ui="image-import-file-meta">
          <span>{{ importFile.name }}</span>
          <span>{{ formatBytes(importFile.size) }}</span>
        </div>

        <VustAlert
          v-if="importError"
          type="error"
          :title="t('app.docker.images.archiveImport.failedTitle')"
          :description="importError"
          show-icon
          data-ui="image-import-error"
        />

        <div v-if="importInProgress" class="image-import-progress" data-ui="image-import-progress">
          <div class="image-import-progress-meta">
            <span>{{ importStatusText }}</span>
            <span v-if="importStage === 'uploading'">{{ uploadProgress }}%</span>
          </div>
          <div
            class="image-import-progress-track"
            role="progressbar"
            :aria-label="importStatusText"
            aria-valuemin="0"
            aria-valuemax="100"
            :aria-valuenow="importStage === 'uploading' ? uploadProgress : undefined"
            :aria-busy="importStage === 'loading'"
          >
            <div
              class="image-import-progress-value"
              :class="{ 'is-indeterminate': importStage === 'loading' }"
              :style="importStage === 'uploading' ? { width: `${uploadProgress}%` } : undefined"
            />
          </div>
          <p v-if="importStage === 'loading'" class="image-import-progress-warning">
            {{ t('app.docker.images.archiveImport.loadingWarning') }}
          </p>
        </div>
      </div>

      <template #footer>
        <VustButton @click="closeImportDialog()">
          {{ t('common.cancel') }}
        </VustButton>
        <VustButton
          type="primary"
          :loading="importInProgress"
          :disabled="!importFile || importInProgress"
          data-ui="image-import-submit"
          @click="startImageImport"
        >
          {{ t('app.docker.images.archiveImport.submit') }}
        </VustButton>
      </template>
    </VustDialog>
  </div>
</template>

<style scoped>
.image-list-page {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
  height: 100%;
  min-height: 0;
}

.image-toolbar {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-3);
}

.search-input {
  width: min(320px, 100%);
}

.image-toolbar-actions {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
  margin-left: auto;
}

.image-table-shell {
  position: relative;
  flex: 1;
  min-height: 160px;
  overflow: auto;
}

.resource-name-cell {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: var(--vdl-space-2);
}

.resource-name-cell > span:first-child {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.image-id {
  color: var(--vdl-text-secondary);
  font-family: var(--vdl-font-mono);
}

.image-import-body {
  display: grid;
  gap: var(--vdl-space-4);
}

.image-import-file-row {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
}

.native-file-input {
  display: none;
}

.image-import-file-name {
  min-width: 0;
  flex: 1;
}

.image-import-file-meta,
.image-import-progress-meta {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-3);
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-caption);
}

.image-import-file-meta > span:first-child {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.image-import-file-meta > span:last-child,
.image-import-progress-meta > span:last-child {
  flex-shrink: 0;
  font-variant-numeric: tabular-nums;
}

.image-import-progress {
  display: grid;
  gap: var(--vdl-space-2);
}

.image-import-progress-track {
  width: 100%;
  height: 8px;
  overflow: hidden;
  border-radius: var(--vdl-radius-pill);
  background: var(--vdl-bg-muted);
}

.image-import-progress-value {
  height: 100%;
  min-width: 2px;
  border-radius: inherit;
  background: var(--vdl-primary);
  transition: width 180ms ease;
}

.image-import-progress-value.is-indeterminate {
  width: 38%;
  animation: image-import-progress-indeterminate 1.2s ease-in-out infinite;
}

.image-import-progress-warning {
  margin: 0;
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-caption);
}

@keyframes image-import-progress-indeterminate {
  0% {
    transform: translateX(-110%);
  }

  100% {
    transform: translateX(300%);
  }
}

@media (max-width: 640px) {
  .image-toolbar {
    align-items: stretch;
    flex-direction: column;
  }

  .search-input {
    width: 100%;
  }

  .image-toolbar-actions {
    align-self: flex-end;
    margin-left: 0;
  }

  .image-import-file-row {
    align-items: stretch;
    flex-wrap: wrap;
  }

  .image-import-file-name {
    min-width: 100%;
  }
}

@media (prefers-reduced-motion: reduce) {
  .image-import-progress-value {
    transition: none;
  }

  .image-import-progress-value.is-indeterminate {
    animation: none;
  }
}
</style>
