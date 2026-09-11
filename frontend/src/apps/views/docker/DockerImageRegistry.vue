<script setup lang="ts">
/**
 * @file DockerImageRegistry.vue
 * @description Docker 镜像仓库页面，负责搜索镜像、选择 tag 并拉取到当前节点。
 */

import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  dockerApi,
  type ImagePullProgress,
  type ImageSearchResult,
  type ImageTagInfo,
} from '@/api/modules/docker'
import { useDockerStore } from '@/stores/docker'
import { useNodeStore } from '@/stores/node'
import { useToastStore } from '@/stores/toast'
import {
  VustAlert,
  VustButton,
  VustDialog,
  VustInput,
  VustSelect,
  VustTag,
  VustTabs,
} from '@/components/ui'

const SEARCH_PAGE_SIZE = 20
const TAG_PAGE_SIZE = 10
const DEFAULT_TAG = 'latest'

const { t } = useI18n()
const dockerStore = useDockerStore()
const nodeStore = useNodeStore()
const toastStore = useToastStore()
const activeMode = ref<'search' | 'reference'>('search')
const directReference = ref('')
const directReferenceError = ref('')
const activeImageRef = ref('')

const keyword = ref('')
const searchResults = ref<ImageSearchResult[]>([])
const searchPage = ref(1)
const searchHasMore = ref(false)
const isSearching = ref(false)
const searchError = ref('')

const dialogVisible = ref(false)
const selectedImage = ref<ImageSearchResult | null>(null)
const tags = ref<ImageTagInfo[]>([])
const selectedTag = ref('')
const tagPage = ref(1)
const tagHasMore = ref(false)
const isLoadingTags = ref(false)
const tagError = ref('')

const pullProgress = ref<ImagePullProgress | null>(null)
const isStartingPull = ref(false)
const isCanceling = ref(false)
let pollTimer: number | null = null
let searchRequestSequence = 0
let tagRequestSequence = 0

const currentNodeName = computed(() => {
  const node = nodeStore.nodes.find((item) => item.id === nodeStore.currentNodeId)
  return node?.name || nodeStore.currentNodeId || 'local'
})

const canSearch = computed(() => keyword.value.trim().length > 0 && !isSearching.value)
const modeTabs = computed(() => [
  { label: t('app.docker.images.registry.modes.search'), name: 'search' },
  { label: t('app.docker.images.registry.modes.reference'), name: 'reference' },
])
const canPullReference = computed(
  () => directReference.value.trim().length > 0 && !isPulling.value && !isStartingPull.value,
)
const isPulling = computed(
  () => pullProgress.value?.status === 'pending' || pullProgress.value?.status === 'running',
)
const canPull = computed(
  () => Boolean(selectedImage.value) && !isPulling.value && !isStartingPull.value,
)
const pullActionDisabled = computed(() => (isPulling.value ? isCanceling.value : !canPull.value))
const pullStatusText = computed(() => {
  if (!pullProgress.value) return ''
  return (
    pullProgress.value.statusText ||
    t(`app.docker.images.registry.status.${pullProgress.value.status}`)
  )
})
const visiblePullProgress = computed(() => {
  if (!pullProgress.value || !['pending', 'running'].includes(pullProgress.value.status)) {
    return null
  }
  return pullProgress.value
})
const tagOptions = computed(() =>
  tags.value.map((tag) => ({
    label: tag.name,
    value: tag.name,
  })),
)

const formatCount = (value?: number) => {
  if (value == null) return '-'
  return new Intl.NumberFormat().format(value)
}

const stopPolling = () => {
  if (pollTimer !== null) {
    window.clearTimeout(pollTimer)
    pollTimer = null
  }
}

const resetPullState = () => {
  stopPolling()
  pullProgress.value = null
  isStartingPull.value = false
  isCanceling.value = false
}

const resetDialogState = () => {
  resetPullState()
  selectedImage.value = null
  tags.value = []
  selectedTag.value = ''
  tagPage.value = 1
  tagHasMore.value = false
  isLoadingTags.value = false
  tagError.value = ''
  dialogVisible.value = false
}

const searchImages = async (append = false) => {
  const normalizedKeyword = keyword.value.trim()
  if (!normalizedKeyword || isSearching.value) return

  const nodeId = nodeStore.currentNodeId
  const sequence = ++searchRequestSequence
  isSearching.value = true
  searchError.value = ''
  const nextPage = append ? searchPage.value + 1 : 1
  try {
    const response = await dockerApi.forNode(nodeId).searchImages({
      keyword: normalizedKeyword,
      page: nextPage,
      pageSize: SEARCH_PAGE_SIZE,
    })
    if (sequence !== searchRequestSequence || nodeId !== nodeStore.currentNodeId) return
    if (!response.success || !response.data) {
      searchError.value = response.message || t('common.unknownError')
      if (!append) searchResults.value = []
      return
    }
    searchResults.value = append
      ? [...searchResults.value, ...response.data.results]
      : response.data.results
    searchPage.value = response.data.page
    searchHasMore.value = response.data.hasMore
  } catch (error) {
    console.error('Failed to search images:', error)
    searchError.value = error instanceof Error ? error.message : String(error)
    if (!append && sequence === searchRequestSequence) searchResults.value = []
  } finally {
    if (sequence === searchRequestSequence) isSearching.value = false
  }
}

const openDownloadDialog = async (image: ImageSearchResult) => {
  resetPullState()
  selectedImage.value = image
  tags.value = []
  selectedTag.value = ''
  tagPage.value = 1
  tagHasMore.value = false
  tagError.value = ''
  dialogVisible.value = true
  await loadTags(false)
}

const closeDialog = () => {
  if (isPulling.value || isStartingPull.value || isCanceling.value) return
  resetDialogState()
}

const loadTags = async (append = false) => {
  if (!selectedImage.value || isLoadingTags.value) return
  if (append && !tagHasMore.value) return

  const nodeId = nodeStore.currentNodeId
  const sequence = ++tagRequestSequence
  isLoadingTags.value = true
  tagError.value = ''
  const nextPage = append ? tagPage.value + 1 : 1
  try {
    const response = await dockerApi.forNode(nodeId).fetchImageTags({
      repository: selectedImage.value.repository,
      page: nextPage,
      pageSize: TAG_PAGE_SIZE,
    })
    if (sequence !== tagRequestSequence || nodeId !== nodeStore.currentNodeId) return
    if (!response.success || !response.data) {
      tagError.value = response.message || t('common.unknownError')
      if (!append) tags.value = []
      return
    }
    tags.value = append ? [...tags.value, ...response.data.tags] : response.data.tags
    tagPage.value = response.data.page
    tagHasMore.value = response.data.hasMore
  } catch (error) {
    console.error('Failed to load image tags:', error)
    tagError.value = error instanceof Error ? error.message : String(error)
    if (!append && sequence === tagRequestSequence) tags.value = []
  } finally {
    if (sequence === tagRequestSequence) isLoadingTags.value = false
  }
}

const fetchPullProgress = async (taskId: string) => {
  try {
    const response = await dockerApi.fetchImageTaskProgress(taskId)
    if (!response.success || !response.data) return

    pullProgress.value = response.data
    if (['success', 'failed', 'cancelled'].includes(response.data.status)) {
      stopPolling()
      isCanceling.value = false
      if (response.data.status === 'success') {
        toastStore.success(t('app.docker.images.registry.success'))
        await dockerStore.fetchImagesList()
      } else if (response.data.status === 'failed') {
        toastStore.error(
          t('app.docker.images.registry.failed', {
            error:
              response.data.registryError ||
              response.data.controllerError ||
              response.data.statusText,
          }),
        )
      } else if (response.data.status === 'cancelled') {
        toastStore.success(t('app.docker.images.registry.cancelled'))
      }
    }
  } catch (error) {
    console.error('Failed to fetch image pull progress:', error)
    stopPolling()
    const errMsg = error instanceof Error ? error.message : String(error)
    pullProgress.value = {
      taskId,
      nodeId: nodeStore.currentNodeId,
      imageRef: activeImageRef.value,
      status: 'failed',
      stage: 'pulling',
      progressPercent: pullProgress.value?.progressPercent ?? 0,
      statusText: t('app.docker.images.registry.progressFailed'),
      registryError: errMsg,
    }
    isCanceling.value = false
    toastStore.error(t('app.docker.images.registry.failed', { error: errMsg }))
  }
}

const startPolling = (taskId: string) => {
  stopPolling()
  const poll = async () => {
    await fetchPullProgress(taskId)
    if (isPulling.value) pollTimer = window.setTimeout(poll, 1000)
  }
  void poll()
}

const startImagePull = async (imageRef: string) => {
  if (!imageRef || isStartingPull.value) return
  isStartingPull.value = true
  pullProgress.value = null
  activeImageRef.value = imageRef
  try {
    const response = await dockerApi.startImageTask({
      nodeId: nodeStore.currentNodeId,
      imageRef,
      sourceMode: 'controller-first',
    })
    if (response.success && response.data?.taskId) {
      startPolling(response.data.taskId)
      return
    }
    toastStore.error(
      t('app.docker.images.registry.failed', {
        error: response.message || t('common.unknownError'),
      }),
    )
  } catch (error) {
    console.error('Failed to start image pull:', error)
    const errMsg = error instanceof Error ? error.message : String(error)
    toastStore.error(t('app.docker.images.registry.failed', { error: errMsg }))
  } finally {
    isStartingPull.value = false
  }
}

const pullImage = async () => {
  if (!selectedImage.value) return
  await startImagePull(`${selectedImage.value.repository}:${selectedTag.value || DEFAULT_TAG}`)
}

const pullDirectReference = async () => {
  const imageRef = directReference.value.trim()
  directReferenceError.value = ''
  if (!imageRef || /\s/.test(imageRef) || imageRef.includes('://')) {
    directReferenceError.value = t('app.docker.images.registry.referenceInvalid')
    return
  }
  await startImagePull(imageRef)
}

const cancelPull = async () => {
  const taskId = pullProgress.value?.taskId
  if (!taskId || isCanceling.value) return

  isCanceling.value = true
  try {
    const response = await dockerApi.cancelImageTask(taskId)
    if (response.success && response.data) {
      pullProgress.value = response.data
      if (['success', 'failed', 'cancelled'].includes(response.data.status)) {
        isCanceling.value = false
      }
    } else {
      isCanceling.value = false
    }
  } catch (error) {
    console.error('Failed to cancel image pull:', error)
    const errMsg = error instanceof Error ? error.message : String(error)
    isCanceling.value = false
    toastStore.error(t('app.docker.images.registry.cancelFailed', { error: errMsg }))
  }
}

watch(keyword, () => {
  searchRequestSequence += 1
  isSearching.value = false
  searchResults.value = []
  searchError.value = ''
  searchPage.value = 1
  searchHasMore.value = false
})
watch(
  () => nodeStore.currentNodeId,
  () => {
    searchRequestSequence += 1
    tagRequestSequence += 1
    keyword.value = ''
    searchResults.value = []
    searchError.value = ''
    searchPage.value = 1
    searchHasMore.value = false
    isSearching.value = false
    isLoadingTags.value = false
    directReference.value = ''
    directReferenceError.value = ''
    resetDialogState()
  },
)

onBeforeUnmount(() => {
  stopPolling()
})
</script>

<template>
  <div class="registry-page" data-ui="docker-image-registry">
    <VustAlert
      type="info"
      show-icon
      :description="t('app.docker.images.registry.targetHint', { node: currentNodeName })"
      data-ui="registry-target-hint"
    />

    <VustTabs v-model="activeMode" :tabs="modeTabs" data-ui="registry-mode-tabs" />

    <div v-if="activeMode === 'search'" class="registry-mode" data-slot="docker-hub-search">
      <div class="registry-form" data-ui="registry-search-form">
        <VustInput
          id="docker-image-registry-search"
          v-model="keyword"
          name="docker-image-registry-search"
          :placeholder="t('app.docker.images.registry.placeholder')"
          :disabled="isSearching"
          data-ui="registry-image-input"
          @keyup.enter="searchImages(false)"
        />
        <VustButton
          type="primary"
          :disabled="!canSearch"
          class="registry-action-button"
          data-ui="registry-search-button"
          @click="searchImages(false)"
        >
          {{
            isSearching
              ? t('app.docker.images.registry.searching')
              : t('app.docker.images.registry.search')
          }}
        </VustButton>
      </div>

      <VustAlert
        v-if="searchError"
        type="error"
        show-icon
        :description="t('app.docker.images.registry.searchFailed', { error: searchError })"
        data-ui="registry-search-error"
      />

      <div class="image-results" data-ui="registry-search-results">
        <div v-if="searchResults.length" class="image-list">
          <div
            v-for="image in searchResults"
            :key="image.repository"
            class="image-row"
            data-ui="registry-image-result"
          >
            <div class="image-main">
              <div class="image-title">
                <span>{{ image.displayName }}</span>
                <VustTag v-if="image.isOfficial" type="success">
                  {{ t('app.docker.images.registry.official') }}
                </VustTag>
                <VustTag v-else-if="image.isAutomated" type="warning">
                  {{ t('app.docker.images.registry.automated') }}
                </VustTag>
              </div>
              <p class="image-description">
                {{ image.description || t('app.docker.images.registry.noDescription') }}
              </p>
              <div class="image-meta">
                <span>{{
                  t('app.docker.images.registry.stars', { count: formatCount(image.starCount) })
                }}</span>
                <span>{{
                  t('app.docker.images.registry.pulls', { count: formatCount(image.pullCount) })
                }}</span>
              </div>
            </div>
            <VustButton
              type="primary"
              size="small"
              data-ui="registry-download-button"
              @click="openDownloadDialog(image)"
            >
              {{ t('app.docker.images.registry.download') }}
            </VustButton>
          </div>
        </div>
        <VustButton
          v-if="searchHasMore"
          :disabled="isSearching"
          class="load-more-button"
          data-ui="registry-load-more"
          @click="searchImages(true)"
        >
          {{
            isSearching
              ? t('app.docker.images.registry.loadingMore')
              : t('app.docker.images.registry.loadMore')
          }}
        </VustButton>
      </div>
    </div>

    <div v-else class="registry-mode reference-mode" data-slot="direct-reference">
      <VustAlert
        type="info"
        show-icon
        :description="t('app.docker.images.registry.referenceHint')"
      />
      <div class="registry-form" data-ui="registry-reference-form">
        <VustInput
          id="docker-image-reference"
          v-model="directReference"
          name="docker-image-reference"
          :placeholder="t('app.docker.images.registry.referencePlaceholder')"
          :disabled="isPulling || isStartingPull"
          data-ui="registry-reference-input"
          @keyup.enter="pullDirectReference"
        />
        <VustButton
          type="primary"
          :loading="isStartingPull"
          :disabled="!canPullReference"
          class="registry-action-button"
          data-ui="registry-reference-pull"
          @click="pullDirectReference"
        >
          {{ t('app.docker.images.registry.pull') }}
        </VustButton>
      </div>
      <VustAlert
        v-if="directReferenceError"
        type="error"
        show-icon
        :description="directReferenceError"
      />
      <div
        v-if="visiblePullProgress"
        class="inline-pull-progress"
        data-ui="reference-pull-progress"
      >
        <div class="progress-track">
          <div
            class="progress-fill"
            :class="visiblePullProgress.status"
            :style="{ width: `${visiblePullProgress.progressPercent}%` }"
          />
        </div>
        <div class="progress-inline-meta">
          <span>{{ visiblePullProgress.progressPercent }}%</span>
          <span>{{ pullStatusText }}</span>
        </div>
      </div>
      <VustButton
        v-if="isPulling"
        type="danger"
        size="small"
        :loading="isCanceling"
        class="reference-cancel-button"
        @click="cancelPull"
      >
        {{ t('app.docker.images.registry.cancel') }}
      </VustButton>
    </div>

    <VustDialog
      :visible="dialogVisible"
      :title="t('app.docker.images.registry.downloadTitle')"
      width="520px"
      :close-on-click-overlay="false"
      data-ui="registry-download-dialog"
      @close="closeDialog"
    >
      <div v-if="selectedImage" class="download-dialog-body" data-slot="body">
        <div class="download-field" data-ui="registry-download-image-name">
          <div class="download-field-label">{{ t('app.docker.images.registry.imageName') }}</div>
          <div class="download-field-value">{{ selectedImage.displayName }}</div>
        </div>
        <div class="download-field" data-ui="registry-tag-select">
          <div class="download-field-label">{{ t('app.docker.images.registry.version') }}</div>
          <div class="tag-select-row">
            <VustSelect
              v-model="selectedTag"
              :options="tagOptions"
              :disabled="isPulling || isLoadingTags"
              :placeholder="t('app.docker.images.registry.selectTag')"
              data-ui="registry-tag-select-control"
              @dropdown-reach-bottom="loadTags(true)"
            >
              <template v-if="isLoadingTags" #dropdown-footer>
                <div class="tag-loading-state" data-ui="registry-tag-loading">
                  <span class="tag-loading-dot" />
                  <span>{{ t('app.docker.images.registry.loadingTags') }}</span>
                </div>
              </template>
            </VustSelect>
          </div>
        </div>

        <VustAlert
          v-if="tagError"
          type="error"
          show-icon
          :description="t('app.docker.images.registry.tagsFailed', { error: tagError })"
          data-ui="registry-tags-error"
        />

        <div
          v-if="visiblePullProgress"
          class="inline-pull-progress"
          data-ui="registry-pull-progress"
        >
          <div class="progress-track">
            <div
              class="progress-fill"
              :class="visiblePullProgress.status"
              :style="{ width: `${visiblePullProgress.progressPercent}%` }"
            />
          </div>
          <div class="progress-inline-meta">
            <span>{{ visiblePullProgress.progressPercent }}%</span>
            <span>{{ pullStatusText }}</span>
          </div>
        </div>
      </div>

      <template #footer>
        <VustButton :disabled="isPulling || isStartingPull || isCanceling" @click="closeDialog">
          {{ t('common.cancel') }}
        </VustButton>
        <VustButton
          :type="isPulling ? 'danger' : 'primary'"
          :disabled="pullActionDisabled"
          data-ui="registry-pull-button"
          @click="isPulling ? cancelPull() : pullImage()"
        >
          {{
            isCanceling
              ? t('app.docker.images.registry.canceling')
              : isPulling
                ? t('app.docker.images.registry.cancel')
                : isStartingPull
                  ? t('app.docker.images.registry.pulling')
                  : t('app.docker.images.registry.pull')
          }}
        </VustButton>
      </template>
    </VustDialog>
  </div>
</template>

<style scoped>
.registry-page {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-4);
  height: 100%;
  min-height: 0;
  padding: var(--vdl-space-1);
}

.registry-form {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-3);
  width: 100%;
}

.registry-mode {
  display: flex;
  flex: 1;
  min-height: 0;
  flex-direction: column;
  gap: var(--vdl-space-3);
}

.reference-cancel-button {
  align-self: flex-start;
}

.registry-form :deep(.vl-input-wrapper) {
  flex: 1;
}

.registry-action-button {
  min-width: 108px;
  height: 38px;
  flex-shrink: 0;
}

.image-results {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
  max-width: 860px;
  min-height: 0;
}

.image-list {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-2);
  min-height: 0;
  overflow: auto;
}

.image-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-4);
  padding: var(--vdl-space-3);
  border: 1px solid var(--vdl-color-border);
  border-radius: 8px;
  background: var(--vdl-color-bg-container);
}

.image-main {
  min-width: 0;
}

.image-title {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
  color: var(--vdl-color-text);
  font-size: 14px;
  font-weight: 600;
}

.image-description {
  margin: var(--vdl-space-1) 0;
  overflow: hidden;
  color: var(--vdl-color-text-secondary);
  font-size: 12px;
  line-height: 1.5;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.image-meta {
  display: flex;
  gap: var(--vdl-space-3);
  color: var(--vdl-color-text-secondary);
  font-size: 12px;
}

.load-more-button {
  align-self: center;
}

.download-dialog-body {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
}

.download-field {
  display: grid;
  grid-template-columns: 72px minmax(0, 1fr);
  align-items: center;
  gap: var(--vdl-space-3);
}

.download-field-label {
  color: var(--vdl-color-text-secondary);
  font-size: 13px;
  text-align: right;
}

.download-field-value {
  min-width: 0;
  overflow: hidden;
  padding: var(--vdl-space-2) var(--vdl-space-3);
  color: var(--vdl-color-text);
  font-size: 14px;
  font-weight: 600;
  border: 1px solid var(--vdl-color-border);
  border-radius: 6px;
  background: var(--vdl-color-fill-tertiary);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tag-select-row {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
  min-width: 0;
}

.tag-select-row :deep(.vl-select) {
  flex: 1;
  min-width: 0;
}

.tag-loading-state {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: var(--vdl-space-2);
}

.tag-loading-dot {
  width: 8px;
  height: 8px;
  border-radius: 999px;
  background: var(--vdl-color-primary);
  animation: tag-loading-pulse 0.9s ease-in-out infinite;
}

@keyframes tag-loading-pulse {
  0%,
  100% {
    opacity: 0.35;
    transform: scale(0.85);
  }

  50% {
    opacity: 1;
    transform: scale(1);
  }
}

.inline-pull-progress {
  display: grid;
  grid-template-columns: minmax(160px, 1fr) auto;
  align-items: center;
  gap: var(--vdl-space-3);
}

.progress-inline-meta {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-3);
  min-width: 180px;
  color: var(--vdl-color-text-secondary);
  font-size: 12px;
}

.progress-track {
  width: 100%;
  height: 8px;
  overflow: hidden;
  border-radius: 999px;
  background: var(--vdl-color-fill-secondary);
}

.progress-fill {
  height: 100%;
  min-width: 2px;
  border-radius: inherit;
  background: var(--vdl-color-primary);
  transition: width 0.2s ease;
}

.progress-fill.success {
  background: var(--vdl-color-success);
}

.progress-fill.failed {
  background: var(--vdl-color-danger);
}

.progress-fill.cancelled {
  background: var(--vdl-color-warning);
}

@media (max-width: 720px) {
  .registry-form,
  .image-row,
  .progress-inline-meta {
    align-items: stretch;
    flex-direction: column;
  }

  .registry-action-button,
  .image-row :deep(.vl-button) {
    width: 100%;
  }

  .download-field {
    grid-template-columns: 1fr;
  }

  .download-field-label {
    text-align: left;
  }

  .tag-select-row {
    align-items: stretch;
    flex-direction: column;
  }

  .inline-pull-progress {
    grid-template-columns: 1fr;
  }
}
</style>
