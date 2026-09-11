import { describe, expect, it } from 'vitest'
import { formatDockerImageBytes, formatPercent } from '@/utils/docker-format'

describe('formatDockerImageBytes', () => {
  it('按 Docker CLI 的十进制单位和三位有效数字格式化', () => {
    expect(formatDockerImageBytes(999)).toBe('999B')
    expect(formatDockerImageBytes(1_000)).toBe('1kB')
    expect(formatDockerImageBytes(1_000_000)).toBe('1MB')
    expect(formatDockerImageBytes(3_183_928_878)).toBe('3.18GB')
    expect(formatDockerImageBytes(1_000_000_000_000)).toBe('1TB')
  })

  it('对缺失、非法和非正字节数使用零值回退', () => {
    expect(formatDockerImageBytes()).toBe('0B')
    expect(formatDockerImageBytes(Number.NaN)).toBe('0B')
    expect(formatDockerImageBytes(Number.POSITIVE_INFINITY)).toBe('0B')
    expect(formatDockerImageBytes(-1)).toBe('0B')
    expect(formatDockerImageBytes(0)).toBe('0B')
  })
})

describe('formatPercent', () => {
  it('formats zero, ordinary, and multi-core percentages', () => {
    expect(formatPercent(0)).toBe('0.0%')
    expect(formatPercent(12.34)).toBe('12.3%')
    expect(formatPercent(125.45)).toBe('125.5%')
  })

  it('distinguishes low positive usage from zero', () => {
    expect(formatPercent(0.0017)).toBe('<0.1%')
    expect(formatPercent(0.099)).toBe('<0.1%')
    expect(formatPercent(0.1)).toBe('0.1%')
  })

  it('uses the unavailable fallback for missing or invalid values', () => {
    expect(formatPercent()).toBe('0.0%')
    expect(formatPercent(Number.NaN)).toBe('0.0%')
  })
})
