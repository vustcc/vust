/**
 * 统一时间格式化工具函数
 * 将字符串 ISO 格式、时间戳毫秒数、时间戳秒数等，统一格式化为 "YYYY-MM-DD HH:mm:ss" 的补零本地时间
 */
export const formatDateTime = (value?: string | number | null): string => {
  if (value === undefined || value === null) return '--'

  let date: Date
  if (typeof value === 'number') {
    // 自动判定秒还是毫秒：若大于 50000000000 则是毫秒（2026年的秒级时间戳约为 1772000000）
    const isMs = value > 50000000000
    date = new Date(isMs ? value : value * 1000)
  } else {
    const trimmed = value.trim()
    if (!trimmed) return '--'
    date = new Date(trimmed)
  }

  if (isNaN(date.getTime())) {
    return '--'
  }

  const pad = (n: number) => n.toString().padStart(2, '0')

  const year = date.getFullYear()
  const month = pad(date.getMonth() + 1)
  const day = pad(date.getDate())
  const hours = pad(date.getHours())
  const minutes = pad(date.getMinutes())
  const seconds = pad(date.getSeconds())

  return `${year}-${month}-${day} ${hours}:${minutes}:${seconds}`
}
