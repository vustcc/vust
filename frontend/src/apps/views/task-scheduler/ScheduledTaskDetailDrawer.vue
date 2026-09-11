<script setup lang="ts">
/**
 * @file ScheduledTaskDetailDrawer.vue
 * @description 计划任务详情、执行记录和受限输出摘要。
 */

import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import type {
  ScheduledTaskDetail,
  ScheduledTaskRun,
  ScheduledTaskRunOutput,
  ScheduledTaskSummary,
} from '@/api/generated/scheduled-tasks'
import type { TaskRequestState } from '@/composables/useTaskScheduler'
import {
  VustAlert,
  VustButton,
  VustDescriptions,
  VustDrawer,
  VustEmpty,
  VustLoading,
  VustTable,
  VustTabs,
  VustTag,
} from '@/components/ui'
import type { VustTableColumn } from '@/components/ui/VustTable.vue'

const props = defineProps<{
  modelValue: boolean
  task: ScheduledTaskSummary | null
  detail: ScheduledTaskDetail | null
  detailState: TaskRequestState
  runs: ScheduledTaskRun[]
  runsState: TaskRequestState
  output: ScheduledTaskRunOutput | null
  outputState: TaskRequestState
  isCancelPending: (runId: string) => boolean
}>()
const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  output: [run: ScheduledTaskRun]
  cancel: [run: ScheduledTaskRun]
}>()

const { t } = useI18n()
const activeTab = ref('detail')
const outputVisible = ref(false)
const runColumns = computed<VustTableColumn[]>(() => [
  {
    prop: 'queuedAt',
    label: t('app.taskScheduler.runs.columns.queuedAt'),
    minWidth: 170,
    slot: 'queuedAt',
  },
  {
    prop: 'triggerSource',
    label: t('app.taskScheduler.runs.columns.triggerSource'),
    width: 105,
    slot: 'triggerSource',
  },
  {
    prop: 'status',
    label: t('app.taskScheduler.runs.columns.status'),
    width: 120,
    align: 'center',
    slot: 'status',
  },
  {
    prop: 'duration',
    label: t('app.taskScheduler.runs.columns.duration'),
    width: 110,
    slot: 'duration',
  },
  {
    label: t('app.taskScheduler.columns.actions'),
    width: 160,
    align: 'center',
    slot: 'actions',
  },
])

const formatTime = (value?: string) => (value ? new Date(value).toLocaleString() : '')
const statusLabel = (status: string) => t(`app.taskScheduler.status.${status}`)
const triggerLabel = (source: string) => t(`app.taskScheduler.trigger.${source}`)
const statusTag = (status?: string): 'success' | 'danger' | 'info' | 'warning' | 'default' => {
  if (status === 'succeeded' || status === 'ready') return 'success'
  if (status === 'failed' || status === 'timedOut') return 'danger'
  if (status === 'waitingForNode' || status === 'partial' || status === 'cancelled')
    return 'warning'
  if (status && ['queued', 'starting', 'running', 'cancelling'].includes(status)) return 'info'
  return 'default'
}
const duration = (run: ScheduledTaskRun) => {
  if (!run.startedAt) return t('app.taskScheduler.status.notStarted')
  const end = run.finishedAt ? new Date(run.finishedAt).getTime() : Date.now()
  const seconds = Math.max(0, Math.round((end - new Date(run.startedAt).getTime()) / 1000))
  return t('app.taskScheduler.runs.seconds', { seconds })
}
const detailItems = computed(() => {
  if (!props.detail) return []
  return [
    { label: t('app.taskScheduler.detail.name'), value: props.detail.name },
    ...(props.detail.description
      ? [{ label: t('app.taskScheduler.detail.description'), value: props.detail.description }]
      : []),
    { label: t('app.taskScheduler.detail.node'), value: props.detail.node.nodeName },
    { label: t('app.taskScheduler.detail.schedule'), value: props.detail.schedule.summary },
    { label: t('app.taskScheduler.detail.cron'), value: props.detail.schedule.cronExpr },
    { label: t('app.taskScheduler.detail.timeZone'), value: props.detail.schedule.timeZone },
    {
      label: t('app.taskScheduler.detail.timeout'),
      value: t('app.taskScheduler.runs.seconds', {
        seconds: props.detail.execution.timeoutSeconds,
      }),
    },
    {
      label: t('app.taskScheduler.detail.preventOverlap'),
      value: props.detail.execution.preventOverlap
        ? t('app.taskScheduler.status.yes')
        : t('app.taskScheduler.status.no'),
    },
    { label: t('app.taskScheduler.detail.revision'), value: props.detail.deployment.revision },
    ...(props.detail.deployment.lastSyncedAt
      ? [
          {
            label: t('app.taskScheduler.detail.lastSyncedAt'),
            value: formatTime(props.detail.deployment.lastSyncedAt),
          },
        ]
      : []),
  ]
})

const openOutput = (run: ScheduledTaskRun) => {
  outputVisible.value = true
  emit('output', run)
}
</script>

<template>
  <VustDrawer
    :model-value="modelValue"
    data-ui="runs"
    :title="task?.name ?? t('app.taskScheduler.detail.title')"
    width="860px"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <div class="detail-content" data-slot="detail">
      <VustTabs
        v-model="activeTab"
        :tabs="[
          { name: 'detail', label: t('app.taskScheduler.detail.tab') },
          { name: 'runs', label: t('app.taskScheduler.runs.tab') },
        ]"
      />
      <template v-if="activeTab === 'detail'">
        <VustAlert
          v-if="detail?.deployment.errorSummary"
          type="warning"
          :title="statusLabel(detail.deployment.status)"
          :description="detail.deployment.errorSummary"
          show-icon
        />
        <VustDescriptions v-if="detail" :items="detailItems" :column="2" border />
        <div v-if="detail" class="command-detail">
          <span class="block-title">{{ t('app.taskScheduler.detail.command') }}</span>
          <pre>{{ detail.execution.command }}</pre>
        </div>
        <VustEmpty v-else-if="detailState.error" :description="detailState.error" />
        <VustLoading :loading="detailState.initialLoading" cover />
      </template>
      <template v-else>
        <div class="runs-shell">
          <VustTable :data="runs" :columns="runColumns" row-key="runId" border>
            <template #queuedAt="{ row }: { row: ScheduledTaskRun }">
              {{ formatTime(row.queuedAt) }}
            </template>
            <template #triggerSource="{ row }: { row: ScheduledTaskRun }">
              {{ triggerLabel(row.triggerSource) }}
            </template>
            <template #status="{ row }: { row: ScheduledTaskRun }">
              <VustTag :type="statusTag(row.status)">{{ statusLabel(row.status) }}</VustTag>
              <span v-if="row.phase && !row.finishedAt" class="run-phase">{{ row.phase }}</span>
            </template>
            <template #duration="{ row }: { row: ScheduledTaskRun }">
              {{ duration(row) }}
            </template>
            <template #actions="{ row }: { row: ScheduledTaskRun }">
              <div class="inline-actions">
                <VustButton v-if="row.output.available" size="small" @click="openOutput(row)">
                  {{ t('app.taskScheduler.runs.output') }}
                </VustButton>
                <VustButton
                  v-if="row.capabilities.canCancel"
                  size="small"
                  type="danger"
                  :loading="isCancelPending(row.runId)"
                  @click="emit('cancel', row)"
                >
                  {{ t('common.cancel') }}
                </VustButton>
              </div>
            </template>
            <template #empty>
              <VustEmpty :description="t('app.taskScheduler.runs.empty')" />
            </template>
          </VustTable>
          <VustLoading :loading="runsState.initialLoading" cover />
        </div>
      </template>
    </div>
  </VustDrawer>

  <VustDrawer
    v-model="outputVisible"
    data-ui="run-output"
    :title="t('app.taskScheduler.runs.outputTitle')"
    width="760px"
  >
    <div class="output-content" data-slot="content">
      <VustAlert
        v-if="output?.truncated"
        type="warning"
        :title="t('app.taskScheduler.runs.outputTruncated')"
        show-icon
      />
      <pre v-if="output">{{ output.content }}</pre>
      <VustEmpty v-else-if="outputState.error" :description="outputState.error" />
      <VustLoading :loading="outputState.initialLoading" cover />
    </div>
  </VustDrawer>
</template>

<style scoped>
.detail-content,
.command-detail,
.output-content {
  position: relative;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
}

.runs-shell {
  position: relative;
  min-height: 260px;
  overflow: auto;
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-panel);
}

.command-detail pre,
.output-content pre {
  margin: 0;
  padding: var(--vdl-space-3);
  overflow: auto;
  white-space: pre-wrap;
  word-break: break-word;
  border: 1px solid var(--vdl-border-subtle);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-subtle);
  color: var(--vdl-text-primary);
  font-family: var(--vdl-font-mono);
}

.output-content {
  height: 100%;
}

.output-content pre {
  flex: 1;
  min-height: 240px;
}

.block-title {
  color: var(--vdl-text-primary);
  font-weight: 600;
}

.run-phase {
  display: block;
  margin-top: var(--vdl-space-1);
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-caption);
}

.inline-actions {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
}
</style>
