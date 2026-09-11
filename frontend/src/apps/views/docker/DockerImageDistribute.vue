<script setup lang="ts">
/**
 * @file DockerImageDistribute.vue
 * @description 从主控镜像库向多个在线节点创建并跟踪批量分发任务。
 */

import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { nodesApi, type NodeSummaryResponse } from '@/api/modules/nodes'
import type {
  DockerImageDistributionTarget,
  DockerImageStage,
  DockerImageTaskStatus,
} from '@/api/interface/docker'
import { useDockerStore } from '@/stores/docker'
import {
  VustAlert,
  VustButton,
  VustCheckbox,
  VustDialog,
  VustEmpty,
  VustInput,
  VustLoading,
  VustSelect,
  VustTable,
  VustTag,
} from '@/components/ui'
import type { VustTableColumn } from '@/components/ui/VustTable.vue'

const POLL_INTERVAL_MS = 1500
const terminalStatuses: DockerImageTaskStatus[] = ['success', 'failed', 'cancelled']

const { t } = useI18n()
const store = useDockerStore()
const selectedImageRef = ref<string | null>(null)
const selectedNodeIds = ref<string[]>([])
const draftSelectedNodeIds = ref<string[]>([])
const nodeDialogVisible = ref(false)
const nodeSearch = ref('')
const nodes = ref<NodeSummaryResponse[]>([])
const nodesLoading = ref(false)
const nodesError = ref('')
let nodeRequestSequence = 0
let pollTimer: number | null = null
let viewActive = false

const controllerImageOptions = computed(() =>
  store.controllerImages.flatMap((image) => {
    if (image.tags.length) return image.tags.map((tag) => ({ label: tag, value: tag }))
    const shortId = image.id.replace(/^sha256:/, '').substring(0, 12)
    return [{ label: `<none> (${shortId})`, value: image.id }]
  }),
)

const remoteNodes = computed(() => nodes.value.filter((node) => node.nodeId !== 'local'))
const filteredNodes = computed(() => {
  const query = nodeSearch.value.trim().toLowerCase()
  if (!query) return remoteNodes.value
  return remoteNodes.value.filter(
    (node) =>
      node.name.toLowerCase().includes(query) ||
      node.nodeId.toLowerCase().includes(query) ||
      (node.address || '').toLowerCase().includes(query),
  )
})
const selectableFilteredNodes = computed(() =>
  filteredNodes.value.filter((node) => node.status === 'online'),
)
const allFilteredSelected = computed(
  () =>
    selectableFilteredNodes.value.length > 0 &&
    selectableFilteredNodes.value.every((node) => draftSelectedNodeIds.value.includes(node.nodeId)),
)
const taskActive = computed(() =>
  store.imageDistributionTask
    ? !terminalStatuses.includes(store.imageDistributionTask.status)
    : false,
)
const failedTargets = computed(
  () => store.imageDistributionTask?.targets.filter((target) => target.status === 'failed') || [],
)

const nodeColumns = computed<VustTableColumn[]>(() => [
  { label: '', width: 52, slot: 'selection', headerSlot: 'selectionHeader', align: 'center' },
  {
    label: t('app.docker.images.distribute.nodes.name'),
    minWidth: 180,
    prop: 'name',
    align: 'center',
  },
  {
    label: t('app.docker.images.distribute.nodes.address'),
    minWidth: 180,
    slot: 'address',
    align: 'center',
  },
  {
    label: t('app.docker.images.distribute.nodes.status'),
    width: 100,
    slot: 'status',
    align: 'center',
  },
])
const progressColumns = computed<VustTableColumn[]>(() => [
  {
    label: t('app.docker.images.distribute.nodes.name'),
    minWidth: 160,
    prop: 'nodeName',
    align: 'center',
  },
  {
    label: t('app.docker.images.distribute.progress.status'),
    width: 110,
    slot: 'status',
    align: 'center',
  },
  {
    label: t('app.docker.images.distribute.progress.stage'),
    width: 120,
    slot: 'stage',
    align: 'center',
  },
  { label: t('app.docker.images.distribute.progress.progress'), minWidth: 180, slot: 'progress' },
  { label: t('app.docker.images.distribute.progress.error'), minWidth: 220, slot: 'error' },
])

/** 获取节点列表，并阻止旧响应覆盖较新的选择数据。 */
async function fetchNodes() {
  if (nodesLoading.value) return
  const sequence = ++nodeRequestSequence
  nodesLoading.value = true
  nodesError.value = ''
  try {
    const response = await nodesApi.list()
    if (sequence !== nodeRequestSequence) return
    if (!response.success) {
      nodesError.value = response.message || t('common.unknownError')
      return
    }
    nodes.value = response.data || []
    const validIds = new Set(
      remoteNodes.value.filter((node) => node.status === 'online').map((node) => node.nodeId),
    )
    selectedNodeIds.value = selectedNodeIds.value.filter((id) => validIds.has(id))
  } catch (error) {
    if (sequence === nodeRequestSequence) {
      nodesError.value = error instanceof Error ? error.message : String(error)
    }
  } finally {
    if (sequence === nodeRequestSequence) nodesLoading.value = false
  }
}

function openNodeDialog() {
  draftSelectedNodeIds.value = [...selectedNodeIds.value]
  nodeSearch.value = ''
  nodeDialogVisible.value = true
  void fetchNodes()
}

function closeNodeDialog() {
  nodeDialogVisible.value = false
}

function applyNodeSelection() {
  selectedNodeIds.value = [...draftSelectedNodeIds.value]
  nodeDialogVisible.value = false
}

function toggleNode(node: NodeSummaryResponse, checked: boolean) {
  if (node.status !== 'online') return
  const ids = new Set(draftSelectedNodeIds.value)
  if (checked) ids.add(node.nodeId)
  else ids.delete(node.nodeId)
  draftSelectedNodeIds.value = [...ids]
}

function toggleAllFiltered(checked: boolean) {
  const ids = new Set(draftSelectedNodeIds.value)
  for (const node of selectableFilteredNodes.value) {
    if (checked) ids.add(node.nodeId)
    else ids.delete(node.nodeId)
  }
  draftSelectedNodeIds.value = [...ids]
}

async function startDistribution() {
  if (!selectedImageRef.value || !selectedNodeIds.value.length) return
  const started = await store.startImageDistribution({
    imageRef: selectedImageRef.value,
    targetNodeIds: selectedNodeIds.value,
  })
  if (started) schedulePoll(0)
}

async function retryFailed() {
  const task = store.imageDistributionTask
  if (!task || !failedTargets.value.length) return
  selectedImageRef.value = task.imageRef
  selectedNodeIds.value = failedTargets.value.map((target) => target.nodeId)
  await startDistribution()
}

function clearPollTimer() {
  if (pollTimer !== null) window.clearTimeout(pollTimer)
  pollTimer = null
}

function schedulePoll(delay = POLL_INTERVAL_MS) {
  clearPollTimer()
  if (!viewActive || !taskActive.value || document.hidden) return
  pollTimer = window.setTimeout(async () => {
    await store.fetchImageDistributionTask()
    schedulePoll()
  }, delay)
}

function handleVisibilityChange() {
  if (document.hidden) clearPollTimer()
  else if (taskActive.value) schedulePoll(0)
}

function statusType(status: DockerImageTaskStatus) {
  if (status === 'success') return 'success'
  if (status === 'failed') return 'danger'
  if (status === 'cancelled') return 'default'
  if (status === 'running') return 'primary'
  return 'warning'
}

function statusLabel(status: DockerImageTaskStatus) {
  return t(`app.docker.images.distribute.status.${status}`)
}

function stageLabel(stage: DockerImageStage) {
  return t(`app.docker.images.distribute.stage.${stage}`)
}

function targetStageLabel(target: DockerImageDistributionTarget) {
  if (target.status === 'success' && target.source) {
    return t(`app.docker.images.distribute.finalStage.${target.source}`)
  }
  if (target.status === 'cancelled') {
    return t('app.docker.images.distribute.finalStage.cancelled')
  }
  return stageLabel(target.stage)
}

watch(
  () => store.imageDistributionTask?.status,
  () => {
    if (taskActive.value) schedulePoll()
    else clearPollTimer()
  },
)

watch(
  controllerImageOptions,
  (options) => {
    if (!selectedImageRef.value && options[0]) selectedImageRef.value = String(options[0].value)
  },
  { immediate: true },
)

onMounted(async () => {
  viewActive = true
  document.addEventListener('visibilitychange', handleVisibilityChange)
  await Promise.all([
    store.fetchControllerImages(),
    store.fetchRecentImageDistributionTask(),
    fetchNodes(),
  ])
  if (viewActive && taskActive.value) schedulePoll(0)
})

onBeforeUnmount(() => {
  viewActive = false
  nodeRequestSequence += 1
  clearPollTimer()
  document.removeEventListener('visibilitychange', handleVisibilityChange)
  store.leaveImageDistributionView()
})
</script>

<template>
  <div class="distribution-page" data-page="docker-image-distribution">
    <div class="distribution-toolbar" data-ui="toolbar">
      <div class="toolbar-field image-field">
        <span class="field-label">{{ t('app.docker.images.distribute.selectLocal.title') }}</span>
        <VustSelect
          v-model="selectedImageRef"
          :options="controllerImageOptions"
          :placeholder="t('app.docker.images.distribute.selectLocal.placeholder')"
          :disabled="store.controllerImageLoading || taskActive"
          data-ui="distribution-image-select"
        />
      </div>
      <div class="toolbar-field node-field">
        <span class="field-label">{{ t('app.docker.images.distribute.nodes.title') }}</span>
        <VustButton
          :disabled="taskActive"
          data-ui="distribution-node-select"
          @click="openNodeDialog"
        >
          {{ t('app.docker.images.distribute.nodes.selected', { count: selectedNodeIds.length }) }}
        </VustButton>
      </div>
      <VustButton
        type="primary"
        :loading="store.imageDistributionStarting"
        :disabled="!selectedImageRef || !selectedNodeIds.length || taskActive"
        class="start-button"
        data-ui="distribution-start-button"
        @click="startDistribution"
      >
        {{ t('app.docker.images.distribute.actions.submit') }}
      </VustButton>
    </div>

    <VustAlert
      v-if="store.controllerImageError"
      type="warning"
      :title="t('app.docker.images.distribute.selectLocal.loadFailed')"
      :description="store.controllerImageError"
      show-icon
    />
    <VustAlert
      v-if="store.imageDistributionError"
      type="warning"
      :title="t('app.docker.images.distribute.progress.refreshFailed')"
      :description="store.imageDistributionError"
      show-icon
    />

    <div class="distribution-content" data-slot="content">
      <div v-if="store.imageDistributionTask" class="task-result" data-ui="distribution-task">
        <div class="task-header" data-slot="header">
          <div class="task-title">
            <span>{{ store.imageDistributionTask.imageRef }}</span>
          </div>
          <div class="task-actions">
            <VustButton
              v-if="taskActive"
              type="danger"
              size="small"
              :loading="store.imageDistributionCanceling"
              @click="store.cancelImageDistribution"
            >
              {{ t('app.docker.images.distribute.actions.cancel') }}
            </VustButton>
            <VustButton v-if="failedTargets.length" size="small" @click="retryFailed">
              {{ t('app.docker.images.distribute.actions.retryFailed') }}
            </VustButton>
          </div>
        </div>
        <VustTable
          :data="store.imageDistributionTask.targets"
          :columns="progressColumns"
          class="distribution-table"
          border
        >
          <template #status="{ row }: { row: DockerImageDistributionTarget }">
            <VustTag :type="statusType(row.status)" size="small">
              {{ statusLabel(row.status) }}
            </VustTag>
          </template>
          <template #stage="{ row }: { row: DockerImageDistributionTarget }">
            {{ targetStageLabel(row) }}
          </template>
          <template #progress="{ row }: { row: DockerImageDistributionTarget }">
            <div class="progress-cell">
              <div class="progress-track">
                <div
                  class="progress-fill"
                  :class="row.status"
                  :style="{ width: `${row.progressPercent}%` }"
                />
              </div>
              <span>{{ row.progressPercent }}%</span>
            </div>
          </template>
          <template #error="{ row }: { row: DockerImageDistributionTarget }">
            <span class="error-summary" :title="row.errorSummary">{{
              row.errorSummary || '-'
            }}</span>
          </template>
        </VustTable>
      </div>
      <VustEmpty
        v-else-if="!store.imageDistributionLoading"
        :description="t('app.docker.images.distribute.progress.empty')"
      />
      <VustLoading
        :loading="store.imageDistributionLoading && !store.imageDistributionTask"
        cover
      />
    </div>

    <VustDialog
      :visible="nodeDialogVisible"
      :title="t('app.docker.images.distribute.nodes.dialogTitle')"
      width="720px"
      data-ui="distribution-node-dialog"
      @close="closeNodeDialog"
    >
      <div class="node-dialog-body" data-slot="body">
        <VustInput
          v-model="nodeSearch"
          :placeholder="t('app.docker.images.distribute.nodes.searchPlaceholder')"
          clearable
        />
        <VustAlert
          v-if="nodesError"
          type="error"
          :description="nodesError"
          show-icon
          data-ui="distribution-node-error"
        />
        <div class="node-table-shell">
          <VustTable :data="filteredNodes" :columns="nodeColumns" border>
            <template #selectionHeader>
              <VustCheckbox
                :model-value="allFilteredSelected"
                :disabled="!selectableFilteredNodes.length"
                @change="toggleAllFiltered"
              />
            </template>
            <template #selection="{ row }: { row: NodeSummaryResponse }">
              <VustCheckbox
                :model-value="draftSelectedNodeIds.includes(row.nodeId)"
                :disabled="row.status !== 'online'"
                @change="(checked) => toggleNode(row, checked)"
              />
            </template>
            <template #address="{ row }: { row: NodeSummaryResponse }">
              {{ row.address || '-' }}
            </template>
            <template #status="{ row }: { row: NodeSummaryResponse }">
              <VustTag :type="row.status === 'online' ? 'success' : 'default'" size="small">
                {{
                  row.status === 'online'
                    ? t('app.docker.images.distribute.nodes.online')
                    : t('app.docker.images.distribute.nodes.offline')
                }}
              </VustTag>
            </template>
            <template #empty>
              <VustEmpty :description="t('app.docker.images.distribute.nodes.empty')" />
            </template>
          </VustTable>
          <VustLoading :loading="nodesLoading && !nodes.length" cover />
        </div>
      </div>
      <template #footer>
        <VustButton @click="closeNodeDialog">{{ t('common.cancel') }}</VustButton>
        <VustButton type="primary" @click="applyNodeSelection">
          {{
            t('app.docker.images.distribute.nodes.confirmSelection', {
              count: draftSelectedNodeIds.length,
            })
          }}
        </VustButton>
      </template>
    </VustDialog>
  </div>
</template>

<style scoped>
.distribution-page {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
  height: 100%;
  min-height: 0;
}

.distribution-toolbar {
  display: grid;
  grid-template-columns: minmax(180px, 1fr) auto;
  align-items: end;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-3);
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-input);
}

.image-field {
  grid-column: 1 / -1;
}

.image-field :deep(.vl-select),
.image-field :deep(.vl-select-trigger) {
  width: 100%;
  min-width: 0;
}

.image-field :deep(.vl-select-label) {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.node-field {
  min-width: 0;
}

.toolbar-field {
  display: grid;
  min-width: 0;
  gap: var(--vdl-space-1);
}

.field-label {
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-body-xs);
  font-weight: 600;
}

.start-button {
  min-width: 150px;
}

.distribution-content {
  position: relative;
  flex: 1;
  min-height: 180px;
  overflow: hidden;
}

.task-result {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  gap: var(--vdl-space-3);
}

.distribution-table {
  flex: 1;
  height: auto;
  min-height: 0;
}

.task-header,
.task-title,
.task-actions {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
}

.task-header {
  justify-content: space-between;
}

.task-title > span:first-child {
  font-family: var(--vdl-font-mono);
  font-weight: 600;
}

.progress-cell {
  display: grid;
  grid-template-columns: minmax(80px, 1fr) 40px;
  align-items: center;
  gap: var(--vdl-space-2);
}

.progress-track {
  height: 6px;
  overflow: hidden;
  border-radius: 3px;
  background: var(--vdl-bg-input);
}

.progress-fill {
  height: 100%;
  border-radius: inherit;
  background: var(--vdl-primary);
  transition: width 0.2s ease;
}

.progress-fill.success {
  background: var(--vdl-success);
}

.progress-fill.failed {
  background: var(--vdl-danger);
}

.error-summary {
  display: block;
  overflow: hidden;
  color: var(--vdl-text-secondary);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.node-dialog-body {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
}

.node-table-shell {
  position: relative;
  max-height: 360px;
  min-height: 160px;
  overflow: auto;
}

@media (max-width: 760px) {
  .distribution-toolbar {
    grid-template-columns: 1fr;
    align-items: stretch;
  }

  .image-field {
    grid-column: auto;
  }

  .start-button {
    width: 100%;
  }

  .task-header {
    align-items: flex-start;
    flex-direction: column;
  }
}
</style>
