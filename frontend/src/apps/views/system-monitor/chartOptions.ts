/**
 * @file chartOptions.ts
 * @description 系统监控图表配置构建器（纯函数模块）。
 *
 * 为五类图表（负载、CPU、内存、磁盘 IO、网络吞吐）生成完整 ECharts 配置，
 * 包含仅通过底部滑块操作的 dataZoom、缺失点断线和 crosshair 联动。
 */

import { formatScaledThroughput } from '@/utils/units'

/** 图表主题色集合。 */
interface ChartTheme {
  titleColor: string
  textColor: string
  gridLineColor: string
  tooltipBackground: string
  tooltipBorder: string
  primary: string
  success: string
  warning: string
}

/** 吞吐量缩放信息。 */
interface ThroughputScale {
  base: number
  unit: string
}

/** 通用 dataZoom 配置。 */
const buildDataZoom = () => [
  {
    type: 'slider',
    start: 0,
    end: 100,
    show: true,
    height: 20,
    bottom: 0,
    borderColor: 'transparent',
    backgroundColor: 'rgba(148, 163, 184, 0.06)',
    fillerColor: 'rgba(0, 200, 255, 0.12)',
    handleStyle: { color: 'currentColor' },
  },
]

/** 通用 tooltip 配置。 */
const buildTooltip = (theme: ChartTheme, valueFormatter?: (value: unknown) => string) => ({
  trigger: 'axis' as const,
  backgroundColor: theme.tooltipBackground,
  borderColor: theme.tooltipBorder,
  textStyle: { color: theme.titleColor, fontSize: 12 },
  axisPointer: {
    type: 'cross' as const,
    crossStyle: { color: theme.primary },
    lineStyle: { color: theme.primary },
    label: { backgroundColor: theme.primary },
  },
  ...(valueFormatter ? { valueFormatter } : {}),
})

/** 通用 legend 配置。 */
const buildLegend = (theme: ChartTheme) => ({
  top: 4,
  textStyle: { color: theme.textColor, fontSize: 12 },
  itemWidth: 14,
  itemHeight: 8,
  itemGap: 16,
})

/** 通用 X 轴配置。 */
const buildXAxis = (theme: ChartTheme, data: string[]) => ({
  type: 'category' as const,
  data,
  axisLabel: { color: theme.textColor, fontSize: 11 },
  axisLine: { lineStyle: { color: theme.gridLineColor } },
  axisTick: { lineStyle: { color: theme.gridLineColor } },
})

/** 通用 Y 轴配置。 */
const buildYAxis = (theme: ChartTheme) => ({
  type: 'value' as const,
  axisLabel: { color: theme.textColor, fontSize: 11 },
  splitLine: { lineStyle: { color: theme.gridLineColor } },
})

/** 将负载值限制为最多两位小数，不修改原始采样。 */
const formatLoadValue = (value: unknown) => {
  const numericValue = Number(value)
  if (!Number.isFinite(numericValue)) return ''
  return numericValue.toFixed(2).replace(/\.?0+$/, '')
}

// ===== 图表配置构建函数 =====

/**
 * 构建系统负载趋势图配置。
 */
export function buildLoadChartOptions(
  labels: string[],
  load1: Array<number | null>,
  load5: Array<number | null>,
  load15: Array<number | null>,
  theme: ChartTheme,
  seriesNames: { load1: string; load5: string; load15: string },
): Record<string, unknown> {
  return {
    color: [theme.primary, theme.success, theme.warning],
    tooltip: buildTooltip(theme, formatLoadValue),
    legend: buildLegend(theme),
    grid: { left: 44, right: 18, top: 36, bottom: 36 },
    xAxis: buildXAxis(theme, labels),
    yAxis: {
      ...buildYAxis(theme),
      axisLabel: {
        color: theme.textColor,
        fontSize: 11,
        formatter: formatLoadValue,
      },
    },
    dataZoom: buildDataZoom(),
    series: [
      {
        name: seriesNames.load1,
        type: 'line',
        smooth: true,
        data: load1,
        symbol: 'none',
        lineStyle: { width: 1.5 },
        connectNulls: false,
      },
      {
        name: seriesNames.load5,
        type: 'line',
        smooth: true,
        data: load5,
        symbol: 'none',
        lineStyle: { width: 1.5 },
        connectNulls: false,
      },
      {
        name: seriesNames.load15,
        type: 'line',
        smooth: true,
        data: load15,
        symbol: 'none',
        lineStyle: { width: 1.5 },
        connectNulls: false,
      },
    ],
  }
}

/**
 * 构建 CPU 使用率趋势图配置。
 */
export function buildCpuChartOptions(
  labels: string[],
  cpu: Array<number | null>,
  theme: ChartTheme,
  seriesName: string,
): Record<string, unknown> {
  return {
    color: [theme.primary],
    tooltip: buildTooltip(theme, (value) => `${Number(value ?? 0).toFixed(1)}%`),
    grid: { left: 44, right: 18, top: 16, bottom: 36 },
    xAxis: buildXAxis(theme, labels),
    yAxis: {
      ...buildYAxis(theme),
      min: 0,
      max: 100,
      axisLabel: {
        color: theme.textColor,
        fontSize: 11,
        formatter: '{value}%',
      },
    },
    dataZoom: buildDataZoom(),
    series: [
      {
        name: seriesName,
        type: 'line',
        smooth: true,
        data: cpu,
        symbol: 'none',
        lineStyle: { width: 1.5 },
        areaStyle: { opacity: 0.08 },
        connectNulls: false,
      },
    ],
  }
}

/**
 * 构建内存使用率趋势图配置。
 */
export function buildMemoryChartOptions(
  labels: string[],
  memory: Array<number | null>,
  theme: ChartTheme,
  seriesName: string,
): Record<string, unknown> {
  return {
    color: [theme.success],
    tooltip: buildTooltip(theme, (value) => `${Number(value ?? 0).toFixed(1)}%`),
    grid: { left: 44, right: 18, top: 16, bottom: 36 },
    xAxis: buildXAxis(theme, labels),
    yAxis: {
      ...buildYAxis(theme),
      min: 0,
      max: 100,
      axisLabel: {
        color: theme.textColor,
        fontSize: 11,
        formatter: '{value}%',
      },
    },
    dataZoom: buildDataZoom(),
    series: [
      {
        name: seriesName,
        type: 'line',
        smooth: true,
        data: memory,
        symbol: 'none',
        lineStyle: { width: 1.5 },
        areaStyle: { opacity: 0.08 },
        connectNulls: false,
      },
    ],
  }
}

/**
 * 构建磁盘 I/O 趋势图配置。
 */
export function buildDiskIoChartOptions(
  labels: string[],
  diskRead: Array<number | null>,
  diskWrite: Array<number | null>,
  scale: ThroughputScale,
  theme: ChartTheme,
  seriesNames: { read: string; write: string },
): Record<string, unknown> {
  const formatter = (value: unknown) =>
    `${formatScaledThroughput(Number(value ?? 0), scale.base)} ${scale.unit}`
  return {
    color: [theme.primary, theme.success],
    tooltip: buildTooltip(theme, formatter),
    legend: buildLegend(theme),
    grid: { left: 54, right: 18, top: 36, bottom: 36 },
    xAxis: buildXAxis(theme, labels),
    yAxis: {
      ...buildYAxis(theme),
      axisLabel: {
        color: theme.textColor,
        fontSize: 11,
        formatter: (value: number) => `${formatScaledThroughput(value, scale.base)} ${scale.unit}`,
      },
    },
    dataZoom: buildDataZoom(),
    series: [
      {
        name: seriesNames.read,
        type: 'line',
        smooth: true,
        data: diskRead,
        symbol: 'none',
        lineStyle: { width: 1.5 },
        connectNulls: false,
      },
      {
        name: seriesNames.write,
        type: 'line',
        smooth: true,
        data: diskWrite,
        symbol: 'none',
        lineStyle: { width: 1.5 },
        connectNulls: false,
      },
    ],
  }
}

/**
 * 构建网络吞吐趋势图配置。
 */
export function buildNetworkChartOptions(
  labels: string[],
  netRx: Array<number | null>,
  netTx: Array<number | null>,
  scale: ThroughputScale,
  theme: ChartTheme,
  seriesNames: { rx: string; tx: string },
): Record<string, unknown> {
  const formatter = (value: unknown) =>
    `${formatScaledThroughput(Number(value ?? 0), scale.base)} ${scale.unit}`
  return {
    color: [theme.primary, theme.warning],
    tooltip: buildTooltip(theme, formatter),
    legend: buildLegend(theme),
    grid: { left: 54, right: 18, top: 36, bottom: 36 },
    xAxis: buildXAxis(theme, labels),
    yAxis: {
      ...buildYAxis(theme),
      axisLabel: {
        color: theme.textColor,
        fontSize: 11,
        formatter: (value: number) => `${formatScaledThroughput(value, scale.base)} ${scale.unit}`,
      },
    },
    dataZoom: buildDataZoom(),
    series: [
      {
        name: seriesNames.rx,
        type: 'line',
        smooth: true,
        data: netRx,
        symbol: 'none',
        lineStyle: { width: 1.5 },
        connectNulls: false,
      },
      {
        name: seriesNames.tx,
        type: 'line',
        smooth: true,
        data: netTx,
        symbol: 'none',
        lineStyle: { width: 1.5 },
        connectNulls: false,
      },
    ],
  }
}
