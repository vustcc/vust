<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ProcessSignalResult } from '@/api/generated'
import type { NetworkConnectionSummary, ProcessSummary } from '@/api/modules/process'
import { useToastStore } from '@/stores/toast'
import { useConfirmationModalStore } from '@/stores/confirmation-modal'
import { useWindowManagerStore } from '@/stores/window-manager'
import { useNodeStore } from '@/stores/node'
import { useProcessManager } from '@/composables/useProcessManager'
import {
  VustAlert,
  VustButton,
  VustCard,
  VustEmpty,
  VustInput,
  VustLoading,
  VustPagination,
  VustSelect,
  VustTable,
  VustTag,
} from '@/components/ui'
import type { VustTableColumn } from '@/components/ui/VustTable.vue'

const props = defineProps<{
  isMaximized?: boolean
  windowId?: string
  payload?: Record<string, unknown>
}>()

type ActiveTab = 'process' | 'network'
type ProcessSortKey = 'pid' | 'cpuPercent' | 'memoryPercent' | 'connectionCount'
type ProcessState = ProcessSummary['state']
type NetworkDisplayRow = NetworkConnectionSummary & {
  localAddress: string
  remoteAddress: string
  pid: string
  processName: string
}

const PROCESS_STATUS_OPTIONS: ProcessState[] = [
  'running',
  'sleeping',
  'stopped',
  'idle',
  'uninterruptible',
  'zombie',
  'dead',
  'unknown',
]

const { t } = useI18n()
const toastStore = useToastStore()
const confirmationModal = useConfirmationModalStore()
const windowStore = useWindowManagerStore()
const nodeStore = useNodeStore()
const targetNodeId =
  typeof props.payload?.nodeId === 'string'
    ? props.payload.nodeId
    : nodeStore.currentNodeId || 'local'
const processManager = useProcessManager(targetNodeId)

const activeTab = ref<ActiveTab>('process')
const networkKeyword = ref('')
const processStatus = ref<string>('ALL')
const networkState = ref<string>('ALL')
let networkSearchTimer: ReturnType<typeof setTimeout> | undefined

const processRows = computed(() => processManager.processPage.value?.entries ?? [])
const availableProcessStatuses = computed(() => {
  const counts = processManager.processPage.value?.counts
  if (!counts) return []
  return PROCESS_STATUS_OPTIONS.filter((status) => (counts[status] ?? 0) > 0)
})
const networkRows = computed<NetworkDisplayRow[]>(() =>
  (processManager.networkPage.value?.entries ?? []).map((entry) => ({
    ...entry,
    localAddress: formatEndpoint(entry.localEndpoint),
    remoteAddress: entry.remoteEndpoint ? formatEndpoint(entry.remoteEndpoint) : '--',
    pid: entry.owners.map((owner) => owner.pid).join(', ') || '--',
    processName: entry.owners.map((owner) => owner.processName).join(', ') || '--',
  })),
)
const processTotalPages = computed(() =>
  Math.max(
    1,
    Math.ceil(
      (processManager.processPage.value?.total ?? 0) / processManager.processQuery.pageSize,
    ),
  ),
)
const networkTotalPages = computed(() =>
  Math.max(
    1,
    Math.ceil(
      (processManager.networkPage.value?.total ?? 0) / processManager.networkQuery.pageSize,
    ),
  ),
)
const loadingProcess = computed(() => processManager.processPhase.value === 'initialLoading')
const loadingNetwork = computed(() => processManager.networkPhase.value === 'initialLoading')
const operationBusy = computed(() => processManager.pendingProcessIds.value.size > 0)

watch(
  operationBusy,
  (busy) => {
    if (!props.windowId) return
    windowStore.updateWindowRuntimeState(props.windowId, {
      activeSession: false,
      busy,
      allowsNodeSwitch: false,
      blockLevel: busy ? 'busy' : 'open',
      blockReason: busy ? t('app.processManager.guardBusy') : t('app.processManager.guardOpen'),
    })
  },
  { immediate: true },
)

watch(processStatus, (status) => {
  processManager.processQuery.status = status === 'ALL' ? undefined : (status as ProcessState)
  processManager.processQuery.page = 1
})

watch(
  () => processManager.processPage.value?.counts,
  (counts) => {
    if (!counts || processStatus.value === 'ALL') return
    if ((counts[processStatus.value] ?? 0) === 0) processStatus.value = 'ALL'
  },
)

watch(networkState, (state) => {
  processManager.networkQuery.state =
    state === 'ALL' ? undefined : (state as NetworkConnectionSummary['state'])
  processManager.networkQuery.page = 1
})

watch(networkKeyword, (keyword) => {
  if (networkSearchTimer) clearTimeout(networkSearchTimer)
  networkSearchTimer = setTimeout(() => {
    processManager.networkQuery.query = keyword.trim() || undefined
    processManager.networkQuery.page = 1
  }, 250)
})

watch(
  activeTab,
  (tab) => {
    processManager.setActiveView(tab)
  },
  { immediate: true },
)

const networkStateOptions = computed(() => [
  'ALL',
  ...Object.keys(processManager.networkPage.value?.byState ?? {}).sort(),
])

const networkSummary = computed(() => {
  const page = processManager.networkPage.value
  return {
    total: page?.availableTotal ?? 0,
    tcp: (page?.byProtocol.tcp ?? 0) + (page?.byProtocol.tcp6 ?? 0),
    udp: (page?.byProtocol.udp ?? 0) + (page?.byProtocol.udp6 ?? 0),
  }
})

const formatEndpoint = (endpoint: { address: string; port: number }) => {
  const address = endpoint.address.includes(':') ? `[${endpoint.address}]` : endpoint.address
  return `${address}:${endpoint.port}`
}

const formatNetworkState = (state: string) =>
  state.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toUpperCase()

const formatPercent = (value?: number) => (value == null ? '--' : `${value.toFixed(2)}%`)

const formatStartTime = (value?: string) => {
  if (!value) return '--'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return '--'
  return date.toLocaleString()
}

const setProcessSort = (key: ProcessSortKey) => {
  if (processManager.processQuery.sortBy === key) {
    processManager.processQuery.sortOrder =
      processManager.processQuery.sortOrder === 'asc' ? 'desc' : 'asc'
  } else {
    processManager.processQuery.sortBy = key
    processManager.processQuery.sortOrder = 'desc'
  }
  processManager.processQuery.page = 1
}

const processSortIndicator = (key: ProcessSortKey) => {
  if (processManager.processQuery.sortBy !== key) return ''
  return processManager.processQuery.sortOrder === 'desc' ? ' ↓' : ' ↑'
}

const processAriaSort = (key: ProcessSortKey) => {
  if (processManager.processQuery.sortBy !== key) return 'none'
  return processManager.processQuery.sortOrder === 'desc' ? 'descending' : 'ascending'
}

const formatProcessStatus = (status: ProcessState) =>
  t(`app.processManager.process.statuses.${status}`)

const formatSignalAction = (signal: 'TERM' | 'KILL' | 'term' | 'kill') =>
  signal.toUpperCase() === 'TERM'
    ? t('app.processManager.process.actions.exit')
    : t('app.processManager.process.actions.end')

const statusTagType = (status: ProcessState) => {
  if (status === 'running') return 'success'
  if (status === 'zombie' || status === 'dead') return 'danger'
  if (status === 'uninterruptible' || status === 'stopped') return 'warning'
  return 'default'
}

const notifySignalResult = (result: ProcessSignalResult) => {
  if (result.status === 'outcomeUnknown') {
    toastStore.warning(t('app.processManager.messages.outcomeUnknown', { pid: result.pid ?? '--' }))
    return
  }
  toastStore.success(
    t('app.processManager.messages.signalDelivered', {
      pid: result.pid ?? '--',
      signal: formatSignalAction(result.signal),
    }),
  )
}

const terminateProcess = async (process: ProcessSummary, signal: 'TERM' | 'KILL') => {
  if (processManager.pendingProcessIds.value.has(process.processId)) return
  const actionLabel = formatSignalAction(signal)
  const confirmed = await confirmationModal.showConfirmation(
    t('app.processManager.confirmTerminate', { pid: process.pid, signal: actionLabel }),
    t('app.processManager.title'),
    actionLabel,
    t('app.nodes.deploy.cancel'),
  )
  if (!confirmed) return
  try {
    if (signal === 'TERM') {
      const result = await processManager.terminate(process.processId)
      if (result) notifySignalResult(result)
      return
    }
    const confirmation = await processManager.createForceKillConfirmation(process.processId)
    const irreversibleConfirmed = await confirmationModal.showConfirmation(
      t('app.processManager.confirmForceKill', { pid: process.pid }),
      t('app.processManager.forceKillTitle'),
      t('app.processManager.forceKillAction'),
      t('app.nodes.deploy.cancel'),
    )
    if (!irreversibleConfirmed) return
    const result = await processManager.forceKill(process.processId, confirmation.confirmationToken)
    if (result) notifySignalResult(result)
  } catch (error) {
    toastStore.error(
      error instanceof Error ? error.message : t('app.processManager.messages.killFailed'),
    )
  }
}

const processEmptyText = computed(() =>
  (processManager.processPage.value?.availableTotal ?? 0) === 0
    ? t('app.processManager.process.empty')
    : t('app.processManager.process.filteredEmpty'),
)
const networkEmptyText = computed(() =>
  (processManager.networkPage.value?.availableTotal ?? 0) === 0
    ? t('app.processManager.network.empty')
    : t('app.processManager.network.filteredEmpty'),
)

const processColumns = computed<VustTableColumn[]>(() => [
  { prop: 'pid', label: 'PID', width: 80, align: 'center', headerSlot: 'header-pid' },
  { prop: 'name', label: t('app.processManager.process.columns.name'), minWidth: 150 },
  { prop: 'parentPid', label: 'PPID', width: 80, align: 'center' },
  {
    prop: 'threadCount',
    label: t('app.processManager.process.columns.threads'),
    width: 80,
    align: 'center',
  },
  { prop: 'userName', label: t('app.processManager.process.columns.user'), width: 100 },
  {
    prop: 'cpuPercent',
    label: 'CPU',
    width: 100,
    align: 'center',
    slot: 'cpu',
    headerSlot: 'header-cpu',
  },
  {
    prop: 'memoryPercent',
    label: 'MEM',
    width: 100,
    align: 'center',
    slot: 'mem',
    headerSlot: 'header-mem',
  },
  {
    prop: 'connectionCount',
    label: t('app.processManager.process.columns.connections'),
    width: 100,
    align: 'center',
    headerSlot: 'header-connections',
  },
  {
    prop: 'state',
    label: t('app.processManager.process.columns.status'),
    width: 110,
    slot: 'status',
    align: 'center',
  },
  {
    prop: 'startedAt',
    label: t('app.processManager.process.columns.startTime'),
    width: 180,
    slot: 'startTime',
    align: 'center',
  },
  {
    label: t('app.processManager.process.columns.actions'),
    width: 150,
    align: 'center',
    slot: 'actions',
    fixed: 'right',
  },
])

const networkColumns = computed<VustTableColumn[]>(() => [
  {
    prop: 'protocol',
    label: t('app.processManager.network.columns.protocol'),
    width: 80,
    align: 'center',
  },
  {
    prop: 'localAddress',
    label: t('app.processManager.network.columns.localAddress'),
    minWidth: 200,
    align: 'center',
  },
  {
    prop: 'remoteAddress',
    label: t('app.processManager.network.columns.remoteAddress'),
    minWidth: 200,
    align: 'center',
  },
  {
    prop: 'state',
    label: t('app.processManager.network.columns.state'),
    width: 120,
    slot: 'state',
    align: 'center',
  },
  { prop: 'pid', label: 'PID', width: 100, align: 'center' },
  {
    prop: 'processName',
    label: t('app.processManager.network.columns.processName'),
    minWidth: 150,
  },
])
</script>

<template>
  <div
    class="process-manager"
    data-vust-app="process-manager"
    data-page="process-manager"
    data-ui="process-manager"
  >
    <VustCard shadow="never" class="header-card" data-slot="header-card">
      <div class="header" data-slot="header">
        <div v-if="activeTab === 'process'" class="filters">
          <label class="filter-label" for="process-status-filter">
            {{ t('app.processManager.process.columns.status') }}:
          </label>
          <VustSelect
            id="process-status-filter"
            v-model="processStatus"
            name="processStatus"
            class="status-select"
            :options="[
              { label: t('app.processManager.process.filters.allStatuses'), value: 'ALL' },
              ...availableProcessStatuses.map((status) => ({
                label: t(`app.processManager.process.statuses.${status}`),
                value: status,
              })),
            ]"
          />
        </div>
        <div v-else class="network-summary">
          <div class="summary-metric">
            <span class="summary-label">{{ t('app.processManager.network.summary.total') }}</span>
            <strong class="summary-value">{{ networkSummary.total }}</strong>
          </div>
          <div class="summary-metric">
            <span class="summary-label">{{
              t('app.processManager.network.summary.protocol')
            }}</span>
            <strong class="summary-value">
              TCP: {{ networkSummary.tcp }} · UDP: {{ networkSummary.udp }}
            </strong>
          </div>
        </div>
        <div class="tabs" role="tablist" :aria-label="t('app.processManager.title')">
          <VustButton
            role="tab"
            :aria-selected="activeTab === 'process'"
            :type="activeTab === 'process' ? 'primary' : 'secondary'"
            data-slot="process-tab"
            @click="activeTab = 'process'"
          >
            {{ t('app.processManager.tabs.process') }}
          </VustButton>
          <VustButton
            role="tab"
            :aria-selected="activeTab === 'network'"
            :type="activeTab === 'network' ? 'primary' : 'secondary'"
            data-slot="network-tab"
            @click="activeTab = 'network'"
          >
            {{ t('app.processManager.tabs.network') }}
          </VustButton>
        </div>
      </div>
    </VustCard>

    <VustAlert
      v-if="activeTab === 'process' && processManager.processPhase.value === 'initialError'"
      type="error"
      :title="processManager.processError.value || t('app.processManager.messages.loadFailed')"
      show-icon
      data-ui="process-initial-error"
    />
    <VustAlert
      v-else-if="activeTab === 'process' && processManager.processPhase.value === 'stale'"
      type="warning"
      :title="t('app.processManager.messages.stale')"
      :description="processManager.processError.value || undefined"
      show-icon
      data-ui="process-stale-warning"
    />
    <VustAlert
      v-else-if="activeTab === 'process' && processManager.processPartial.value"
      type="warning"
      :title="t('app.processManager.messages.processPartial')"
      show-icon
      data-ui="process-partial-warning"
    />
    <VustAlert
      v-if="activeTab === 'network' && processManager.networkPhase.value === 'initialError'"
      type="error"
      :title="processManager.networkError.value || t('app.processManager.messages.loadFailed')"
      show-icon
      data-ui="network-initial-error"
    />
    <VustAlert
      v-else-if="activeTab === 'network' && processManager.networkPhase.value === 'stale'"
      type="warning"
      :title="t('app.processManager.messages.stale')"
      :description="processManager.networkError.value || undefined"
      show-icon
      data-ui="network-stale-warning"
    />
    <VustAlert
      v-else-if="activeTab === 'network' && processManager.networkPartial.value"
      type="warning"
      :title="t('app.processManager.messages.networkPartial')"
      show-icon
      data-ui="network-partial-warning"
    />
    <VustAlert
      v-else-if="activeTab === 'network' && processManager.networkOwnerPartial.value"
      type="warning"
      :title="
        t('app.processManager.messages.networkOwnerPartial', {
          coverage: processManager.networkOwnerCoveragePercent.value,
        })
      "
      show-icon
      data-ui="network-owner-partial-warning"
    />

    <div v-if="activeTab === 'process'" class="content-wrapper" data-slot="process-panel">
      <VustCard shadow="never" class="table-card" full-height>
        <VustTable :data="processRows" :columns="processColumns" border data-ui="process-table">
          <template #header-pid>
            <button
              class="sortable-header"
              type="button"
              :aria-sort="processAriaSort('pid')"
              @click="setProcessSort('pid')"
            >
              PID{{ processSortIndicator('pid') }}
            </button>
          </template>
          <template #header-cpu>
            <button
              class="sortable-header"
              type="button"
              :aria-sort="processAriaSort('cpuPercent')"
              @click="setProcessSort('cpuPercent')"
            >
              CPU{{ processSortIndicator('cpuPercent') }}
            </button>
          </template>
          <template #header-mem>
            <button
              class="sortable-header"
              type="button"
              :aria-sort="processAriaSort('memoryPercent')"
              @click="setProcessSort('memoryPercent')"
            >
              MEM{{ processSortIndicator('memoryPercent') }}
            </button>
          </template>
          <template #header-connections>
            <button
              class="sortable-header"
              type="button"
              :aria-sort="processAriaSort('connectionCount')"
              @click="setProcessSort('connectionCount')"
            >
              {{ t('app.processManager.process.columns.connections')
              }}{{ processSortIndicator('connectionCount') }}
            </button>
          </template>
          <template #cpu="{ row }">{{ formatPercent(row.cpuPercent) }}</template>
          <template #mem="{ row }">{{ formatPercent(row.memoryPercent) }}</template>
          <template #status="{ row }">
            <VustTag :type="statusTagType(row.state)">{{ formatProcessStatus(row.state) }}</VustTag>
          </template>
          <template #startTime="{ row }">{{ formatStartTime(row.startedAt) }}</template>
          <template #actions="{ row }">
            <div class="action-buttons">
              <VustButton
                type="danger"
                size="small"
                :disabled="
                  !row.capabilities.canTerminate ||
                  processManager.pendingProcessIds.value.has(row.processId)
                "
                @click="terminateProcess(row, 'TERM')"
                >{{ t('app.processManager.process.actions.exit') }}</VustButton
              >
              <VustButton
                type="danger"
                size="small"
                plain
                :disabled="
                  !row.capabilities.canForceKill ||
                  processManager.pendingProcessIds.value.has(row.processId)
                "
                @click="terminateProcess(row, 'KILL')"
                >{{ t('app.processManager.process.actions.end') }}</VustButton
              >
            </div>
          </template>
          <template #empty><VustEmpty :description="processEmptyText" /></template>
        </VustTable>
      </VustCard>
      <div class="pagination-bar" data-slot="process-pagination">
        <VustPagination
          :current-page="processManager.processQuery.page"
          :total-pages="processTotalPages"
          @page-change="(page) => (processManager.processQuery.page = page)"
        />
      </div>
    </div>

    <div v-else class="content-wrapper" data-slot="network-panel">
      <VustCard shadow="never" class="toolbar-card">
        <div class="toolbar">
          <VustInput
            id="network-search"
            v-model="networkKeyword"
            name="networkSearch"
            class="search-input"
            :placeholder="t('app.processManager.network.searchPlaceholder')"
            :aria-label="t('app.processManager.network.searchPlaceholder')"
          />
          <VustSelect
            id="network-state-filter"
            v-model="networkState"
            name="networkState"
            class="state-select"
            :aria-label="t('app.processManager.network.columns.state')"
            :options="
              networkStateOptions.map((state) => ({
                label: formatNetworkState(state),
                value: state,
              }))
            "
          />
        </div>
      </VustCard>
      <VustCard shadow="never" class="table-card" full-height>
        <VustTable :data="networkRows" :columns="networkColumns" border data-ui="network-table">
          <template #state="{ row }">{{ formatNetworkState(row.state) }}</template>
          <template #empty><VustEmpty :description="networkEmptyText" /></template>
        </VustTable>
      </VustCard>
      <div class="pagination-bar" data-slot="network-pagination">
        <VustPagination
          :current-page="processManager.networkQuery.page"
          :total-pages="networkTotalPages"
          @page-change="(page) => (processManager.networkQuery.page = page)"
        />
      </div>
    </div>

    <VustLoading :loading="activeTab === 'process' ? loadingProcess : loadingNetwork" cover />
  </div>
</template>

<style scoped>
.process-manager {
  height: 100%;
  padding: var(--vdl-space-3);
  background: var(--vdl-bg-canvas);
  overflow: hidden;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
  min-height: 0;
  box-sizing: border-box;
}

.header-card,
.toolbar-card,
.pagination-bar {
  flex-shrink: 0;
}

.header,
.toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: var(--vdl-space-3);
}

.tabs,
.filters,
.network-summary,
.summary-metric,
.action-buttons {
  display: flex;
  align-items: center;
}

.tabs,
.filters,
.summary-metric,
.action-buttons {
  gap: var(--vdl-space-2);
}

.action-buttons {
  width: 100%;
  justify-content: center;
}

.content-wrapper {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
  min-height: 0;
}

.network-summary {
  gap: var(--vdl-space-4);
  min-width: 0;
}

.summary-metric {
  align-items: baseline;
  min-width: 0;
}

.summary-label,
.filter-label {
  color: var(--vdl-text-muted);
  white-space: nowrap;
}

.summary-label {
  font-size: var(--vdl-font-caption);
}

.filter-label {
  font-size: var(--vdl-font-body-sm);
  color: var(--vdl-text-secondary);
}

.summary-value {
  font-size: var(--vdl-font-body);
  color: var(--vdl-text-primary);
  font-weight: 700;
  white-space: nowrap;
}

.status-select {
  width: 160px;
}

.search-input {
  flex: 1;
}

.state-select {
  width: 160px;
}

.table-card {
  flex: 1;
  min-height: 0;
  padding: 0;
}

.sortable-header {
  appearance: none;
  border: 0;
  padding: 0;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 100%;
  color: inherit;
  background: transparent;
  font: inherit;
}

.sortable-header:hover,
.sortable-header:focus-visible {
  color: var(--vdl-primary);
}

.pagination-bar {
  display: flex;
  justify-content: center;
}

@media (max-width: 1024px) {
  .header {
    align-items: stretch;
    flex-direction: column;
  }

  .network-summary {
    align-items: flex-start;
    flex-wrap: wrap;
  }
}
</style>
