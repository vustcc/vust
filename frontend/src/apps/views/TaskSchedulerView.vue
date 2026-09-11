<script setup lang="ts">
/**
 * @file TaskSchedulerView.vue
 * @description 自定义计划任务列表及领域操作编排。
 */

import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import type {
  CreateScheduledTaskRequest,
  ScheduledTaskBatchAction,
  ScheduledTaskRun,
  ScheduledTaskSummary,
  UpdateScheduledTaskRequest,
} from '@/api/generated/scheduled-tasks'
import { useTaskScheduler } from '@/composables/useTaskScheduler'
import { useConfirmationModalStore } from '@/stores/confirmation-modal'
import { useToastStore } from '@/stores/toast'
import {
  VustActionMenu,
  VustAlert,
  VustButton,
  VustCheckbox,
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
import VustIcon from '@/components/icons/VustIcon.vue'
import type { VustTableColumn } from '@/components/ui/VustTable.vue'
import ScheduledTaskDetailDrawer from './task-scheduler/ScheduledTaskDetailDrawer.vue'
import ScheduledTaskFormDrawer, {
  type ScheduledTaskFormValue,
} from './task-scheduler/ScheduledTaskFormDrawer.vue'

defineProps<{
  isMaximized?: boolean
  payload?: Record<string, unknown>
}>()

const { t } = useI18n()
const notifications = useToastStore()
const confirmation = useConfirmationModalStore()
const scheduler = useTaskScheduler()
const selectedTaskIds = ref<string[]>([])
const formVisible = ref(false)
const formMode = ref<'create' | 'edit'>('create')
const editingTaskId = ref('')
const detailVisible = ref(false)
const detailTask = ref<ScheduledTaskSummary | null>(null)
const migrationVisible = ref(false)
const migrationTask = ref<ScheduledTaskSummary | null>(null)
const migrationTargetNodeId = ref('')
const batchVisible = ref(false)
const currentBatchId = ref('')

interface TaskMenuAction {
  label: string
  icon: string
  className?: string
  handler: () => void
}

const nodeOptions = computed(() => [
  { value: '', label: t('app.taskScheduler.filters.allNodes') },
  ...scheduler.nodes.value
    .filter((node) => node.nodeId)
    .map((node) => ({
      value: node.nodeId,
      label: node.nodeId === 'local' ? t('app.taskScheduler.localNode') : node.name || node.nodeId,
    })),
])
const executionNodeOptions = computed(() => nodeOptions.value.filter((option) => option.value))
const migrationNodeOptions = computed(() =>
  executionNodeOptions.value.filter((option) => option.value !== migrationTask.value?.node.nodeId),
)
const enabledFilter = computed({
  get: () =>
    scheduler.filters.enabled === undefined
      ? ''
      : scheduler.filters.enabled
        ? 'enabled'
        : 'disabled',
  set: (value: string | number | null) => {
    scheduler.filters.page = 1
    scheduler.filters.enabled = value === '' ? undefined : value === 'enabled'
  },
})
const deploymentFilter = computed({
  get: () => scheduler.filters.deploymentStatus,
  set: (value: string | number | null) => {
    scheduler.filters.page = 1
    scheduler.filters.deploymentStatus = String(value ?? '')
  },
})
const totalPages = computed(() =>
  Math.max(1, Math.ceil(scheduler.total.value / scheduler.filters.pageSize)),
)
const hasFilters = computed(
  () =>
    Boolean(scheduler.filters.nodeId) ||
    Boolean(scheduler.filters.keyword.trim()) ||
    scheduler.filters.enabled !== undefined ||
    Boolean(scheduler.filters.deploymentStatus),
)
const allSelected = computed(
  () =>
    scheduler.tasks.value.length > 0 &&
    scheduler.tasks.value.every((task) => selectedTaskIds.value.includes(task.taskId)),
)
const currentBatch = computed(() =>
  currentBatchId.value ? scheduler.trackedBatches.value[currentBatchId.value] : undefined,
)

const taskColumns = computed<VustTableColumn[]>(() => [
  { label: '', width: 48, align: 'center', slot: 'selection', headerSlot: 'selectionHeader' },
  { prop: 'name', label: t('app.taskScheduler.columns.name'), minWidth: 190, slot: 'name' },
  { prop: 'node', label: t('app.taskScheduler.columns.node'), minWidth: 130, slot: 'node' },
  {
    prop: 'schedule',
    label: t('app.taskScheduler.columns.schedule'),
    minWidth: 190,
    slot: 'schedule',
  },
  {
    prop: 'nextRun',
    label: t('app.taskScheduler.columns.nextRun'),
    minWidth: 165,
    slot: 'nextRun',
  },
  {
    prop: 'lastRun',
    label: t('app.taskScheduler.columns.lastResult'),
    width: 120,
    align: 'center',
    slot: 'lastRun',
  },
  {
    prop: 'deployment',
    label: t('app.taskScheduler.columns.deployment'),
    width: 130,
    align: 'center',
    slot: 'deployment',
  },
  {
    label: t('app.taskScheduler.columns.actions'),
    width: 110,
    align: 'center',
    slot: 'actions',
    fixed: 'right',
  },
])
const batchColumns = computed<VustTableColumn[]>(() => [
  { prop: 'taskId', label: t('app.taskScheduler.batch.task'), minWidth: 240 },
  { prop: 'result', label: t('app.taskScheduler.batch.result'), minWidth: 180, slot: 'result' },
])

const formatTime = (value?: string) => (value ? new Date(value).toLocaleString() : '')
const statusLabel = (status: string) => t(`app.taskScheduler.status.${status}`)
const statusTag = (status?: string): 'success' | 'danger' | 'info' | 'warning' | 'default' => {
  if (status === 'succeeded' || status === 'ready') return 'success'
  if (status === 'failed' || status === 'timedOut') return 'danger'
  if (status === 'waitingForNode' || status === 'partial' || status === 'cancelled')
    return 'warning'
  if (
    status &&
    ['queued', 'starting', 'running', 'cancelling', 'pending', 'applying', 'migrating'].includes(
      status,
    )
  )
    return 'info'
  return 'default'
}
const nextRunText = (task: ScheduledTaskSummary) =>
  task.nextRun.status === 'scheduled' && task.nextRun.at
    ? formatTime(task.nextRun.at)
    : statusLabel(task.nextRun.status)

const resetFilters = () => {
  scheduler.filters.nodeId = ''
  scheduler.filters.keyword = ''
  scheduler.filters.enabled = undefined
  scheduler.filters.deploymentStatus = ''
  scheduler.filters.page = 1
}
const toggleAll = (checked: boolean) => {
  selectedTaskIds.value = checked ? scheduler.tasks.value.map((task) => task.taskId) : []
}
const toggleTask = (taskId: string, checked: boolean) => {
  selectedTaskIds.value = checked
    ? [...new Set([...selectedTaskIds.value, taskId])]
    : selectedTaskIds.value.filter((id) => id !== taskId)
}

const openCreate = () => {
  formMode.value = 'create'
  editingTaskId.value = ''
  formVisible.value = true
}
const openEdit = async (task: ScheduledTaskSummary) => {
  const detail = await scheduler.loadDetail(task.taskId)
  if (!detail) return
  formMode.value = 'edit'
  editingTaskId.value = task.taskId
  formVisible.value = true
}
const submitForm = async (value: ScheduledTaskFormValue) => {
  try {
    if (formMode.value === 'create') {
      const payload: CreateScheduledTaskRequest = value
      if (!(await scheduler.createTask(payload))) return
    } else {
      const payload: UpdateScheduledTaskRequest = {
        name: value.name,
        description: value.description,
        cronExpr: value.cronExpr,
        timeZone: value.timeZone,
        command: value.command,
        timeoutSeconds: value.timeoutSeconds,
        preventOverlap: value.preventOverlap,
      }
      if (!(await scheduler.updateTask(editingTaskId.value, payload))) return
    }
    formVisible.value = false
    notifications.success(t('app.taskScheduler.messages.saveAccepted'))
  } catch (error) {
    notifications.error(
      error instanceof Error ? error.message : t('app.taskScheduler.messages.saveFailed'),
    )
  }
}

const setEnabled = async (task: ScheduledTaskSummary) => {
  try {
    await scheduler.setTaskEnabled(task.taskId, task.desiredState !== 'enabled')
  } catch (error) {
    notifications.error(
      error instanceof Error ? error.message : t('app.taskScheduler.messages.toggleFailed'),
    )
  }
}
const runNow = async (task: ScheduledTaskSummary) => {
  try {
    if (await scheduler.startRun(task.taskId)) {
      notifications.success(t('app.taskScheduler.messages.runAccepted'))
    }
  } catch (error) {
    notifications.error(
      error instanceof Error ? error.message : t('app.taskScheduler.messages.runFailed'),
    )
  }
}
const removeTask = async (task: ScheduledTaskSummary) => {
  const confirmed = await confirmation.showConfirmation(
    t('app.taskScheduler.delete.confirmMessage', { name: task.name }),
    t('app.taskScheduler.delete.confirmTitle'),
    t('app.taskScheduler.delete.confirmAction'),
    t('common.cancel'),
  )
  if (!confirmed) return
  try {
    if (await scheduler.removeTask(task.taskId)) {
      notifications.success(t('app.taskScheduler.messages.deleteAccepted'))
    }
  } catch (error) {
    notifications.error(
      error instanceof Error ? error.message : t('app.taskScheduler.messages.deleteFailed'),
    )
  }
}
const openDetail = async (task: ScheduledTaskSummary) => {
  detailTask.value = task
  detailVisible.value = true
  await Promise.all([scheduler.loadDetail(task.taskId), scheduler.loadRuns(task.taskId)])
}
const cancelRun = async (run: ScheduledTaskRun) => {
  try {
    await scheduler.cancelRun(run.taskId, run.runId)
    await scheduler.loadRuns(run.taskId)
  } catch (error) {
    notifications.error(
      error instanceof Error ? error.message : t('app.taskScheduler.messages.cancelRunFailed'),
    )
  }
}
const openMigration = (task: ScheduledTaskSummary) => {
  migrationTask.value = task
  migrationTargetNodeId.value = ''
  migrationVisible.value = true
}
const submitMigration = async () => {
  if (!migrationTask.value || !migrationTargetNodeId.value) return
  try {
    if (
      await scheduler.migrateTask(migrationTask.value.taskId, {
        targetNodeId: migrationTargetNodeId.value,
      })
    ) {
      migrationVisible.value = false
      notifications.success(t('app.taskScheduler.messages.migrateAccepted'))
    }
  } catch (error) {
    notifications.error(
      error instanceof Error ? error.message : t('app.taskScheduler.messages.migrateFailed'),
    )
  }
}

const rowActions = (task: ScheduledTaskSummary): TaskMenuAction[] => {
  const actions: TaskMenuAction[] = [
    {
      label: t('app.taskScheduler.actions.detail'),
      icon: 'info',
      handler: () => void openDetail(task),
    },
  ]
  if (task.capabilities.canUpdate)
    actions.push({
      label: t('app.taskScheduler.actions.edit'),
      icon: 'edit',
      handler: () => void openEdit(task),
    })
  if (task.capabilities.canChangeState)
    actions.push({
      label:
        task.desiredState === 'enabled'
          ? t('app.taskScheduler.actions.disable')
          : t('app.taskScheduler.actions.enable'),
      icon: task.desiredState === 'enabled' ? 'pause' : 'play',
      handler: () => void setEnabled(task),
    })
  if (task.capabilities.canRun)
    actions.push({
      label: t('app.taskScheduler.actions.runNow'),
      icon: 'play',
      handler: () => void runNow(task),
    })
  if (task.capabilities.canMigrate)
    actions.push({
      label: t('app.taskScheduler.actions.migrate'),
      icon: 'swap',
      handler: () => openMigration(task),
    })
  if (task.capabilities.canRemove)
    actions.push({
      label: t('app.taskScheduler.actions.delete'),
      icon: 'trash',
      className: 'is-danger',
      handler: () => void removeTask(task),
    })
  return actions
}

const runBatch = async (action: ScheduledTaskBatchAction) => {
  if (!selectedTaskIds.value.length) return
  if (action === 'remove') {
    const confirmed = await confirmation.showConfirmation(
      t('app.taskScheduler.batch.removeConfirm', { count: selectedTaskIds.value.length }),
      t('app.taskScheduler.batch.removeTitle'),
      t('common.confirm'),
      t('common.cancel'),
    )
    if (!confirmed) return
  }
  try {
    const batch = await scheduler.createBatch({ action, taskIds: selectedTaskIds.value })
    if (!batch) return
    currentBatchId.value = batch.batchId
    batchVisible.value = true
    selectedTaskIds.value = []
  } catch (error) {
    notifications.error(
      error instanceof Error ? error.message : t('app.taskScheduler.messages.batchFailed'),
    )
  }
}
const batchActions = computed(() => [
  {
    label: t('app.taskScheduler.actions.batchEnable'),
    icon: 'play',
    handler: () => void runBatch('enable'),
  },
  {
    label: t('app.taskScheduler.actions.batchDisable'),
    icon: 'pause',
    handler: () => void runBatch('disable'),
  },
  {
    label: t('app.taskScheduler.actions.batchRun'),
    icon: 'play',
    handler: () => void runBatch('run'),
  },
  {
    label: t('app.taskScheduler.actions.batchDelete'),
    icon: 'trash',
    className: 'is-danger',
    handler: () => void runBatch('remove'),
  },
])
</script>

<template>
  <div
    class="task-scheduler"
    data-page="scheduled-tasks"
    data-ui="scheduled-task-page"
    data-slot="content"
  >
    <div class="toolbar" data-ui="toolbar">
      <div class="toolbar-filters">
        <VustSelect
          id="scheduled-task-node-filter"
          v-model="scheduler.filters.nodeId"
          name="scheduledTaskNodeFilter"
          :aria-label="t('app.taskScheduler.filters.node')"
          class="filter-select"
          :options="nodeOptions"
        />
        <VustSelect
          id="scheduled-task-enabled-filter"
          v-model="enabledFilter"
          name="scheduledTaskEnabledFilter"
          :aria-label="t('app.taskScheduler.filters.state')"
          class="filter-select compact-filter"
          :options="[
            { value: '', label: t('app.taskScheduler.filters.allStates') },
            { value: 'enabled', label: t('app.taskScheduler.enabled') },
            { value: 'disabled', label: t('app.taskScheduler.disabled') },
          ]"
        />
        <VustSelect
          id="scheduled-task-deployment-filter"
          v-model="deploymentFilter"
          name="scheduledTaskDeploymentFilter"
          :aria-label="t('app.taskScheduler.filters.deployment')"
          class="filter-select compact-filter"
          :options="[
            { value: '', label: t('app.taskScheduler.filters.allDeployments') },
            { value: 'ready', label: statusLabel('ready') },
            { value: 'waitingForNode', label: statusLabel('waitingForNode') },
            { value: 'failed', label: statusLabel('failed') },
          ]"
        />
        <VustInput
          id="scheduled-task-search"
          v-model="scheduler.filters.keyword"
          name="scheduledTaskSearch"
          :aria-label="t('app.taskScheduler.filters.search')"
          class="search-input"
          :placeholder="t('app.taskScheduler.filters.searchPlaceholder')"
        />
      </div>
      <div class="toolbar-actions">
        <VustActionMenu
          v-if="selectedTaskIds.length"
          :label="t('app.taskScheduler.actions.batch', { count: selectedTaskIds.length })"
          :actions="batchActions"
          :disabled="scheduler.isActionPending('batch')"
        />
        <VustButton
          :loading="scheduler.listState.value.refreshing"
          :disabled="scheduler.listState.value.initialLoading"
          @click="scheduler.refreshTasks"
        >
          <VustIcon name="refresh" :size="14" />
          {{ t('common.refresh') }}
        </VustButton>
        <VustButton type="primary" @click="openCreate">
          <VustIcon name="plus" :size="14" />
          {{ t('app.taskScheduler.actions.create') }}
        </VustButton>
      </div>
    </div>

    <VustAlert
      v-if="scheduler.listState.value.warning"
      data-ui="alert"
      type="warning"
      :title="t('app.taskScheduler.messages.refreshWarning')"
      :description="scheduler.listState.value.warning"
      show-icon
    />
    <VustAlert
      v-if="scheduler.activeOperations.value.length || scheduler.activeRuns.value.length"
      data-ui="alert"
      type="info"
      :title="
        t('app.taskScheduler.activity.summary', {
          operations: scheduler.activeOperations.value.length,
          runs: scheduler.activeRuns.value.length,
        })
      "
      show-icon
    />

    <div class="table-shell" data-ui="table">
      <VustTable
        v-if="!scheduler.listState.value.error || scheduler.tasks.value.length"
        :data="scheduler.tasks.value"
        :columns="taskColumns"
        row-key="taskId"
        border
      >
        <template #selectionHeader>
          <VustCheckbox
            id="scheduled-task-select-all"
            name="scheduledTaskSelectAll"
            :aria-label="t('app.taskScheduler.selection.all')"
            :model-value="allSelected"
            @update:model-value="toggleAll"
          />
        </template>
        <template #selection="{ row }: { row: ScheduledTaskSummary }">
          <VustCheckbox
            :id="`scheduled-task-select-${row.taskId}`"
            name="scheduledTaskSelection"
            :aria-label="t('app.taskScheduler.selection.task', { name: row.name })"
            :model-value="selectedTaskIds.includes(row.taskId)"
            @update:model-value="(checked) => toggleTask(row.taskId, checked)"
          />
        </template>
        <template #name="{ row }: { row: ScheduledTaskSummary }">
          <button class="task-name" type="button" @click="openDetail(row)">{{ row.name }}</button>
          <VustTag :type="row.desiredState === 'enabled' ? 'success' : 'default'" size="small">
            {{
              row.desiredState === 'enabled'
                ? t('app.taskScheduler.enabled')
                : t('app.taskScheduler.disabled')
            }}
          </VustTag>
        </template>
        <template #node="{ row }: { row: ScheduledTaskSummary }">
          {{ row.node.nodeName }}
        </template>
        <template #schedule="{ row }: { row: ScheduledTaskSummary }">
          <span class="schedule-summary">{{ row.schedule.summary }}</span>
        </template>
        <template #nextRun="{ row }: { row: ScheduledTaskSummary }">
          <span class="secondary-text">{{ nextRunText(row) }}</span>
        </template>
        <template #lastRun="{ row }: { row: ScheduledTaskSummary }">
          <VustTag v-if="row.lastRun" :type="statusTag(row.lastRun.status)">
            {{ statusLabel(row.lastRun.status) }}
          </VustTag>
          <span v-else class="secondary-text">{{ t('app.taskScheduler.status.neverRun') }}</span>
        </template>
        <template #deployment="{ row }: { row: ScheduledTaskSummary }">
          <VustTag :type="statusTag(row.deployment.status)" :title="row.deployment.errorSummary">
            {{ statusLabel(row.deployment.status) }}
          </VustTag>
        </template>
        <template #actions="{ row }: { row: ScheduledTaskSummary }">
          <VustActionMenu :label="t('app.taskScheduler.actions.menu')" :actions="rowActions(row)" />
        </template>
        <template #empty>
          <VustEmpty
            :description="
              hasFilters
                ? t('app.taskScheduler.emptyFiltered')
                : t('app.taskScheduler.emptyInitial')
            "
          >
            <template #extra>
              <VustButton v-if="hasFilters" @click="resetFilters">
                {{ t('app.taskScheduler.filters.clear') }}
              </VustButton>
              <VustButton v-else type="primary" @click="openCreate">
                {{ t('app.taskScheduler.actions.create') }}
              </VustButton>
            </template>
          </VustEmpty>
        </template>
      </VustTable>
      <VustEmpty
        v-if="scheduler.listState.value.error && !scheduler.tasks.value.length"
        :description="scheduler.listState.value.error"
      >
        <template #extra>
          <VustButton @click="scheduler.refreshTasks">
            {{ t('app.taskScheduler.actions.retry') }}
          </VustButton>
        </template>
      </VustEmpty>
      <VustLoading :loading="scheduler.listState.value.initialLoading" cover />
    </div>

    <div v-if="scheduler.total.value" class="pagination-row" data-slot="footer">
      <span class="secondary-text">
        {{ t('app.taskScheduler.pagination.total', { total: scheduler.total.value }) }}
      </span>
      <VustPagination
        :current-page="scheduler.filters.page"
        :total-pages="totalPages"
        @page-change="(page) => (scheduler.filters.page = page)"
      />
    </div>

    <ScheduledTaskFormDrawer
      v-model="formVisible"
      :mode="formMode"
      :detail="formMode === 'edit' ? scheduler.detail.value : null"
      :default-node-id="scheduler.filters.nodeId || 'local'"
      :node-options="executionNodeOptions"
      :saving="
        scheduler.isActionPending(formMode === 'create' ? 'create' : `update:${editingTaskId}`)
      "
      @submit="submitForm"
    />

    <ScheduledTaskDetailDrawer
      v-model="detailVisible"
      :task="detailTask"
      :detail="scheduler.detail.value"
      :detail-state="scheduler.detailState.value"
      :runs="scheduler.runs.value"
      :runs-state="scheduler.runsState.value"
      :output="scheduler.output.value"
      :output-state="scheduler.outputState.value"
      :is-cancel-pending="(runId) => scheduler.isActionPending(`cancel-run:${runId}`)"
      @output="(run) => scheduler.loadOutput(run.taskId, run.runId)"
      @cancel="cancelRun"
    />

    <VustDialog
      :visible="migrationVisible"
      data-ui="migration-dialog"
      :title="t('app.taskScheduler.migration.title')"
      width="480px"
      @close="migrationVisible = false"
    >
      <VustAlert type="info" :title="t('app.taskScheduler.migration.notice')" show-icon />
      <VustFormItem
        :label="t('app.taskScheduler.migration.targetNode')"
        for="scheduled-task-migration-node"
        required
      >
        <VustSelect
          id="scheduled-task-migration-node"
          v-model="migrationTargetNodeId"
          name="scheduledTaskMigrationNode"
          :options="migrationNodeOptions"
          :placeholder="t('app.taskScheduler.migration.placeholder')"
        />
      </VustFormItem>
      <template #footer>
        <VustButton @click="migrationVisible = false">{{ t('common.cancel') }}</VustButton>
        <VustButton
          type="primary"
          :disabled="!migrationTargetNodeId"
          :loading="
            migrationTask ? scheduler.isActionPending(`migrate:${migrationTask.taskId}`) : false
          "
          @click="submitMigration"
        >
          {{ t('app.taskScheduler.actions.migrate') }}
        </VustButton>
      </template>
    </VustDialog>

    <VustDialog
      :visible="batchVisible"
      data-ui="batch-result"
      :title="t('app.taskScheduler.batch.title')"
      width="720px"
      @close="batchVisible = false"
    >
      <VustTag v-if="currentBatch" :type="statusTag(currentBatch.status)">
        {{ statusLabel(currentBatch.status) }}
      </VustTag>
      <VustTable
        v-if="currentBatch"
        :data="currentBatch.items"
        :columns="batchColumns"
        row-key="taskId"
        border
      >
        <template #result="{ row }">
          <VustTag :type="row.errorCode ? 'danger' : 'info'">
            {{
              row.errorSummary ??
              (row.runId
                ? t('app.taskScheduler.batch.runCreated')
                : t('app.taskScheduler.batch.operationCreated'))
            }}
          </VustTag>
        </template>
      </VustTable>
    </VustDialog>
  </div>
</template>

<style scoped>
.task-scheduler {
  height: 100%;
  min-height: 0;
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-3);
  overflow: hidden;
  background: var(--vdl-bg-canvas);
}

.toolbar,
.toolbar-filters,
.toolbar-actions,
.pagination-row {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
}

.toolbar {
  flex: none;
  justify-content: space-between;
  padding: var(--vdl-space-3);
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-panel);
}

.toolbar-filters {
  flex: 1;
  min-width: 0;
}

.filter-select {
  width: 180px;
}

.compact-filter {
  width: 145px;
}

.search-input {
  width: min(300px, 28vw);
}

.table-shell {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: auto;
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-panel);
}

.task-name {
  margin-right: var(--vdl-space-2);
  padding: 0;
  border: 0;
  background: transparent;
  color: var(--vdl-primary);
  font: inherit;
  font-weight: 600;
  cursor: pointer;
}

.task-name:hover {
  text-decoration: underline;
}

.schedule-summary,
.secondary-text {
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-caption);
}

.pagination-row {
  flex: none;
  justify-content: space-between;
}

:deep(.vl-button-content) {
  display: inline-flex;
  align-items: center;
  gap: var(--vdl-space-1);
}

@media (max-width: 900px) {
  .toolbar {
    align-items: stretch;
    flex-direction: column;
  }

  .toolbar-filters {
    flex-wrap: wrap;
  }

  .filter-select,
  .compact-filter,
  .search-input {
    width: min(100%, 220px);
    flex: 1 1 160px;
  }

  .toolbar-actions {
    justify-content: flex-end;
  }
}

@media (max-width: 620px) {
  .task-scheduler {
    padding: var(--vdl-space-2);
  }

  .toolbar-actions {
    flex-wrap: wrap;
  }
}
</style>
