<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type {
  SuiteCatalogItem,
  SuiteInstallTaskResponse,
  SuiteInstanceSummary,
} from '@/api/interface/suites'
import { VustActionMenu, VustButton, VustDescriptions, VustDrawer, VustTag } from '@/components/ui'
import AppIcon from '@/components/icons/AppIcon.vue'
import SuiteInstallProgress from './SuiteInstallProgress.vue'

const props = defineProps<{
  suite: SuiteCatalogItem | null
  instance?: SuiteInstanceSummary
  task?: SuiteInstallTaskResponse
  statusLabel: string
  statusType: 'primary' | 'success' | 'warning' | 'danger' | 'info' | 'default'
  nodeName: string
  busy: boolean
  nodeUnavailable: boolean
  pollingError?: string
}>()

const emit = defineEmits<{
  close: []
  install: []
  enable: []
  disable: []
  uninstall: []
  delete: []
  cancel: [taskId: string]
  retry: [taskId: string]
}>()

const { t } = useI18n()
const visible = computed({
  get: () => props.suite !== null,
  set: (value) => {
    if (!value) emit('close')
  },
})
const canEnable = computed(
  () =>
    !!props.instance &&
    ['installed', 'disabled', 'error'].includes(props.instance.status) &&
    !props.task,
)
const canInstall = computed(() => !props.instance && !props.task)
const descriptions = computed(() => {
  if (!props.suite) return []
  return [
    { label: t('app.suiteCenter.fields.version'), value: props.suite.version },
    { label: t('app.suiteCenter.fields.node'), value: props.nodeName },
    { label: t('app.suiteCenter.fields.category'), value: props.suite.category || 'other' },
    { label: t('app.suiteCenter.fields.instanceCount'), value: props.suite.instanceCount },
    ...(props.instance
      ? [
          { label: t('app.suiteCenter.fields.instanceId'), value: props.instance.instanceId },
          {
            label: t('app.suiteCenter.fields.projectName'),
            value: props.instance.composeProjectName,
          },
        ]
      : []),
  ]
})
const secondaryActions = computed(() => {
  if (!props.suite) return []
  const actions: Array<{
    label: string
    icon?: string
    className?: string
    disabled?: boolean
    handler: () => void
  }> = []
  if (props.instance?.status === 'enabled') {
    actions.push({
      label: t('app.suiteCenter.actions.disable'),
      icon: 'pause',
      disabled: props.busy,
      handler: () => emit('disable'),
    })
  }
  if (props.instance) {
    actions.push({
      label: t('app.suiteCenter.actions.uninstall'),
      icon: 'trash',
      className: 'is-danger',
      disabled: props.busy || !!props.task,
      handler: () => emit('uninstall'),
    })
  } else if (props.suite.instanceCount === 0) {
    actions.push({
      label: t('app.suiteCenter.actions.deletePackage'),
      icon: 'trash',
      className: 'is-danger',
      disabled: props.busy,
      handler: () => emit('delete'),
    })
  }
  return actions
})
</script>

<template>
  <VustDrawer
    v-model="visible"
    :title="suite?.name || ''"
    width="min(36rem, 100vw)"
    data-ui="suite-detail-drawer"
  >
    <div v-if="suite" class="suite-detail" data-slot="suite-detail-content">
      <div class="suite-detail__head">
        <AppIcon :name="suite.icon" :size="52" :label="suite.name" />
        <div class="suite-detail__identity">
          <div class="suite-detail__title">{{ suite.name }}</div>
          <div class="suite-detail__id">{{ suite.suiteId }}</div>
        </div>
        <VustTag :type="statusType" effect="plain">{{ statusLabel }}</VustTag>
      </div>
      <p class="suite-detail__summary">{{ suite.summary }}</p>
      <VustDescriptions :items="descriptions" :column="1" border />
      <div v-if="instance?.lastError" class="suite-detail__error" role="alert">
        {{ instance.lastError }}
      </div>
      <SuiteInstallProgress
        v-if="task"
        :task="task"
        :error="pollingError"
        @cancel="$emit('cancel', task.taskId)"
        @retry="$emit('retry', task.taskId)"
      />
    </div>
    <template #footer>
      <div class="suite-detail__actions" data-slot="suite-detail-actions">
        <VustButton
          v-if="canInstall"
          type="primary"
          :loading="busy"
          :disabled="nodeUnavailable"
          data-ui="suite-install-action"
          @click="$emit('install')"
        >
          {{ t('app.suiteCenter.actions.install') }}
        </VustButton>
        <VustButton
          v-else-if="canEnable"
          type="primary"
          :loading="busy"
          :disabled="nodeUnavailable"
          data-ui="suite-enable-action"
          @click="$emit('enable')"
        >
          {{ t('app.suiteCenter.actions.enable') }}
        </VustButton>
        <VustActionMenu
          v-if="secondaryActions.length"
          :actions="secondaryActions"
          :label="t('common.more')"
          :disabled="busy"
          data-ui="suite-more-actions"
        />
      </div>
    </template>
  </VustDrawer>
</template>

<style scoped>
.suite-detail {
  display: grid;
  gap: var(--vdl-space-5);
  min-width: 0;
}

.suite-detail__head,
.suite-detail__actions {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-3);
  min-width: 0;
}

.suite-detail__identity {
  min-width: 0;
  flex: 1;
}

.suite-detail__title {
  overflow: hidden;
  color: var(--vdl-text-primary);
  font: var(--vdl-font-title);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.suite-detail__id,
.suite-detail__summary {
  color: var(--vdl-text-muted);
  font: var(--vdl-font-body-sm);
}

.suite-detail__id {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.suite-detail__summary {
  margin: 0;
  line-height: 1.6;
}

.suite-detail__error {
  padding: var(--vdl-space-3);
  border-radius: var(--vdl-radius-md);
  color: var(--vdl-danger);
  font: var(--vdl-font-body-sm);
  background: var(--vdl-danger-soft);
}

.suite-detail__actions {
  justify-content: flex-end;
  flex-wrap: wrap;
  width: 100%;
}
</style>
