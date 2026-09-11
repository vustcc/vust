import type * as dockerType from '@/api/interface/docker'
import { formatPercent } from '@/utils/docker-format'
import { nextTick, ref } from 'vue'
import * as echarts from 'echarts/core'
import { LineChart } from 'echarts/charts'
import {
  GridComponent,
  LegendComponent,
  TitleComponent,
  TooltipComponent,
} from 'echarts/components'
import { CanvasRenderer } from 'echarts/renderers'

echarts.use([
  LineChart,
  GridComponent,
  LegendComponent,
  TitleComponent,
  TooltipComponent,
  CanvasRenderer,
])

type UseContainerHistoryChartsOptions = {
  t: (key: string) => string
}

export const useContainerHistoryCharts = ({ t }: UseContainerHistoryChartsOptions) => {
  const historyCpuChartRef = ref<HTMLDivElement | null>(null)
  const historyMemoryChartRef = ref<HTMLDivElement | null>(null)
  const historyNetworkChartRef = ref<HTMLDivElement | null>(null)

  let cpuHistoryChart: echarts.ECharts | null = null
  let memoryHistoryChart: echarts.ECharts | null = null
  let networkHistoryChart: echarts.ECharts | null = null
  let historyResizeObserver: ResizeObserver | null = null

  const formatPercentAxis = (value: number) => {
    const normalized = Math.max(value, 0)
    if (normalized === 0) return '0%'
    if (normalized < 1) return `${Number(normalized.toPrecision(2))}%`
    return `${normalized.toFixed(0)}%`
  }

  const formatBytes = (bytes?: number) => {
    if (!bytes || bytes <= 0) return '0 B'
    const units = ['B', 'KB', 'MB', 'GB', 'TB']
    const base = 1024
    const index = Math.max(
      0,
      Math.min(Math.floor(Math.log(bytes) / Math.log(base)), units.length - 1),
    )
    const value = bytes / Math.pow(base, index)
    return `${value.toFixed(1)} ${units[index]}`
  }

  const formatBytesPerSecond = (bytesPerSecond: number) => {
    if (!bytesPerSecond || bytesPerSecond <= 0) return '0 B/s'
    const units = ['B/s', 'KB/s', 'MB/s', 'GB/s', 'TB/s']
    const base = 1024
    const index = Math.max(
      0,
      Math.min(Math.floor(Math.log(bytesPerSecond) / Math.log(base)), units.length - 1),
    )
    const value = bytesPerSecond / Math.pow(base, index)
    return `${value.toFixed(1)} ${units[index]}`
  }

  const formatTimeLabel = (timestamp: number) => {
    const date = new Date(timestamp * 1000)
    if (Number.isNaN(date.getTime())) return '-'
    return date.toLocaleString(undefined, {
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
    })
  }

  const ensureChartInstance = (
    chartRef: HTMLDivElement | null,
    chart: echarts.ECharts | null,
  ): echarts.ECharts | null => {
    if (!chartRef) return chart
    if (!chart) return echarts.init(chartRef)
    if (chart.getDom() !== chartRef) {
      chart.dispose()
      return echarts.init(chartRef)
    }
    return chart
  }

  const buildPercentLineOption = (
    title: string,
    labels: string[],
    values: number[],
    color: string,
  ) => ({
    title: {
      text: title,
      left: 12,
      top: 8,
      textStyle: { fontSize: 12, color: '#495057', fontWeight: 600 },
    },
    tooltip: {
      trigger: 'axis',
      formatter: (params: Array<{ data: number; dataIndex: number }>) => {
        const first = params[0]
        if (!first) return ''
        const label = labels[first.dataIndex] || ''
        return `${label}<br/>${formatPercent(first.data)}`
      },
    },
    grid: { left: 36, right: 16, bottom: 28, top: 36 },
    xAxis: {
      type: 'category',
      data: labels,
      axisTick: { show: false },
      axisLabel: { fontSize: 10, color: '#6c757d' },
    },
    yAxis: {
      type: 'value',
      min: 0,
      axisLabel: {
        formatter: (val: number) => formatPercentAxis(val),
        fontSize: 10,
        color: '#6c757d',
      },
      splitLine: { lineStyle: { color: '#f1f3f5' } },
    },
    series: [
      {
        type: 'line',
        data: values,
        smooth: false,
        connectNulls: false,
        showSymbol: false,
        lineStyle: { color },
        areaStyle: { color: `${color}22` },
      },
    ],
  })

  const buildMemoryLineOption = (
    title: string,
    labels: string[],
    percents: number[],
    usages: number[],
    limits: number[],
  ) => ({
    title: {
      text: title,
      left: 12,
      top: 8,
      textStyle: { fontSize: 12, color: '#495057', fontWeight: 600 },
    },
    tooltip: {
      trigger: 'axis',
      formatter: (params: Array<{ data: number; dataIndex: number }>) => {
        const first = params[0]
        if (!first) return ''
        const idx = first.dataIndex
        const label = labels[idx] || ''
        const usage = usages[idx] ?? 0
        const limit = limits[idx] ?? 0
        return [
          label,
          `${t('app.docker.containers.chartTooltip.usagePercent')}: ${formatPercent(first.data)}`,
          `${t('app.docker.containers.chartTooltip.usage')}: ${formatBytes(usage)}`,
          `${t('app.docker.containers.chartTooltip.limit')}: ${formatBytes(limit)}`,
        ].join('<br/>')
      },
    },
    grid: { left: 36, right: 16, bottom: 28, top: 36 },
    xAxis: {
      type: 'category',
      data: labels,
      axisTick: { show: false },
      axisLabel: { fontSize: 10, color: '#6c757d' },
    },
    yAxis: {
      type: 'value',
      max: 100,
      axisLabel: {
        formatter: (val: number) => `${val.toFixed(0)}%`,
        fontSize: 10,
        color: '#6c757d',
      },
      splitLine: { lineStyle: { color: '#f1f3f5' } },
    },
    series: [
      {
        type: 'line',
        data: percents,
        smooth: false,
        connectNulls: false,
        showSymbol: false,
        lineStyle: { color: '#2f9e44' },
        areaStyle: { color: 'rgba(47,158,68,0.12)' },
      },
    ],
  })

  const buildNetworkLineOption = (
    labels: string[],
    rxSeries: Array<number | null>,
    txSeries: Array<number | null>,
  ) => ({
    title: {
      text: t('app.docker.containers.networkTrend'),
      left: 12,
      top: 8,
      textStyle: { fontSize: 12, color: '#495057', fontWeight: 600 },
    },
    tooltip: {
      trigger: 'axis',
      formatter: (
        params: Array<{ seriesName: string; data: number | null; dataIndex: number }>,
      ) => {
        const first = params[0]
        if (!first) return ''
        const label = labels[first.dataIndex] || ''
        const lines = params.map(
          (item) =>
            `${item.seriesName}: ${item.data === null ? '-' : formatBytesPerSecond(item.data)}`,
        )
        return [label, ...lines].join('<br/>')
      },
    },
    legend: {
      top: 8,
      right: 12,
      textStyle: { fontSize: 10, color: '#6c757d' },
    },
    grid: { left: 36, right: 16, bottom: 28, top: 36 },
    xAxis: {
      type: 'category',
      data: labels,
      axisTick: { show: false },
      axisLabel: { fontSize: 10, color: '#6c757d' },
    },
    yAxis: {
      type: 'value',
      max: (value: { max: number }) => Math.max(value.max * 1.2, 1),
      axisLabel: {
        fontSize: 10,
        color: '#6c757d',
        formatter: (val: number) => formatBytesPerSecond(val),
      },
      splitLine: { lineStyle: { color: '#f1f3f5' } },
    },
    series: [
      {
        type: 'line',
        name: t('app.docker.containers.download'),
        data: rxSeries,
        smooth: false,
        connectNulls: false,
        showSymbol: false,
        lineStyle: { color: '#1d63ed' },
        areaStyle: { color: 'rgba(29,99,237,0.12)' },
      },
      {
        type: 'line',
        name: t('app.docker.containers.upload'),
        data: txSeries,
        smooth: false,
        connectNulls: false,
        showSymbol: false,
        lineStyle: { color: '#f39c12' },
        areaStyle: { color: 'rgba(243,156,18,0.12)' },
      },
    ],
  })

  const initCharts = async () => {
    await nextTick()

    cpuHistoryChart = ensureChartInstance(historyCpuChartRef.value, cpuHistoryChart)
    memoryHistoryChart = ensureChartInstance(historyMemoryChartRef.value, memoryHistoryChart)
    networkHistoryChart = ensureChartInstance(historyNetworkChartRef.value, networkHistoryChart)

    if (!historyResizeObserver) {
      historyResizeObserver = new ResizeObserver(() => {
        cpuHistoryChart?.resize()
        memoryHistoryChart?.resize()
        networkHistoryChart?.resize()
      })
    }

    historyResizeObserver.disconnect()
    if (historyCpuChartRef.value) historyResizeObserver.observe(historyCpuChartRef.value)
    if (historyMemoryChartRef.value) historyResizeObserver.observe(historyMemoryChartRef.value)
    if (historyNetworkChartRef.value) historyResizeObserver.observe(historyNetworkChartRef.value)
  }

  const clearHistory = () => {
    cpuHistoryChart?.clear()
    memoryHistoryChart?.clear()
    networkHistoryChart?.clear()
  }

  const renderHistory = async (history: dockerType.ContainerResourceUsageHistory | null) => {
    await initCharts()

    const points = history?.points || []
    if (!points.length) {
      clearHistory()
      return
    }

    const labels = points.map((point) => formatTimeLabel(point.timestamp))
    const cpuSeries = points.map((point) => point.cpuCorePercent)
    const memSeries = points.map((point) => point.memoryPercent)
    const memUsage = points.map((point) => point.memoryWorkingSetBytes)
    const memLimit = points.map((point) => point.memoryLimitBytes)

    const rxSeries = points.map((point) => point.networkRxBytesPerSecond)
    const txSeries = points.map((point) => point.networkTxBytesPerSecond)

    cpuHistoryChart?.setOption(
      buildPercentLineOption(t('app.docker.containers.cpuTrend'), labels, cpuSeries, '#1d63ed'),
    )
    memoryHistoryChart?.setOption(
      buildMemoryLineOption(
        t('app.docker.containers.memTrend'),
        labels,
        memSeries,
        memUsage,
        memLimit,
      ),
    )
    networkHistoryChart?.setOption(buildNetworkLineOption(labels, rxSeries, txSeries))
  }

  const dispose = () => {
    historyResizeObserver?.disconnect()
    cpuHistoryChart?.dispose()
    memoryHistoryChart?.dispose()
    networkHistoryChart?.dispose()
    cpuHistoryChart = null
    memoryHistoryChart = null
    networkHistoryChart = null
  }

  return {
    historyCpuChartRef,
    historyMemoryChartRef,
    historyNetworkChartRef,
    renderHistory,
    clearHistory,
    dispose,
  }
}
