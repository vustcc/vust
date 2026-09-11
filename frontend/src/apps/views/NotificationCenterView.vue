<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type {
  NotificationArchiveScope,
  NotificationAttentionLevel,
  NotificationCategory,
  NotificationCode,
  NotificationDetail,
  NotificationReadFilter,
  NotificationSummary,
  OperationModule,
} from '@/api/generated'
import { notificationsApi } from '@/api/modules/notifications'
import { useNotificationCenterStore } from '@/stores/notification-center'
import { useToastStore } from '@/stores/toast'
import { useWindowManagerStore } from '@/stores/window-manager'
import type { AppId } from '@/apps/registry'
import { formatDateTime } from '@/utils/time'
import {
  VustActionMenu,
  VustAlert,
  VustButton,
  VustDescriptions,
  VustDrawer,
  VustEmpty,
  VustFormItem,
  VustInput,
  VustLoading,
  VustPagination,
  VustSelect,
  VustSelectionBar,
  VustTable,
  VustTag,
} from '@/components/ui'
import type { VustTableColumn } from '@/components/ui/VustTable.vue'

defineProps<{
  isMaximized?: boolean
  payload?: Record<string, unknown>
}>()

const { t } = useI18n()
const toastStore = useToastStore()
const centerStore = useNotificationCenterStore()
const windowStore = useWindowManagerStore()

const page = ref(1)
const pageSize = ref(20)
const total = ref(0)
const items = ref<NotificationSummary[]>([])
const archiveScope = ref<NotificationArchiveScope>('active')
const readFilter = ref<NotificationReadFilter>('all')
const category = ref<'all' | NotificationCategory>('all')
const attentionLevel = ref<'all' | NotificationAttentionLevel>('all')
const module = ref<'all' | OperationModule>('all')
const keyword = ref('')
const createdFrom = ref('')
const createdTo = ref('')
const filtersExpanded = ref(false)
const selectedIds = ref<string[]>([])

const initialLoading = ref(false)
const refreshing = ref(false)
const loadError = ref<string>()
const refreshError = ref<string>()
const loadedOnce = ref(false)

const drawerOpen = ref(false)
const detail = ref<NotificationDetail>()
const detailLoading = ref(false)
const detailError = ref<string>()
const pendingIds = ref(new Set<string>())
const readAllPending = ref(false)
const batchPending = ref(false)
const emptyValue = '—'

let listSequence = 0
let listController: AbortController | undefined
let detailSequence = 0
let detailController: AbortController | undefined

const pageSizeOptions = [20, 50, 100].map((value) => ({ label: String(value), value }))
const readOptions = computed(() => [
  { label: t('app.notificationCenter.filters.all'), value: 'all' },
  { label: t('app.notificationCenter.filters.unread'), value: 'unread' },
  { label: t('app.notificationCenter.filters.read'), value: 'read' },
])
const archiveOptions = computed(() => [
  { label: t('app.notificationCenter.filters.active'), value: 'active' },
  { label: t('app.notificationCenter.filters.archived'), value: 'archived' },
])
const categoryOptions = computed(() => [
  { label: t('app.notificationCenter.filters.all'), value: 'all' },
  ...(['task', 'security', 'system'] as NotificationCategory[]).map((value) => ({
    label: t(`app.notificationCenter.category.${value}`),
    value,
  })),
])
const attentionLevelOptions = computed(() => [
  { label: t('app.notificationCenter.filters.all'), value: 'all' },
  ...(['info', 'warning', 'critical'] as NotificationAttentionLevel[]).map((value) => ({
    label: t(`app.notificationCenter.attentionLevel.${value}`),
    value,
  })),
])
const moduleOptions = computed(() => [
  { label: t('app.notificationCenter.filters.all'), value: 'all' },
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
  ).map((value) => ({ label: t(`app.operationLog.modules.${value}`), value })),
])

const tableColumns = computed<VustTableColumn[]>(() => [
  { label: t('app.notificationCenter.columns.time'), width: 176, slot: 'time', align: 'center' },
  {
    label: t('app.notificationCenter.columns.attentionLevel'),
    width: 92,
    slot: 'attentionLevel',
    align: 'center',
  },
  {
    label: t('app.notificationCenter.columns.outcome'),
    width: 104,
    slot: 'outcome',
    align: 'center',
  },
  {
    label: t('app.notificationCenter.columns.content'),
    minWidth: 260,
    slot: 'content',
    align: 'center',
  },
  { label: t('app.notificationCenter.columns.target'), minWidth: 170, slot: 'target' },
  {
    label: t('app.notificationCenter.columns.actions'),
    width: 120,
    align: 'center',
    slot: 'actions',
  },
])

const totalPages = computed(() => Math.max(1, Math.ceil(total.value / pageSize.value)))
const hasQueryFilters = computed(
  () =>
    readFilter.value !== 'all' ||
    category.value !== 'all' ||
    attentionLevel.value !== 'all' ||
    module.value !== 'all' ||
    keyword.value.trim() !== '' ||
    createdFrom.value !== '' ||
    createdTo.value !== '',
)
const emptyDescription = computed(() => {
  if (hasQueryFilters.value) return t('app.notificationCenter.emptyFiltered')
  return archiveScope.value === 'archived'
    ? t('app.notificationCenter.emptyArchived')
    : t('app.notificationCenter.emptyActive')
})

const detailItems = computed(() => {
  if (!detail.value) return []
  const current = detail.value
  const values = [
    {
      label: t('app.notificationCenter.detail.createdAt'),
      value: formatDateTime(current.createdAt),
      span: 2,
    },
    {
      label: t('app.notificationCenter.detail.category'),
      value: t(`app.notificationCenter.category.${current.category}`),
    },
    {
      label: t('app.notificationCenter.detail.attentionLevel'),
      value: t(`app.notificationCenter.attentionLevel.${current.attentionLevel}`),
    },
    {
      label: t('app.notificationCenter.detail.outcome'),
      value: current.outcome ? t(`app.notificationCenter.outcome.${current.outcome}`) : emptyValue,
    },
    {
      label: t('app.notificationCenter.detail.module'),
      value: t(`app.operationLog.modules.${current.source.module}`),
    },
    current.source.nodeName || current.source.nodeId
      ? {
          label: t('app.notificationCenter.detail.node'),
          value: current.source.nodeName ?? current.source.nodeId,
          span: 2,
        }
      : null,
    current.subject
      ? {
          label: t('app.notificationCenter.detail.subject'),
          value: current.subject.displayName ?? current.subject.id,
          span: 2,
        }
      : null,
    current.taskId
      ? {
          label: t('app.notificationCenter.detail.taskId'),
          value: current.taskId,
          span: 2,
        }
      : null,
    {
      label: t('app.notificationCenter.detail.operationEventId'),
      value: current.operationEventId,
      span: 2,
    },
    { label: t('app.notificationCenter.detail.traceId'), value: current.traceId, span: 2 },
    current.errorCode
      ? { label: t('app.notificationCenter.detail.errorCode'), value: current.errorCode }
      : null,
    current.errorSummary
      ? {
          label: t('app.notificationCenter.detail.errorSummary'),
          value: current.errorSummary,
          span: 2,
        }
      : null,
    Object.keys(current.parameters).length > 0
      ? {
          label: t('app.notificationCenter.detail.parameters'),
          value: JSON.stringify(current.parameters),
          span: 2,
        }
      : null,
  ]
  return values.filter((value): value is NonNullable<typeof value> => value !== null)
})

function buildQuery() {
  return {
    page: page.value,
    pageSize: pageSize.value,
    archiveScope: archiveScope.value,
    readFilter: readFilter.value,
    categories: category.value === 'all' ? undefined : [category.value],
    attentionLevels: attentionLevel.value === 'all' ? undefined : [attentionLevel.value],
    modules: module.value === 'all' ? undefined : [module.value],
    createdFrom: createdFrom.value ? new Date(createdFrom.value).toISOString() : undefined,
    createdTo: createdTo.value ? new Date(createdTo.value).toISOString() : undefined,
    keyword: keyword.value.trim() || undefined,
  }
}

/** 执行 latest-request-wins 列表查询；静默刷新失败时保留现有结果。 */
async function loadNotifications(silent = false) {
  const sequence = ++listSequence
  listController?.abort()
  const controller = new AbortController()
  listController = controller
  if (loadedOnce.value || silent) refreshing.value = true
  else initialLoading.value = true
  if (!silent) loadError.value = undefined
  refreshError.value = undefined
  try {
    const response = await notificationsApi.query(buildQuery(), controller.signal)
    if (sequence !== listSequence || controller.signal.aborted) return
    if (!response.success || !response.data) throw new Error(response.message)
    items.value = response.data.items
    total.value = response.data.total
    selectedIds.value = selectedIds.value.filter((id) =>
      response.data?.items.some((item) => item.notificationId === id),
    )
    loadedOnce.value = true
  } catch (cause) {
    if (sequence !== listSequence || controller.signal.aborted) return
    const message = cause instanceof Error ? cause.message : t('app.notificationCenter.fetchFailed')
    if (loadedOnce.value) refreshError.value = message
    else loadError.value = message
  } finally {
    if (sequence === listSequence) {
      initialLoading.value = false
      refreshing.value = false
      listController = undefined
    }
  }
}

function applyFilters() {
  page.value = 1
  void loadNotifications()
}

/** 返回通知行选择控件的本地化无障碍名称。 */
function selectionRowLabel(row: NotificationSummary) {
  return t('app.notificationCenter.selection.row', { name: notificationTitle(row.code) })
}

function setPending(notificationId: string, pending: boolean) {
  const next = new Set(pendingIds.value)
  if (pending) next.add(notificationId)
  else next.delete(notificationId)
  pendingIds.value = next
}

async function updateReadState(item: NotificationSummary, read: boolean) {
  if (pendingIds.value.has(item.notificationId)) return
  setPending(item.notificationId, true)
  try {
    const response = await notificationsApi.updateReadState(item.notificationId, { read })
    if (!response.success) throw new Error(response.message)
    const readAt = read ? new Date().toISOString() : undefined
    item.readAt = readAt
    if (detail.value?.notificationId === item.notificationId) detail.value.readAt = readAt
    await centerStore.refreshUnreadSummary()
  } catch {
    toastStore.error(t('app.notificationCenter.updateFailed'))
  } finally {
    setPending(item.notificationId, false)
  }
}

async function updateArchiveState(item: NotificationSummary, archived: boolean) {
  if (pendingIds.value.has(item.notificationId)) return
  setPending(item.notificationId, true)
  try {
    const response = await notificationsApi.updateArchiveState(item.notificationId, { archived })
    if (!response.success) throw new Error(response.message)
    items.value = items.value.filter((row) => row.notificationId !== item.notificationId)
    total.value = Math.max(0, total.value - 1)
    selectedIds.value = selectedIds.value.filter((id) => id !== item.notificationId)
    if (detail.value?.notificationId === item.notificationId) drawerOpen.value = false
    await centerStore.refreshUnreadSummary()
  } catch {
    toastStore.error(t('app.notificationCenter.updateFailed'))
  } finally {
    setPending(item.notificationId, false)
  }
}

async function markAllRead() {
  if (readAllPending.value) return
  readAllPending.value = true
  try {
    const response = await notificationsApi.readAll()
    if (!response.success) throw new Error(response.message)
    const now = new Date().toISOString()
    items.value.forEach((item) => {
      item.readAt ??= now
    })
    await centerStore.refreshUnreadSummary()
  } catch {
    toastStore.error(t('app.notificationCenter.updateFailed'))
  } finally {
    readAllPending.value = false
  }
}

async function updateSelectedArchiveState() {
  if (batchPending.value || selectedIds.value.length === 0) return
  batchPending.value = true
  try {
    const response = await notificationsApi.updateBatchArchiveState({
      notificationIds: selectedIds.value,
      archived: archiveScope.value === 'active',
    })
    if (!response.success) throw new Error(response.message)
    const selected = new Set(selectedIds.value)
    items.value = items.value.filter((item) => !selected.has(item.notificationId))
    total.value = Math.max(0, total.value - selected.size)
    selectedIds.value = []
    await centerStore.refreshUnreadSummary()
  } catch {
    toastStore.error(t('app.notificationCenter.updateFailed'))
  } finally {
    batchPending.value = false
  }
}

async function openDetail(item: NotificationSummary) {
  drawerOpen.value = true
  detail.value = undefined
  detailError.value = undefined
  detailLoading.value = true
  const sequence = ++detailSequence
  detailController?.abort()
  const controller = new AbortController()
  detailController = controller
  try {
    const response = await notificationsApi.detail(item.notificationId, controller.signal)
    if (sequence !== detailSequence || controller.signal.aborted) return
    if (!response.success || !response.data) throw new Error(response.message)
    detail.value = response.data
    if (!item.readAt) await updateReadState(item, true)
  } catch (cause) {
    if (sequence !== detailSequence || controller.signal.aborted) return
    detailError.value =
      cause instanceof Error ? cause.message : t('app.notificationCenter.detailFailed')
  } finally {
    if (sequence === detailSequence) {
      detailLoading.value = false
      detailController = undefined
    }
  }
}

function openTarget(item: NotificationSummary) {
  if (!item.action) return
  windowStore.openWindowWithPayload(item.action.appId as AppId, item.action.payload)
}

function rowActions(item: NotificationSummary) {
  return [
    {
      label: t('app.notificationCenter.actions.details'),
      handler: () => void openDetail(item),
    },
    item.readAt
      ? {
          label: t('app.notificationCenter.actions.markUnread'),
          disabled: pendingIds.value.has(item.notificationId),
          handler: () => void updateReadState(item, false),
        }
      : {
          label: t('app.notificationCenter.actions.markRead'),
          disabled: pendingIds.value.has(item.notificationId),
          handler: () => void updateReadState(item, true),
        },
    item.archivedAt
      ? {
          label: t('app.notificationCenter.actions.restore'),
          disabled: pendingIds.value.has(item.notificationId),
          handler: () => void updateArchiveState(item, false),
        }
      : {
          label: t('app.notificationCenter.actions.archive'),
          disabled: pendingIds.value.has(item.notificationId),
          handler: () => void updateArchiveState(item, true),
        },
  ]
}

function notificationTitle(code: NotificationCode) {
  return t(`app.notificationCenter.events.${code}`)
}

function targetText(item: NotificationSummary) {
  if (item.code === 'fileTaskFinished') {
    const totalItemCount = item.parameters.totalItemCount
    const count =
      typeof totalItemCount === 'number' && Number.isFinite(totalItemCount) ? totalItemCount : 0
    return t('app.notificationCenter.fileTaskItemCount', { count })
  }
  return item.subject?.displayName ?? item.subject?.id ?? t('common.none')
}

function attentionLevelTagType(value: NotificationAttentionLevel): 'info' | 'warning' | 'danger' {
  if (value === 'critical') return 'danger'
  return value
}

function outcomeTagType(
  value: NonNullable<NotificationSummary['outcome']>,
): 'success' | 'info' | 'warning' | 'danger' {
  if (value === 'success') return 'success'
  if (value === 'canceled') return 'info'
  if (value === 'partial') return 'warning'
  return 'danger'
}

watch([page, pageSize], () => void loadNotifications())
watch(archiveScope, () => {
  selectedIds.value = []
  applyFilters()
})
watch(
  () => centerStore.version,
  (next, previous) => {
    if (previous && next !== previous && loadedOnce.value) void loadNotifications(true)
  },
)
watch(drawerOpen, (open) => {
  if (!open) {
    detailSequence += 1
    detailController?.abort()
    detailController = undefined
  }
})

onMounted(() => void loadNotifications())
onBeforeUnmount(() => {
  listSequence += 1
  detailSequence += 1
  listController?.abort()
  detailController?.abort()
})
</script>

<template>
  <div
    class="notification-center"
    data-page="notification-center"
    data-ui="notification-center-app"
  >
    <div class="toolbar" data-slot="toolbar">
      <div class="toolbar-filters">
        <VustFormItem
          :label="t('app.notificationCenter.filters.readState')"
          for="notification-read-filter"
        >
          <VustSelect
            id="notification-read-filter"
            v-model="readFilter"
            name="notificationReadFilter"
            :aria-label="t('app.notificationCenter.filters.readState')"
            :options="readOptions"
          />
        </VustFormItem>
        <VustFormItem :label="t('common.search')" for="notification-keyword">
          <VustInput
            id="notification-keyword"
            v-model="keyword"
            name="notificationKeyword"
            :aria-label="t('common.search')"
            :maxlength="100"
            :placeholder="t('app.notificationCenter.searchPlaceholder')"
            @keyup.enter="applyFilters"
          />
        </VustFormItem>
      </div>
      <div class="toolbar-actions" data-slot="primary-actions">
        <VustButton @click="filtersExpanded = !filtersExpanded">
          {{
            filtersExpanded
              ? t('app.notificationCenter.hideFilters')
              : t('app.notificationCenter.moreFilters')
          }}
        </VustButton>
        <VustButton :loading="refreshing" @click="loadNotifications(true)">
          {{ t('app.notificationCenter.refresh') }}
        </VustButton>
        <VustButton
          :disabled="centerStore.unreadCount === 0"
          :loading="readAllPending"
          @click="markAllRead"
        >
          {{ t('app.notificationCenter.markAllRead') }}
        </VustButton>
      </div>
    </div>

    <div v-if="filtersExpanded" class="advanced-filters" data-slot="advanced-filters">
      <VustFormItem
        :label="t('app.notificationCenter.filters.archiveScope')"
        for="notification-archive-filter"
      >
        <VustSelect
          id="notification-archive-filter"
          v-model="archiveScope"
          name="notificationArchiveScope"
          :aria-label="t('app.notificationCenter.filters.archiveScope')"
          :options="archiveOptions"
        />
      </VustFormItem>
      <VustFormItem
        :label="t('app.notificationCenter.filters.category')"
        for="notification-category-filter"
      >
        <VustSelect
          id="notification-category-filter"
          v-model="category"
          name="notificationCategory"
          :aria-label="t('app.notificationCenter.filters.category')"
          :options="categoryOptions"
        />
      </VustFormItem>
      <VustFormItem
        :label="t('app.notificationCenter.filters.attentionLevel')"
        for="notification-attention-level-filter"
      >
        <VustSelect
          id="notification-attention-level-filter"
          v-model="attentionLevel"
          name="notificationAttentionLevel"
          :aria-label="t('app.notificationCenter.filters.attentionLevel')"
          :options="attentionLevelOptions"
        />
      </VustFormItem>
      <VustFormItem
        :label="t('app.notificationCenter.filters.module')"
        for="notification-module-filter"
      >
        <VustSelect
          id="notification-module-filter"
          v-model="module"
          name="notificationModule"
          :aria-label="t('app.notificationCenter.filters.module')"
          :options="moduleOptions"
        />
      </VustFormItem>
      <VustFormItem
        :label="t('app.notificationCenter.filters.from')"
        for="notification-created-from"
      >
        <VustInput
          id="notification-created-from"
          v-model="createdFrom"
          name="notificationCreatedFrom"
          type="datetime-local"
          :aria-label="t('app.notificationCenter.filters.from')"
        />
      </VustFormItem>
      <VustFormItem :label="t('app.notificationCenter.filters.to')" for="notification-created-to">
        <VustInput
          id="notification-created-to"
          v-model="createdTo"
          name="notificationCreatedTo"
          type="datetime-local"
          :aria-label="t('app.notificationCenter.filters.to')"
        />
      </VustFormItem>
      <VustButton class="apply-filter-button" type="primary" @click="applyFilters">
        {{ t('common.search') }}
      </VustButton>
    </div>

    <VustAlert
      v-if="refreshError"
      type="warning"
      show-icon
      closable
      :title="t('app.notificationCenter.refreshFailed')"
      :description="refreshError"
      :close-label="t('common.close')"
      data-slot="refresh-warning"
      @close="refreshError = undefined"
    />
    <VustAlert
      v-if="loadError"
      type="error"
      show-icon
      :title="t('app.notificationCenter.fetchFailed')"
      :description="loadError"
      data-slot="load-error"
    />

    <div class="table-shell" data-slot="notification-list">
      <VustSelectionBar
        v-if="selectedIds.length"
        class="selection-bar"
        :count="selectedIds.length"
        :label="t('app.notificationCenter.selection.selected')"
        :clear-label="t('app.notificationCenter.selection.clear')"
        :aria-label="t('app.notificationCenter.selection.summary', { count: selectedIds.length })"
        data-slot="batch-actions"
        @clear="selectedIds = []"
      >
        <VustButton :loading="batchPending" @click="updateSelectedArchiveState">
          {{
            archiveScope === 'active'
              ? t('app.notificationCenter.archiveSelected')
              : t('app.notificationCenter.restoreSelected')
          }}
        </VustButton>
      </VustSelectionBar>
      <VustTable
        v-model:selected-row-keys="selectedIds"
        :data="items"
        :columns="tableColumns"
        row-key="notificationId"
        selectable
        :select-all-label="t('app.notificationCenter.selection.all')"
        :select-row-label="selectionRowLabel"
        border
        data-ui="notification-table"
      >
        <template #time="{ row }: { row: NotificationSummary }">
          <span class="time-cell">{{ formatDateTime(row.createdAt) }}</span>
        </template>
        <template #attentionLevel="{ row }: { row: NotificationSummary }">
          <VustTag :type="attentionLevelTagType(row.attentionLevel)" size="small">
            {{ t(`app.notificationCenter.attentionLevel.${row.attentionLevel}`) }}
          </VustTag>
        </template>
        <template #outcome="{ row }: { row: NotificationSummary }">
          <VustTag v-if="row.outcome" :type="outcomeTagType(row.outcome)" size="small">
            {{ t(`app.notificationCenter.outcome.${row.outcome}`) }}
          </VustTag>
          <span v-else class="empty-value" data-slot="outcome-empty">{{ emptyValue }}</span>
        </template>
        <template #content="{ row }: { row: NotificationSummary }">
          <button
            class="content-button"
            :class="{ 'is-read': Boolean(row.readAt) }"
            type="button"
            :aria-label="`${notificationTitle(row.code)} · ${row.readAt ? t('app.notificationCenter.readLabel') : t('app.notificationCenter.unreadLabel')}`"
            @click="openDetail(row)"
          >
            <strong>{{ notificationTitle(row.code) }}</strong>
          </button>
        </template>
        <template #target="{ row }: { row: NotificationSummary }">
          <span class="target-cell">{{ targetText(row) }}</span>
        </template>
        <template #actions="{ row }: { row: NotificationSummary }">
          <VustActionMenu
            :actions="rowActions(row)"
            :disabled="pendingIds.has(row.notificationId)"
            :label="t('common.actions')"
          />
        </template>
        <template #empty>
          <VustEmpty :description="emptyDescription">
            <template #extra>
              <VustButton v-if="loadError" @click="loadNotifications()">
                {{ t('app.notificationCenter.refresh') }}
              </VustButton>
            </template>
          </VustEmpty>
        </template>
      </VustTable>
      <VustLoading :loading="initialLoading" cover />
    </div>

    <div class="footer" data-slot="pagination">
      <div class="pagination-controls" data-ui="pagination-controls">
        <VustPagination
          :current-page="page"
          :total-pages="totalPages"
          @page-change="(value) => (page = value)"
        />
        <VustSelect
          id="notification-page-size"
          v-model="pageSize"
          name="notificationPageSize"
          :aria-label="t('app.notificationCenter.pageSize')"
          :options="pageSizeOptions"
        />
      </div>
    </div>

    <VustDrawer
      v-model="drawerOpen"
      :title="t('app.notificationCenter.details')"
      width="min(560px, 92vw)"
      data-ui="notification-detail-drawer"
    >
      <VustAlert
        v-if="detailError"
        type="error"
        show-icon
        :title="t('app.notificationCenter.detailFailed')"
        :description="detailError"
      />
      <VustLoading :loading="detailLoading" />
      <div v-if="detail" class="detail-content" data-slot="notification-detail">
        <h3>{{ notificationTitle(detail.code) }}</h3>
        <VustDescriptions :items="detailItems" :column="2" border />
      </div>
      <template #footer>
        <VustButton v-if="detail?.action" type="primary" @click="openTarget(detail)">
          {{ t(detail.action.labelKey) }}
        </VustButton>
      </template>
    </VustDrawer>
  </div>
</template>

<style scoped>
.notification-center {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-3);
  box-sizing: border-box;
  background: var(--vdl-bg-canvas);
  overflow: hidden;
}

.toolbar,
.toolbar-filters,
.toolbar-actions,
.advanced-filters,
.selection-bar,
.footer,
.state-cell {
  display: flex;
  align-items: end;
  gap: var(--vdl-space-3);
}

.toolbar {
  align-items: flex-end;
  justify-content: space-between;
  flex: 0 0 auto;
}

.toolbar-filters {
  align-items: flex-end;
  flex: 1 1 420px;
  min-width: 0;
}

.toolbar-filters :deep(.vl-form-item) {
  margin-bottom: 0;
}

.toolbar-filters :deep(.vl-form-item:first-child) {
  flex: 0 0 150px;
}

.toolbar-filters :deep(.vl-form-item:last-child) {
  flex: 1 1 280px;
  max-width: 460px;
}

.toolbar-actions {
  align-self: flex-end;
  align-items: center;
  justify-content: flex-end;
  flex: 0 0 auto;
}

.advanced-filters {
  align-items: end;
  flex-wrap: wrap;
  padding: var(--vdl-space-3);
  border: 1px solid var(--vdl-border-subtle);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-surface);
}

.advanced-filters :deep(.vl-form-item) {
  flex: 1 1 150px;
  min-width: 140px;
  max-width: 220px;
  margin-bottom: 0;
}

.apply-filter-button {
  align-self: flex-end;
}

.table-shell {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-2);
  flex: 1 1 auto;
  min-height: 0;
  overflow: hidden;
  border-radius: var(--vdl-radius-md);
}

.selection-bar {
  flex: 0 0 auto;
}

.table-shell :deep(.vl-table-container) {
  flex: 1 1 auto;
  height: auto;
  min-height: 0;
}

.time-cell {
  font-family: var(--vdl-font-mono);
  font-size: var(--vdl-font-caption);
  color: var(--vdl-text-secondary);
  white-space: nowrap;
}

.content-button {
  width: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--vdl-space-1);
  padding: 0;
  color: var(--vdl-text-primary);
  border: 0;
  background: transparent;
  text-align: center;
  cursor: pointer;
}

.content-button.is-read {
  color: var(--vdl-text-muted);
}

.content-button.is-read strong {
  font-weight: 500;
}

.content-button span,
.target-cell {
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-body-sm);
}

.target-cell {
  word-break: break-word;
}

.empty-value {
  color: var(--vdl-text-muted);
}

.footer {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: center;
  min-height: 32px;
}

.pagination-controls {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: var(--vdl-space-3);
}

.pagination-controls :deep(.vl-select) {
  flex: 0 0 72px;
  width: 72px;
}

.detail-content h3 {
  margin: 0 0 var(--vdl-space-4);
  color: var(--vdl-text-primary);
  font-size: var(--vdl-font-title);
}

.detail-content :deep(.vl-descriptions-content) {
  word-break: normal;
  overflow-wrap: anywhere;
}

@media (max-width: 760px) {
  .notification-center {
    padding: var(--vdl-space-2);
  }

  .toolbar,
  .toolbar-filters,
  .toolbar-actions {
    width: 100%;
    flex-wrap: wrap;
  }

  .toolbar-actions {
    justify-content: flex-start;
  }

  .toolbar-filters :deep(.vl-form-item) {
    flex: 1 1 100%;
    max-width: none;
  }

  .advanced-filters :deep(.vl-form-item) {
    max-width: none;
  }
}
</style>
