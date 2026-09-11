import { dockerApi } from '@/api/modules/docker'
import type * as dockerType from '@/api/interface/docker'
import { computed, ref, watch, type ComputedRef, type Ref } from 'vue'

export type HistoryStatus = 'idle' | 'loading' | 'ready' | 'error'

type UseContainerHistoryDataOptions = {
  selectedContainerId: Ref<string | null>
  nodeId: Ref<string> | ComputedRef<string>
  activeTab: Ref<'basic' | 'processes' | 'logs' | 'terminal'>
  t: (key: string) => string
}

export const useContainerHistoryData = ({
  selectedContainerId,
  nodeId,
  activeTab,
  t,
}: UseContainerHistoryDataOptions) => {
  const historyStatus = ref<HistoryStatus>('idle')
  const historyError = ref<string | null>(null)
  const containerHistory = ref<dockerType.ContainerResourceUsageHistory | null>(null)
  const dockerClient = computed(() => dockerApi.forNode(nodeId.value))

  let requestVersion = 0

  const historyLoading = computed(() => historyStatus.value === 'loading')

  const reset = () => {
    requestVersion += 1
    historyStatus.value = 'idle'
    historyError.value = null
    containerHistory.value = null
  }

  const fetchHistory = async (id: string, showLoading: boolean) => {
    const currentRequest = ++requestVersion
    if (showLoading) {
      historyStatus.value = 'loading'
    }
    historyError.value = null

    const res = await dockerClient.value.fetchContainerResourceUsageHistory(id, 1)

    if (currentRequest !== requestVersion) return
    if (selectedContainerId.value !== id) return

    if (res.success && res.data) {
      containerHistory.value = res.data.containers[0]
        ? { points: res.data.containers[0].points }
        : { points: [] }
      historyStatus.value = 'ready'
      return
    }

    containerHistory.value = null
    historyStatus.value = 'error'
    historyError.value = res.message || t('app.docker.containers.noTrendData')
  }

  watch(
    () => [selectedContainerId.value, nodeId.value] as const,
    ([id]) => {
      requestVersion += 1

      if (!id) {
        historyStatus.value = 'idle'
        historyError.value = null
        containerHistory.value = null
        return
      }

      containerHistory.value = null
      historyError.value = null

      if (activeTab.value !== 'basic') {
        historyStatus.value = 'idle'
        return
      }

      void fetchHistory(id, true)
    },
    { immediate: true },
  )

  watch(activeTab, (tab) => {
    const id = selectedContainerId.value
    if (!id || tab !== 'basic') return

    void fetchHistory(id, !containerHistory.value)
  })

  const dispose = () => {
    requestVersion += 1
  }

  return {
    containerHistory,
    historyStatus,
    historyLoading,
    historyError,
    reset,
    dispose,
  }
}
