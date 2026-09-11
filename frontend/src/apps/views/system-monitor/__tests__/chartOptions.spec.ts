import { describe, expect, it } from 'vitest'
import { buildCpuChartOptions, buildLoadChartOptions } from '../chartOptions'

const theme = {
  titleColor: '#111',
  textColor: '#222',
  gridLineColor: '#333',
  tooltipBackground: '#fff',
  tooltipBorder: '#ddd',
  primary: '#00f',
  success: '#0f0',
  warning: '#f90',
}

describe('system monitoring chart options', () => {
  it('平均负载显示最多两位小数且保留原始采样精度', () => {
    const options = buildLoadChartOptions(['a'], [1.23456], [1.2], [1], theme, {
      load1: '1m',
      load5: '5m',
      load15: '15m',
    })
    const tooltip = options.tooltip as { valueFormatter: (value: unknown) => string }
    const yAxis = options.yAxis as {
      axisLabel: { formatter: (value: unknown) => string }
    }
    const series = options.series as Array<{ data: Array<number | null> }>

    expect(tooltip.valueFormatter(1.23456)).toBe('1.23')
    expect(tooltip.valueFormatter(1.2)).toBe('1.2')
    expect(yAxis.axisLabel.formatter(1.236)).toBe('1.24')
    expect(series[0]?.data).toEqual([1.23456])
  })

  it('只保留滑块缩放，不注册会阻塞页面滚动的 inside dataZoom', () => {
    const options = buildCpuChartOptions(['a', 'b'], [12, 18], theme, 'CPU')
    const dataZoom = options.dataZoom as Array<Record<string, unknown>>

    expect(dataZoom).toHaveLength(1)
    expect(dataZoom[0]?.type).toBe('slider')
    expect(dataZoom.some((item) => item.type === 'inside')).toBe(false)
  })

  it('保留缺失点并禁止连接不存在的趋势', () => {
    const options = buildCpuChartOptions(['a', 'b', 'c'], [12, null, 18], theme, 'CPU')
    const series = (options.series as Array<Record<string, unknown>>)[0]

    expect(series?.data).toEqual([12, null, 18])
    expect(series?.connectNulls).toBe(false)
  })
})
