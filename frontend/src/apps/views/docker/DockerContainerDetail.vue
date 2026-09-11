<script setup lang="ts">
/**
 * @file DockerContainerDetail.vue
 * @description Docker 容器配置、趋势、进程、日志和终端详情页。
 */

import { computed, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useDockerStore } from '@/stores/docker'
import { formatDateTime } from '@/utils/time'
import {
  VustAlert,
  VustButton,
  VustDescriptions,
  VustEmpty,
  VustLoading,
  VustTable,
  VustTabs,
  VustTag,
} from '@/components/ui'
import type { VustTableColumn } from '@/components/ui/VustTable.vue'
import DockerContainerLogsPanel from './DockerContainerLogsPanel.vue'
import DockerContainerTerminalPanel from './terminal/DockerContainerTerminalPanel.vue'
import { useContainerHistoryData } from './composables/useContainerHistoryData'
import { useContainerHistoryCharts } from './composables/useContainerHistoryCharts'
import { useContainerInspectData } from './composables/useContainerInspectData'
import { useContainerProcesses } from './composables/useContainerProcesses'

const props = defineProps<{
  containerId: string
  nodeId: string
}>()

const emit = defineEmits<{
  (event: 'back'): void
  (event: 'logsActiveChange', value: boolean): void
  (event: 'terminalActiveChange', value: boolean): void
}>()

const { t, locale } = useI18n()
const store = useDockerStore()
const activeTab = ref<'basic' | 'processes' | 'logs' | 'terminal'>('basic')
const containerIdRef = computed(() => props.containerId)
const nodeIdRef = computed(() => props.nodeId)
const selectedContainer = computed(
  () => store.containers.find((container) => container.id === props.containerId) || null,
)

const { inspectDetail, isInspectLoading, inspectError, reloadInspect } = useContainerInspectData({
  selectedContainerId: containerIdRef,
  nodeId: nodeIdRef,
})
const summary = computed(() => inspectDetail.value?.summary || selectedContainer.value)
const selectedContainerName = computed(() => summary.value?.name || '')

const tabs = computed(() => {
  const items = [
    { label: t('app.docker.containers.basicInfo'), name: 'basic' },
    { label: t('app.docker.containers.processes'), name: 'processes' },
    { label: t('app.docker.containers.logs'), name: 'logs' },
  ]
  if (summary.value?.capabilities.canExec) {
    items.push({ label: t('app.docker.containers.terminal'), name: 'terminal' })
  }
  return items
})

const basicItems = computed(() => {
  const detail = inspectDetail.value
  const current = summary.value
  if (!current) return []
  return [
    { label: t('app.docker.containers.containerId'), value: current.id, span: 2 },
    {
      label: t('app.docker.containers.managementLabel'),
      value: managementLabel(current.management.kind, current.management.ownerName),
    },
    { label: t('app.docker.containers.status'), value: stateLabel(current.state) },
    { label: t('app.docker.containers.imageFull'), value: current.imageRef || '-' },
    { label: t('app.docker.containers.createdAt'), value: formatDateTime(detail?.createdAt) },
    { label: t('app.docker.containers.startedAt'), value: formatDateTime(detail?.startedAt) },
    { label: t('app.docker.containers.finishedAt'), value: formatDateTime(detail?.finishedAt) },
    {
      label: t('app.docker.containers.restartPolicy'),
      value: detail
        ? `${detail.restartPolicy.name}${
            detail.restartPolicy.maximumRetryCount > 0
              ? ` (${detail.restartPolicy.maximumRetryCount})`
              : ''
          }`
        : '-',
    },
    { label: t('app.docker.containers.restartCount'), value: detail?.restartCount ?? 0 },
    { label: t('app.docker.containers.exitCode'), value: detail?.exitCode ?? '-' },
    { label: t('app.docker.containers.logDriver'), value: detail?.logDriver || '-' },
    {
      label: t('app.docker.containers.startCommand'),
      value: formatCommand(detail?.command),
      span: 2,
    },
  ].filter(({ value }) => value !== undefined && value !== null && value !== '' && value !== '-')
})

const environmentColumns = computed<VustTableColumn[]>(() => [
  { label: t('app.docker.containers.varName'), minWidth: 180, prop: 'name' },
  { label: t('app.docker.containers.varValue'), minWidth: 260, slot: 'value' },
])
const mountColumns = computed<VustTableColumn[]>(() => [
  { label: t('app.docker.containers.mountType'), width: 110, prop: 'kind', align: 'center' },
  { label: t('app.docker.containers.mountSource'), minWidth: 200, slot: 'source', align: 'center' },
  {
    label: t('app.docker.containers.containerPath'),
    minWidth: 180,
    slot: 'target',
    align: 'center',
  },
  { label: t('app.docker.containers.accessMode'), width: 100, slot: 'mode', align: 'center' },
])
const networkColumns = computed<VustTableColumn[]>(() => [
  { label: t('app.docker.containers.network'), minWidth: 150, prop: 'name', align: 'center' },
  {
    label: t('app.docker.containers.ipv4Address'),
    minWidth: 150,
    prop: 'ipv4Address',
    align: 'center',
  },
  {
    label: t('app.docker.containers.ipv6Address'),
    minWidth: 190,
    prop: 'ipv6Address',
    align: 'center',
  },
  { label: t('app.docker.containers.gateway'), minWidth: 140, prop: 'gateway', align: 'center' },
  {
    label: t('app.docker.containers.macAddress'),
    minWidth: 150,
    prop: 'macAddress',
    align: 'center',
  },
])
const processTableColumns = computed<VustTableColumn[]>(() => [
  { label: t('app.docker.containers.pid'), width: 90, slot: 'pid', align: 'center' },
  { label: t('app.docker.containers.cpuUsage'), width: 110, slot: 'cpu', align: 'center' },
  { label: t('app.docker.containers.memUsage'), width: 110, slot: 'memory', align: 'center' },
  { label: t('app.docker.containers.command'), minWidth: 280, slot: 'command' },
])

const labelsText = computed(() => {
  const labels = inspectDetail.value?.labels
  if (!labels || Object.keys(labels).length === 0) return '-'
  return Object.entries(labels)
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([key, value]) => `${key}=${value}`)
    .join('\n')
})

const {
  containerHistory,
  historyLoading,
  historyError,
  historyStatus,
  dispose: disposeContainerHistory,
} = useContainerHistoryData({
  selectedContainerId: containerIdRef,
  nodeId: nodeIdRef,
  activeTab,
  t,
})

const {
  historyCpuChartRef,
  historyMemoryChartRef,
  historyNetworkChartRef,
  renderHistory,
  clearHistory,
  dispose: disposeHistoryCharts,
} = useContainerHistoryCharts({ t })

const { processLoading, processError, sortedProcessRows, loadProcessList } = useContainerProcesses({
  selectedContainerId: containerIdRef,
  nodeId: nodeIdRef,
  activeTab,
  t,
})

function managementLabel(kind: string, ownerName?: string): string {
  const label = t(`app.docker.containers.management.${kind}`)
  return ownerName ? `${label} · ${ownerName}` : label
}

function stateLabel(state?: string): string {
  return state ? t(`app.docker.containers.states.${state}`) : '-'
}

function formatCommand(command?: string[]): string {
  return command?.length ? command.join(' ') : '-'
}

watch(
  () => store.containers,
  (containers) => {
    if (!containers.some((container) => container.id === props.containerId)) emit('back')
  },
)

watch(
  () => [activeTab.value, historyStatus.value, containerHistory.value] as const,
  async ([tab, status, history]) => {
    if (tab !== 'basic') return
    if (status !== 'ready' || !history?.points.length) {
      clearHistory()
      return
    }
    await renderHistory(history)
  },
  { deep: true },
)

watch(locale, async () => {
  if (activeTab.value === 'basic' && containerHistory.value?.points.length) {
    await renderHistory(containerHistory.value)
  }
})

watch(tabs, (items) => {
  if (!items.some((item) => item.name === activeTab.value)) activeTab.value = 'basic'
})

onUnmounted(() => {
  disposeContainerHistory()
  disposeHistoryCharts()
  emit('logsActiveChange', false)
  emit('terminalActiveChange', false)
})
</script>

<template>
  <div class="container-detail" data-page="docker-container-detail">
    <div class="detail-toolbar" data-ui="toolbar">
      <VustButton size="small" type="secondary" data-ui="back" @click="emit('back')">
        {{ t('app.docker.containers.backToList') }}
      </VustButton>
      <div class="detail-title" data-slot="title">
        <span>{{ selectedContainerName || t('app.docker.containers.containerDetails') }}</span>
        <span class="mono detail-id">{{ props.containerId.slice(0, 12) }}</span>
      </div>
      <VustTag :type="summary?.state === 'running' ? 'success' : 'info'" effect="light">
        {{ stateLabel(summary?.state) }}
      </VustTag>
    </div>

    <VustTabs v-model="activeTab" :tabs="tabs" class="detail-tabs" data-ui="detail-tabs" />

    <div class="detail-content" data-slot="content">
      <div v-show="activeTab === 'basic'" class="tab-panel basic-panel" data-slot="basic">
        <VustAlert
          v-if="inspectError"
          type="error"
          :title="t('app.docker.containers.detailLoadFailed')"
          :description="inspectError"
        >
          <template #action>
            <VustButton size="small" @click="reloadInspect">{{ t('common.retry') }}</VustButton>
          </template>
        </VustAlert>
        <template v-else-if="inspectDetail">
          <VustDescriptions :items="basicItems" :column="2" border data-ui="basic-info" />

          <div v-if="inspectDetail.environment.length" class="detail-block" data-slot="environment">
            <div class="block-title">{{ t('app.docker.containers.envVars') }}</div>
            <VustTable :data="inspectDetail.environment" :columns="environmentColumns" border>
              <template #value="{ row }"
                ><span class="mono">{{ row.value }}</span></template
              >
            </VustTable>
          </div>

          <div v-if="inspectDetail.mounts.length" class="detail-block" data-slot="mounts">
            <div class="block-title">{{ t('app.docker.containers.mounts') }}</div>
            <VustTable :data="inspectDetail.mounts" :columns="mountColumns" border>
              <template #source="{ row }"
                ><span class="mono">{{ row.source }}</span></template
              >
              <template #target="{ row }"
                ><span class="mono">{{ row.target }}</span></template
              >
              <template #mode="{ row }">
                {{
                  row.readOnly
                    ? t('app.docker.containers.readOnlyMount')
                    : t('app.docker.containers.readWriteMount')
                }}
              </template>
            </VustTable>
          </div>

          <div v-if="inspectDetail.networks.length" class="detail-block" data-slot="networks">
            <div class="block-title">{{ t('app.docker.containers.network') }}</div>
            <VustTable :data="inspectDetail.networks" :columns="networkColumns" border />
          </div>

          <div
            v-if="Object.keys(inspectDetail.labels).length"
            class="detail-block"
            data-slot="labels"
          >
            <div class="block-title">{{ t('app.docker.containers.labels') }}</div>
            <pre class="label-list mono">{{ labelsText }}</pre>
          </div>
        </template>
        <VustLoading :loading="isInspectLoading" cover />

        <div class="detail-block trend-block" data-slot="resource-trend">
          <div class="block-title-row">
            <div class="block-title">{{ t('app.docker.containers.resourceTrend') }}</div>
            <span>{{ t('app.docker.containers.trendWindow') }}</span>
          </div>
          <div class="history-body">
            <div class="history-charts">
              <div ref="historyCpuChartRef" class="history-chart" />
              <div ref="historyMemoryChartRef" class="history-chart" />
              <div ref="historyNetworkChartRef" class="history-chart" />
            </div>
            <VustLoading :loading="historyLoading" cover />
            <VustAlert v-if="historyError" type="error" :title="historyError" />
            <VustEmpty
              v-else-if="!historyLoading && !containerHistory?.points.length"
              :description="t('app.docker.containers.noTrendData')"
            />
          </div>
        </div>
      </div>

      <div v-show="activeTab === 'processes'" class="tab-panel process-panel" data-slot="processes">
        <div class="block-title-row">
          <div class="block-title">{{ t('app.docker.containers.containerProcesses') }}</div>
          <VustButton size="small" :loading="processLoading" @click="loadProcessList">
            {{ t('common.refresh') }}
          </VustButton>
        </div>
        <VustAlert v-if="processError" type="error" :title="processError" />
        <VustTable
          v-else-if="sortedProcessRows.length"
          :data="sortedProcessRows"
          :columns="processTableColumns"
          border
        >
          <template #pid="{ row }"
            ><span class="mono">{{ row[0] }}</span></template
          >
          <template #cpu="{ row }">{{ row[1] }}</template>
          <template #memory="{ row }">{{ row[2] }}</template>
          <template #command="{ row }"
            ><span class="mono">{{ row[3] }}</span></template
          >
        </VustTable>
        <VustEmpty
          v-else-if="!processLoading"
          :description="t('app.docker.containers.noProcessInfo')"
        />
        <VustLoading :loading="processLoading" cover />
      </div>

      <div v-show="activeTab === 'logs'" class="tab-panel fill-panel" data-slot="logs">
        <DockerContainerLogsPanel
          :container-id="props.containerId"
          :container-name="selectedContainerName"
          :node-id="props.nodeId"
          :active="activeTab === 'logs'"
          @active-change="(value) => emit('logsActiveChange', value)"
        />
      </div>

      <div
        v-if="summary?.capabilities.canExec"
        v-show="activeTab === 'terminal'"
        class="tab-panel fill-panel"
        data-slot="terminal"
      >
        <DockerContainerTerminalPanel
          :container-id="props.containerId"
          :node-id="props.nodeId"
          :active="activeTab === 'terminal'"
          @active-change="(value) => emit('terminalActiveChange', value)"
        />
      </div>
    </div>
  </div>
</template>

<style scoped>
.container-detail {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--vdl-bg-panel);
}

.detail-toolbar,
.block-title-row {
  display: flex;
  align-items: center;
}

.detail-toolbar {
  flex-shrink: 0;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-3) var(--vdl-space-4);
  border-bottom: 1px solid var(--vdl-border-subtle);
}

.detail-title {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: baseline;
  gap: var(--vdl-space-2);
  font-weight: var(--vdl-font-weight-semibold);
}

.detail-id,
.block-title-row span {
  color: var(--vdl-text-subtle);
  font-size: var(--vdl-font-caption);
  font-weight: normal;
}

.detail-tabs {
  flex-shrink: 0;
}

.detail-content {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}

.tab-panel {
  position: relative;
  height: 100%;
  min-height: 0;
  padding: var(--vdl-space-4);
  overflow: auto;
}

.basic-panel,
.detail-block {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
}

.basic-panel {
  gap: var(--vdl-space-5);
}

.block-title-row {
  justify-content: space-between;
  gap: var(--vdl-space-3);
}

.block-title {
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-body-sm);
  font-weight: var(--vdl-font-weight-semibold);
}

.label-list {
  max-height: 220px;
  margin: 0;
  overflow: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-caption);
}

.trend-block {
  min-height: 300px;
}

.history-body {
  position: relative;
  min-height: 260px;
}

.history-charts {
  display: grid;
  grid-template-columns: repeat(3, minmax(260px, 1fr));
  gap: var(--vdl-space-3);
}

.history-chart {
  height: 250px;
  min-width: 0;
  border: 1px solid var(--vdl-border-subtle);
  border-radius: var(--vdl-radius-md);
}

.process-panel {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
}

.fill-panel {
  display: flex;
  flex-direction: column;
  padding: 0;
  overflow: hidden;
}

.mono {
  font-family: var(--vdl-font-mono);
}

@media (max-width: 1100px) {
  .history-charts {
    grid-template-columns: 1fr;
  }
}
</style>
