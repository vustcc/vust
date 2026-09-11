<script setup lang="ts">
/**
 * @file DockerOverview.vue
 * @description Docker 概览、资源趋势、系统信息与磁盘清理视图。
 */

import {
  VustAlert,
  VustButton,
  VustCard,
  VustCheckbox,
  VustDescriptions,
  VustEmpty,
  VustLoading,
  VustTabs,
  VustTag,
} from '@/components/ui'
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import * as echarts from 'echarts/core'
import { LineChart, PieChart } from 'echarts/charts'
import {
  GridComponent,
  LegendComponent,
  TitleComponent,
  TooltipComponent,
} from 'echarts/components'
import { CanvasRenderer } from 'echarts/renderers'
import { useI18n } from 'vue-i18n'
import { MAX_OVERVIEW_CONTAINERS, useDockerStore } from '@/stores/docker'
import { formatPercent as formatDockerPercent } from '@/utils/docker-format'

echarts.use([
  PieChart,
  LineChart,
  GridComponent,
  LegendComponent,
  TitleComponent,
  TooltipComponent,
  CanvasRenderer,
])

type MetricType = 'cpu' | 'memory' | 'networkRx' | 'networkTx'

const { t, locale } = useI18n()
const store = useDockerStore()
const activeTab = ref<'resources' | 'system'>('resources')
const metricType = ref<MetricType>('cpu')
const isContainerSelectorOpen = ref(false)
const cpuChartRef = ref<HTMLDivElement | null>(null)
const memoryChartRef = ref<HTMLDivElement | null>(null)
const trendChartRef = ref<HTMLDivElement | null>(null)
let cpuChart: echarts.ECharts | null = null
let memoryChart: echarts.ECharts | null = null
let trendChart: echarts.ECharts | null = null
let resizeObserver: ResizeObserver | null = null

const overviewTabs = computed(() => [
  { label: t('app.docker.overview.tabs.resources'), name: 'resources' },
  { label: t('app.docker.overview.systemInfo.title'), name: 'system' },
])

const metricTabs = computed<Array<{ type: MetricType; label: string }>>(() => [
  { type: 'cpu', label: t('app.docker.overview.cpuCoreTrend') },
  { type: 'memory', label: t('app.docker.overview.memoryWorkingSet') },
  { type: 'networkRx', label: t('app.docker.overview.download') },
  { type: 'networkTx', label: t('app.docker.overview.upload') },
])

const systemInfoItems = computed(() => [
  {
    label: t('app.docker.overview.systemInfo.version'),
    value: store.systemInfo?.ServerVersion || '-',
  },
  { label: t('app.docker.overview.systemInfo.driver'), value: store.systemInfo?.Driver || '-' },
])

const diskCategories = computed(() => {
  const usage = store.diskUsage
  if (!usage) return []
  return [
    { key: 'images', label: t('app.docker.overview.disk.images'), value: usage.images },
    { key: 'containers', label: t('app.docker.overview.disk.containers'), value: usage.containers },
    { key: 'volumes', label: t('app.docker.overview.disk.volumes'), value: usage.volumes },
    { key: 'buildCache', label: t('app.docker.overview.disk.buildCache'), value: usage.buildCache },
  ]
})

const sampleStatusType = computed<'success' | 'warning' | 'danger' | 'info'>(() => {
  switch (store.resourceUsage?.status) {
    case 'fresh':
      return 'success'
    case 'partial':
    case 'stale':
      return 'warning'
    case 'unavailable':
      return 'danger'
    default:
      return 'info'
  }
})

const sampleStatusLabel = computed(() =>
  t(`app.docker.overview.sampleStatus.${store.resourceUsage?.status || 'unavailable'}`),
)

const sampleAlert = computed(() => {
  if (store.overviewError) return { type: 'error' as const, title: store.overviewError }
  if (store.resourceUsage?.status === 'partial') {
    return {
      type: 'warning' as const,
      title: t('app.docker.overview.samplePartial', {
        sampled: store.resourceUsage.sampledContainerCount,
        running: store.resourceUsage.runningContainerCount,
      }),
    }
  }
  if (store.resourceUsage?.status === 'stale') {
    return { type: 'warning' as const, title: t('app.docker.overview.sampleStale') }
  }
  if (!store.resourceUsage || store.resourceUsage.status === 'unavailable') {
    return { type: 'error' as const, title: t('app.docker.overview.sampleUnavailable') }
  }
  return null
})

function formatBytes(bytes: number) {
  if (bytes <= 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  const index = Math.min(
    Math.max(Math.floor(Math.log(bytes) / Math.log(1024)), 0),
    units.length - 1,
  )
  return `${(bytes / 1024 ** index).toFixed(1)} ${units[index]}`
}

/** 按 Docker CLI 的十进制单位和四位有效数字格式化磁盘容量。 */
function formatDockerDiskBytes(bytes: number) {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0B'
  const units = ['B', 'kB', 'MB', 'GB', 'TB', 'PB']
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1000)), units.length - 1)
  const value = bytes / 1000 ** index
  return `${Number(value.toPrecision(4))}${units[index]}`
}

function formatBytesPerSecond(value: number) {
  return `${formatBytes(Math.max(value, 0))}/s`
}

function formatPercent(value?: number) {
  return value === undefined ? '-' : formatDockerPercent(value)
}

function formatTimestamp(timestamp?: number | null, full = false) {
  if (!timestamp) return '-'
  return new Intl.DateTimeFormat(locale.value, {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: full ? '2-digit' : undefined,
    hour12: false,
  }).format(new Date(timestamp * 1000))
}

function getCssVar(name: string) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim()
}

function buildPieOption(title: string, percent?: number, usageBytes?: number, limitBytes?: number) {
  const normalized = percent === undefined ? 0 : Math.min(Math.max(percent, 0), 100)
  return {
    backgroundColor: 'transparent',
    title: {
      text: title,
      left: 10,
      top: 'middle',
      textStyle: {
        fontSize: 12,
        color: getCssVar('--vdl-text-primary') || '#E6EDF7',
        fontWeight: 600,
      },
    },
    tooltip: {
      trigger: 'item',
      backgroundColor: getCssVar('--vdl-bg-panel') || '#162033',
      borderColor: getCssVar('--vdl-border-default') || '#202B40',
      textStyle: { color: getCssVar('--vdl-text-primary') || '#E6EDF7' },
      formatter: () => {
        if (percent === undefined) return t('app.docker.overview.noData')
        const lines = [`${t('app.docker.overview.usagePercent')}: ${formatPercent(percent)}`]
        if (usageBytes !== undefined && limitBytes !== undefined) {
          lines.push(`${t('app.docker.overview.usage')}: ${formatBytes(usageBytes)}`)
          lines.push(`${t('app.docker.overview.limit')}: ${formatBytes(limitBytes)}`)
        }
        return lines.join('<br/>')
      },
    },
    series: [
      {
        type: 'pie',
        radius: ['48%', '72%'],
        center: ['70%', '50%'],
        label: {
          show: true,
          position: 'center',
          formatter: percent === undefined ? '-' : formatPercent(percent),
          color: getCssVar('--vdl-text-muted') || '#91A2B8',
          fontSize: 12,
        },
        data: [
          { value: normalized, itemStyle: { color: getCssVar('--vdl-primary') || '#00C8FF' } },
          {
            value: 100 - normalized,
            itemStyle: { color: getCssVar('--vdl-bg-muted') || '#202B40' },
          },
        ],
      },
    ],
  }
}

function resolveContainerName(id: string) {
  return store.overviewContainers.find((container) => container.id === id)?.name || id.slice(0, 12)
}

function containerStateLabel(state: string) {
  const normalized = state.toLowerCase()
  const key = ['running', 'paused', 'restarting', 'exited'].includes(normalized)
    ? normalized
    : 'unknown'
  return t(`app.docker.overview.states.${key}`)
}

function formatTrendValue(value: number) {
  if (metricType.value === 'cpu') return formatPercent(value)
  if (metricType.value === 'memory') return formatBytes(value)
  return formatBytesPerSecond(value)
}

function buildTrendOption(
  timestamps: number[],
  series: Array<{ name: string; data: Array<number | null> }>,
) {
  return {
    backgroundColor: 'transparent',
    tooltip: {
      trigger: 'axis',
      backgroundColor: getCssVar('--vdl-bg-panel') || '#162033',
      borderColor: getCssVar('--vdl-border-default') || '#202B40',
      textStyle: { color: getCssVar('--vdl-text-primary') || '#E6EDF7' },
      formatter: (
        params: Array<{
          seriesName: string
          marker: string
          data: number | null
          dataIndex: number
        }>,
      ) => {
        const first = params[0]
        if (!first) return ''
        const lines = params.map((item) =>
          item.data === null || item.data === undefined
            ? `${item.marker}${item.seriesName}: -`
            : `${item.marker}${item.seriesName}: ${formatTrendValue(item.data)}`,
        )
        return [formatTimestamp(timestamps[first.dataIndex], true), ...lines].join('<br/>')
      },
    },
    legend: {
      type: 'scroll',
      orient: 'horizontal',
      bottom: 6,
      left: 'center',
      width: '88%',
      pageButtonPosition: 'end',
      pageButtonItemGap: 2,
      pageButtonGap: 6,
      pageIconSize: 10,
      pageFormatter: () => '',
      pageIconColor: getCssVar('--vdl-primary') || '#00C8FF',
      pageIconInactiveColor: getCssVar('--vdl-text-muted') || '#91A2B8',
      pageTextStyle: {
        fontSize: 1,
        lineHeight: 1,
        color: 'transparent',
      },
      textStyle: { fontSize: 10, color: getCssVar('--vdl-text-muted') || '#91A2B8' },
    },
    grid: { left: 58, right: 20, bottom: 56, top: 24 },
    xAxis: {
      type: 'category',
      data: timestamps,
      axisLabel: {
        fontSize: 10,
        color: getCssVar('--vdl-text-muted') || '#91A2B8',
        formatter: (value: number) => formatTimestamp(value),
      },
      axisTick: { show: false },
      axisLine: { lineStyle: { color: getCssVar('--vdl-border-subtle') || '#202B40' } },
    },
    yAxis: {
      type: 'value',
      min: 0,
      axisLabel: {
        fontSize: 10,
        color: getCssVar('--vdl-text-muted') || '#91A2B8',
        formatter: (value: number) => formatTrendValue(value),
      },
      splitLine: { lineStyle: { color: 'rgba(148, 163, 184, 0.12)' } },
    },
    color: ['#00C8FF', '#00D4B4', '#7C6CFF', '#FFB547', '#FF5E7A', '#1D63ED', '#D96BCB'],
    series: series.map((item) => ({
      type: 'line',
      name: item.name,
      data: item.data,
      smooth: false,
      connectNulls: false,
      showSymbol: false,
    })),
  }
}

function buildEmptyTrendOption(message: string) {
  return {
    ...buildTrendOption([], []),
    title: {
      show: true,
      text: message,
      left: 'center',
      top: 'middle',
      textStyle: {
        fontSize: 12,
        color: getCssVar('--vdl-text-muted') || '#91A2B8',
        fontWeight: 500,
      },
    },
    tooltip: { show: false },
    legend: { show: false },
  }
}

function updateTrendChart() {
  if (!trendChart) return
  const selectedIds = store.overviewSelectedContainerIds
  if (!selectedIds.length) {
    trendChart.setOption(buildEmptyTrendOption(t('app.docker.overview.noData')), true)
    return
  }
  const timestamps = new Set<number>()
  for (const id of selectedIds) {
    for (const point of store.overviewHistoryMap[id]?.points || []) timestamps.add(point.timestamp)
  }
  const timeValues = Array.from(timestamps).sort((a, b) => a - b)
  if (!timeValues.length) {
    trendChart.setOption(buildEmptyTrendOption(t('app.docker.overview.noData')), true)
    return
  }
  const series = selectedIds.map((id) => {
    const values = new Map<number, number | null>()
    for (const point of store.overviewHistoryMap[id]?.points || []) {
      let value: number | null
      switch (metricType.value) {
        case 'cpu':
          value = point.cpuCorePercent
          break
        case 'memory':
          value = point.memoryWorkingSetBytes
          break
        case 'networkRx':
          value = point.networkRxBytesPerSecond
          break
        case 'networkTx':
          value = point.networkTxBytesPerSecond
          break
      }
      values.set(point.timestamp, value)
    }
    return {
      name: resolveContainerName(id),
      data: timeValues.map((timestamp) => values.get(timestamp) ?? null),
    }
  })
  trendChart.setOption(buildTrendOption(timeValues, series), true)
}

function updateCharts() {
  const usage = store.resourceUsage
  const hasData = usage && usage.status !== 'unavailable'
  cpuChart?.setOption(
    buildPieOption(
      t('app.docker.overview.cpuHostUsage'),
      hasData ? usage.cpuHostPercent : undefined,
    ),
    true,
  )
  memoryChart?.setOption(
    buildPieOption(
      t('app.docker.overview.memUsage'),
      hasData ? usage.memoryPercent : undefined,
      hasData ? usage.memoryWorkingSetBytes : undefined,
      hasData ? usage.memoryLimitBytes : undefined,
    ),
    true,
  )
  updateTrendChart()
}

function initCharts() {
  if (cpuChartRef.value && !cpuChart) cpuChart = echarts.init(cpuChartRef.value)
  if (memoryChartRef.value && !memoryChart) memoryChart = echarts.init(memoryChartRef.value)
  if (trendChartRef.value && !trendChart) trendChart = echarts.init(trendChartRef.value)
  updateCharts()
  if (!resizeObserver)
    resizeObserver = new ResizeObserver(() =>
      [cpuChart, memoryChart, trendChart].forEach((chart) => chart?.resize()),
    )
  ;[cpuChartRef.value, memoryChartRef.value, trendChartRef.value].forEach((element) => {
    if (element) resizeObserver?.observe(element)
  })
}

function toggleContainer(id: string, checked: boolean) {
  const current = new Set(store.overviewSelectedContainerIds)
  if (checked) current.add(id)
  else current.delete(id)
  store.updateOverviewSelectedContainers(Array.from(current))
}

watch(activeTab, (tab) => {
  if (tab === 'system') {
    void Promise.all([store.fetchDockerInfo(), store.fetchDockerDiskUsage()])
  } else {
    void nextTick(initCharts)
  }
})
watch(
  () => [
    store.resourceUsage,
    store.overviewHistoryMap,
    store.overviewSelectedContainerIds,
    metricType.value,
    locale.value,
  ],
  () => updateCharts(),
  { deep: true },
)

onMounted(() => void nextTick(initCharts))
onUnmounted(() => {
  resizeObserver?.disconnect()
  ;[cpuChart, memoryChart, trendChart].forEach((chart) => chart?.dispose())
})
</script>

<template>
  <div class="docker-overview" data-page="docker-overview">
    <div class="overview-summary" data-ui="overview-summary">
      <div class="summary-item" data-slot="service-status">
        <span class="summary-label">{{ t('app.docker.overview.status') }}</span>
        <VustTag :type="store.dockerStatus ? 'success' : 'danger'" size="small">
          {{
            store.dockerStatus ? t('app.docker.overview.running') : t('app.docker.overview.stopped')
          }}
        </VustTag>
      </div>
      <div class="summary-item" data-slot="project-states">
        <span class="summary-label">{{ t('app.docker.overview.projects') }}</span>
        <span class="summary-value"
          >{{ store.projectStates.healthy }}/{{ store.projectStates.total }}</span
        >
      </div>
      <div class="summary-item" data-slot="container-states">
        <span class="summary-label">{{ t('app.docker.overview.containers') }}</span>
        <span class="summary-value"
          >{{ store.containerStates.running }}/{{ store.containerStates.total }}</span
        >
      </div>
      <div class="summary-item" data-slot="image-states">
        <span class="summary-label">{{ t('app.docker.overview.images') }}</span>
        <span class="summary-value">{{ store.imageCounts.total }}</span>
      </div>
    </div>

    <VustTabs v-model="activeTab" :tabs="overviewTabs" data-ui="overview-tabs" />

    <div v-show="activeTab === 'resources'" class="overview-content" data-slot="resources">
      <VustAlert v-if="sampleAlert" :type="sampleAlert.type" :title="sampleAlert.title" show-icon />
      <div class="resource-meta" data-ui="sample-status">
        <VustTag :type="sampleStatusType" size="small">{{ sampleStatusLabel }}</VustTag>
        <span>
          {{ t('app.docker.overview.sampleCoverage') }}:
          {{ store.resourceUsage?.sampledContainerCount ?? 0 }}/{{
            store.resourceUsage?.runningContainerCount ?? 0
          }}
        </span>
        <span
          >{{ t('app.docker.overview.updatedAt') }}:
          {{ formatTimestamp(store.resourceUsage?.collectedAt) }}</span
        >
      </div>

      <div class="resource-grid">
        <VustCard shadow="never" class="resource-card">
          <div ref="cpuChartRef" class="resource-chart" data-ui="cpu-host-chart"></div>
        </VustCard>
        <VustCard shadow="never" class="resource-card">
          <div ref="memoryChartRef" class="resource-chart" data-ui="memory-chart"></div>
        </VustCard>
        <VustCard shadow="never" class="resource-card resource-wide">
          <div class="resource-toolbar" data-ui="trend-toolbar">
            <VustTabs
              :model-value="metricType"
              :tabs="metricTabs.map((tab) => ({ label: tab.label, name: tab.type }))"
              @update:model-value="metricType = $event as MetricType"
            />
            <div class="range-actions" data-slot="time-range">
              <VustButton
                v-for="hours in [1, 6, 12] as const"
                :key="hours"
                size="small"
                :type="store.overviewHistoryHours === hours ? 'primary' : 'secondary'"
                @click="store.setOverviewHistoryHours(hours)"
              >
                {{ t('app.docker.overview.hours', { count: hours }) }}
              </VustButton>
            </div>
            <div class="selector-wrap">
              <VustButton size="small" @click="isContainerSelectorOpen = !isContainerSelectorOpen">
                {{ t('app.docker.overview.containerSelector') }} ({{
                  store.overviewSelectedContainerIds.length
                }}/{{ MAX_OVERVIEW_CONTAINERS }})
              </VustButton>
              <div
                v-if="isContainerSelectorOpen"
                class="selector-panel"
                data-ui="container-selector"
              >
                <div v-if="store.overviewContainers.length" class="selector-list">
                  <VustCheckbox
                    v-for="container in store.overviewContainers"
                    :key="container.id"
                    :model-value="store.overviewSelectedContainerIds.includes(container.id)"
                    :disabled="
                      !store.overviewSelectedContainerIds.includes(container.id) &&
                      store.overviewSelectedContainerIds.length >= MAX_OVERVIEW_CONTAINERS
                    "
                    @update:model-value="toggleContainer(container.id, $event)"
                  >
                    <span class="selector-label">{{ container.name }}</span>
                    <VustTag
                      :type="container.state.toLowerCase() === 'running' ? 'success' : 'info'"
                      size="small"
                    >
                      {{ containerStateLabel(container.state) }}
                    </VustTag>
                  </VustCheckbox>
                </div>
                <VustEmpty v-else :description="t('app.docker.overview.noContainers')" />
              </div>
            </div>
          </div>
          <VustAlert
            v-if="store.overviewHistoryError"
            type="error"
            :title="store.overviewHistoryError"
          />
          <div
            ref="trendChartRef"
            class="resource-chart resource-chart-wide"
            data-ui="trend-chart"
          ></div>
        </VustCard>
      </div>
    </div>

    <div v-if="activeTab === 'system'" class="overview-content system-content" data-slot="system">
      <VustAlert
        v-if="store.systemInfoError"
        type="error"
        :title="store.systemInfoError"
        show-icon
      />
      <div class="system-panel" data-ui="system-info">
        <VustDescriptions :items="systemInfoItems" :column="2" border />
        <VustLoading :loading="store.systemInfoLoading" cover />
      </div>

      <div class="disk-panel" data-ui="disk-usage">
        <div class="panel-heading">
          <h3>{{ t('app.docker.overview.disk.title') }}</h3>
          <span
            >{{ t('app.docker.overview.updatedAt') }}:
            {{ formatTimestamp(store.diskUsage?.collectedAt) }}</span
          >
        </div>
        <VustAlert
          v-if="store.diskUsageError"
          type="error"
          :title="store.diskUsageError"
          show-icon
        />
        <div v-if="diskCategories.length" class="disk-grid">
          <div v-for="category in diskCategories" :key="category.key" class="disk-item">
            <span class="disk-label">{{ category.label }}</span>
            <strong>{{ formatDockerDiskBytes(category.value.sizeBytes) }}</strong>
            <span>
              {{ t('app.docker.overview.disk.reclaimable') }}:
              {{ formatDockerDiskBytes(category.value.reclaimableBytes) }}
            </span>
            <span>{{ category.value.activeCount }}/{{ category.value.totalCount }}</span>
          </div>
        </div>
        <VustEmpty
          v-else-if="!store.diskUsageLoading"
          :description="t('app.docker.overview.noData')"
        />
        <VustLoading :loading="store.diskUsageLoading" cover />
      </div>

      <div class="cleanup-panel" data-ui="system-cleanup">
        <div>
          <h3>{{ t('app.docker.overview.cleanup.title') }}</h3>
          <p>{{ t('app.docker.overview.cleanup.desc') }}</p>
        </div>
        <VustButton
          type="danger"
          :loading="store.pruneLoading"
          :disabled="store.pruneLoading"
          @click="store.handlePruneSystem"
        >
          {{ t('app.docker.overview.cleanup.action') }}
        </VustButton>
      </div>
    </div>
  </div>
</template>

<style scoped>
.docker-overview {
  display: flex;
  flex: 1;
  min-height: 0;
  flex-direction: column;
  overflow: hidden;
  color: var(--vdl-text-primary);
}

.overview-summary {
  display: grid;
  flex-shrink: 0;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  border-bottom: 1px solid var(--vdl-border-subtle);
}

.summary-item {
  display: flex;
  min-width: 0;
  min-height: 72px;
  flex-direction: column;
  justify-content: center;
  gap: var(--vdl-space-1);
  padding: var(--vdl-space-3) var(--vdl-space-4);
  border-right: 1px solid var(--vdl-border-subtle);
}

.summary-item:last-child {
  border-right: 0;
}
.summary-item[data-slot='service-status'] :deep(.vl-tag) {
  align-self: flex-start;
}
.summary-label,
.resource-meta,
.panel-heading span,
.disk-item span {
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-body-sm);
}
.summary-value {
  font-size: 22px;
  font-weight: 650;
  line-height: 1.1;
}
:deep(.vl-tabs-nav) {
  padding: 0 var(--vdl-space-4);
}

.overview-content {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: var(--vdl-space-3) var(--vdl-space-4) var(--vdl-space-4);
}

.resource-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--vdl-space-3);
  margin-bottom: var(--vdl-space-3);
}
.resource-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--vdl-space-3);
}
.resource-card {
  min-width: 0;
}
.resource-wide {
  grid-column: 1 / -1;
}
.resource-chart {
  width: 100%;
  height: 170px;
}
.resource-chart-wide {
  height: 320px;
}
.resource-toolbar {
  position: relative;
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--vdl-space-3);
  margin-bottom: var(--vdl-space-2);
}
.resource-toolbar :deep(.vl-tabs) {
  min-width: 320px;
  flex: 1;
}
.range-actions {
  display: flex;
  gap: var(--vdl-space-1);
}
.selector-wrap {
  position: relative;
}
.selector-panel {
  position: absolute;
  z-index: 20;
  top: calc(100% + var(--vdl-space-2));
  right: 0;
  width: 300px;
  max-height: 280px;
  overflow: auto;
  padding: var(--vdl-space-3);
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-panel);
  box-shadow: var(--vdl-shadow-panel);
}
.selector-list {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-2);
}
.selector-list :deep(.vl-checkbox) {
  width: 100%;
}
.selector-list :deep(.vl-checkbox-label) {
  display: flex;
  min-width: 0;
  flex: 1;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-2);
}
.selector-label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.system-content {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-4);
}
.system-panel,
.disk-panel {
  position: relative;
}
.panel-heading,
.cleanup-panel {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-4);
}
.panel-heading h3,
.cleanup-panel h3 {
  margin: 0;
  font-size: var(--vdl-font-subtitle);
}
.disk-grid {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  margin-top: var(--vdl-space-3);
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  overflow: hidden;
}
.disk-item {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: var(--vdl-space-1);
  padding: var(--vdl-space-3);
  border-right: 1px solid var(--vdl-border-subtle);
}
.disk-item:last-child {
  border-right: 0;
}
.disk-item strong {
  font-size: var(--vdl-font-title);
}
.cleanup-panel {
  padding-top: var(--vdl-space-4);
  border-top: 1px solid var(--vdl-border-subtle);
}
.cleanup-panel p {
  margin: var(--vdl-space-1) 0 0;
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-body-sm);
}

@media (max-width: 920px) {
  .overview-summary {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
  .summary-item:nth-child(2) {
    border-right: 0;
  }
  .summary-item:nth-child(-n + 2) {
    border-bottom: 1px solid var(--vdl-border-subtle);
  }
  .disk-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
  .disk-item:nth-child(2) {
    border-right: 0;
  }
  .disk-item:nth-child(-n + 2) {
    border-bottom: 1px solid var(--vdl-border-subtle);
  }
}

@media (max-width: 640px) {
  .resource-grid {
    grid-template-columns: 1fr;
  }
  .resource-wide {
    grid-column: auto;
  }
  .resource-toolbar {
    align-items: stretch;
    flex-direction: column;
  }
  .selector-wrap,
  .selector-wrap :deep(.vl-button) {
    width: 100%;
  }
  .selector-panel {
    right: auto;
    left: 0;
    width: min(300px, 100%);
  }
  .disk-grid {
    grid-template-columns: 1fr;
  }
  .disk-item {
    border-right: 0;
    border-bottom: 1px solid var(--vdl-border-subtle);
  }
  .disk-item:last-child {
    border-bottom: 0;
  }
  .cleanup-panel {
    align-items: flex-start;
    flex-direction: column;
  }
}
</style>
