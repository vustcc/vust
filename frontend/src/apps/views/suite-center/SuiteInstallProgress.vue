<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { SuiteInstallTaskResponse } from '@/api/interface/suites'
import { VustButton, VustTag } from '@/components/ui'

const props = defineProps<{
  task: SuiteInstallTaskResponse
  error?: string
}>()

defineEmits<{
  cancel: []
  retry: []
}>()

const { t } = useI18n()
const percent = computed(() => Math.max(0, Math.min(100, props.task.progressPercent)))
const statusType = computed(() => {
  if (props.task.status === 'success') return 'success'
  if (props.task.status === 'failed') return 'danger'
  if (props.task.status === 'canceled') return 'default'
  if (props.task.status === 'canceling') return 'warning'
  return 'primary'
})

function stepLabel() {
  const key = `app.suiteCenter.installProgress.steps.${props.task.currentStep}`
  const label = t(key)
  return label === key ? t('app.suiteCenter.status.unknown') : label
}
</script>

<template>
  <div class="install-task" data-ui="suite-install-task" :data-slot="task.taskId">
    <div class="install-task__header">
      <div>
        <div class="install-task__step">{{ stepLabel() }}</div>
        <div v-if="task.currentImage" class="install-task__image">{{ task.currentImage }}</div>
      </div>
      <VustTag :type="statusType" effect="plain">{{ percent }}%</VustTag>
    </div>
    <div
      class="install-task__track"
      role="progressbar"
      :aria-label="t('app.suiteCenter.installProgress.ariaLabel')"
      aria-valuemin="0"
      aria-valuemax="100"
      :aria-valuenow="percent"
    >
      <div class="install-task__bar" :style="{ width: `${percent}%` }" />
    </div>
    <div v-if="error || task.error" class="install-task__error" role="alert">
      <span>{{ error || task.error }}</span>
      <VustButton v-if="error" size="small" type="secondary" @click="$emit('retry')">
        {{ t('common.retry') }}
      </VustButton>
    </div>
    <div v-if="!task.isFinished" class="install-task__actions">
      <VustButton
        size="small"
        type="secondary"
        :disabled="task.cancelRequested"
        data-ui="suite-install-cancel"
        @click="$emit('cancel')"
      >
        {{
          task.cancelRequested
            ? t('app.suiteCenter.installProgress.canceling')
            : t('app.suiteCenter.installProgress.cancel')
        }}
      </VustButton>
    </div>
  </div>
</template>

<style scoped>
.install-task {
  display: grid;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-4);
  border: 1px solid var(--vdl-border-subtle);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-muted);
}

.install-task__header,
.install-task__error,
.install-task__actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-3);
}

.install-task__step {
  color: var(--vdl-text-primary);
  font: var(--vdl-font-body);
}

.install-task__image {
  overflow: hidden;
  max-width: 34rem;
  color: var(--vdl-text-muted);
  font: var(--vdl-font-code);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.install-task__track {
  overflow: hidden;
  height: var(--vdl-space-2);
  border-radius: var(--vdl-radius-pill);
  background: var(--vdl-border-default);
}

.install-task__bar {
  height: 100%;
  border-radius: inherit;
  background: var(--vdl-primary);
  transition: width 180ms ease;
}

.install-task__error {
  align-items: flex-start;
  color: var(--vdl-danger);
  font: var(--vdl-font-body-sm);
}

.install-task__actions {
  justify-content: flex-end;
}
</style>
