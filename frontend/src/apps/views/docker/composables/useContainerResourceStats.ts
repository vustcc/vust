import type * as dockerType from '@/api/interface/docker'
import type { ComputedRef } from 'vue'

type ContainerStatsEntry = {
  data: dockerType.ContainerResourceUsageSummary
  fetchedAt: number
}

type UseContainerResourceStatsOptions = {
  containerResourceStats: ComputedRef<Record<string, ContainerStatsEntry> | undefined>
  onFetchContainerStats: (id: string) => void
  formatBytes: (bytes?: number) => string
}

export const useContainerResourceStats = ({
  containerResourceStats,
  onFetchContainerStats,
  formatBytes,
}: UseContainerResourceStatsOptions) => {
  const getContainerStats = (id: string | undefined) => {
    if (!id) return null
    return containerResourceStats.value?.[id]?.data || null
  }

  const requestContainerStats = (id: string | undefined, state?: string) => {
    if (!id) return
    if (state && state !== 'running') return
    onFetchContainerStats(id)
  }

  const handleContainerRowMouseEnter = (row: dockerType.DockerContainerSummary) => {
    requestContainerStats(row.id, row.state)
  }

  const formatNetworkUsage = (id: string | undefined) => {
    if (!id) return '-'
    const stats = getContainerStats(id)
    if (
      !stats ||
      stats.networkRxBytesPerSecond === null ||
      stats.networkTxBytesPerSecond === null
    ) {
      return '-'
    }
    return `↓ ${formatBytes(stats.networkRxBytesPerSecond)}/s / ↑ ${formatBytes(stats.networkTxBytesPerSecond)}/s`
  }

  return {
    getContainerStats,
    requestContainerStats,
    handleContainerRowMouseEnter,
    formatNetworkUsage,
  }
}
