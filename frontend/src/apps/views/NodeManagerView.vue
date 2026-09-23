/* 节点管理视图：展示节点列表，支持新增/编辑节点并进行预检与部署。 */
<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { upgradesApi, type UpgradeReleaseRecord } from '@/api/modules/upgrades'
import { DEFAULT_AGENT_PORT } from '@/utils/constants'
import {
  nodesApi,
  type NodeCheckResponse,
  type NodeDetailResponse,
  type NodePrecheckResponse,
} from '@/api/modules/nodes'
import { useNodeStore, type NodeStatus, type NodeSummary } from '@/stores/node'
import { useWindowManagerStore } from '@/stores/window-manager'
import VustTable, { type VustTableColumn } from '@/components/ui/VustTable.vue'
import {
  VustButton,
  VustCard,
  VustTag,
  VustInput,
  VustSelect,
  VustAlert,
  VustFormItem,
  VustDescriptions,
  VustActionMenu,
  VustCheckbox,
} from '@/components/ui'
import { useToastStore } from '@/stores/toast'
import { formatDateTime } from '@/utils/time'
import ApplicationDialog from '@/components/layout/ApplicationDialog.vue'
import ApplicationConfirmationDialog from '@/components/layout/ApplicationConfirmationDialog.vue'
import { useApplicationConfirmation } from '@/composables/useApplicationConfirmation'
import NodeProvisionWorkspace, {
  type NodeProvisionPhase,
} from './node-manager/NodeProvisionWorkspace.vue'

const props = defineProps<{
  isMaximized?: boolean
  windowId?: string
  payload?: Record<string, unknown>
}>()

const router = useRouter()
const { t } = useI18n()
const nodeStore = useNodeStore()
const windowStore = useWindowManagerStore()
const toastStore = useToastStore()
const { confirmationState, showConfirmation, handleConfirmationResponse } =
  useApplicationConfirmation()

const nodes = computed(() => nodeStore.nodes)
const currentNodeId = computed(() => nodeStore.currentNodeId)
const selectedNodeId = computed({
  get: () => nodeStore.currentNodeId,
  set: (value) => {
    if (value === null) return
    void switchCurrentNode(String(value))
  },
})
const activeView = ref<'list' | NodeProvisionPhase>('list')
const isEditActive = ref(false)
const editTarget = ref<NodeSummary | null>(null)
const precheckSubmitting = ref(false)
const editSubmitting = ref(false)
const editDetailLoading = ref(false)
const editError = ref('')
const isCheckDrawerVisible = ref(false)
const checkDetail = ref<NodeCheckResponse | null>(null)
const checkDetailTarget = ref<NodeSummary | null>(null)
const nodeDetailLoading = ref(false)
const selectedNodeDetail = ref<NodeDetailResponse | null>(null)
const precheckResult = ref<NodePrecheckResponse | null>(null)
const pendingDeployPayload = ref<Record<string, unknown> | null>(null)
const deployLogs = ref<string[]>([])
const deployError = ref('')
const deployTarget = ref<NodeSummary | null>(null)
const deployProgressPercent = ref(0)
const deployLaunching = ref(false)
const deployRunning = ref(false)
const deployFinished = ref(false)
let deployPollInterval: number | null = null
let deployOperationId: string | null = null
const hasDeploySession = computed(
  () => deployLaunching.value || deployRunning.value || deployFinished.value,
)

const buildNodeSwitchBlockMessage = () => {
  const guard = windowStore.checkBeforeNodeSwitch()
  if (guard.allowed) return ''
  const details = guard.blockers
    .slice(0, 5)
    .map((item) => `${item.title}: ${item.reason}`)
    .join('；')
  const restCount = Math.max(0, guard.blockers.length - 5)
  const restText = restCount > 0 ? t('app.nodes.switchBlockedRest', { count: restCount }) : ''
  return t('app.nodes.switchBlocked', { details, rest: restText })
}

const switchCurrentNode = async (nodeId: string) => {
  if (nodeId === nodeStore.currentNodeId) return
  const guardMessage = buildNodeSwitchBlockMessage()
  if (guardMessage) {
    toastStore.error(guardMessage)
    return
  }
  const result = nodeStore.requestSwitchCurrentNode(nodeId)
  if (!result.switched) {
    toastStore.error(result.reason || t('app.nodes.switchFailed'))
    return
  }
  const targetNode = nodeStore.nodes.find((node) => node.id === nodeId)
  toastStore.success(t('app.nodes.switchSuccess', { name: targetNode?.name || nodeId }))
}
const createFormDefaults = () => ({
  name: '',
  groupId: 'default',
  groupCustom: '',
  description: '',
  addr: '',
  port: '22',
  user: 'root',
  authMode: 'key',
  pwd: '',
  privateKey: '~/.ssh/id_ed25519',
  privateKeyPassphrase: '',
  tags: '',
  installDir: '/opt',
  servicePort: String(DEFAULT_AGENT_PORT),
  vustUrl: '',
})
const createForm = reactive(createFormDefaults())
const editForm = reactive({
  name: '',
  groupId: 'default',
  groupCustom: '',
  tags: '',
  description: '',
  addr: '',
  port: '',
  user: 'root',
  authMode: 'key',
  pwd: '',
  privateKey: '~/.ssh/id_ed25519',
  privateKeyPassphrase: '',
  servicePort: '',
  vustUrl: '',
})
let originalFormJson = ''
const checkingIds = ref<Set<string>>(new Set())
const deletingIds = ref<Set<string>>(new Set())
const refreshInterval = ref(5000)
const refreshOptions = computed(() => [
  { value: 0, label: t('app.nodes.refresh.off') },
  { value: 5000, label: t('app.nodes.refresh.seconds', { value: 5 }) },
  { value: 10000, label: t('app.nodes.refresh.seconds', { value: 10 }) },
  { value: 30000, label: t('app.nodes.refresh.seconds', { value: 30 }) },
])
const nodeDisplayName = (node: NodeSummary) => {
  if (node.id === 'local') return t('app.nodes.local')
  return node.name
}

const nodeOptions = computed(() =>
  nodes.value.map((node) => {
    const isAbnormal = node.status !== 'online'
    return {
      value: node.id,
      label: nodeDisplayName(node),
      disabled: isAbnormal,
    }
  }),
)

const statusLabel = (status: NodeStatus) => {
  switch (status) {
    case 'draft':
      return t('app.nodes.status.draft')
    case 'deploying':
      return t('app.nodes.status.deploying')
    case 'deploy_failed':
      return t('app.nodes.status.deployFailed')
    case 'awaiting_registration':
      return t('app.nodes.status.awaitingRegistration')
    case 'online':
      return t('app.nodes.status.online')
    case 'degraded':
      return t('app.nodes.status.degraded')
    case 'offline':
      return t('app.nodes.status.offline')
    case 'conflict':
      return t('app.nodes.status.conflict')
    case 'retired':
      return t('app.nodes.status.retired')
    default:
      return t('app.nodes.status.unknown')
  }
}

const statusTagType = (status: NodeStatus): 'success' | 'danger' | 'warning' | 'info' => {
  if (status === 'deploying' || status === 'awaiting_registration') return 'warning'
  if (status === 'online') return 'success'
  if (status === 'degraded') return 'warning'
  if (status === 'offline') return 'danger'
  return 'info'
}

const runtimeTagType = (status?: string): 'success' | 'warning' | 'info' => {
  const normalized = (status ?? '').toLowerCase()
  if (normalized === 'active') return 'success'
  if (!normalized) return 'info'
  return 'warning'
}

const healthTagType = (status?: string): 'success' | 'danger' | 'info' => {
  const normalized = (status ?? '').toLowerCase()
  if (normalized === 'online') return 'success'
  if (!normalized) return 'info'
  return 'danger'
}

const formatBytes = (bytes?: number) => {
  if (!bytes || bytes <= 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB']
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1)
  const value = bytes / Math.pow(1024, index)
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[index]}`
}

const formatPercent = (value?: number) => {
  if (value === undefined || Number.isNaN(value)) return '--'
  return `${value.toFixed(1)}%`
}

const resourceLabel = (node: NodeSummary) => {
  const resource = node.metadata?.resource as Record<string, unknown> | undefined
  if (!resource) return 'CPU -- · MEM --'
  const cpu = resource.cpuPercent as number | undefined
  const memUsed = resource.memoryUsedBytes as number | undefined
  const memTotal = resource.memoryTotalBytes as number | undefined
  return `CPU ${formatPercent(cpu)} · MEM ${formatBytes(memUsed)}/${formatBytes(memTotal)}`
}

const checkDetailItems = computed(() => {
  if (!checkDetail.value) return []
  return [
    { key: 'ssh', label: t('app.nodes.check.checks.ssh'), item: checkDetail.value.ssh },
    { key: 'service', label: t('app.nodes.check.checks.service'), item: checkDetail.value.service },
    { key: 'api', label: t('app.nodes.check.checks.api'), item: checkDetail.value.api },
  ]
})

const checkItemDetail = (item: NodeCheckResponse['ssh']) => {
  const base = item.message || ''
  if (item.code === 'service_missing') {
    const hint = t('app.nodes.check.messages.serviceMissingHint')
    return base ? `${base} · ${hint}` : hint
  }
  return base
}

const checkItemTagType = (status: string) => {
  if (status === 'passed') return 'success'
  if (status === 'warning') return 'warning'
  if (status === 'skipped') return 'info'
  return 'danger'
}

const checkItemStatusText = (status: string) => t(`app.nodes.precheck.status.${status}Short`)

const checkStatusLabel = computed(() => {
  const status = (checkDetail.value?.status ?? '').toLowerCase()
  if (status === 'online') return t('app.nodes.check.statusOnline')
  if (status === 'offline') return t('app.nodes.check.statusOffline')
  return t('app.nodes.check.statusUnknown')
})

/** 节点基础详情定义 */
const nodeBaseDetailItems = computed(() => [
  { label: t('app.nodes.detail.lifecycleStatus'), slot: 'lifecycle' },
  { label: t('app.nodes.detail.runtimeStatus'), slot: 'runtime' },
  { label: t('app.nodes.detail.healthStatus'), slot: 'health' },
  { label: t('app.nodes.detail.lastSeenAt'), slot: 'lastSeen' },
  { label: t('app.nodes.detail.group'), slot: 'group' },
])

/** 节点部署详情定义 */
const nodeDeployDetailItems = computed(() => {
  const base = [
    { label: t('app.nodes.detail.deployMethod'), slot: 'deployMethod' },
    { label: t('app.nodes.detail.deployStatus'), slot: 'deployStatus' },
    { label: t('app.nodes.detail.sshTarget'), slot: 'sshTarget' },
    { label: t('app.nodes.detail.installDir'), slot: 'installDir' },
    { label: t('app.nodes.detail.deployAt'), slot: 'deployAt' },
  ]
  if (selectedNodeDetail.value?.provisioning?.lastDeployErrorSummary) {
    base.push({ label: t('app.nodes.detail.deployError'), slot: 'deployError' })
  }
  return base
})

/** 节点会话详情定义 */
const nodeSessionDetailItems = computed(() => [
  { label: t('app.nodes.detail.sessionId'), slot: 'sessionId' },
  { label: t('app.nodes.detail.registeredAt'), slot: 'registeredAt' },
  { label: t('app.nodes.detail.lastHeartbeatAt'), slot: 'lastHeartbeatAt' },
  { label: t('app.nodes.detail.leaseExpiresAt'), slot: 'leaseExpiresAt' },
  { label: t('app.nodes.detail.agentVersion'), slot: 'agentVersion' },
])

/** 批量升级选中的节点 ID 列表 */
const selectedNodeIds = ref<string[]>([])

/** 是否展示精细化升级引导 */
const showUpgradeGuide = ref(!!props.payload?.highlightUpgrade)

watch(
  () => props.payload,
  (newPayload) => {
    if (newPayload?.highlightUpgrade) {
      showUpgradeGuide.value = true
    }
  },
  { deep: true },
)

/** 判断节点是否允许独立升级（排除 local 节点，且必须在线） */
const isNodeUpgradable = (node: NodeSummary) => {
  return node.id !== 'local' && node.status === 'online'
}

/** 是否全部在线可升级节点均已选中 */
const isAllSelected = computed(() => {
  const upgradableNodes = nodes.value.filter(isNodeUpgradable)
  if (upgradableNodes.length === 0) return false
  return upgradableNodes.every((n) => selectedNodeIds.value.includes(n.id))
})

/** 全选/全取消 */
const handleSelectAll = (val: boolean) => {
  if (val) {
    const upgradableNodes = nodes.value.filter(isNodeUpgradable)
    selectedNodeIds.value = upgradableNodes.map((n) => n.id)
  } else {
    selectedNodeIds.value = []
  }
}

/** 单选/取消单选 */
const handleSelectChange = (nodeId: string, val: boolean) => {
  if (val) {
    if (!selectedNodeIds.value.includes(nodeId)) {
      selectedNodeIds.value.push(nodeId)
    }
  } else {
    selectedNodeIds.value = selectedNodeIds.value.filter((id) => id !== nodeId)
  }
}

/** 本地节点表格列定义 */
const columns = computed<VustTableColumn[]>(() => [
  { label: '', slot: 'selection', headerSlot: 'selectionHeader', width: 50, align: 'center' },
  { label: t('app.nodes.columns.name'), minWidth: 180, slot: 'name', align: 'center' },
  { prop: 'address', label: t('app.nodes.columns.address'), minWidth: 180, align: 'center' },
  { label: t('app.nodes.columns.status'), minWidth: 120, slot: 'status', align: 'center' },
  { label: t('app.nodes.columns.resources'), minWidth: 240, slot: 'resource', align: 'center' },
  { label: t('app.nodes.columns.tags'), minWidth: 180, slot: 'tags', align: 'center' },
  {
    label: t('app.nodes.columns.actions'),
    width: 140,
    fixed: 'right',
    slot: 'actions',
    align: 'center',
  },
])

const closeCheckDrawer = () => {
  isCheckDrawerVisible.value = false
}

const selectedRemoteNodeId = computed(() => {
  const value = selectedNodeId.value
  if (!value || value === 'local') return null
  return value
})

const selectedNodeName = computed(() => {
  const nodeId = selectedRemoteNodeId.value
  if (!nodeId) return ''
  const found = nodeStore.nodes.find((n) => n.id === nodeId)
  return found ? nodeDisplayName(found) : nodeId
})

const loadingNodeId = ref<string | null>(null)

const loadSelectedNodeDetail = async (silent = false) => {
  const nodeId = selectedRemoteNodeId.value
  if (!nodeId) {
    selectedNodeDetail.value = null
    return
  }
  if (loadingNodeId.value === nodeId) return
  loadingNodeId.value = nodeId
  if (!silent) {
    nodeDetailLoading.value = true
  }
  try {
    const res = await nodesApi.detail(nodeId)
    if (selectedRemoteNodeId.value === nodeId) {
      selectedNodeDetail.value = res.success && res.data ? res.data : null
    }
  } catch {
    if (selectedRemoteNodeId.value === nodeId) {
      selectedNodeDetail.value = null
    }
  } finally {
    if (loadingNodeId.value === nodeId) {
      loadingNodeId.value = null
      nodeDetailLoading.value = false
    }
  }
}

const fetchNodes = async () => {
  try {
    await nodeStore.refreshNodes()
    await loadSelectedNodeDetail()
  } catch {
    return
  }
}

const isChecking = (id?: string) => {
  if (!id) return false
  return checkingIds.value.has(id)
}

const setChecking = (id: string, active: boolean) => {
  const next = new Set(checkingIds.value)
  if (active) {
    next.add(id)
  } else {
    next.delete(id)
  }
  checkingIds.value = next
}

const isDeleting = (id?: string) => {
  if (!id) return false
  return deletingIds.value.has(id)
}

const setDeleting = (id: string, active: boolean) => {
  const next = new Set(deletingIds.value)
  if (active) {
    next.add(id)
  } else {
    next.delete(id)
  }
  deletingIds.value = next
}

const checkNode = async (node: NodeSummary) => {
  if (!node.id || node.id === 'local') return
  if (isChecking(node.id)) return
  setChecking(node.id, true)
  try {
    const res = await nodesApi.check(node.id)
    if (!res.success) {
      toastStore.error(res.message || t('app.nodes.check.failed'))
      return
    }
    const payload = res.data as NodeCheckResponse | undefined
    if (!payload) {
      toastStore.error(t('app.nodes.check.failed'))
      return
    }
    checkDetail.value = payload
    checkDetailTarget.value = node
    isCheckDrawerVisible.value = true
    await fetchNodes()
  } catch {
    toastStore.error(t('app.nodes.check.failed'))
  } finally {
    setChecking(node.id, false)
  }
}

const startCreate = () => {
  if (hasDeploySession.value) return
  Object.assign(createForm, createFormDefaults())
  activeView.value = 'create'
  isEditActive.value = false
  editTarget.value = null
  precheckResult.value = null
  pendingDeployPayload.value = null
  deployError.value = ''
}

const cancelCreate = () => {
  activeView.value = 'list'
  Object.assign(createForm, createFormDefaults())
  precheckResult.value = null
  pendingDeployPayload.value = null
  deployError.value = ''
}

const returnToCreate = () => {
  precheckResult.value = null
  pendingDeployPayload.value = null
  activeView.value = 'create'
}

const validateCreateForm = () => {
  if (createForm.addr.trim() === '') {
    toastStore.error(t('app.nodes.create.addrRequired'))
    return false
  }
  if (createForm.user.trim() === '') {
    toastStore.error(t('app.nodes.create.userRequired'))
    return false
  }
  if (createForm.authMode === 'password' && createForm.pwd.trim() === '') {
    toastStore.error(t('app.nodes.create.passwordRequired'))
    return false
  }
  if (createForm.authMode === 'key' && createForm.privateKey.trim() === '') {
    toastStore.error(t('app.nodes.create.privateKeyRequired'))
    return false
  }
  if (createForm.vustUrl.trim() !== '') {
    try {
      const url = new URL(createForm.vustUrl.trim())
      if (url.protocol !== 'https:') {
        toastStore.error(t('app.nodes.create.vustUrlHttpsRequired'))
        return false
      }
      if (url.pathname !== '/' || url.search !== '' || url.hash !== '') {
        toastStore.error(t('app.nodes.create.vustUrlBaseRequired'))
        return false
      }
    } catch {
      toastStore.error(t('app.nodes.create.vustUrlInvalid'))
      return false
    }
  }
  return true
}

const submitCreate = async () => {
  if (precheckSubmitting.value) return
  precheckSubmitting.value = true
  precheckResult.value = null
  deployError.value = ''
  if (!validateCreateForm()) {
    precheckSubmitting.value = false
    return
  }
  const groupId = createForm.groupId === 'custom' ? createForm.groupCustom : createForm.groupId
  const payload = {
    name: createForm.name || undefined,
    groupId: groupId || undefined,
    description: createForm.description || undefined,
    addr: createForm.addr || undefined,
    port: createForm.port || undefined,
    user: createForm.user || undefined,
    authMode: createForm.authMode || undefined,
    pwd: createForm.pwd || undefined,
    privateKey: createForm.privateKey || undefined,
    privateKeyPassphrase: createForm.privateKeyPassphrase || undefined,
    tags: createForm.tags
      ? createForm.tags
          .split(',')
          .map((item) => item.trim())
          .filter(Boolean)
      : undefined,
    installDir: createForm.installDir || undefined,
    servicePort: createForm.servicePort || undefined,
    vustUrl: createForm.vustUrl.trim() || undefined,
  }
  const precheckPayload = {
    name: createForm.name || undefined,
    addr: createForm.addr || undefined,
    port: createForm.port || undefined,
    user: createForm.user || undefined,
    authMode: createForm.authMode || undefined,
    pwd: createForm.authMode === 'password' ? createForm.pwd || undefined : undefined,
    privateKey: createForm.authMode === 'key' ? createForm.privateKey || undefined : undefined,
    privateKeyPassphrase:
      createForm.authMode === 'key' ? createForm.privateKeyPassphrase || undefined : undefined,
    servicePort: createForm.servicePort || undefined,
    installDir: createForm.installDir || undefined,
    vustUrl: createForm.vustUrl.trim() || undefined,
  }
  try {
    const res = await nodesApi.precheck(precheckPayload)
    if (!res.success || !res.data) {
      const message = res.message || t('app.nodes.precheck.failed')
      toastStore.error(message)
      precheckSubmitting.value = false
      return
    }
    precheckResult.value = res.data
    pendingDeployPayload.value = res.data.passed ? payload : null
    activeView.value = 'precheck'
    deployLogs.value = []
    deployError.value = ''
    deployTarget.value = {
      id: 'pending',
      name: createForm.name || createForm.addr || '-',
      groupId: undefined,
      address: createForm.addr || '-',
      status: 'unknown',
      tags: [],
    }
    if (!res.data.passed) {
      precheckSubmitting.value = false
      return
    }
  } catch {
    toastStore.error(t('app.nodes.precheck.failed'))
    precheckSubmitting.value = false
    return
  }
  precheckSubmitting.value = false
}

const parseAddress = (address: string) => {
  const trimmed = address.trim()
  const match = trimmed.match(/^(.*):(\d+)$/)
  if (match) {
    return { addr: match[1] ?? '', port: match[2] ?? '' }
  }
  return { addr: trimmed, port: '' }
}

const startEdit = async (node: NodeSummary) => {
  if (!node.id || node.id === 'local') return
  activeView.value = 'list'
  isEditActive.value = true
  editTarget.value = node
  editError.value = ''
  const parsed = parseAddress(node.address)

  // 默认初始占位
  editForm.name = node.name
  if (node.groupId === 'default') {
    editForm.groupId = 'default'
    editForm.groupCustom = ''
  } else {
    editForm.groupId = 'custom'
    editForm.groupCustom = node.groupId || ''
  }
  editForm.tags = node.tags ? node.tags.join(', ') : ''
  editForm.description = node.description || ''
  editForm.addr = parsed.addr
  editForm.port = parsed.port || '22'
  editForm.user = 'root'
  editForm.authMode = 'key'
  editForm.pwd = ''
  editForm.privateKey = ''
  editForm.privateKeyPassphrase = ''
  editForm.servicePort = node.servicePort ?? ''
  editForm.vustUrl = ''

  editDetailLoading.value = true
  try {
    await Promise.all([
      (async () => {
        try {
          const res = await nodesApi.detail(node.id)
          if (editTarget.value?.id === node.id && res.success && res.data) {
            const prov = res.data.provisioning
            if (prov) {
              editForm.addr = prov.sshAddr || parsed.addr
              editForm.port = prov.sshPort ? String(prov.sshPort) : parsed.port || '22'
              editForm.user = prov.sshUser || 'root'
              editForm.authMode = prov.sshAuthMode || 'password'
              editForm.servicePort = prov.expectedListenPort
                ? String(prov.expectedListenPort)
                : (node.servicePort ?? '')
              if (prov.sshAuthMode === 'key') {
                editForm.privateKey = '~/.ssh/id_ed25519'
              }
            }
          }
        } catch (err) {
          console.error('Failed to load node detail for editing:', err)
        }
      })(),
      (async () => {
        try {
          const res = await nodesApi.about(node.id)
          if (editTarget.value?.id === node.id && res.success && res.data) {
            editForm.vustUrl = res.data.vustUrl || ''
          }
        } catch (err) {
          console.error('Failed to load remote node vustUrl:', err)
        }
      })(),
    ])
  } finally {
    editDetailLoading.value = false
    if (editTarget.value?.id === node.id) {
      // 在所有异步详情数据填充后，保存表单初始状态快照
      originalFormJson = JSON.stringify(editForm)
    }
  }
}

const cancelEdit = () => {
  editTarget.value = null
  editError.value = ''
  isEditActive.value = false
}

const submitEdit = async () => {
  if (!editTarget.value?.id || editTarget.value.id === 'local') return

  // 检查表单是否有改动。如果没有改动，直接关闭弹窗返回，不发送请求
  if (JSON.stringify(editForm) === originalFormJson) {
    cancelEdit()
    return
  }

  const originalForm = JSON.parse(originalFormJson)
  const isVustUrlChanged = editForm.vustUrl !== originalForm.vustUrl

  editSubmitting.value = true
  editError.value = ''
  const payload = {
    name: editForm.name || undefined,
    groupId: editForm.groupId === 'custom' ? editForm.groupCustom || 'default' : 'default',
    tags: editForm.tags
      ? editForm.tags
          .split(',')
          .map((t) => t.trim())
          .filter(Boolean)
      : [],
    description: editForm.description || '',
    addr: editForm.addr || undefined,
    port: editForm.port || undefined,
    user: editForm.user || undefined,
    authMode: editForm.authMode || undefined,
    pwd: editForm.authMode === 'password' ? editForm.pwd || undefined : undefined,
    privateKey: editForm.authMode === 'key' ? editForm.privateKey || undefined : undefined,
    privateKeyPassphrase:
      editForm.authMode === 'key' ? editForm.privateKeyPassphrase || undefined : undefined,
    servicePort: editForm.servicePort || undefined,
    vustUrl: isVustUrlChanged ? editForm.vustUrl || undefined : undefined,
  }
  try {
    const res = await nodesApi.update(editTarget.value.id, payload)
    if (!res.success) {
      const msg = res.message || t('app.nodes.edit.failed')
      toastStore.error(msg)
      editError.value = msg
      editSubmitting.value = false
      return
    }
  } catch {
    const msg = t('app.nodes.edit.failed')
    toastStore.error(msg)
    editError.value = msg
    editSubmitting.value = false
    return
  }
  editSubmitting.value = false
  cancelEdit()
  await fetchNodes()
}

const handleDeleteAction = async (node: NodeSummary) => {
  if (!node.id || node.id === 'local') return
  const useSimpleMessage = node.status === 'retired' || node.status === 'draft'
  const message = useSimpleMessage
    ? t('app.nodes.delete.confirmMessageSimple', { name: node.name })
    : t('app.nodes.delete.confirmMessage', { name: node.name })
  const confirmed = await showConfirmation(
    message,
    t('app.nodes.delete.confirmTitle'),
    t('app.nodes.delete.confirmAction'),
    t('confirmation.cancel'),
    'danger',
  )
  if (!confirmed) return
  if (isDeleting(node.id)) return
  setDeleting(node.id, true)
  try {
    const res = await nodesApi.remove(node.id)
    if (!res.success) {
      toastStore.error(res.message || t('app.nodes.delete.failed'))
      return
    }
    toastStore.success(t('app.nodes.delete.success'))
    await fetchNodes()
  } catch {
    toastStore.error(t('app.nodes.delete.failed'))
  } finally {
    setDeleting(node.id, false)
  }
}

/**
 * 根据节点的当前生命周期状态，动态计算其可用的操作菜单项。
 *
 * @param node 节点摘要信息
 * @returns 过滤并设置状态后的操作项数组
 */
const getNodeActions = (node: NodeSummary) => {
  const actions = []
  const status = node.status

  // 1. 检测 (Check) - 当节点非草稿、非部署中、且非退役时可用
  const canCheck = !['draft', 'deploying', 'retired'].includes(status)
  if (canCheck) {
    actions.push({
      label: isChecking(node.id) ? t('app.nodes.check.submitting') : t('app.nodes.check.action'),
      handler: () => checkNode(node),
      disabled: isChecking(node.id) || status === 'deploying',
    })
  }

  // 2. 编辑 (Edit) - 非部署中、且非退役状态下可用（已退役节点无法编辑，只能删除重新录入）
  const canEdit = !['deploying', 'retired'].includes(status)
  if (canEdit) {
    actions.push({
      label: t('app.nodes.actions.edit'),
      handler: () => startEdit(node),
    })
  }
  // 3. 修复/部署 (Repair) - 在非退役、非部署中状态下可用
  const canRepair = !['retired', 'deploying'].includes(status)
  if (canRepair) {
    actions.push({
      label: t('app.nodes.actions.repair'),
      handler: () => executeNodeAction(node, 'repair'),
    })
  }

  // 4. 退役 (Retire) - 在非草稿、非部署中、且非退役的状态下可用
  const canRetire = !['draft', 'deploying', 'retired'].includes(status)
  if (canRetire) {
    actions.push({
      label: t('app.nodes.actions.retire'),
      handler: () => executeNodeAction(node, 'retire'),
      class: 'btn-stop',
    })
  }

  // 5. 卸载 (Uninstall) - 在非草稿、非部署中、且非退役的状态下可用
  const canUninstall = !['draft', 'deploying', 'retired'].includes(status)
  if (canUninstall) {
    actions.push({
      label: t('app.nodes.actions.uninstall'),
      handler: () => executeNodeAction(node, 'uninstall'),
      class: 'btn-delete',
    })
  }

  // 6. 删除 (Delete) 只用于清理主控侧记录。在线类节点必须先卸载或退役，
  // 避免远端 agent 继续运行并重新注册，形成孤儿节点或会话冲突。
  const canDelete = [
    'draft',
    'deploy_failed',
    'offline',
    'unknown',
    'conflict',
    'retired',
  ].includes(status)
  if (canDelete) {
    actions.push({
      label: isDeleting(node.id) ? t('app.nodes.delete.submitting') : t('app.nodes.actions.delete'),
      handler: () => handleDeleteAction(node),
      disabled: isDeleting(node.id),
      class: 'btn-delete',
    })
  }

  return actions
}

const startPollingProgress = (nodeId: string) => {
  stopPollingProgress()
  deployPollInterval = window.setInterval(async () => {
    try {
      const res = await nodesApi.getDeployProgress(nodeId)
      if (res.success && res.data) {
        deployLogs.value = res.data.logs ?? []
        deployProgressPercent.value = res.data.progressPercent ?? 0
        if (res.data.isFinished) {
          stopPollingProgress()
          deployRunning.value = false
          deployFinished.value = true
          if (deployOperationId) {
            windowStore.finishGlobalOperation(deployOperationId)
            deployOperationId = null
          }
          if (res.data.error) {
            deployError.value = res.data.error
          } else {
            toastStore.success(
              t('app.nodes.deploy.success', { name: deployTarget.value?.name || '' }),
            )
          }
          void fetchNodes()
        }
      }
    } catch {
      // Ignore poll failures
    }
  }, 1500)
}

const stopPollingProgress = () => {
  if (deployPollInterval) {
    clearInterval(deployPollInterval)
    deployPollInterval = null
  }
}

const startDeploy = async (payload: Record<string, unknown>) => {
  deployLaunching.value = true
  deployRunning.value = false
  deployFinished.value = false
  deployError.value = ''
  deployLogs.value = []
  deployProgressPercent.value = 0
  activeView.value = 'deploy'
  deployTarget.value = {
    id: 'pending',
    name: createForm.name || createForm.addr || '-',
    groupId: undefined,
    address: createForm.addr || '-',
    status: 'unknown',
    tags: [],
  }
  try {
    const res = await nodesApi.deployCreate(payload)
    if (!res.success || !res.data?.node?.nodeId) {
      deployError.value = res.message || t('app.nodes.deploy.failed')
      deployLogs.value = [deployError.value]
      deployFinished.value = true
      return false
    }
    const nodeId = res.data.node.nodeId
    deployLogs.value = res.data.logs ?? []
    deployOperationId = `node-deploy:${nodeId}`
    windowStore.registerGlobalOperation({
      operationId: deployOperationId,
      sourceAppId: 'node-manager',
      title: deployTarget.value?.name
        ? t('app.nodes.deploy.operationTitle', { name: deployTarget.value.name })
        : t('app.nodes.deploy.drawerTitle'),
      cancellable: false,
      reason: t('app.nodes.deploy.operationBusy'),
    })
    deployRunning.value = true
    startPollingProgress(nodeId)
    return true
  } catch {
    deployError.value = t('app.nodes.deploy.failed')
    deployLogs.value = [deployError.value]
    deployFinished.value = true
    return false
  } finally {
    deployLaunching.value = false
  }
}

const showDeployProgress = () => {
  if (deployRunning.value || deployFinished.value) activeView.value = 'deploy'
}

const backgroundDeploy = () => {
  if (deployRunning.value && !deployLaunching.value) activeView.value = 'list'
}

const finishDeploy = () => {
  if (!deployFinished.value) return
  activeView.value = 'list'
  stopPollingProgress()
  deployLogs.value = []
  deployError.value = ''
  deployProgressPercent.value = 0
  deployTarget.value = null
  deployFinished.value = false
  pendingDeployPayload.value = null
  precheckResult.value = null
  Object.assign(createForm, createFormDefaults())
  void fetchNodes()
}

const handleProvisionBack = () => {
  if (activeView.value === 'deploy') {
    if (deployFinished.value) finishDeploy()
    else backgroundDeploy()
    return
  }
  cancelCreate()
}

const confirmDeploy = async () => {
  if (!pendingDeployPayload.value || !precheckResult.value?.passed) return
  isEditActive.value = false
  await startDeploy(pendingDeployPayload.value)
}

const executeNodeAction = async (node: NodeSummary, action: 'repair' | 'retire' | 'uninstall') => {
  const actionLabel = t(`app.nodes.actions.${action}`)
  const isUninstallingCurrentNode = action === 'uninstall' && currentNodeId.value === node.id
  if (isUninstallingCurrentNode) {
    const guardMessage = buildNodeSwitchBlockMessage()
    if (guardMessage) {
      toastStore.error(guardMessage)
      return
    }
  }
  const confirmMessage = isUninstallingCurrentNode
    ? t('app.nodes.actions.confirmCurrentUninstallMessage', { name: node.name })
    : t('app.nodes.actions.confirmMessage', { action: actionLabel, name: node.name })
  const confirmed = await showConfirmation(
    confirmMessage,
    t('app.nodes.actions.confirmTitle'),
    actionLabel,
    t('confirmation.cancel'),
    action === 'repair' ? 'primary' : 'danger',
  )
  if (!confirmed) return

  try {
    let res
    if (action === 'repair') {
      res = await nodesApi.repair(node.id)
    } else if (action === 'retire') {
      res = await nodesApi.retire(node.id)
    } else {
      res = await nodesApi.uninstall(node.id)
    }
    if (!res.success) {
      toastStore.error(res.message || t('app.nodes.actions.failed', { action: actionLabel }))
      return
    }
    if (isUninstallingCurrentNode) {
      const switchResult = nodeStore.requestSwitchCurrentNode('local')
      if (switchResult.switched) {
        toastStore.success(t('app.nodes.actions.currentUninstallSuccess'))
      } else {
        toastStore.error(switchResult.reason || t('app.nodes.switchFailed'))
      }
    } else {
      toastStore.success(t('app.nodes.actions.success', { action: actionLabel }))
    }
    await fetchNodes()
  } catch {
    toastStore.error(t('app.nodes.actions.failed', { action: actionLabel }))
  }
}

watch(
  refreshInterval,
  (value) => {
    nodeStore.startAutoRefresh(value)
  },
  { immediate: true },
)

watch(selectedRemoteNodeId, () => {
  void loadSelectedNodeDetail()
})

watch(
  () => nodeStore.nodes,
  () => {
    void loadSelectedNodeDetail(true)
  },
)

onUnmounted(() => {
  nodeStore.startAutoRefresh(10000)
  stopPollingProgress()
})

const isUpgradeDialogOpen = ref(false)
const upgradeSubmitting = ref(false)
const releases = ref<UpgradeReleaseRecord[]>([])
const upgradeForm = reactive({
  targetVersion: '',
  overwriteSameVersion: false,
})

const releaseOptions = computed(() =>
  releases.value.map((r) => ({
    value: r.version,
    label: `${r.version} (${r.channel})`,
  })),
)

const getNodeNameById = (id: string) => {
  const node = nodes.value.find((n) => n.id === id)
  return node ? nodeDisplayName(node) : id
}

const openUpgradeDialog = async () => {
  upgradeForm.targetVersion = ''
  upgradeForm.overwriteSameVersion = false
  isUpgradeDialogOpen.value = true
  try {
    const res = await upgradesApi.listReleases()
    if (res.success && res.data) {
      releases.value = res.data.filter((r) => r.upgradeEligible)
    }
  } catch (err) {
    console.error('Failed to load releases', err)
    toastStore.error(t('app.nodes.upgradeDialog.noReleases'))
  }
}

const submitUpgradePlan = async () => {
  if (!upgradeForm.targetVersion) return
  upgradeSubmitting.value = true
  try {
    const payload = {
      targetVersion: upgradeForm.targetVersion,
      nodeIds: selectedNodeIds.value,
      overwriteSameVersion: upgradeForm.overwriteSameVersion,
    }
    const createResponse = await upgradesApi.createPlan(payload)
    if (!createResponse.success || !createResponse.data) {
      toastStore.error(createResponse.message || t('app.nodes.upgradeDialog.createPlanFailed'))
      return
    }
    const startResponse = await upgradesApi.startPlan(createResponse.data.plan.planId)
    if (!startResponse.success || !startResponse.data) {
      toastStore.error(startResponse.message || t('app.nodes.upgradeDialog.startPlanFailed'))
      return
    }

    selectedNodeIds.value = []
    isUpgradeDialogOpen.value = false

    // 缓存升级状态并跳转
    window.sessionStorage.setItem('vust.activeUpgradePlanId', startResponse.data.plan.planId)
    window.sessionStorage.setItem(
      'vust.activeUpgradePlanDetail',
      JSON.stringify(startResponse.data),
    )

    toastStore.success(t('app.nodes.upgradeDialog.startPlanSuccess') || '升级任务已成功启动')
    router.push({ path: '/upgrade-progress', query: { planId: startResponse.data.plan.planId } })
  } catch (err) {
    console.error('Failed to submit upgrade plan', err)
    toastStore.error(t('app.nodes.upgradeDialog.startPlanFailed'))
  } finally {
    upgradeSubmitting.value = false
  }
}

const nodeManagerBusy = computed(
  () =>
    precheckSubmitting.value ||
    deployLaunching.value ||
    deployRunning.value ||
    editSubmitting.value ||
    upgradeSubmitting.value,
)

watch(
  nodeManagerBusy,
  (busy) => {
    if (!props.windowId) return
    windowStore.updateWindowRuntimeState(props.windowId, {
      busy,
      allowsNodeSwitch: !busy,
      blockLevel: busy ? 'busy' : 'open',
      blockReason: busy ? t('app.nodes.guardBusy') : t('app.nodes.guardOpen'),
    })
  },
  { immediate: true },
)

onMounted(() => {
  fetchNodes()
})
</script>

<template>
  <div class="node-manager" data-page="node-manager" data-vust-app="nodes">
    <!-- 升级引导 Alert 提示条 -->
    <VustAlert
      v-if="showUpgradeGuide && activeView === 'list'"
      :title="t('app.nodes.upgradeGuideTitle')"
      type="info"
      closeable
      style="margin-bottom: 12px"
      @close="showUpgradeGuide = false"
    >
      {{ t('app.nodes.upgradeGuideDesc') }}
    </VustAlert>

    <VustCard v-if="activeView === 'list'" shadow="never" class="toolbar-card">
      <div class="toolbar" data-ui="toolbar">
        <div class="toolbar-left">
          <VustButton
            type="primary"
            :disabled="hasDeploySession"
            data-ui="node-create"
            @click="startCreate"
          >
            {{ t('app.nodes.create.action') }}
          </VustButton>
          <VustButton
            v-if="hasDeploySession"
            type="secondary"
            data-ui="node-deploy-resume"
            @click="showDeployProgress"
          >
            {{
              deployFinished ? t('app.nodes.deploy.viewResult') : t('app.nodes.deploy.viewProgress')
            }}
          </VustButton>
          <VustButton
            type="primary"
            :class="{ 'upgrade-btn-highlight': showUpgradeGuide }"
            :disabled="selectedNodeIds.length === 0"
            data-ui="node-upgrade-selected"
            @click="openUpgradeDialog"
          >
            {{ t('app.nodes.upgradeSelected') }}
            {{ selectedNodeIds.length > 0 ? `(${selectedNodeIds.length})` : '' }}
          </VustButton>
        </div>

        <div class="toolbar-right">
          <VustSelect
            v-model="selectedNodeId"
            class="agent-select"
            :options="nodeOptions"
            :placeholder="t('desktop.header.nodePlaceholder')"
          />

          <VustSelect v-model="refreshInterval" class="refresh-select" :options="refreshOptions" />

          <VustButton @click="fetchNodes">{{ t('app.nodes.refresh.manual') }}</VustButton>
        </div>
      </div>
    </VustCard>

    <VustCard
      v-if="activeView === 'list' && selectedRemoteNodeId"
      shadow="never"
      class="detail-card"
      data-slot="detail"
    >
      <template #header>
        <div class="drawer-list-header">
          <span>{{ t('app.nodes.detail.title') }}</span>
          <VustTag :type="selectedNodeDetail?.node?.status === 'online' ? 'success' : 'warning'">
            {{ selectedNodeName }}
          </VustTag>
        </div>
      </template>

      <div class="detail-body">
        <VustAlert
          v-if="selectedNodeDetail?.node?.lifecycleStatus === 'conflict'"
          :title="t('app.nodes.detail.conflict')"
          type="error"
          show-icon
        />

        <div class="detail-grid">
          <VustDescriptions
            :items="nodeBaseDetailItems"
            :data="selectedNodeDetail?.node as any"
            border
          >
            <template #lifecycle="{ data }: { data: any }">
              <VustTag type="info">{{ data?.lifecycleStatus ?? '--' }}</VustTag>
            </template>
            <template #runtime="{ data }: { data: any }">
              <VustTag :type="runtimeTagType(data?.runtimeStatus)">
                {{ data?.runtimeStatus ?? '--' }}
              </VustTag>
            </template>
            <template #health="{ data }: { data: any }">
              <VustTag :type="healthTagType(data?.healthStatus)">
                {{ data?.healthStatus ?? '--' }}
              </VustTag>
            </template>
            <template #lastSeen="{ data }: { data: any }">
              {{ formatDateTime(data?.lastSeenAt) }}
            </template>
            <template #group="{ data }: { data: any }">
              {{ data?.groupName ?? '--' }}
            </template>
          </VustDescriptions>

          <VustDescriptions
            :items="nodeDeployDetailItems"
            :data="selectedNodeDetail?.provisioning as any"
            border
          >
            <template #deployMethod="{ data }: { data: any }">
              {{ data?.deployMethod ?? '--' }}
            </template>
            <template #deployStatus="{ data }: { data: any }">
              <VustTag :type="data?.lastDeployResultStatus === 'failed' ? 'danger' : 'success'">
                {{ data?.lastDeployResultStatus ?? '--' }}
              </VustTag>
            </template>
            <template #sshTarget="{ data }: { data: any }">
              {{ data?.sshAddr ? `${data.sshAddr}:${data.sshPort ?? 22}` : '--' }}
            </template>
            <template #installDir="{ data }: { data: any }">
              {{ data?.installDir ?? '--' }}
            </template>
            <template #deployAt="{ data }: { data: any }">
              {{ formatDateTime(data?.lastDeployAt) }}
            </template>
            <template #deployError="{ data }: { data: any }">
              {{ data?.lastDeployErrorSummary }}
            </template>
          </VustDescriptions>

          <VustDescriptions
            :items="nodeSessionDetailItems"
            :data="selectedNodeDetail?.session as any"
            border
          >
            <template #sessionId="{ data }: { data: any }">
              {{ data?.sessionId ? data.sessionId.split('-')[0] : '--' }}
            </template>
            <template #registeredAt="{ data }: { data: any }">
              {{ formatDateTime(data?.registeredAt) }}
            </template>
            <template #lastHeartbeatAt="{ data }: { data: any }">
              {{ formatDateTime(data?.lastHeartbeatAt) }}
            </template>
            <template #leaseExpiresAt="{ data }: { data: any }">
              {{ formatDateTime(data?.leaseExpiresAt) }}
            </template>
            <template #agentVersion>
              {{ selectedNodeDetail?.node?.metadata?.resource?.version || '--' }}
            </template>
          </VustDescriptions>
        </div>
      </div>
      <!-- 节点加载遮罩：仅覆盖详情卡片，不遮挡右上角切换下拉框和窗口 Header 强关按钮 -->
      <VustLoading :loading="nodeDetailLoading" cover />
    </VustCard>

    <ApplicationDialog
      :visible="isEditActive"
      :title="t('app.nodes.form.editNode')"
      width="720px"
      :close-on-click-overlay="false"
      :close-disabled="editSubmitting"
      initial-focus-selector="[data-ui='node-edit-name'] input"
      data-ui="node-edit-dialog"
      @close="cancelEdit"
    >
      <form class="node-edit-form" style="position: relative" @submit.prevent>
        <!-- 基础配置 -->
        <div class="form-section">
          <div class="form-section-title">{{ t('app.nodes.sections.basic') }}</div>
          <div class="form-grid-2col">
            <VustFormItem :label="t('app.nodes.create.name')">
              <VustInput v-model="editForm.name" data-ui="node-edit-name" />
            </VustFormItem>
            <VustFormItem :label="t('app.nodes.create.addr')">
              <VustInput v-model="editForm.addr" />
            </VustFormItem>
            <VustFormItem :label="t('app.nodes.create.port')">
              <VustInput v-model="editForm.port" />
            </VustFormItem>
            <VustFormItem :label="t('app.nodes.create.user')">
              <VustInput v-model="editForm.user" autocomplete="username" />
            </VustFormItem>
          </div>
        </div>

        <!-- 认证配置 -->
        <div class="form-section">
          <div class="form-section-title">{{ t('app.nodes.sections.auth') }}</div>
          <div class="form-grid-2col">
            <VustFormItem :label="t('app.nodes.create.authMode')">
              <VustSelect
                v-model="editForm.authMode"
                :options="[
                  { label: t('app.nodes.create.authPassword'), value: 'password' },
                  { label: t('app.nodes.create.authKey'), value: 'key' },
                ]"
              />
            </VustFormItem>
            <VustFormItem
              v-if="editForm.authMode === 'password'"
              :label="t('app.nodes.create.password')"
            >
              <input type="text" autocomplete="username" style="display: none" />
              <VustInput
                v-model="editForm.pwd"
                type="password"
                showPassword
                autocomplete="new-password"
              />
            </VustFormItem>
            <VustFormItem v-else :label="t('app.nodes.create.privateKeyPassphrase')">
              <input type="text" autocomplete="username" style="display: none" />
              <VustInput
                v-model="editForm.privateKeyPassphrase"
                type="password"
                showPassword
                autocomplete="new-password"
              />
            </VustFormItem>

            <template v-if="editForm.authMode === 'key'">
              <div class="grid-col-2">
                <VustFormItem :label="t('app.nodes.create.privateKey')">
                  <VustInput
                    v-model="editForm.privateKey"
                    :placeholder="t('app.nodes.create.privateKeyPlaceholder')"
                  />
                </VustFormItem>
              </div>
            </template>
          </div>
        </div>

        <!-- 分组与描述 -->
        <div class="form-section">
          <div class="form-section-title">{{ t('app.nodes.sections.group') }}</div>
          <div class="form-grid-2col">
            <VustFormItem :label="t('app.nodes.create.groupId')">
              <VustSelect
                v-model="editForm.groupId"
                :options="[
                  { label: t('app.nodes.create.groupDefault'), value: 'default' },
                  { label: t('app.nodes.create.groupCustom'), value: 'custom' },
                ]"
              />
            </VustFormItem>
            <VustFormItem
              v-if="editForm.groupId === 'custom'"
              :label="t('app.nodes.create.groupCustomInput')"
            >
              <VustInput v-model="editForm.groupCustom" />
            </VustFormItem>
            <VustFormItem :label="t('app.nodes.create.tags')">
              <VustInput v-model="editForm.tags" />
            </VustFormItem>
            <VustFormItem :label="t('app.nodes.create.description')" class="grid-col-2">
              <VustInput v-model="editForm.description" />
            </VustFormItem>
          </div>
        </div>

        <!-- 部署配置 -->
        <div class="form-section">
          <div class="form-section-title">{{ t('app.nodes.sections.install') }}</div>
          <div class="form-grid-2col">
            <VustFormItem :label="t('app.nodes.create.servicePort')">
              <VustInput v-model="editForm.servicePort" />
            </VustFormItem>
            <VustFormItem :label="t('app.nodes.create.vustUrl')">
              <VustInput
                v-model="editForm.vustUrl"
                :placeholder="t('app.nodes.create.vustUrlPlaceholder')"
              />
            </VustFormItem>
          </div>
        </div>

        <VustAlert v-if="editError" :title="editError" type="error" show-icon />
        <VustLoading :loading="editDetailLoading" cover />
      </form>

      <template #footer>
        <VustButton :disabled="editSubmitting" @click="cancelEdit">
          {{ t('app.nodes.edit.cancel') }}
        </VustButton>
        <VustButton
          type="primary"
          :loading="editSubmitting"
          :disabled="editDetailLoading"
          @click="submitEdit"
        >
          {{ t('app.nodes.edit.submit') }}
        </VustButton>
      </template>
    </ApplicationDialog>

    <VustCard v-if="activeView === 'list'" shadow="never" class="table-card">
      <VustTable :data="nodes" :columns="columns" border class="nodes-table" data-ui="table">
        <!-- 这里只需处理具有 slot 的列 -->
        <template #selectionHeader>
          <VustCheckbox
            :model-value="isAllSelected"
            :disabled="nodes.filter(isNodeUpgradable).length === 0"
            @change="handleSelectAll"
          />
        </template>

        <template #selection="{ row: node }: { row: any }">
          <VustCheckbox
            :model-value="selectedNodeIds.includes(node.id)"
            :disabled="!isNodeUpgradable(node)"
            @change="(val) => handleSelectChange(node.id, val)"
          />
        </template>

        <template #name="{ row: node }: { row: any }">
          <div class="name-cell">
            <span>{{ nodeDisplayName(node) }}</span>
            <VustTag v-if="node.id === currentNodeId" type="primary">
              {{ t('app.nodes.current') }}
            </VustTag>
          </div>
        </template>

        <template #address="{ row: node }: { row: any }">
          {{ node.address || '-' }}
        </template>

        <template #status="{ row: node }: { row: any }">
          <VustTag :type="statusTagType(node.status)">{{ statusLabel(node.status) }}</VustTag>
        </template>

        <template #resource="{ row: node }: { row: any }">
          <span class="resource-text">{{ resourceLabel(node) }}</span>
        </template>

        <template #tags="{ row: node }: { row: any }">
          <div class="tag-list">
            <VustTag v-if="node.tags.length === 0" type="default">
              {{ t('app.nodes.noTags') }}
            </VustTag>
            <VustTag v-for="tag in node.tags" :key="tag">{{ tag }}</VustTag>
          </div>
        </template>

        <template #actions="{ row: node }: { row: any }">
          <VustActionMenu
            v-if="node.id !== 'local'"
            :label="t('app.nodes.actions.menu')"
            :actions="getNodeActions(node)"
          />
          <span v-else class="vl-text-muted">-</span>
        </template>

        <template #empty>
          <div class="empty-state">
            <span>{{ t('app.nodes.messages.empty') }}</span>
          </div>
        </template>
      </VustTable>
      <!-- 节点加载遮罩：仅覆盖表格卡片，不遮挡右上角切换下拉框和窗口 Header 强关按钮 -->
      <VustLoading :loading="nodeDetailLoading" cover />
    </VustCard>

    <ApplicationDialog
      :visible="isCheckDrawerVisible"
      :title="t('app.nodes.check.drawerTitle')"
      width="680px"
      data-ui="node-check-dialog"
      @close="closeCheckDrawer"
    >
      <VustDescriptions
        :items="[
          { label: t('app.nodes.check.target'), slot: 'target' },
          { label: t('app.nodes.check.statusLabel'), slot: 'status' },
        ]"
        border
        class="drawer-meta"
      >
        <template #target>
          {{ checkDetailTarget?.name ?? '-' }}
        </template>
        <template #status>
          <VustTag :type="checkDetail?.status?.toLowerCase() === 'online' ? 'success' : 'danger'">
            {{ checkStatusLabel }}
          </VustTag>
        </template>
      </VustDescriptions>

      <div class="drawer-list">
        <VustCard
          v-for="detail in checkDetailItems"
          :key="detail.key"
          shadow="never"
          class="drawer-list-item"
        >
          <template #header>
            <div class="drawer-list-header">
              <span>{{ detail.label }}</span>
              <VustTag :type="checkItemTagType(detail.item.status)">
                {{ checkItemStatusText(detail.item.status) }}
              </VustTag>
            </div>
          </template>
          <div class="check-item-detail">{{ checkItemDetail(detail.item) || '-' }}</div>
        </VustCard>
      </div>

      <template #footer>
        <VustButton type="primary" @click="closeCheckDrawer">{{
          t('app.nodes.check.close')
        }}</VustButton>
      </template>
    </ApplicationDialog>

    <NodeProvisionWorkspace
      v-if="activeView !== 'list'"
      :phase="activeView"
      :form="createForm"
      :precheck-result="precheckResult"
      :precheck-submitting="precheckSubmitting"
      :deploy-launching="deployLaunching"
      :deploy-running="deployRunning"
      :deploy-finished="deployFinished"
      :deploy-target-name="deployTarget?.name || ''"
      :deploy-progress-percent="deployProgressPercent"
      :deploy-logs="deployLogs"
      :deploy-error="deployError"
      @back="handleProvisionBack"
      @precheck="submitCreate"
      @edit="returnToCreate"
      @deploy="confirmDeploy"
      @background="backgroundDeploy"
    />

    <!-- 弹窗：精细化升级配置 -->
    <ApplicationDialog
      :visible="isUpgradeDialogOpen"
      :title="t('app.nodes.upgradeDialog.title')"
      width="600px"
      :close-disabled="upgradeSubmitting"
      return-focus-selector="[data-ui='node-upgrade-selected']"
      data-ui="node-upgrade-dialog"
      @close="isUpgradeDialogOpen = false"
    >
      <div class="dialog-detail-content flex-column gap-layout">
        <VustAlert
          :title="t('app.nodes.upgradeDialog.selectedCount', { count: selectedNodeIds.length })"
          type="info"
          show-icon
        />

        <div
          style="
            max-height: 100px;
            overflow-y: auto;
            display: flex;
            flex-wrap: wrap;
            gap: 6px;
            padding: 4px;
            border: 1px dashed var(--vdl-border-default);
            border-radius: 4px;
          "
        >
          <VustTag v-for="id in selectedNodeIds" :key="id" type="default">
            {{ getNodeNameById(id) }}
          </VustTag>
        </div>

        <div class="form-group" style="margin-top: 10px">
          <label style="display: block; margin-bottom: 6px; font-weight: 600">
            {{ t('app.nodes.upgradeDialog.targetVersion') }}
          </label>
          <VustSelect
            v-model="upgradeForm.targetVersion"
            :options="releaseOptions"
            :placeholder="t('app.nodes.upgradeDialog.versionPlaceholder')"
            style="width: 100%"
          />
        </div>

        <div class="form-group" style="margin-top: 10px">
          <VustCheckbox v-model="upgradeForm.overwriteSameVersion">
            {{ t('app.nodes.upgradeDialog.overwriteSameVersion') }}
          </VustCheckbox>
          <div class="form-hint">
            {{ t('app.nodes.upgradeDialog.overwriteSameVersionTip') }}
          </div>
        </div>
      </div>

      <template #footer>
        <VustButton :disabled="upgradeSubmitting" @click="isUpgradeDialogOpen = false">
          {{ t('app.nodes.upgradeDialog.cancel') }}
        </VustButton>
        <VustButton
          type="primary"
          :loading="upgradeSubmitting"
          :disabled="!upgradeForm.targetVersion"
          @click="submitUpgradePlan"
        >
          {{ t('app.nodes.upgradeDialog.submit') }}
        </VustButton>
      </template>
    </ApplicationDialog>

    <ApplicationConfirmationDialog
      :visible="confirmationState.visible"
      :title="confirmationState.title"
      :message="confirmationState.message"
      :confirm-text="confirmationState.confirmText"
      :cancel-text="confirmationState.cancelText"
      :type="confirmationState.type"
      dialog-ui="node-confirmation-dialog"
      @confirm="handleConfirmationResponse(true)"
      @cancel="handleConfirmationResponse(false)"
    />
  </div>
</template>

<style scoped>
/* 确保表格最后一列操作栏下拉菜单能够正常弹出越界显示 */
:deep(.vl-table-cell:last-child),
:deep(.vl-table-cell:last-child .vl-cell) {
  overflow: visible !important;
}

.node-manager {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-3);
  background: var(--vdl-bg-canvas);
  box-sizing: border-box;
}

.toolbar-card {
  flex-shrink: 0;
  position: relative;
  z-index: 10;
}

.toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: var(--vdl-space-3);
}

.toolbar-right {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
}

.agent-select {
  width: 180px;
}

.refresh-select {
  width: 150px;
}

.table-card {
  flex: 1;
  min-height: 0;
  position: relative;
  z-index: 1;
}

.detail-card {
  min-height: auto;
  position: relative;
  z-index: 1;
}

.name-cell {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: var(--vdl-space-2);
}

.resource-text {
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-caption);
}

.tag-list {
  display: flex;
  flex-wrap: wrap;
  justify-content: center;
  gap: var(--vdl-space-1);
}

.form-hint {
  margin-top: var(--vdl-space-1);
  font-size: var(--vdl-font-caption);
  color: var(--vdl-text-muted);
}

.detail-body {
  position: relative;
  min-height: 100px;
}

.detail-grid {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: var(--vdl-space-4);
}

.drawer-meta {
  margin-bottom: var(--vdl-space-3);
}

.drawer-list {
  display: grid;
  gap: var(--vdl-space-2);
}

.drawer-list-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-2);
}

.check-item-detail {
  font-size: var(--vdl-font-body-sm);
  line-height: 1.5;
}

.empty-state {
  padding: var(--vdl-space-8);
  text-align: center;
  color: var(--vdl-text-muted);
}

@media (max-width: 1200px) {
  .detail-grid {
    grid-template-columns: 1fr;
  }
}

.form-section {
  background-color: var(--vdl-bg-card);
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  padding: var(--vdl-space-4);
}

.form-section-title {
  font-size: var(--vdl-font-body);
  font-weight: 700;
  color: var(--vdl-text-primary);
  margin-bottom: var(--vdl-space-3);
  padding-left: var(--vdl-space-2);
  border-left: 3px solid var(--vdl-primary);
  line-height: 1.2;
}

.form-grid-2col {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: var(--vdl-space-3) var(--vdl-space-4);
}

.grid-col-2 {
  grid-column: span 2;
}

.toolbar-left {
  display: flex;
  gap: var(--vdl-space-3);
  align-items: center;
}

@keyframes pulse-border {
  0% {
    box-shadow: 0 0 0 0 color-mix(in srgb, var(--vdl-primary) 70%, transparent);
  }
  70% {
    box-shadow: 0 0 0 6px transparent;
  }
  100% {
    box-shadow: 0 0 0 0 transparent;
  }
}

.upgrade-btn-highlight {
  animation: pulse-border 1.5s infinite;
  border: 1px solid var(--vdl-primary);
}
</style>
