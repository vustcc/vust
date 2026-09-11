<script setup lang="ts">
/**
 * @file DockerContainerList.vue
 * @description Docker 容器摘要、筛选、资源状态和生命周期操作管理页。
 */

import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useDockerStore } from '@/stores/docker'
import { useContainerResourceStats } from './composables/useContainerResourceStats'
import { getStateIcon, formatPercent, formatBytes } from '@/utils/docker-format'
import {
  VustActionMenu,
  VustAlert,
  VustButton,
  VustCheckbox,
  VustEmpty,
  VustInput,
  VustLoading,
  VustSelect,
  VustTable,
  VustTag,
} from '@/components/ui'
import type { VustTableColumn } from '@/components/ui/VustTable.vue'
import VustIcon from '@/components/icons/VustIcon.vue'
import type {
  DockerContainerAction,
  DockerContainerManagementKind,
  DockerContainerState,
  DockerContainerSummary,
} from '@/api/interface/docker'

const emit = defineEmits<{
  /** 打开指定容器的详情。 */
  (event: 'open-detail', id: string): void
}>()

const { t } = useI18n()
const store = useDockerStore()
const search = ref('')
const stateFilter = ref<'' | DockerContainerState>('')
const managementFilter = ref<'' | DockerContainerManagementKind>('')
const selectedIds = ref<Set<string>>(new Set())

const stateOptions = computed(() => [
  { label: t('app.docker.containers.filters.allStates'), value: '' },
  ...(['running', 'paused', 'restarting', 'created', 'exited', 'dead', 'unknown'] as const).map(
    (value) => ({ label: t(`app.docker.containers.states.${value}`), value }),
  ),
])

const managementOptions = computed(() => [
  { label: t('app.docker.containers.filters.allManagement'), value: '' },
  ...(['custom', 'compose', 'suite'] as const).map((value) => ({
    label: t(`app.docker.containers.management.${value}`),
    value,
  })),
])

const filteredContainers = computed(() => {
  const keyword = search.value.trim().toLowerCase()
  return store.containers.filter((container) => {
    if (
      keyword &&
      !container.name.toLowerCase().includes(keyword) &&
      !container.id.toLowerCase().includes(keyword) &&
      !container.imageRef.toLowerCase().includes(keyword)
    ) {
      return false
    }
    if (stateFilter.value && container.state !== stateFilter.value) return false
    if (managementFilter.value && container.management.kind !== managementFilter.value) return false
    return true
  })
})

const selectableContainers = computed(() => filteredContainers.value)
const isAllSelected = computed(
  () =>
    selectableContainers.value.length > 0 &&
    selectableContainers.value.every((container) => selectedIds.value.has(container.id)),
)
const isIndeterminate = computed(() => {
  const selectedCount = selectableContainers.value.filter((container) =>
    selectedIds.value.has(container.id),
  ).length
  return selectedCount > 0 && selectedCount < selectableContainers.value.length
})
const selectedContainers = computed(() =>
  store.containers.filter((container) => selectedIds.value.has(container.id)),
)

watch(
  () => store.containers,
  (containers) => {
    const availableIds = new Set(containers.map((container) => container.id))
    selectedIds.value = new Set([...selectedIds.value].filter((id) => availableIds.has(id)))
  },
)

const {
  getContainerStats,
  requestContainerStats,
  handleContainerRowMouseEnter,
  formatNetworkUsage,
} = useContainerResourceStats({
  containerResourceStats: computed(() => store.containerResourceStats),
  onFetchContainerStats: (id) => store.fetchContainerResourceStats(id),
  formatBytes,
})

const columns = computed<VustTableColumn[]>(() => [
  { label: '', width: 48, align: 'center', slot: 'selection', headerSlot: 'selectionHeader' },
  { label: t('app.docker.containers.name'), minWidth: 180, slot: 'name' },
  { label: t('app.docker.containers.managementLabel'), width: 145, slot: 'management' },
  { label: t('app.docker.containers.status'), width: 145, slot: 'status', align: 'center' },
  { label: t('app.docker.containers.ports'), minWidth: 165, slot: 'ports', align: 'center' },
  { label: t('app.docker.containers.resources'), minWidth: 260, slot: 'resource' },
  { label: t('app.docker.containers.actions'), width: 100, align: 'center', slot: 'actions' },
])

function setSelectAll(selected: boolean): void {
  selectedIds.value = selected
    ? new Set(selectableContainers.value.map((container) => container.id))
    : new Set()
}

function setRowSelected(id: string, selected: boolean): void {
  const next = new Set(selectedIds.value)
  if (selected) next.add(id)
  else next.delete(id)
  selectedIds.value = next
}

function managementLabel(container: DockerContainerSummary): string {
  const label = t(`app.docker.containers.management.${container.management.kind}`)
  return container.management.ownerName ? `${label} · ${container.management.ownerName}` : label
}

function managementTagType(container: DockerContainerSummary): 'primary' | 'info' | 'default' {
  if (container.management.kind === 'suite') return 'primary'
  if (container.management.kind === 'compose') return 'info'
  return 'default'
}

function stateTagType(
  state: DockerContainerState,
): 'success' | 'warning' | 'danger' | 'info' | 'default' {
  if (state === 'running') return 'success'
  if (state === 'paused' || state === 'restarting' || state === 'stopping') return 'warning'
  if (state === 'dead') return 'danger'
  if (state === 'exited') return 'info'
  return 'default'
}

function formatPorts(container: DockerContainerSummary): string {
  if (container.ports.length === 0) return '-'
  return container.ports
    .map((port) => {
      const target = `${port.containerPort}/${port.protocol}`
      if (!port.hostPort) return target
      return `${port.hostIp || '0.0.0.0'}:${port.hostPort} → ${target}`
    })
    .join(', ')
}

function openDetail(container: DockerContainerSummary): void {
  emit('open-detail', container.id)
  requestContainerStats(container.id, container.state)
}

async function handleAction(
  container: DockerContainerSummary,
  action: DockerContainerAction,
): Promise<void> {
  await store.handleContainerAction(container.id, container.name, action)
}

async function handleBatchAction(action: DockerContainerAction): Promise<void> {
  const containers = selectedContainers.value
  if (containers.length === 0) return
  const succeeded = await store.handleContainerAction(
    containers.map((container) => container.id),
    containers.map((container) => container.name),
    action,
  )
  if (succeeded) selectedIds.value = new Set()
}

function supportsSelectedAction(capability: keyof DockerContainerSummary['capabilities']): boolean {
  return selectedContainers.value.some((container) => container.capabilities[capability])
}

const batchActions = computed(() => [
  {
    label: t('app.docker.containers.batchStart'),
    handler: () => handleBatchAction('start'),
    disabled: !supportsSelectedAction('canStart'),
  },
  {
    label: t('app.docker.containers.batchStop'),
    handler: () => handleBatchAction('stop'),
    disabled: !supportsSelectedAction('canStop'),
  },
  {
    label: t('app.docker.containers.batchRestart'),
    handler: () => handleBatchAction('restart'),
    disabled: !supportsSelectedAction('canRestart'),
  },
  {
    label: t('app.docker.containers.batchPause'),
    handler: () => handleBatchAction('pause'),
    disabled: !supportsSelectedAction('canPause'),
  },
  {
    label: t('app.docker.containers.batchUnpause'),
    handler: () => handleBatchAction('unpause'),
    disabled: !supportsSelectedAction('canUnpause'),
  },
  {
    label: t('app.docker.containers.batchKill'),
    handler: () => handleBatchAction('kill'),
    disabled: !supportsSelectedAction('canKill'),
    class: 'btn-delete',
  },
  {
    label: t('app.docker.containers.batchDelete'),
    handler: () => handleBatchAction('remove'),
    disabled: !supportsSelectedAction('canRemove'),
    class: 'btn-delete',
  },
])

function rowActions(container: DockerContainerSummary) {
  const loading = store.containerActionLoadingIds.includes(container.id)
  return [
    {
      label: t('app.docker.containers.startContainer'),
      handler: () => handleAction(container, 'start'),
      disabled: loading || !container.capabilities.canStart,
    },
    {
      label: t('app.docker.containers.stopContainer'),
      handler: () => handleAction(container, 'stop'),
      disabled: loading || !container.capabilities.canStop,
    },
    {
      label: t('app.docker.containers.restartContainer'),
      handler: () => handleAction(container, 'restart'),
      disabled: loading || !container.capabilities.canRestart,
    },
    {
      label: t('app.docker.containers.pauseContainer'),
      handler: () => handleAction(container, 'pause'),
      disabled: loading || !container.capabilities.canPause,
    },
    {
      label: t('app.docker.containers.unpauseContainer'),
      handler: () => handleAction(container, 'unpause'),
      disabled: loading || !container.capabilities.canUnpause,
    },
    {
      label: t('app.docker.containers.killContainer'),
      handler: () => handleAction(container, 'kill'),
      disabled: loading || !container.capabilities.canKill,
      class: 'btn-delete',
    },
    {
      label: t('app.docker.containers.removeContainer'),
      handler: () => handleAction(container, 'remove'),
      disabled: loading || !container.capabilities.canRemove,
      class: 'btn-delete',
    },
  ]
}
</script>

<template>
  <div class="container-list" data-page="docker-container-list">
    <div class="container-toolbar" data-ui="toolbar">
      <div class="toolbar-filters" data-slot="filters">
        <VustInput
          id="docker-container-search"
          v-model="search"
          name="docker-container-search"
          :placeholder="t('app.docker.containers.containerNamePlaceholder')"
          clearable
          class="search-input"
          data-slot="search"
        />
        <VustSelect
          v-model="stateFilter"
          :options="stateOptions"
          class="filter-select"
          data-slot="state-filter"
        />
        <VustSelect
          v-model="managementFilter"
          :options="managementOptions"
          class="filter-select"
          data-slot="management-filter"
        />
      </div>
      <div class="toolbar-actions" data-slot="actions">
        <div v-if="selectedIds.size > 0" class="batch-control" data-slot="batch-actions">
          <span>{{ t('app.docker.containers.selectedCount', { count: selectedIds.size }) }}</span>
          <VustActionMenu
            :label="t('app.docker.containers.batchActions')"
            :actions="batchActions"
          />
        </div>
        <div class="primary-actions" data-slot="primary-actions">
          <VustButton
            type="secondary"
            :loading="store.containerListLoading && store.containers.length > 0"
            data-ui="refresh-containers"
            @click="store.fetchContainers"
          >
            {{ t('common.refresh') }}
          </VustButton>
          <VustButton
            type="primary"
            data-ui="create-container"
            @click="store.startContainerCreateFlow"
          >
            {{ t('app.docker.containers.create') }}
          </VustButton>
        </div>
      </div>
    </div>

    <VustAlert
      v-if="store.containerListError && store.containers.length > 0"
      type="warning"
      :title="t('app.docker.containers.refreshFailed')"
      :description="store.containerListError"
      data-ui="container-refresh-error"
    />
    <VustAlert
      v-if="store.containerStatsError"
      type="warning"
      :title="t('app.docker.containers.statsUnavailable')"
      :description="store.containerStatsError"
      data-ui="container-stats-error"
    />

    <div class="container-table-wrapper" data-ui="table">
      <VustAlert
        v-if="store.containerListError && store.containers.length === 0"
        type="error"
        :title="t('app.docker.containers.loadFailed')"
        :description="store.containerListError"
      />
      <VustEmpty
        v-else-if="!store.containerListLoading && filteredContainers.length === 0"
        :description="
          store.containers.length > 0
            ? t('app.docker.containers.filteredEmpty')
            : t('app.docker.containers.noContainers')
        "
      />
      <VustTable
        v-else-if="filteredContainers.length > 0"
        :data="filteredContainers"
        :columns="columns"
        border
        @row-mouseenter="handleContainerRowMouseEnter"
      >
        <template #selectionHeader>
          <VustCheckbox
            :model-value="isAllSelected"
            :indeterminate="isIndeterminate"
            :disabled="selectableContainers.length === 0"
            @update:model-value="setSelectAll"
          />
        </template>

        <template #selection="{ row: container }: { row: DockerContainerSummary }">
          <VustCheckbox
            :model-value="selectedIds.has(container.id)"
            @update:model-value="(selected) => setRowSelected(container.id, selected)"
          />
        </template>

        <template #name="{ row: container }: { row: DockerContainerSummary }">
          <button type="button" class="name-link" @click="openDetail(container)">
            {{ container.name }}
          </button>
        </template>

        <template #management="{ row: container }: { row: DockerContainerSummary }">
          <VustTag :type="managementTagType(container)" effect="light">
            {{ managementLabel(container) }}
          </VustTag>
        </template>

        <template #status="{ row: container }: { row: DockerContainerSummary }">
          <div class="status-cell">
            <VustTag :type="stateTagType(container.state)" effect="light">
              <VustIcon :name="getStateIcon(container.state)" :size="14" />
              {{ t(`app.docker.containers.states.${container.state}`) }}
            </VustTag>
          </div>
        </template>

        <template #ports="{ row: container }: { row: DockerContainerSummary }">
          <span class="mono ports-text">{{ formatPorts(container) }}</span>
        </template>

        <template #resource="{ row: container }: { row: DockerContainerSummary }">
          <div
            v-if="getContainerStats(container.id)?.status === 'fresh'"
            class="resource-cell mono"
          >
            <span>CPU {{ formatPercent(getContainerStats(container.id)?.cpuCorePercent) }}</span>
            <span>MEM {{ formatPercent(getContainerStats(container.id)?.memoryPercent) }}</span>
            <span>NET {{ formatNetworkUsage(container.id) }}</span>
          </div>
          <VustTag
            v-else-if="getContainerStats(container.id)?.status === 'stale'"
            type="warning"
            effect="plain"
          >
            {{ t('app.docker.containers.statsStale') }}
          </VustTag>
          <span v-else class="muted">-</span>
        </template>

        <template #actions="{ row: container }: { row: DockerContainerSummary }">
          <VustActionMenu :label="t('common.actions')" :actions="rowActions(container)" />
        </template>
      </VustTable>
      <VustLoading :loading="store.containerListLoading && store.containers.length === 0" cover />
    </div>
  </div>
</template>

<style scoped>
.container-list {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--vdl-bg-panel);
  container-name: docker-container-list;
  container-type: inline-size;
}

.container-toolbar {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-3) var(--vdl-space-4);
  border-bottom: 1px solid var(--vdl-border-subtle);
}

.toolbar-filters,
.toolbar-actions,
.primary-actions {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-3);
}

.toolbar-filters {
  flex: 1;
  min-width: 0;
}

.toolbar-actions {
  flex-shrink: 0;
}

.primary-actions {
  flex-shrink: 0;
}

.search-input {
  flex: 1 1 300px;
  width: auto;
  min-width: 180px;
  max-width: 300px;
}

.filter-select {
  flex: 0 0 150px;
  width: 150px;
}

.batch-control {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-caption);
  white-space: nowrap;
}

.container-table-wrapper {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: var(--vdl-space-4);
}

.status-cell,
.resource-cell {
  display: flex;
  min-width: 0;
}

.name-link {
  border: 0;
  padding: 0;
  background: transparent;
  color: var(--vdl-accent-primary);
  font: inherit;
  font-weight: var(--vdl-font-weight-semibold);
  cursor: pointer;
}

.name-link:hover {
  text-decoration: underline;
}

.muted {
  color: var(--vdl-text-subtle);
  font-size: var(--vdl-font-caption);
}

.status-cell {
  flex-wrap: wrap;
  align-items: center;
  justify-content: center;
  gap: var(--vdl-space-1);
}

.resource-cell {
  flex-direction: column;
  gap: 2px;
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-caption);
}

.mono {
  font-family: var(--vdl-font-mono);
}

.ports-text {
  display: -webkit-box;
  overflow: hidden;
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-caption);
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
}

@container docker-container-list (max-width: 900px) {
  .container-toolbar {
    align-items: stretch;
    flex-direction: column;
  }

  .toolbar-filters,
  .toolbar-actions {
    width: 100%;
  }

  .toolbar-actions {
    justify-content: flex-end;
  }

  .search-input {
    max-width: none;
  }
}

@container docker-container-list (max-width: 640px) {
  .toolbar-filters {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }

  .search-input {
    grid-column: 1 / -1;
    width: 100%;
    min-width: 0;
  }

  .filter-select {
    width: 100%;
  }

  .batch-control {
    margin-right: auto;
  }
}

@container docker-container-list (max-width: 460px) {
  .toolbar-actions {
    align-items: stretch;
    flex-direction: column;
  }

  .batch-control {
    margin-right: 0;
  }

  .primary-actions {
    justify-content: flex-end;
  }
}
</style>
