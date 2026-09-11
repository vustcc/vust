<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type {
  OperationImpact,
  OperationItemStatus,
  OperationLogDetail,
  OperationLogItem,
  OperationLogQuery,
  OperationLogSummary,
  OperationModule,
  OperationOutcome,
} from '@/api/generated'
import { operationLogApi } from '@/api/modules/operation-log'
import { useNodeStore } from '@/stores/node'
import {
  VustAlert,
  VustButton,
  VustDescriptions,
  VustDialog,
  VustEmpty,
  VustFormItem,
  VustInput,
  VustLoading,
  VustPagination,
  VustSelect,
  VustTable,
  VustTag,
} from '@/components/ui'
import type { VustTableColumn } from '@/components/ui/VustTable.vue'

const { t, te, locale } = useI18n()
const nodeStore = useNodeStore()
const props = defineProps<{
  module?: OperationModule
  nodeId?: string
  embedded?: boolean
  payload?: Record<string, unknown>
}>()
const PAGE_SIZE = 20
const items = ref<OperationLogSummary[]>([])
const total = ref(0)
const totalPages = computed(() => Math.max(1, Math.ceil(total.value / PAGE_SIZE)))
const page = ref(1)
const initialLoading = ref(false)
const refreshing = ref(false)
const initialError = ref('')
const refreshWarning = ref('')
const loadedAt = ref<string>()
const filtersExpanded = ref(false)
const detailOpen = ref(false)
const detail = ref<OperationLogDetail>()
const detailLoading = ref(false)
const detailError = ref('')
let listSequence = 0
let detailSequence = 0
let listController: AbortController | undefined
let detailController: AbortController | undefined
let pollTimer: number | undefined

const filters = reactive({
  range: '24h',
  outcome: '' as OperationOutcome | '',
  keyword: '',
  module: '' as OperationModule | '',
  impact: '' as OperationImpact | '',
  nodeId: '',
})

const columns = computed<VustTableColumn[]>(() => [
  { label: t('app.operationLog.columns.index'), width: 72, slot: 'index', align: 'center' },
  { label: t('app.operationLog.columns.time'), width: 190, slot: 'time', align: 'center' },
  { label: t('app.operationLog.columns.impact'), width: 100, slot: 'impact', align: 'center' },
  { label: t('app.operationLog.columns.actor'), minWidth: 160, slot: 'actor' },
  { label: t('app.operationLog.columns.originNode'), minWidth: 150, slot: 'originNode' },
  {
    label: t('app.operationLog.columns.operation'),
    minWidth: 210,
    slot: 'operation',
    align: 'center',
  },
  { label: t('app.operationLog.columns.outcome'), width: 110, slot: 'outcome', align: 'center' },
  { label: t('common.actions'), width: 90, slot: 'actions', align: 'center' },
])
const itemColumns = computed<VustTableColumn[]>(() => [
  { label: t('app.operationLog.items.sequence'), width: 64, prop: 'sequence', align: 'center' },
  { label: t('app.operationLog.items.source'), minWidth: 210, slot: 'itemSource' },
  { label: t('app.operationLog.items.destination'), minWidth: 210, slot: 'itemDestination' },
  { label: t('app.operationLog.items.status'), width: 104, slot: 'itemStatus', align: 'center' },
  { label: t('app.operationLog.items.errorSummary'), minWidth: 180, slot: 'itemError' },
])

const rangeOptions = computed(() => [
  { label: t('app.operationLog.ranges.hour'), value: '1h' },
  { label: t('app.operationLog.ranges.day'), value: '24h' },
  { label: t('app.operationLog.ranges.week'), value: '7d' },
])
const outcomeOptions = computed(() => [
  { label: t('app.operationLog.allOutcomes'), value: '' },
  ...(['success', 'failure', 'partial', 'canceled', 'timedOut'] as OperationOutcome[]).map(
    (value) => ({ label: label('outcomes', value), value }),
  ),
])
const moduleOptions = computed(() => [
  { label: t('app.operationLog.allModules'), value: '' },
  ...(
    [
      'auth',
      'nodes',
      'suites',
      'docker',
      'files',
      'processes',
      'disks',
      'monitoring',
      'scripts',
      'scheduledTasks',
      'upgrades',
      'terminal',
      'settings',
    ] as OperationModule[]
  ).map((value) => ({ label: label('modules', value), value })),
])
const impactOptions = computed(() => [
  { label: t('app.operationLog.allImpacts'), value: '' },
  ...(['info', 'warning', 'error'] as OperationImpact[]).map((value) => ({
    label: label('impacts', value),
    value,
  })),
])
const nodeOptions = computed(() => [
  { label: t('app.operationLog.allNodes'), value: '' },
  ...nodeStore.nodes.map((node) => ({ label: node.name || node.id, value: node.id })),
])
const detailItems = computed(() => {
  if (!detail.value) return []
  const value = detail.value
  return [
    { label: t('app.operationLog.columns.time'), value: formatTime(value.occurredAt) },
    { label: t('app.operationLog.columns.operation'), value: eventLabel(value) },
    { label: t('app.operationLog.columns.actor'), value: actorLabel(value.actor) },
    { label: t('app.operationLog.columns.originNode'), value: originNodeLabel(value) },
    ...(value.clientIp
      ? [{ label: t('app.operationLog.detail.clientIp'), value: value.clientIp }]
      : []),
    { label: t('app.operationLog.columns.outcome'), value: label('outcomes', value.outcome) },
    ...(value.target?.displayName
      ? [{ label: t('app.operationLog.columns.target'), value: value.target.displayName }]
      : []),
    ...(value.target
      ? [
          { label: t('app.operationLog.detail.targetKind'), value: value.target.kind },
          { label: t('app.operationLog.detail.targetId'), value: value.target.id },
        ]
      : []),
    { label: t('app.operationLog.detail.traceId'), value: value.traceId },
    ...(value.taskId ? [{ label: t('app.operationLog.detail.taskId'), value: value.taskId }] : []),
    ...(value.requestMethod
      ? [
          {
            label: t('app.operationLog.detail.request'),
            value: `${value.requestMethod} ${value.routeTemplate ?? ''}`.trim(),
          },
        ]
      : []),
    ...(value.errorCode
      ? [{ label: t('app.operationLog.detail.errorCode'), value: value.errorCode }]
      : []),
    ...(value.errorSummary
      ? [{ label: t('app.operationLog.detail.errorSummary'), value: value.errorSummary }]
      : []),
  ]
})
const parameterItems = computed(() => {
  if (!detail.value) return []
  const value = detail.value
  const parameters = value.parameters
  const hasNodeId = Object.prototype.hasOwnProperty.call(parameters, 'nodeId')
  return Object.entries(parameters).flatMap(([key, parameterValue]) => {
    if (key === 'nodeName') {
      if (hasNodeId) return []
      return [
        {
          label: t('app.operationLog.detail.node'),
          value: nodeDisplayName(value, undefined, parameterValue),
        },
      ]
    }
    if (key === 'nodeId') {
      return [
        {
          label: t('app.operationLog.detail.node'),
          value: nodeDisplayName(value, parameterValue, parameters.nodeName),
        },
      ]
    }
    return [{ label: key, value: String(parameterValue) }]
  })
})

function label(group: string, value: string) {
  const key = `app.operationLog.${group}.${value}`
  return te(key) ? t(key) : value
}
function eventLabel(value: Pick<OperationLogSummary, 'eventCode' | 'eventLabel'>) {
  if (value.eventLabel) {
    return locale.value === 'en' ? value.eventLabel.enUs : value.eventLabel.zhCn
  }
  const key = `app.operationLog.events.${value.eventCode}`
  return te(key) ? t(key) : value.eventCode.replaceAll('_', ' ')
}
function actorLabel(actor: OperationLogSummary['actor']) {
  if (actor.kind === 'anonymous') return t('app.operationLog.actorKinds.anonymous')
  if (actor.kind === 'system') return t('app.operationLog.actorKinds.system')
  return actor.displayName
}
function actorText(value: OperationLogSummary) {
  const actor = actorLabel(value.actor)
  return value.clientIp ? `${actor} (${value.clientIp})` : actor
}
function originNodeLabel(value: OperationLogSummary) {
  if (value.origin.kind === 'master') return t('app.operationLog.origins.master')
  if (value.origin.nodeId === 'local') return t('app.operationLog.detail.localNode')
  return value.origin.nodeName?.trim() || value.origin.nodeId || t('app.operationLog.origins.agent')
}
function nodeDisplayName(
  value: OperationLogDetail,
  rawNodeId: string | number | boolean | undefined,
  rawNodeName: string | number | boolean | undefined,
) {
  const nodeId = rawNodeId === undefined ? undefined : String(rawNodeId)
  const parameterNodeName = rawNodeName === undefined ? '' : String(rawNodeName).trim()
  if (nodeId === 'local' || (!nodeId && parameterNodeName === 'local')) {
    return t('app.operationLog.detail.localNode')
  }
  if (parameterNodeName) return parameterNodeName
  if (nodeId && value.origin.nodeId === nodeId && value.origin.nodeName?.trim()) {
    return value.origin.nodeName
  }
  if (
    nodeId &&
    value.target?.kind === 'node' &&
    value.target.id === nodeId &&
    value.target.displayName?.trim()
  ) {
    return value.target.displayName
  }
  return nodeId ?? ''
}
function formatTime(value: string) {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  const pad = (part: number) => String(part).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}
function rangeStart() {
  const hours = filters.range === '1h' ? 1 : filters.range === '7d' ? 168 : 24
  return new Date(Date.now() - hours * 60 * 60 * 1000).toISOString()
}
function buildQuery(): OperationLogQuery {
  return {
    page: page.value,
    pageSize: PAGE_SIZE,
    occurredFrom: rangeStart(),
    occurredTo: new Date().toISOString(),
    outcomes: filters.outcome ? [filters.outcome] : undefined,
    modules: props.module ? [props.module] : filters.module ? [filters.module] : undefined,
    nodeIds: props.nodeId ? [props.nodeId] : filters.nodeId ? [filters.nodeId] : undefined,
    impacts: filters.impact ? [filters.impact] : undefined,
    keyword: filters.keyword.trim() || undefined,
  }
}
async function load(options: { silent?: boolean } = {}) {
  const sequence = ++listSequence
  listController?.abort()
  listController = new AbortController()
  const hasData = loadedAt.value !== undefined
  if (!hasData) initialLoading.value = true
  else if (!options.silent) refreshing.value = true
  initialError.value = ''
  try {
    const response = await operationLogApi.query(buildQuery(), listController.signal)
    if (sequence !== listSequence) return
    if (!response.success || !response.data) throw new Error(response.message)
    items.value = response.data.items
    total.value = response.data.total
    loadedAt.value = new Date().toISOString()
    refreshWarning.value = ''
  } catch (error) {
    if (sequence !== listSequence || (error instanceof DOMException && error.name === 'AbortError'))
      return
    const message = error instanceof Error ? error.message : t('app.operationLog.loadFailed')
    if (hasData) refreshWarning.value = message
    else initialError.value = message
  } finally {
    if (sequence === listSequence) {
      initialLoading.value = false
      refreshing.value = false
    }
  }
}
function applyFilters() {
  page.value = 1
  void load()
}
async function openDetailById(eventId: string) {
  const sequence = ++detailSequence
  detailController?.abort()
  detailController = new AbortController()
  detailOpen.value = true
  detail.value = undefined
  detailError.value = ''
  detailLoading.value = true
  try {
    const response = await operationLogApi.detail(eventId, detailController.signal)
    if (sequence !== detailSequence) return
    if (!response.success || !response.data) throw new Error(response.message)
    detail.value = response.data
  } catch (error) {
    if (
      sequence !== detailSequence ||
      (error instanceof DOMException && error.name === 'AbortError')
    )
      return
    detailError.value =
      error instanceof Error ? error.message : t('app.operationLog.detail.loadFailed')
  } finally {
    if (sequence === detailSequence) detailLoading.value = false
  }
}
async function openDetail(row: OperationLogSummary) {
  await openDetailById(row.eventId)
}
function closeDetail() {
  detailOpen.value = false
  detailSequence += 1
  detailController?.abort()
  detailController = undefined
  detailLoading.value = false
}
function impactTag(value: OperationImpact) {
  return value === 'error' ? 'danger' : value === 'warning' ? 'warning' : 'info'
}
function outcomeTag(value: OperationOutcome) {
  return value === 'failure' || value === 'timedOut'
    ? 'danger'
    : value === 'partial'
      ? 'warning'
      : value === 'success'
        ? 'success'
        : 'info'
}
function itemStatusTag(value: OperationItemStatus): 'success' | 'info' | 'warning' | 'danger' {
  if (value === 'succeeded') return 'success'
  if (value === 'failed') return 'danger'
  if (value === 'canceled') return 'warning'
  return 'info'
}
function itemPath(item: OperationLogItem['source']) {
  return item.displayName ?? item.id
}
function poll() {
  if (document.visibilityState === 'visible' && !initialLoading.value && !refreshing.value)
    void load({ silent: true })
}
watch(page, () => void load())
watch(
  () => props.payload,
  (payload) => {
    const eventId = payload?.eventId
    if (typeof eventId === 'string' && eventId.trim()) void openDetailById(eventId)
  },
  { immediate: true },
)
onMounted(() => {
  void nodeStore.refreshNodes().catch(() => undefined)
  void load()
  pollTimer = window.setInterval(poll, 30_000)
})
onBeforeUnmount(() => {
  listController?.abort()
  detailController?.abort()
  if (pollTimer) window.clearInterval(pollTimer)
})
</script>

<template>
  <div
    class="operation-log"
    :class="{ embedded: props.embedded }"
    data-page="operation-log"
    data-vust-app="operation-log"
  >
    <div class="toolbar" data-ui="toolbar" data-slot="controls">
      <VustFormItem
        v-if="!props.embedded"
        class="filter-select"
        :label="t('app.operationLog.timeRange')"
        for="operation-log-range"
      >
        <VustSelect
          id="operation-log-range"
          v-model="filters.range"
          name="operationLogRange"
          :options="rangeOptions"
        />
      </VustFormItem>
      <VustFormItem
        v-if="!props.embedded"
        class="filter-select"
        :label="t('app.operationLog.outcome')"
        for="operation-log-outcome"
      >
        <VustSelect
          id="operation-log-outcome"
          v-model="filters.outcome"
          name="operationLogOutcome"
          :options="outcomeOptions"
        />
      </VustFormItem>
      <VustFormItem
        class="search"
        :label="t('app.operationLog.keyword')"
        for="operation-log-keyword"
      >
        <VustInput
          id="operation-log-keyword"
          v-model="filters.keyword"
          name="operationLogKeyword"
          :aria-label="t('app.operationLog.keyword')"
          @keyup.enter="applyFilters"
        />
      </VustFormItem>
      <div class="toolbar-actions" data-ui="toolbar-actions" data-slot="actions">
        <VustButton
          :aria-expanded="filtersExpanded"
          aria-controls="operation-log-advanced-filters"
          @click="filtersExpanded = !filtersExpanded"
          >{{ t('app.operationLog.moreFilters') }}</VustButton
        >
        <VustButton type="primary" @click="applyFilters">{{
          t('app.operationLog.query')
        }}</VustButton>
        <VustButton :loading="refreshing" @click="load()">{{ t('common.refresh') }}</VustButton>
      </div>
    </div>

    <div
      v-if="filtersExpanded"
      id="operation-log-advanced-filters"
      class="advanced-filters"
      data-ui="filters"
      data-slot="controls"
    >
      <VustFormItem
        v-if="props.embedded"
        class="filter-select"
        :label="t('app.operationLog.timeRange')"
        for="operation-log-range"
      >
        <VustSelect
          id="operation-log-range"
          v-model="filters.range"
          name="operationLogRange"
          :options="rangeOptions"
        />
      </VustFormItem>
      <VustFormItem
        v-if="props.embedded"
        class="filter-select"
        :label="t('app.operationLog.outcome')"
        for="operation-log-outcome"
      >
        <VustSelect
          id="operation-log-outcome"
          v-model="filters.outcome"
          name="operationLogOutcome"
          :options="outcomeOptions"
        />
      </VustFormItem>
      <VustFormItem
        v-if="!props.module"
        class="filter-select"
        :label="t('app.operationLog.module')"
        for="operation-log-module"
      >
        <VustSelect
          id="operation-log-module"
          v-model="filters.module"
          name="operationLogModule"
          :options="moduleOptions"
        />
      </VustFormItem>
      <VustFormItem
        v-if="!props.nodeId"
        class="filter-select"
        :label="t('app.operationLog.node')"
        for="operation-log-node"
      >
        <VustSelect
          id="operation-log-node"
          v-model="filters.nodeId"
          name="operationLogNode"
          :options="nodeOptions"
          data-ui="node-filter"
        />
      </VustFormItem>
      <VustFormItem
        class="filter-select"
        :label="t('app.operationLog.impact')"
        for="operation-log-impact"
      >
        <VustSelect
          id="operation-log-impact"
          v-model="filters.impact"
          name="operationLogImpact"
          :options="impactOptions"
        />
      </VustFormItem>
    </div>

    <VustAlert
      v-if="refreshWarning"
      data-ui="refresh-warning"
      type="warning"
      :title="t('app.operationLog.refreshFailed')"
      :description="refreshWarning"
      show-icon
    />
    <VustAlert
      v-if="initialError"
      data-ui="initial-error"
      type="error"
      :title="t('app.operationLog.loadFailed')"
      :description="initialError"
      show-icon
    />

    <div class="table-shell" data-ui="table" data-slot="content">
      <VustTable v-if="items.length" :data="items" :columns="columns" row-key="eventId" border>
        <template #index="{ index }: { index: number }">
          {{ (page - 1) * PAGE_SIZE + index + 1 }}
        </template>
        <template #time="{ row }: { row: OperationLogSummary }">{{
          formatTime(row.occurredAt)
        }}</template>
        <template #impact="{ row }: { row: OperationLogSummary }"
          ><VustTag :type="impactTag(row.impact)">{{
            label('impacts', row.impact)
          }}</VustTag></template
        >
        <template #actor="{ row }: { row: OperationLogSummary }">
          <strong>{{ actorText(row) }}</strong>
        </template>
        <template #originNode="{ row }: { row: OperationLogSummary }">
          <span data-ui="origin-node">{{ originNodeLabel(row) }}</span>
        </template>
        <template #operation="{ row }: { row: OperationLogSummary }">{{
          eventLabel(row)
        }}</template>
        <template #outcome="{ row }: { row: OperationLogSummary }"
          ><VustTag :type="outcomeTag(row.outcome)">{{
            label('outcomes', row.outcome)
          }}</VustTag></template
        >
        <template #actions="{ row }: { row: OperationLogSummary }"
          ><VustButton size="small" @click="openDetail(row)">{{
            t('common.details')
          }}</VustButton></template
        >
      </VustTable>
      <VustEmpty
        v-else-if="!initialLoading"
        :description="
          initialError
            ? t('app.operationLog.unavailable')
            : filters.keyword ||
                filters.outcome ||
                filters.module ||
                filters.impact ||
                filters.nodeId
              ? t('app.operationLog.noMatches')
              : t('app.operationLog.empty')
        "
      />
      <VustLoading :loading="initialLoading" cover />
    </div>
    <VustPagination
      v-if="total > PAGE_SIZE"
      :current-page="page"
      :total-pages="totalPages"
      data-ui="pagination"
      @page-change="(value) => (page = value)"
    />

    <VustDialog
      :visible="detailOpen"
      data-ui="detail"
      :title="t('app.operationLog.detail.title')"
      width="min(1120px, 92vw)"
      @close="closeDetail"
    >
      <div class="detail-content" data-slot="detail">
        <VustAlert
          v-if="detailError"
          type="error"
          :title="t('app.operationLog.detail.loadFailed')"
          :description="detailError"
          show-icon
        />
        <template v-if="detail">
          <VustDescriptions :items="detailItems" :column="2" border />
          <VustDescriptions
            v-if="parameterItems.length"
            class="parameter-descriptions"
            :items="parameterItems"
            :column="2"
            border
            data-ui="parameters"
          />
          <div v-if="detail.items.length" class="operation-items" data-slot="operation-items">
            <h3>{{ t('app.operationLog.items.title') }}</h3>
            <VustTable
              class="operation-items-table"
              :data="detail.items"
              :columns="itemColumns"
              border
              data-ui="operation-log-items"
            >
              <template #itemSource="{ row }: { row: OperationLogItem }">
                <span class="item-path">{{ itemPath(row.source) }}</span>
              </template>
              <template #itemDestination="{ row }: { row: OperationLogItem }">
                <span class="item-path">{{
                  row.destination ? itemPath(row.destination) : t('common.none')
                }}</span>
              </template>
              <template #itemStatus="{ row }: { row: OperationLogItem }">
                <VustTag :type="itemStatusTag(row.status)" size="small">
                  {{ t(`app.operationLog.items.statuses.${row.status}`) }}
                </VustTag>
              </template>
              <template #itemError="{ row }: { row: OperationLogItem }">
                <span>{{ row.errorSummary ?? t('common.none') }}</span>
              </template>
            </VustTable>
          </div>
        </template>
        <VustLoading :loading="detailLoading" cover />
      </div>
    </VustDialog>
  </div>
</template>

<style scoped>
.operation-log {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-3);
  background: var(--vdl-bg-canvas);
  color: var(--vdl-text-primary);
}
.operation-log.embedded {
  padding: var(--vdl-space-3) 0 0;
}
.operation-log.embedded .toolbar {
  flex-wrap: nowrap;
}
.operation-log.embedded .search {
  flex: 1 1 auto;
  inline-size: auto;
  min-inline-size: 180px;
}
.operation-log.embedded .toolbar-actions {
  flex-shrink: 0;
}
.toolbar,
.advanced-filters {
  display: flex;
  align-items: end;
  flex-wrap: wrap;
  gap: var(--vdl-space-2);
}
.search {
  flex: 0 1 260px;
  inline-size: 260px;
  max-inline-size: 100%;
}
.filter-select {
  flex: 0 0 200px;
  inline-size: 200px;
  max-inline-size: 100%;
}
.toolbar-actions {
  display: flex;
  align-items: flex-end;
  justify-content: flex-end;
  gap: var(--vdl-space-2);
  margin-block-end: var(--vdl-space-4);
  margin-inline-start: auto;
}
.advanced-filters {
  padding-block: var(--vdl-space-2);
  border-block: 1px solid var(--vdl-border-subtle);
}
.table-shell {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: auto;
}
.detail-content {
  position: relative;
  min-height: 180px;
  display: grid;
  gap: var(--vdl-space-3);
}
.parameter-descriptions :deep(.vl-descriptions-label) {
  min-width: 0;
  white-space: normal;
  overflow-wrap: anywhere;
}
.operation-items h3 {
  margin: 0 0 var(--vdl-space-2);
  color: var(--vdl-text-primary);
  font-size: var(--vdl-font-body);
}
.operation-items-table {
  height: auto;
}
.item-path {
  font-family: var(--vdl-font-mono);
  overflow-wrap: anywhere;
}
small {
  display: block;
  color: var(--vdl-text-muted);
  margin-top: var(--vdl-space-1);
}
@media (max-width: 760px) {
  .toolbar > *,
  .advanced-filters > * {
    flex: 1 1 180px;
  }
  .operation-log.embedded .toolbar {
    flex-wrap: wrap;
  }
  .operation-log.embedded .toolbar-actions {
    flex-basis: 100%;
    margin-inline-start: 0;
  }
}
</style>
