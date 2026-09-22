<script setup lang="ts">
/** 套件中心内容区导入工作台。 */
import { nextTick, onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { VustButton } from '@/components/ui'
import VustIcon from '@/components/icons/VustIcon.vue'
import { validateSuitePackage } from './useSuiteCenter'

const props = defineProps<{
  busy: boolean
}>()

const emit = defineEmits<{
  back: []
  submit: [file: File]
}>()

const { t } = useI18n()
const titleElement = ref<HTMLHeadingElement | null>(null)
const fileInput = ref<HTMLInputElement | null>(null)
const selectedFile = ref<File | null>(null)
const validationError = ref('')

/** 工作台进入内容区后将键盘焦点移动到标题。 */
onMounted(() => {
  titleElement.value?.focus({ preventScroll: true })
})

/** 工作台离开后将焦点恢复到套件导入入口。 */
onBeforeUnmount(() => {
  void nextTick(() => {
    document.querySelector<HTMLElement>('[data-ui="suite-import"]')?.focus({ preventScroll: true })
  })
})

/** 在未上传时返回套件列表。 */
function handleBack() {
  if (!props.busy) emit('back')
}

/** 打开原生套件文件选择器。 */
function openFilePicker() {
  if (!props.busy) fileInput.value?.click()
}

/** 接收并校验用户选择的套件包。 */
function handleFile(event: Event) {
  selectedFile.value = (event.target as HTMLInputElement).files?.[0] ?? null
  validationError.value = ''
  if (!selectedFile.value) return
  const validation = validateSuitePackage(selectedFile.value)
  if (!validation.valid) {
    validationError.value = t(`app.suiteCenter.importValidation.${validation.reason}`)
  }
}

/** 提交已通过本地校验的套件包。 */
function submit() {
  if (selectedFile.value && !validationError.value && !props.busy) {
    emit('submit', selectedFile.value)
  }
}
</script>

<template>
  <section class="import-workspace" data-ui="suite-import-view">
    <header class="import-workspace__header" data-slot="header">
      <VustButton
        type="secondary"
        :disabled="busy"
        data-ui="suite-import-header-back"
        @click="handleBack"
      >
        {{ t('common.back') }}
      </VustButton>
      <h2 ref="titleElement" tabindex="-1">{{ t('app.suiteCenter.importTitle') }}</h2>
    </header>

    <div class="import-workspace__body" data-slot="body">
      <div class="import-workspace__picker" data-slot="package-picker">
        <VustIcon class="import-workspace__package-icon" name="package" :size="32" />
        <div class="import-workspace__info">
          <div class="import-workspace__filename" data-slot="selected-package">
            {{ selectedFile?.name || t('app.suiteCenter.noPackageSelected') }}
          </div>
          <div class="import-workspace__hint">{{ t('app.suiteCenter.importFormatHint') }}</div>
        </div>
        <div v-if="validationError" class="import-workspace__error" role="alert">
          {{ validationError }}
        </div>
        <input
          id="suite-package-file"
          ref="fileInput"
          class="import-workspace__native-input"
          name="suitePackageFile"
          type="file"
          accept=".vsp"
          :disabled="busy"
          :aria-label="t('app.suiteCenter.selectPackage')"
          @change="handleFile"
        />
        <VustButton
          class="import-workspace__select-button"
          :disabled="busy"
          data-ui="suite-file-select"
          @click="openFilePicker"
        >
          <VustIcon name="package" :size="14" />
          {{ t('app.suiteCenter.selectPackage') }}
        </VustButton>
      </div>
    </div>

    <footer class="import-workspace__footer" data-slot="footer">
      <VustButton
        type="primary"
        :disabled="!selectedFile || !!validationError || busy"
        :loading="busy"
        data-ui="suite-confirm-import"
        @click="submit"
      >
        {{ t('common.import') }}
      </VustButton>
    </footer>
  </section>
</template>

<style scoped>
.import-workspace {
  display: flex;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  overflow: hidden;
  background: var(--vdl-bg-panel);
}

.import-workspace__header,
.import-workspace__footer {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-4) var(--vdl-space-5);
}

.import-workspace__header {
  border-bottom: 1px solid var(--vdl-border-subtle);
}

.import-workspace__header h2 {
  margin: 0;
  color: var(--vdl-text-primary);
  font: var(--vdl-font-title);
}

.import-workspace__body {
  display: flex;
  min-height: 0;
  flex: 1;
  overflow: auto;
  padding: var(--vdl-space-5);
  scrollbar-gutter: stable;
}

.import-workspace__footer {
  justify-content: flex-end;
  border-top: 1px solid var(--vdl-border-subtle);
}

.import-workspace__picker {
  display: flex;
  min-width: 0;
  min-height: 16rem;
  flex: 1;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--vdl-space-4);
  padding: var(--vdl-space-5);
  border: 1px solid var(--vdl-border-subtle);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-muted);
  text-align: center;
}

.import-workspace__info {
  width: 100%;
  min-width: 0;
}

.import-workspace__package-icon {
  flex-shrink: 0;
  color: var(--vdl-primary);
}

.import-workspace__filename {
  overflow: hidden;
  color: var(--vdl-text-primary);
  font: var(--vdl-font-body);
  font-weight: var(--vdl-font-weight-semibold);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.import-workspace__select-button :deep(.vl-button-content) {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: var(--vdl-space-2);
  line-height: 1;
}

.import-workspace__native-input {
  display: none;
}

.import-workspace__hint,
.import-workspace__error {
  color: var(--vdl-text-muted);
  font: var(--vdl-font-body-sm);
}

.import-workspace__error {
  max-width: 36rem;
}

.import-workspace__hint {
  margin-top: var(--vdl-space-1);
}

.import-workspace__error {
  color: var(--vdl-danger);
}

@media (max-width: 30rem) {
  .import-workspace__header {
    align-items: flex-start;
    flex-direction: column;
  }

  .import-workspace__body,
  .import-workspace__header,
  .import-workspace__footer {
    padding: var(--vdl-space-3);
  }

  .import-workspace__picker {
    min-height: 12rem;
    padding: var(--vdl-space-4);
  }

  .import-workspace__select-button {
    width: 100%;
  }
}
</style>
