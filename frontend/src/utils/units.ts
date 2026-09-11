export const throughputUnits = ['B/s', 'KB/s', 'MB/s', 'GB/s'] as const

export const formatBytes = (bytes?: number) => {
  if (!bytes || bytes <= 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  const idx = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1)
  const value = bytes / Math.pow(1024, idx)
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${units[idx]}`
}

export const resolveThroughputUnit = (values: number[]) => {
  const maxValue = values.reduce((max, value) => Math.max(max, value), 0)
  const index = Math.min(
    Math.max(0, Math.floor(Math.log(Math.max(maxValue, 1)) / Math.log(1024))),
    throughputUnits.length - 1,
  )
  const base = Math.pow(1024, index)
  const unit = throughputUnits[index] ?? 'B/s'
  return { base, unit }
}

export const formatScaledThroughput = (value: number, base: number) => {
  const scaled = value / base
  if (scaled >= 100) return scaled.toFixed(0)
  if (scaled >= 10) return scaled.toFixed(1)
  return scaled.toFixed(2)
}
