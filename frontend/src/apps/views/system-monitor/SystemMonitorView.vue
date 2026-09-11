<script setup lang="ts">
/**
 * @file SystemMonitorView.vue
 * @description 系统监控只读观测视图。
 */

import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { LineChart } from 'echarts/charts'
import {
  DataZoomComponent,
  GridComponent,
  LegendComponent,
  TooltipComponent,
} from 'echarts/components'
import { use } from 'echarts/core'
import { CanvasRenderer } from 'echarts/renderers'
import { useThemeStore, buildChartTheme } from '@/stores/theme'
import { useWindowManagerStore } from '@/stores/window-manager'
import { useSystemMonitor } from '@/composables/useSystemMonitor'
import {
  buildLoadChartOptions,
  buildCpuChartOptions,
  buildMemoryChartOptions,
  buildDiskIoChartOptions,
  buildNetworkChartOptions,
} from './chartOptions'
import MonitorChartPanel from './MonitorChartPanel.vue'
import { VustAlert, VustButton, VustEmpty, VustLoading, VustSelect, VustTag } from '@/components/ui'

use([
  LineChart,
  GridComponent,
  TooltipComponent,
  LegendComponent,
  DataZoomComponent,
  CanvasRenderer,
])

const LOADING_PLACEHOLDER_DELAY_MS = 200

const props = defineProps<{
  isMaximized?: boolean
  windowId?: string
  payload?: Record<string, unknown>
}>()

const { t } = useI18n()
const themeStore = useThemeStore()
const windowStore = useWindowManagerStore()
const {
  overview,
  series,
  overviewState,
  seriesState,
  timeRange,
  points,
  labels,
  load1,
  load5,
  load15,
  cpu,
  memory,
  diskRead,
  diskWrite,
  netRx,
  netTx,
  diskScale,
  netScale,
  busy,
  manualRefreshing,
  refreshAll,
} = useSystemMonitor()

const rangeOptions = computed(() => [
  { value: '1h', label: t('app.systemMonitor.ranges.1h') },
  { value: '6h', label: t('app.systemMonitor.ranges.6h') },
  { value: '24h', label: t('app.systemMonitor.ranges.24h') },
  { value: '3d', label: t('app.systemMonitor.ranges.3d') },
  { value: '7d', label: t('app.systemMonitor.ranges.7d') },
])
const chartTheme = computed(() => buildChartTheme(themeStore.currentTheme))
const hasSeriesData = computed(() => points.value.some((point) => point.coveragePercent > 0))
const seriesMatchesRange = computed(() => series.value?.range === timeRange.value)
const awaitingSeriesWithoutChart = computed(
  () =>
    seriesState.value.loading ||
    (seriesState.value.refreshing && !seriesMatchesRange.value && !hasSeriesData.value),
)
const showSeriesLoading = ref(false)
let seriesLoadingTimer: number | null = null
const warnings = computed(() =>
  [overviewState.value.warning, seriesState.value.warning].filter(Boolean),
)
const failures = computed(() =>
  [overviewState.value.error, seriesState.value.error].filter(Boolean),
)
const qualityTag = computed(() => {
  const status = overview.value?.snapshotStatus ?? 'unavailable'
  return {
    type:
      status === 'fresh'
        ? ('success' as const)
        : status === 'partial' || status === 'stale'
          ? ('warning' as const)
          : ('danger' as const),
    label: t(`app.systemMonitor.snapshotStatus.${status}`),
  }
})
const emptyDescription = computed(() =>
  overview.value?.history.state === 'stopped'
    ? t('app.systemMonitor.collectionStopped')
    : t('app.systemMonitor.noHistory'),
)

watch(
  awaitingSeriesWithoutChart,
  (isAwaiting) => {
    if (seriesLoadingTimer !== null) {
      window.clearTimeout(seriesLoadingTimer)
      seriesLoadingTimer = null
    }
    showSeriesLoading.value = false
    if (!isAwaiting) return
    seriesLoadingTimer = window.setTimeout(() => {
      showSeriesLoading.value = true
      seriesLoadingTimer = null
    }, LOADING_PLACEHOLDER_DELAY_MS)
  },
  { immediate: true },
)

onBeforeUnmount(() => {
  if (seriesLoadingTimer !== null) window.clearTimeout(seriesLoadingTimer)
})

const loadChartOptions = computed(() =>
  buildLoadChartOptions(labels.value, load1.value, load5.value, load15.value, chartTheme.value, {
    load1: t('app.systemMonitor.charts.series.load1'),
    load5: t('app.systemMonitor.charts.series.load5'),
    load15: t('app.systemMonitor.charts.series.load15'),
  }),
)
const cpuChartOptions = computed(() =>
  buildCpuChartOptions(
    labels.value,
    cpu.value,
    chartTheme.value,
    t('app.systemMonitor.charts.series.cpu'),
  ),
)
const memoryChartOptions = computed(() =>
  buildMemoryChartOptions(
    labels.value,
    memory.value,
    chartTheme.value,
    t('app.systemMonitor.charts.series.memory'),
  ),
)
const diskIoChartOptions = computed(() =>
  buildDiskIoChartOptions(
    labels.value,
    diskRead.value,
    diskWrite.value,
    diskScale.value,
    chartTheme.value,
    {
      read: t('app.systemMonitor.charts.series.read'),
      write: t('app.systemMonitor.charts.series.write'),
    },
  ),
)
const networkChartOptions = computed(() =>
  buildNetworkChartOptions(
    labels.value,
    netRx.value,
    netTx.value,
    netScale.value,
    chartTheme.value,
    {
      rx: t('app.systemMonitor.charts.series.rx'),
      tx: t('app.systemMonitor.charts.series.tx'),
    },
  ),
)

watch(
  busy,
  (isBusy) => {
    if (!props.windowId) return
    windowStore.updateWindowRuntimeState(props.windowId, {
      busy: isBusy,
      allowsNodeSwitch: true,
      blockLevel: 'open',
      blockReason: t('app.systemMonitor.guardOpen'),
    })
  },
  { immediate: true },
)
</script>

<template>
  <div class="system-monitor" data-page="system-monitoring" data-ui="monitoring-workspace">
    <div class="toolbar" data-ui="toolbar" data-slot="toolbar">
      <div class="status-cluster" data-slot="status">
        <VustTag :type="qualityTag.type" effect="light">{{ qualityTag.label }}</VustTag>
        <span v-if="overview?.observedAt" class="observed-at">
          {{
            t('app.systemMonitor.observedAt', {
              time: new Date(overview.observedAt).toLocaleString(),
            })
          }}
        </span>
      </div>
      <div class="actions" data-slot="actions">
        <VustSelect
          id="system-monitoring-range"
          v-model="timeRange"
          class="range-select"
          name="systemMonitoringRange"
          :aria-label="t('app.systemMonitor.timeRange')"
          :options="rangeOptions"
        />
        <VustButton
          type="secondary"
          :loading="manualRefreshing"
          data-slot="refresh"
          @click="refreshAll"
        >
          {{ t('app.systemMonitor.refresh') }}
        </VustButton>
      </div>
    </div>

    <VustAlert
      v-for="warning in warnings"
      :key="`warning-${warning}`"
      type="warning"
      :title="warning"
      show-icon
      data-ui="refresh-warning"
    />
    <VustAlert
      v-for="failure in failures"
      :key="`failure-${failure}`"
      type="error"
      :title="failure"
      show-icon
      data-ui="load-error"
    />
    <VustAlert
      v-if="overview?.snapshotStatus === 'partial'"
      type="warning"
      :title="
        t('app.systemMonitor.partialCoverage', { coverage: overview.coveragePercent.toFixed(0) })
      "
      show-icon
      data-ui="partial-status"
    />

    <div class="content" data-slot="content">
      <div
        v-if="awaitingSeriesWithoutChart"
        class="series-loading-placeholder"
        aria-busy="true"
        data-ui="history-loading"
      >
        <VustLoading
          v-if="showSeriesLoading"
          :loading="true"
          :text="t('app.systemMonitor.loading')"
        />
      </div>
      <VustEmpty
        v-else-if="seriesMatchesRange && !hasSeriesData"
        :description="emptyDescription"
        data-ui="history-empty"
      />
      <div v-else-if="hasSeriesData" class="chart-grid" data-ui="chart-grid">
        <MonitorChartPanel
          :title="t('app.systemMonitor.charts.load')"
          :options="loadChartOptions"
          full-width
          data-slot="load-chart"
        />
        <MonitorChartPanel
          :title="t('app.systemMonitor.charts.cpu')"
          :options="cpuChartOptions"
          data-slot="cpu-chart"
        />
        <MonitorChartPanel
          :title="t('app.systemMonitor.charts.memory')"
          :options="memoryChartOptions"
          data-slot="memory-chart"
        />
        <MonitorChartPanel
          :title="t('app.systemMonitor.charts.diskIo')"
          :options="diskIoChartOptions"
          data-slot="disk-chart"
        />
        <MonitorChartPanel
          :title="t('app.systemMonitor.charts.network')"
          :options="networkChartOptions"
          data-slot="network-chart"
        />
      </div>
    </div>
  </div>
</template>

<style scoped>
.system-monitor {
  height: 100%;
  min-height: 0;
  padding: var(--vdl-space-3);
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
  overflow: hidden;
  background: var(--vdl-bg-canvas);
}

.toolbar,
.actions,
.status-cluster {
  display: flex;
  align-items: center;
}

.toolbar {
  flex-shrink: 0;
  justify-content: space-between;
  gap: var(--vdl-space-3);
  flex-wrap: wrap;
}

.actions,
.status-cluster {
  gap: var(--vdl-space-2);
}

.range-select {
  width: 130px;
  flex: 0 0 130px;
}

.observed-at {
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-caption);
}

.content {
  position: relative;
  flex: 1;
  min-height: 0;
  display: flex;
  overflow: hidden;
}

.series-loading-placeholder {
  flex: 1;
  min-height: 0;
  display: flex;
}

.series-loading-placeholder > :deep(*) {
  width: 100%;
}

.chart-grid {
  flex: 1;
  min-height: 0;
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--vdl-space-3);
  overflow-y: auto;
  padding-right: var(--vdl-space-1);
}

@media (max-width: 1024px) {
  .chart-grid {
    grid-template-columns: 1fr;
  }
}

@media (max-width: 640px) {
  .toolbar,
  .actions {
    align-items: stretch;
  }

  .toolbar {
    flex-direction: column;
  }

  .actions {
    width: 100%;
  }

  .range-select {
    width: auto;
    min-width: 130px;
    flex: 1 1 130px;
  }
}
</style>
