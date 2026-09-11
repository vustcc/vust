import { dockerApi } from '@/api/modules/docker'
import { computed, ref, watch, type ComputedRef, type Ref } from 'vue'

type UseContainerProcessesOptions = {
  selectedContainerId: Ref<string | null>
  nodeId: Ref<string> | ComputedRef<string>
  activeTab: Ref<'basic' | 'processes' | 'logs' | 'terminal'>
  t: (key: string) => string
}

type ProcessSort = { index: number; direction: 'asc' | 'desc' } | null

export const useContainerProcesses = ({
  selectedContainerId,
  nodeId,
  activeTab,
  t,
}: UseContainerProcessesOptions) => {
  const processLoading = ref(false)
  const processError = ref<string | null>(null)
  const processColumns = ref<string[]>([])
  const processRows = ref<string[][]>([])
  const processSort = ref<ProcessSort>(null)
  const dockerClient = computed(() => dockerApi.forNode(nodeId.value))

  let requestVersion = 0

  const reset = () => {
    requestVersion += 1
    processLoading.value = false
    processError.value = null
    processColumns.value = []
    processRows.value = []
    processSort.value = null
  }

  const loadProcessList = async () => {
    const id = selectedContainerId.value
    if (!id) return

    const currentRequest = ++requestVersion
    processLoading.value = true
    processError.value = null

    try {
      const res = await dockerClient.value.fetchContainerTop(id)

      if (currentRequest !== requestVersion) return
      if (selectedContainerId.value !== id) return

      if (res.success && res.data) {
        const payload = res.data as {
          titles?: string[]
          processes?: string[][]
          Titles?: string[]
          Processes?: string[][]
        }
        const titles = payload.titles || payload.Titles || []
        const processes = payload.processes || payload.Processes || []
        const normalized = titles.map((title) => title.trim().toLowerCase())
        const findIndex = (candidates: string[]) =>
          normalized.findIndex((title) => candidates.includes(title))

        const pidIndex = findIndex(['pid'])
        const cpuIndex = findIndex(['%cpu', 'cpu', 'pcpu'])
        const memIndex = findIndex(['%mem', 'mem', 'pmem'])
        const cmdIndex = findIndex(['command', 'cmd'])
        const hasRequired = [pidIndex, cpuIndex, memIndex, cmdIndex].every((idx) => idx >= 0)

        processColumns.value = [
          t('app.docker.containers.pid'),
          t('app.docker.containers.cpuUsage'),
          t('app.docker.containers.memUsage'),
          t('app.docker.containers.command'),
        ]

        processRows.value = hasRequired
          ? processes.map((row) => [
              row[pidIndex] || '',
              row[cpuIndex] || '',
              row[memIndex] || '',
              row[cmdIndex] || '',
            ])
          : processes.map((row) => row.slice(0, 4))

        return
      }

      processError.value = res.message || t('app.docker.containers.processLoadFailed')
      processRows.value = []
    } finally {
      if (currentRequest === requestVersion) {
        processLoading.value = false
      }
    }
  }

  watch([selectedContainerId, nodeId], () => {
    reset()
    if (activeTab.value === 'processes' && selectedContainerId.value) {
      void loadProcessList()
    }
  })

  watch(activeTab, (tab) => {
    if (tab !== 'processes') return
    if (!selectedContainerId.value) return
    if (!processColumns.value.length && !processLoading.value) {
      void loadProcessList()
    }
  })

  const sortedProcessRows = computed(() => {
    if (!processSort.value) return processRows.value
    const { index, direction } = processSort.value
    const sorted = [...processRows.value].sort((a, b) => {
      const left = a[index] || ''
      const right = b[index] || ''
      const leftNum = Number(left)
      const rightNum = Number(right)
      if (!Number.isNaN(leftNum) && !Number.isNaN(rightNum)) {
        return leftNum - rightNum
      }
      return left.localeCompare(right)
    })
    if (direction === 'desc') sorted.reverse()
    return sorted
  })

  const toggleProcessSort = (index: number) => {
    if (!processSort.value || processSort.value.index !== index) {
      processSort.value = { index, direction: 'asc' }
      return
    }
    processSort.value = processSort.value.direction === 'asc' ? { index, direction: 'desc' } : null
  }

  const getProcessColumnWidth = (index: number): number | undefined => {
    if (index === 0) return 80
    if (index === 1 || index === 2) return 110
    return undefined
  }

  const getProcessColumnLabel = (index: number, fallback: string): string => {
    if (index === 0) return t('app.docker.containers.pid')
    if (index === 1) return t('app.docker.containers.cpuUsage')
    if (index === 2) return t('app.docker.containers.memUsage')
    if (index === 3) return t('app.docker.containers.command')
    return fallback
  }

  return {
    processLoading,
    processError,
    processColumns,
    processSort,
    sortedProcessRows,
    loadProcessList,
    toggleProcessSort,
    getProcessColumnWidth,
    getProcessColumnLabel,
    reset,
  }
}
