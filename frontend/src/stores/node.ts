import { ref, watch } from 'vue'
import { defineStore } from 'pinia'
import { useI18n } from 'vue-i18n'
import { nodesApi, type NodeSummaryResponse } from '@/api/modules/nodes'

export type NodeStatus =
  | 'draft'
  | 'deploying'
  | 'deploy_failed'
  | 'awaiting_registration'
  | 'online'
  | 'degraded'
  | 'offline'
  | 'conflict'
  | 'retired'
  | 'unknown'

export interface NodeSummary {
  id: string
  name: string
  groupId?: string
  description?: string
  address: string
  servicePort?: string
  status: NodeStatus
  lifecycleStatus?: string
  runtimeStatus?: string
  healthStatus?: string
  tags: string[]
  metadata?: Record<string, unknown>
  lastSeenAt?: string
}

const NODE_ID_STORAGE_KEY = 'vust_node_id'
const NODE_REFRESH_INTERVAL_MS = 10_000

export const useNodeStore = defineStore('node', () => {
  const { t, locale } = useI18n()
  const createDefaultNodes = (): NodeSummary[] => [
    {
      id: 'local',
      name: t('app.nodes.master'),
      address: '127.0.0.1',
      status: 'online',
      tags: ['local'],
    },
  ]
  const nodes = ref<NodeSummary[]>(createDefaultNodes())
  const storedNodeId =
    typeof window !== 'undefined' ? localStorage.getItem(NODE_ID_STORAGE_KEY) : null
  const currentNodeId = ref(storedNodeId || nodes.value[0]?.id || '')
  const currentNodeUnavailable = ref(false)
  const isFetching = ref(false)
  const autoRefreshInterval = ref(NODE_REFRESH_INTERVAL_MS)
  let refreshTimer: ReturnType<typeof window.setInterval> | null = null

  const applyCurrentNode = (id: string) => {
    currentNodeId.value = id
    currentNodeUnavailable.value = !nodes.value.some((node) => node.id === id)
  }

  const setCurrentNode = (id: string) => {
    if (!nodes.value.some((node) => node.id === id)) return
    applyCurrentNode(id)
  }

  const requestSwitchCurrentNode = (id: string) => {
    if (!nodes.value.some((node) => node.id === id)) {
      return {
        switched: false,
        reason: t('app.nodes.targetNodeMissing'),
      }
    }
    applyCurrentNode(id)
    return {
      switched: true,
      reason: '',
    }
  }

  const setNodes = (list: NodeSummary[]) => {
    const mergedList = list.map((newNode) => {
      const oldNode = nodes.value.find((n) => n.id === newNode.id)
      if (
        oldNode &&
        oldNode.metadata?.resource &&
        (!newNode.metadata || !newNode.metadata.resource)
      ) {
        return {
          ...newNode,
          metadata: {
            ...newNode.metadata,
            resource: oldNode.metadata.resource,
          },
        }
      }
      return newNode
    })
    nodes.value = mergedList.length > 0 ? mergedList : createDefaultNodes()
    currentNodeUnavailable.value = !nodes.value.some((node) => node.id === currentNodeId.value)
  }

  const mapNodeSummary = (node: NodeSummaryResponse): NodeSummary => ({
    id: node.nodeId,
    name: node.nodeId === 'local' ? t('app.nodes.master') : node.name || node.nodeId,
    groupId: node.groupName || 'default',
    description: node.description,
    address: node.address || '',
    servicePort: node.servicePort,
    status: (node.status as NodeStatus) || 'unknown',
    lifecycleStatus: node.lifecycleStatus,
    runtimeStatus: node.runtimeStatus,
    healthStatus: node.healthStatus,
    tags: node.tags || [],
    metadata: node.metadata,
    lastSeenAt: node.lastSeenAt,
  })

  const refreshNodes = async () => {
    if (isFetching.value) return
    isFetching.value = true
    try {
      const response = await nodesApi.list()
      if (!response.success || !response.data) return
      const mapped = response.data.map(mapNodeSummary)
      setNodes(mapped)
    } finally {
      isFetching.value = false
    }
  }

  const startAutoRefresh = (intervalMs?: number) => {
    if (typeof window === 'undefined') return
    if (intervalMs !== undefined) {
      autoRefreshInterval.value = intervalMs
    }
    if (refreshTimer) {
      window.clearInterval(refreshTimer)
      refreshTimer = null
    }
    if (autoRefreshInterval.value <= 0) return

    void refreshNodes().catch(() => undefined)
    refreshTimer = window.setInterval(() => {
      void refreshNodes().catch(() => undefined)
    }, autoRefreshInterval.value)
  }

  const stopAutoRefresh = () => {
    if (!refreshTimer) return
    window.clearInterval(refreshTimer)
    refreshTimer = null
  }

  watch(locale, () => {
    if (nodes.value.length !== 1 || nodes.value[0]?.id !== 'local') return
    nodes.value = createDefaultNodes()
  })

  watch(
    currentNodeId,
    (value) => {
      if (typeof window === 'undefined') return
      localStorage.setItem(NODE_ID_STORAGE_KEY, value || '')
    },
    { immediate: true },
  )

  return {
    nodes,
    currentNodeId,
    currentNodeUnavailable,
    isFetching,
    setCurrentNode,
    requestSwitchCurrentNode,
    setNodes,
    refreshNodes,
    startAutoRefresh,
    stopAutoRefresh,
  }
})
