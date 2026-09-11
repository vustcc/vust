<script setup lang="ts">
/**
 * @file SystemMonitoringSettings.vue
 * @description 系统监控采集与历史生命周期设置。
 */

import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { SystemMonitoringSettings } from '@/api/generated'
import { systemMonitoringApi } from '@/api/modules/systemMonitoring'
import { useNodeStore } from '@/stores/node'
import {
  VustAlert,
  VustButton,
  VustCard,
  VustDialog,
  VustFormItem,
  VustLoading,
  VustSelect,
  VustSwitch,
  VustTag,
} from '@/components/ui'

const emit = defineEmits<{
  busyChange: [busy: boolean]
}>()

const { t } = useI18n()
const nodeStore = useNodeStore()
const settings = ref<SystemMonitoringSettings | null>(null)
const loading = ref(false)
const saving = ref(false)
const clearing = ref(false)
const errorText = ref('')
const showClearDialog = ref(false)
const form = ref({
  historyCollectionEnabled: true,
  retentionDays: 7 as 1 | 3 | 7,
})

const nodeId = computed(() => nodeStore.currentNodeId || 'local')
const busy = computed(() => loading.value || saving.value || clearing.value)
const canManage = computed(() => settings.value?.capabilities.canManageCollection === true)
const canClear = computed(() => settings.value?.capabilities.canClearHistory === true)
const retentionOptions = computed(() => [
  { value: 1, label: t('app.settings.monitoring.retentionOption', { days: 1 }) },
  { value: 3, label: t('app.settings.monitoring.retentionOption', { days: 3 }) },
  { value: 7, label: t('app.settings.monitoring.retentionOption', { days: 7 }) },
])

watch(busy, (value) => emit('busyChange', value), { immediate: true })

const applySettings = (value: SystemMonitoringSettings) => {
  settings.value = value
  form.value = {
    historyCollectionEnabled: value.historyCollectionEnabled,
    retentionDays: value.retentionDays as 1 | 3 | 7,
  }
}

/** 加载当前节点的监控设置。 */
const loadSettings = async () => {
  loading.value = true
  errorText.value = ''
  try {
    const response = await systemMonitoringApi.fetchSettings(nodeId.value)
    if (!response.success || !response.data) {
      throw new Error(response.message || t('app.settings.monitoring.loadFailed'))
    }
    applySettings(response.data)
  } catch (error) {
    errorText.value =
      error instanceof Error ? error.message : t('app.settings.monitoring.loadFailed')
  } finally {
    loading.value = false
  }
}

/** 保存采集开关与保留期。 */
const saveSettings = async () => {
  if (!canManage.value || saving.value) return
  saving.value = true
  errorText.value = ''
  try {
    const response = await systemMonitoringApi.updateSettings(nodeId.value, form.value)
    if (!response.success || !response.data) {
      throw new Error(response.message || t('app.settings.monitoring.saveFailed'))
    }
    applySettings(response.data)
  } catch (error) {
    if (settings.value) applySettings(settings.value)
    errorText.value =
      error instanceof Error ? error.message : t('app.settings.monitoring.saveFailed')
  } finally {
    saving.value = false
  }
}

/** 清空当前节点历史并刷新存储摘要。 */
const clearHistory = async () => {
  if (!canClear.value || clearing.value) return
  showClearDialog.value = false
  clearing.value = true
  errorText.value = ''
  try {
    const response = await systemMonitoringApi.clearHistory(nodeId.value)
    if (!response.success) {
      throw new Error(response.message || t('app.settings.monitoring.clearFailed'))
    }
    await loadSettings()
  } catch (error) {
    errorText.value =
      error instanceof Error ? error.message : t('app.settings.monitoring.clearFailed')
  } finally {
    clearing.value = false
  }
}

watch(nodeId, () => void loadSettings())
onMounted(() => void loadSettings())
</script>

<template>
  <div class="monitoring-settings" data-page="settings-monitoring" data-ui="monitoring-settings">
    <VustCard class="monitoring-card" shadow="never">
      <template #header>
        <div class="header-row" data-slot="header">
          <h2>{{ t('app.settings.monitoring.label') }}</h2>
          <VustTag v-if="settings" :type="form.historyCollectionEnabled ? 'success' : 'default'">
            {{
              form.historyCollectionEnabled
                ? t('app.settings.monitoring.running')
                : t('app.settings.monitoring.stopped')
            }}
          </VustTag>
        </div>
      </template>

      <div class="settings-body" data-slot="form">
        <p class="description">{{ t('app.settings.monitoring.description') }}</p>
        <VustAlert v-if="errorText" type="error" :title="errorText" show-icon />
        <VustAlert
          v-if="settings && !canManage"
          type="info"
          :title="t('app.settings.monitoring.readOnly')"
          show-icon
        />

        <div v-if="settings" class="monitoring-form">
          <VustFormItem
            :label="t('app.settings.monitoring.collection')"
            for="system-monitoring-collection"
            :hint="t('app.settings.monitoring.collectionHint')"
          >
            <VustSwitch
              v-if="canManage"
              id="system-monitoring-collection"
              v-model="form.historyCollectionEnabled"
              name="systemMonitoringCollection"
              :disabled="saving"
            />
            <span v-else>{{
              form.historyCollectionEnabled ? t('common.enabled') : t('common.disabled')
            }}</span>
          </VustFormItem>

          <VustFormItem
            :label="t('app.settings.monitoring.retention')"
            for="system-monitoring-retention"
            :hint="t('app.settings.monitoring.retentionHint')"
          >
            <VustSelect
              id="system-monitoring-retention"
              v-model="form.retentionDays"
              name="systemMonitoringRetention"
              :aria-label="t('app.settings.monitoring.retention')"
              :options="retentionOptions"
              :disabled="!canManage || saving"
            />
          </VustFormItem>

          <div class="storage-summary" data-ui="storage-summary">
            <span>{{ t('app.settings.monitoring.sampleInterval') }}</span>
            <strong>{{
              t('app.settings.monitoring.seconds', {
                seconds: settings.historySampleIntervalSeconds,
              })
            }}</strong>
            <span>{{ t('app.settings.monitoring.storedSamples') }}</span>
            <strong>{{ settings.storedSampleCount.toLocaleString() }}</strong>
            <template v-if="settings.oldestSampledAt">
              <span>{{ t('app.settings.monitoring.oldestSample') }}</span>
              <strong>{{ new Date(settings.oldestSampledAt).toLocaleString() }}</strong>
            </template>
            <template v-if="settings.newestSampledAt">
              <span>{{ t('app.settings.monitoring.newestSample') }}</span>
              <strong>{{ new Date(settings.newestSampledAt).toLocaleString() }}</strong>
            </template>
          </div>

          <div class="form-actions" data-slot="actions">
            <VustButton
              v-if="canManage"
              type="primary"
              :loading="saving"
              :disabled="clearing"
              @click="saveSettings"
            >
              {{ t('common.save') }}
            </VustButton>
            <VustButton
              v-if="canClear"
              type="danger"
              :loading="clearing"
              :disabled="saving"
              @click="showClearDialog = true"
            >
              {{ t('app.settings.monitoring.clearHistory') }}
            </VustButton>
          </div>
        </div>

        <VustLoading :loading="loading" :text="t('app.settings.monitoring.loading')" cover />
      </div>
    </VustCard>

    <VustDialog
      :visible="showClearDialog"
      :title="t('app.settings.monitoring.clearTitle')"
      width="440px"
      @close="showClearDialog = false"
    >
      <p>{{ t('app.settings.monitoring.clearConfirm') }}</p>
      <template #footer>
        <VustButton type="secondary" @click="showClearDialog = false">
          {{ t('common.cancel') }}
        </VustButton>
        <VustButton type="danger" @click="clearHistory">
          {{ t('common.confirm') }}
        </VustButton>
      </template>
    </VustDialog>
  </div>
</template>

<style scoped>
.monitoring-settings,
.monitoring-card {
  width: 100%;
  height: 100%;
  min-height: 0;
}

.monitoring-card :deep(.vl-card-content) {
  height: 100%;
  min-height: 0;
  box-sizing: border-box;
}

.header-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-3);
}

.header-row h2 {
  margin: 0;
  color: var(--vdl-text-primary);
  font-size: var(--vdl-font-subtitle);
}

.settings-body {
  position: relative;
  min-height: 0;
  height: 100%;
  overflow-y: auto;
}

.description {
  margin: 0 0 var(--vdl-space-4);
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-body-sm);
}

.monitoring-form {
  max-width: 620px;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
  margin-top: var(--vdl-space-4);
}

.storage-summary {
  display: grid;
  grid-template-columns: minmax(140px, auto) minmax(0, 1fr);
  gap: var(--vdl-space-2) var(--vdl-space-4);
  padding: var(--vdl-space-3);
  border: 1px solid var(--vdl-border-subtle);
  border-radius: var(--vdl-radius-md);
}

.storage-summary span {
  color: var(--vdl-text-muted);
}

.storage-summary strong {
  color: var(--vdl-text-primary);
  font-weight: 500;
  word-break: break-word;
}

.form-actions {
  display: flex;
  gap: var(--vdl-space-2);
  flex-wrap: wrap;
}

@media (max-width: 640px) {
  .storage-summary {
    grid-template-columns: 1fr;
  }
}
</style>
