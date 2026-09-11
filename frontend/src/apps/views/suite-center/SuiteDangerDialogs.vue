<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { SuiteCatalogItem, SuiteInstanceSummary } from '@/api/interface/suites'
import { VustButton, VustCheckbox, VustDialog } from '@/components/ui'

const props = defineProps<{
  deleteTarget: SuiteCatalogItem | null
  uninstallSuite: SuiteCatalogItem | null
  uninstallInstance?: SuiteInstanceSummary
  busy: boolean
}>()

defineEmits<{
  closeDelete: []
  confirmDelete: []
  closeUninstall: []
  confirmUninstall: [removeData: boolean]
}>()

const { t } = useI18n()
const removeData = ref(false)

watch(
  () => props.uninstallSuite,
  () => {
    removeData.value = false
  },
)
</script>

<template>
  <VustDialog
    :visible="!!deleteTarget"
    :title="t('app.suiteCenter.deleteTitle')"
    width="30rem"
    :close-on-click-overlay="!busy"
    @close="$emit('closeDelete')"
  >
    <div class="danger-dialog" data-ui="suite-delete-confirmation">
      {{ t('app.suiteCenter.deleteConfirm', { name: deleteTarget?.name }) }}
    </div>
    <template #footer>
      <VustButton type="secondary" :disabled="busy" @click="$emit('closeDelete')">
        {{ t('common.cancel') }}
      </VustButton>
      <VustButton type="danger" :loading="busy" @click="$emit('confirmDelete')">
        {{ t('common.delete') }}
      </VustButton>
    </template>
  </VustDialog>

  <VustDialog
    :visible="!!uninstallSuite && !!uninstallInstance"
    :title="t('app.suiteCenter.uninstallTitle')"
    width="32rem"
    :close-on-click-overlay="!busy"
    @close="$emit('closeUninstall')"
  >
    <div class="danger-dialog" data-ui="suite-uninstall-confirmation">
      <p>{{ t('app.suiteCenter.uninstallConfirm', { name: uninstallSuite?.name }) }}</p>
      <VustCheckbox v-model="removeData" :disabled="busy">
        {{ t('app.suiteCenter.removeData') }}
      </VustCheckbox>
      <div v-if="removeData" class="danger-dialog__warning" role="alert">
        <strong>{{ t('app.suiteCenter.removeDataSecondWarningTitle') }}</strong>
        <span>{{ t('app.suiteCenter.removeDataWarning') }}</span>
      </div>
    </div>
    <template #footer>
      <VustButton type="secondary" :disabled="busy" @click="$emit('closeUninstall')">
        {{ t('common.cancel') }}
      </VustButton>
      <VustButton
        type="danger"
        :loading="busy"
        data-ui="suite-confirm-uninstall"
        @click="$emit('confirmUninstall', removeData)"
      >
        {{
          removeData
            ? t('app.suiteCenter.uninstallAndRemoveData')
            : t('app.suiteCenter.actions.uninstall')
        }}
      </VustButton>
    </template>
  </VustDialog>
</template>

<style scoped>
.danger-dialog {
  display: grid;
  gap: var(--vdl-space-4);
  color: var(--vdl-text-secondary);
  font: var(--vdl-font-body);
}

.danger-dialog p {
  margin: 0;
}

.danger-dialog__warning {
  display: grid;
  gap: var(--vdl-space-2);
  padding: var(--vdl-space-3);
  border: 1px solid var(--vdl-danger);
  border-radius: var(--vdl-radius-md);
  color: var(--vdl-danger);
  background: var(--vdl-danger-soft);
}
</style>
