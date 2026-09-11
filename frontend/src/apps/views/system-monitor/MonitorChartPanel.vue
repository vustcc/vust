<script setup lang="ts">
/**
 * @file MonitorChartPanel.vue
 * @description 通用监控图表面板组件，封装稳定尺寸的 ECharts 实例管理。
 */

import { ref, watch } from 'vue'
import { useEChartsInstance } from '@/composables/useEChartsInstance'

const props = defineProps<{
  /** 图表标题 */
  title: string
  /** ECharts 配置对象 */
  options: Record<string, unknown>
  /** 是否占满整行 */
  fullWidth?: boolean
}>()

const chartRef = ref<HTMLElement | null>(null)
const { instance, setOption } = useEChartsInstance(chartRef)

/** 应用配置到图表实例。 */
const applyOptions = () => {
  if (instance.value && props.options) {
    setOption(props.options)
  }
}

watch(
  () => props.options,
  () => applyOptions(),
  { deep: false },
)

watch(instance, (chart) => {
  if (chart) applyOptions()
})

defineExpose({ instance })
</script>

<template>
  <div class="monitor-chart-panel" :class="{ 'is-full-width': fullWidth }" data-ui="chart-panel">
    <h3 class="chart-title" data-slot="chart-title">{{ title }}</h3>
    <div ref="chartRef" class="chart-body" data-slot="chart-canvas"></div>
  </div>
</template>

<style scoped>
.monitor-chart-panel {
  display: flex;
  flex-direction: column;
  min-height: 280px;
  padding: var(--vdl-space-3);
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-card);
}

.monitor-chart-panel.is-full-width {
  grid-column: 1 / -1;
}

.chart-title {
  margin: 0 0 var(--vdl-space-2);
  font-size: var(--vdl-font-body-sm);
  color: var(--vdl-text-secondary);
  font-weight: 600;
}

.chart-body {
  width: 100%;
  flex: 1;
  min-height: 220px;
}

@media (max-width: 1024px) {
  .monitor-chart-panel.is-full-width {
    grid-column: auto;
  }
}
</style>
