<script setup lang="ts">
/**
 * @file ScheduledTaskFormDrawer.vue
 * @description 计划任务创建/编辑表单；编辑模式明确锁定执行节点。
 */

import { reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ScheduledTaskDetail } from '@/api/generated/scheduled-tasks'
import { useCronBuilder } from '@/composables/useCronBuilder'
import {
  VustAlert,
  VustButton,
  VustDrawer,
  VustFormItem,
  VustInput,
  VustSelect,
  VustSwitch,
  VustTabs,
} from '@/components/ui'

export interface ScheduledTaskFormValue {
  name: string
  description?: string
  nodeId: string
  cronExpr: string
  timeZone: string
  command: string
  timeoutSeconds: number
  preventOverlap: boolean
  enabled: boolean
}

const props = defineProps<{
  modelValue: boolean
  mode: 'create' | 'edit'
  detail: ScheduledTaskDetail | null
  defaultNodeId: string
  nodeOptions: Array<{ value: string | number; label: string }>
  saving: boolean
}>()
const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  submit: [value: ScheduledTaskFormValue]
}>()

const { t } = useI18n()
const validationError = ref('')
const form = reactive({
  name: '',
  description: '',
  nodeId: 'local',
  timeZone: 'Asia/Shanghai',
  command: '',
  timeoutSeconds: 30,
  preventOverlap: true,
  enabled: true,
})
const {
  cronMode,
  advancedCronExpr,
  simpleCron,
  weekdayOptions,
  cronExprForSubmit,
  cronSummary,
  switchCronMode,
  loadCron,
  resetCron,
} = useCronBuilder()

/** 每次打开按模式装载表单，避免上一个任务的数据残留。 */
const initialize = () => {
  validationError.value = ''
  if (props.mode === 'edit' && props.detail) {
    form.name = props.detail.name
    form.description = props.detail.description ?? ''
    form.nodeId = props.detail.node.nodeId
    form.timeZone = props.detail.schedule.timeZone
    form.command = props.detail.execution.command
    form.timeoutSeconds = props.detail.execution.timeoutSeconds
    form.preventOverlap = props.detail.execution.preventOverlap
    form.enabled = props.detail.desiredState === 'enabled'
    loadCron(props.detail.schedule.cronExpr)
    return
  }
  form.name = ''
  form.description = ''
  form.nodeId = props.defaultNodeId || 'local'
  form.timeZone = 'Asia/Shanghai'
  form.command = ''
  form.timeoutSeconds = 30
  form.preventOverlap = true
  form.enabled = true
  resetCron()
}

watch(
  () => [props.modelValue, props.mode, props.detail?.taskId, props.defaultNodeId],
  () => {
    if (props.modelValue) initialize()
  },
)

const close = () => emit('update:modelValue', false)
const submit = () => {
  validationError.value = ''
  if (!form.name.trim()) validationError.value = t('app.taskScheduler.messages.nameRequired')
  else if (!form.nodeId) validationError.value = t('app.taskScheduler.messages.nodeRequired')
  else if (!form.timeZone.trim())
    validationError.value = t('app.taskScheduler.messages.timeZoneRequired')
  else if (cronExprForSubmit.value.trim().split(/\s+/).length !== 5)
    validationError.value = t('app.taskScheduler.messages.invalidCron')
  else if (!form.command.trim())
    validationError.value = t('app.taskScheduler.messages.commandRequired')
  if (validationError.value) return

  emit('submit', {
    name: form.name.trim(),
    description: form.description.trim() || undefined,
    nodeId: form.nodeId,
    cronExpr: cronExprForSubmit.value,
    timeZone: form.timeZone.trim(),
    command: form.command,
    timeoutSeconds: Number(form.timeoutSeconds),
    preventOverlap: form.preventOverlap,
    enabled: form.enabled,
  })
}
</script>

<template>
  <VustDrawer
    :model-value="modelValue"
    data-ui="form"
    :title="
      mode === 'create'
        ? t('app.taskScheduler.form.createTitle')
        : t('app.taskScheduler.form.editTitle')
    "
    width="680px"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <div class="task-form" data-slot="content">
      <VustAlert v-if="validationError" type="error" :title="validationError" show-icon />
      <div class="form-grid">
        <VustFormItem :label="t('app.taskScheduler.form.name')" for="scheduled-task-name" required>
          <VustInput
            id="scheduled-task-name"
            v-model="form.name"
            name="scheduledTaskName"
            :maxlength="80"
          />
        </VustFormItem>
        <VustFormItem :label="t('app.taskScheduler.form.node')" for="scheduled-task-node" required>
          <VustSelect
            id="scheduled-task-node"
            v-model="form.nodeId"
            name="scheduledTaskNode"
            :options="nodeOptions"
            :disabled="mode === 'edit'"
          />
          <span v-if="mode === 'edit'" class="field-hint">
            {{ t('app.taskScheduler.form.nodeMigrationHint') }}
          </span>
        </VustFormItem>
        <VustFormItem
          class="full-row"
          :label="t('app.taskScheduler.form.description')"
          for="scheduled-task-description"
        >
          <VustInput
            id="scheduled-task-description"
            v-model="form.description"
            name="scheduledTaskDescription"
            type="textarea"
            :rows="2"
            :maxlength="500"
          />
        </VustFormItem>
        <VustFormItem
          :label="t('app.taskScheduler.form.timeZone')"
          for="scheduled-task-time-zone"
          required
          :hint="t('app.taskScheduler.form.timeZoneHint')"
        >
          <VustInput
            id="scheduled-task-time-zone"
            v-model="form.timeZone"
            name="scheduledTaskTimeZone"
            placeholder="Asia/Shanghai"
          />
        </VustFormItem>
        <VustFormItem
          :label="t('app.taskScheduler.form.timeout')"
          for="scheduled-task-timeout"
          required
        >
          <VustInput
            id="scheduled-task-timeout"
            v-model="form.timeoutSeconds"
            name="scheduledTaskTimeout"
            type="number"
            :min="1"
            :max="86400"
          />
        </VustFormItem>
      </div>

      <div class="schedule-editor" data-ui="schedule-form">
        <VustTabs
          v-model="cronMode"
          :tabs="[
            { name: 'simple', label: t('app.taskScheduler.form.simpleMode') },
            { name: 'advanced', label: t('app.taskScheduler.form.advancedMode') },
          ]"
          @change="(modeValue) => switchCronMode(modeValue as 'simple' | 'advanced')"
        />
        <div v-if="cronMode === 'simple'" class="form-grid schedule-fields">
          <VustFormItem
            :label="t('app.taskScheduler.form.simple.type')"
            for="scheduled-task-frequency"
          >
            <VustSelect
              id="scheduled-task-frequency"
              v-model="simpleCron.type"
              name="scheduledTaskFrequency"
              :options="[
                {
                  value: 'every_minutes',
                  label: t('app.taskScheduler.form.simple.everyMinutes'),
                },
                { value: 'hourly', label: t('app.taskScheduler.form.simple.hourly') },
                { value: 'daily', label: t('app.taskScheduler.form.simple.daily') },
                { value: 'weekly', label: t('app.taskScheduler.form.simple.weekly') },
              ]"
            />
          </VustFormItem>
          <VustFormItem
            v-if="simpleCron.type === 'every_minutes'"
            :label="t('app.taskScheduler.form.simple.intervalMinutes')"
            for="scheduled-task-interval"
          >
            <VustInput
              id="scheduled-task-interval"
              v-model="simpleCron.intervalMinutes"
              name="scheduledTaskInterval"
              type="number"
              :min="1"
              :max="59"
            />
          </VustFormItem>
          <VustFormItem
            v-if="simpleCron.type !== 'every_minutes'"
            :label="t('app.taskScheduler.form.simple.minute')"
            for="scheduled-task-minute"
          >
            <VustInput
              id="scheduled-task-minute"
              v-model="simpleCron.minute"
              name="scheduledTaskMinute"
              type="number"
              :min="0"
              :max="59"
            />
          </VustFormItem>
          <VustFormItem
            v-if="simpleCron.type === 'daily' || simpleCron.type === 'weekly'"
            :label="t('app.taskScheduler.form.simple.hour')"
            for="scheduled-task-hour"
          >
            <VustInput
              id="scheduled-task-hour"
              v-model="simpleCron.hour"
              name="scheduledTaskHour"
              type="number"
              :min="0"
              :max="23"
            />
          </VustFormItem>
          <VustFormItem
            v-if="simpleCron.type === 'weekly'"
            :label="t('app.taskScheduler.form.simple.weekday')"
            for="scheduled-task-weekday"
          >
            <VustSelect
              id="scheduled-task-weekday"
              v-model="simpleCron.weekday"
              name="scheduledTaskWeekday"
              :options="weekdayOptions"
            />
          </VustFormItem>
        </div>
        <VustFormItem
          v-else
          :label="t('app.taskScheduler.form.cron')"
          for="scheduled-task-cron"
          :hint="t('app.taskScheduler.form.cronHint')"
        >
          <VustInput
            id="scheduled-task-cron"
            v-model="advancedCronExpr"
            name="scheduledTaskCron"
            :placeholder="t('app.taskScheduler.form.cronPlaceholder')"
          />
        </VustFormItem>
        <div class="schedule-preview">
          <span>{{ cronSummary }}</span>
          <code>{{ cronExprForSubmit }}</code>
        </div>
      </div>

      <VustFormItem
        :label="t('app.taskScheduler.form.command')"
        for="scheduled-task-command"
        required
        :hint="t('app.taskScheduler.form.commandHint')"
      >
        <VustInput
          id="scheduled-task-command"
          v-model="form.command"
          name="scheduledTaskCommand"
          type="textarea"
          :rows="8"
          :maxlength="65536"
        />
      </VustFormItem>
      <div class="switch-grid">
        <VustFormItem
          v-if="mode === 'create'"
          :label="t('app.taskScheduler.form.enabled')"
          for="scheduled-task-enabled"
        >
          <VustSwitch
            id="scheduled-task-enabled"
            v-model="form.enabled"
            name="scheduledTaskEnabled"
          />
        </VustFormItem>
        <VustFormItem
          :label="t('app.taskScheduler.form.preventOverlap')"
          for="scheduled-task-prevent-overlap"
        >
          <VustSwitch
            id="scheduled-task-prevent-overlap"
            v-model="form.preventOverlap"
            name="scheduledTaskPreventOverlap"
          />
        </VustFormItem>
      </div>
    </div>
    <template #footer>
      <div class="drawer-actions" data-slot="footer">
        <VustButton @click="close">{{ t('common.cancel') }}</VustButton>
        <VustButton type="primary" :loading="saving" @click="submit">
          {{ t('common.save') }}
        </VustButton>
      </div>
    </template>
  </VustDrawer>
</template>

<style scoped>
.task-form {
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
}

.form-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 0 var(--vdl-space-4);
}

.full-row {
  grid-column: 1 / -1;
}

.field-hint {
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-caption);
}

.schedule-editor {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-3);
  border: 1px solid var(--vdl-border-subtle);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-subtle);
}

.schedule-fields {
  align-items: end;
}

.schedule-preview {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-2) var(--vdl-space-3);
  border-left: 3px solid var(--vdl-primary);
  border-radius: var(--vdl-radius-sm);
  background: var(--vdl-bg-panel);
  color: var(--vdl-text-secondary);
}

.schedule-preview code {
  font-family: var(--vdl-font-mono);
}

.switch-grid,
.drawer-actions {
  display: flex;
  align-items: flex-start;
  gap: var(--vdl-space-6);
}

.drawer-actions {
  justify-content: flex-end;
}

@media (max-width: 620px) {
  .form-grid {
    grid-template-columns: minmax(0, 1fr);
  }

  .full-row {
    grid-column: auto;
  }
}
</style>
