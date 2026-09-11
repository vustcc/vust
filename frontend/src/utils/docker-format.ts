/**
 * Docker 格式化工具函数模块
 *
 * 统一定义所有 Docker 相关的格式化/转换函数，消除各组件中的重复定义。
 * 各子组件直接 import 使用，替代原有的 `utils` prop 传递模式。
 */

import type * as dockerType from '@/api/interface/docker'

// 从 units.ts 重导出 formatBytes，保持向后兼容
export { formatBytes } from '@/utils/units'

/**
 * 按 Docker CLI 的十进制单位和三位有效数字格式化镜像大小。
 * @param bytes Docker Engine 返回的镜像字节数
 */
export const formatDockerImageBytes = (bytes?: number): string => {
  if (bytes === undefined || !Number.isFinite(bytes) || bytes <= 0) return '0B'
  const units = ['B', 'kB', 'MB', 'GB', 'TB', 'PB']
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1000)), units.length - 1)
  const value = bytes / Math.pow(1000, index)
  return `${Number(value.toPrecision(3))}${units[index]}`
}

/**
 * 格式化百分比数值为带 % 后缀的字符串。
 * @param value 百分比数值；多核 CPU 指标允许超过 100
 */
export const formatPercent = (value?: number): string => {
  if (value === undefined || !Number.isFinite(value)) return '0.0%'
  const normalized = Math.max(value, 0)
  if (normalized > 0 && normalized < 0.1) return '<0.1%'
  return `${normalized.toFixed(1)}%`
}

/**
 * 格式化字节速率为可读字符串 (如 "1.2 MB/s")。
 * @param bytesPerSecond 每秒字节数
 */
export const formatBytesPerSecond = (bytesPerSecond: number): string => {
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

/**
 * 根据容器状态返回对应的 VDL 图标名称。
 * @param state 容器状态 (running / exited / paused 等)
 */
export const getStateIcon = (state: string | undefined): string => {
  switch (state) {
    case 'running':
      return 'status-running'
    case 'exited':
    case 'stopped':
      return 'status-stopped'
    case 'paused':
      return 'status-paused'
    case 'restarting':
      return 'status-restarting'
    case 'created':
      return 'status-created'
    default:
      return 'status-unknown'
  }
}

/**
 * 格式化容器端口映射为可读字符串。
 * @param ports 端口映射数组
 */
export const formatPorts = (ports: dockerType.Port[] | undefined): string => {
  if (!ports || ports.length === 0) return '-'
  return ports
    .map((p) => {
      const type = p.Type ? `/${p.Type}` : ''
      if (p.PublicPort) {
        const ip = p.IP || '0.0.0.0'
        return `${ip}:${p.PublicPort}->${p.PrivatePort}${type}`
      }
      return `${p.PrivatePort}${type}`
    })
    .join(', ')
}

/**
 * 获取容器的内部 IP 地址（取第一个可用的网络 IP）。
 * @param networkSettings 容器网络设置
 */
export const getContainerIP = (
  networkSettings: dockerType.ContainerSummaryNetworkSettings | null | undefined,
): string => {
  if (!networkSettings) return '-'
  const networks = networkSettings.Networks
  if (networks) {
    for (const key in networks) {
      if (networks[key]?.IPAddress) {
        return networks[key].IPAddress!
      }
    }
  }
  return '-'
}

/**
 * 格式化镜像标签数组为逗号分隔的字符串。
 * @param tags 镜像标签数组
 */
export const formatImageTags = (tags: string[] | undefined): string => {
  if (!tags || tags.length === 0) return '<none>:<none>'
  return tags.join(', ')
}

/**
 * 格式化 key-value 记录为逗号分隔的字符串。
 * @param input key-value 记录
 */
export const formatKeyValue = (input: Record<string, string> | null | undefined): string => {
  if (!input || Object.keys(input).length === 0) return '-'
  return Object.entries(input)
    .map(([k, v]) => `${k}: ${v}`)
    .join(', ')
}

/**
 * 解析用户输入的标签文本为 key=value 记录。
 * 每行一个 key=value 对，解析失败返回 null。
 * @param input 多行文本输入
 */
export const parseKeyValueLines = (input: string): Record<string, string> | null => {
  const values: Record<string, string> = {}
  const lines = input
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
  for (const line of lines) {
    const separator = line.indexOf('=')
    if (separator <= 0) return null
    const key = line.slice(0, separator).trim()
    const value = line.slice(separator + 1).trim()
    if (!key || Object.hasOwn(values, key)) return null
    values[key] = value
  }
  return values
}

/** 解析 Docker 标签文本，保留既有调用名称。 */
export const parseLabels = parseKeyValueLines

/**
 * 获取 CSS 自定义属性值（用于 ECharts 主题色同步）。
 * @param name CSS 变量名 (如 '--vdl-text-primary')
 */
export const getCssVar = (name: string): string =>
  getComputedStyle(document.documentElement).getPropertyValue(name).trim()
