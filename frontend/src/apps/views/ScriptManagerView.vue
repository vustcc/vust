<script setup lang="ts">
/**
 * @file ScriptManagerView.vue
 * @description 自定义 Shell 脚本资产管理与单次临时执行界面。
 */

import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ScriptDetail, ScriptSummary } from '@/api/modules/scripts'
import type { ScriptRunTerminalServerMessage } from '@/api/generated/scripts'
import { useScriptLibrary } from '@/composables/useScriptLibrary'
import { useApplicationConfirmation } from '@/composables/useApplicationConfirmation'
import { useToastStore } from '@/stores/toast'
import { useWindowManagerStore } from '@/stores/window-manager'
import MonacoEditor from '@/components/editor/MonacoEditor.vue'
import ApplicationDialog from '@/components/layout/ApplicationDialog.vue'
import ApplicationConfirmationDialog from '@/components/layout/ApplicationConfirmationDialog.vue'
import ScriptRunTerminal from './scripts/ScriptRunTerminal.vue'
import VustIcon from '@/components/icons/VustIcon.vue'
import {
  VustActionMenu,
  VustAlert,
  VustButton,
  VustCheckbox,
  VustEmpty,
  VustFormItem,
  VustInput,
  VustLoading,
  VustPagination,
  VustSelect,
  VustTable,
  VustTag,
  VustTooltip,
} from '@/components/ui'
import type { VustTableColumn } from '@/components/ui/VustTable.vue'

const props = defineProps<{ windowId?: string }>()
const { t } = useI18n()
const library = useScriptLibrary()
const { confirmationState, showConfirmation, handleConfirmationResponse } =
  useApplicationConfirmation()
const notifications = useToastStore()
const windowStore = useWindowManagerStore()

type EditorMode = 'create' | 'edit' | 'clone'
interface ScriptMenuAction {
  label: string
  icon: string
  className?: string
  disabled?: boolean
  tooltip?: string
  handler: () => void
}

const editorMode = ref<EditorMode>('create')
const editingScriptId = ref('')
const formDialogVisible = ref(false)
const formReturnFocusSelector = ref('')
const readonlyDialogVisible = ref(false)
const readonlyDetail = ref<ScriptDetail | null>(null)
const baseline = ref('')
const form = ref({
  name: '',
  description: '',
  interactive: false,
  content: '',
  timeoutSeconds: 300,
})
const runDialogVisible = ref(false)
const runScript = ref<ScriptDetail | null>(null)
const runNodeId = ref('')
const runTimeoutSeconds = ref(300)
const runTerminal = ref<InstanceType<typeof ScriptRunTerminal> | null>(null)
const remainingSeconds = ref<number | null>(null)
const confirmationReturnFocusSelector = ref('')
let countdownTimer: number | null = null
const stopCountdown = () => {
  if (countdownTimer !== null) window.clearInterval(countdownTimer)
  countdownTimer = null
  remainingSeconds.value = null
}

const formSnapshot = computed(() => JSON.stringify(form.value))
const isDirty = computed(() => formDialogVisible.value && formSnapshot.value !== baseline.value)
const savePending = computed(() => library.isActionPending('save'))
const runDialogBusy = computed(() => {
  const startPending = runScript.value
    ? library.isActionPending(`run:${runScript.value.scriptId}`)
    : false
  const dismissPending = library.currentRun.value
    ? library.isActionPending(`dismiss:${library.currentRun.value.runId}`)
    : false
  return startPending || dismissPending
})
const totalPages = computed(() =>
  Math.max(1, Math.ceil(library.total.value / library.filters.pageSize)),
)
const onlineNodeOptions = computed(() =>
  library.nodes.value
    .filter((node) => node.status === 'online')
    .map((node) => ({
      value: node.nodeId,
      label: node.nodeId === 'local' ? t('app.nodes.master') : node.name || node.nodeId,
    })),
)
const formTitle = computed(() => {
  if (editorMode.value === 'create') return t('app.scriptManager.dialog.createTitle')
  if (editorMode.value === 'clone') return t('app.scriptManager.dialog.cloneTitle')
  return t('app.scriptManager.dialog.editTitle')
})

const columns = computed<VustTableColumn[]>(() => [
  {
    prop: 'name',
    label: t('app.scriptManager.fields.name'),
    minWidth: 180,
    slot: 'name',
    align: 'center',
  },
  {
    prop: 'interactive',
    label: t('app.scriptManager.fields.interactive'),
    width: 110,
    align: 'center',
    slot: 'interactive',
  },
  {
    prop: 'description',
    label: t('app.scriptManager.fields.description'),
    minWidth: 260,
    slot: 'description',
    align: 'center',
  },
  {
    prop: 'updatedAt',
    label: t('app.scriptManager.fields.time'),
    width: 190,
    slot: 'time',
    align: 'center',
  },
  {
    label: t('common.actions'),
    width: 130,
    fixed: 'right',
    align: 'center',
    slot: 'actions',
  },
])

const formatTime = (value?: string) => (value ? new Date(value).toLocaleString() : '-')
const statusLabel = (status: string) => t(`app.scriptManager.status.${status}`)
const statusTag = (status: string) => {
  if (status === 'succeeded') return 'success'
  if (status === 'failed' || status === 'timedOut') return 'danger'
  if (status === 'cancelled' || status === 'cancelling') return 'warning'
  return 'info'
}

/** 将脚本详情复制到表单，避免编辑过程污染只读数据。 */
const fillForm = (detail: ScriptDetail, name = detail.name) => {
  form.value = {
    name,
    description: detail.description ?? '',
    interactive: detail.interactive,
    content: detail.source.content,
    timeoutSeconds: detail.executionDefaults.timeoutSeconds,
  }
  baseline.value = JSON.stringify(form.value)
}

const confirmDiscard = async () => {
  if (!isDirty.value) return true
  confirmationReturnFocusSelector.value = '[data-ui="script-form-dialog"]'
  return showConfirmation(
    t('app.scriptManager.unsaved.message'),
    t('app.scriptManager.unsaved.title'),
    t('app.scriptManager.unsaved.discard'),
    t('common.cancel'),
    'danger',
  )
}

const loadScriptDetail = async (script: ScriptSummary) => {
  const detail = await library.loadDetail(script.scriptId)
  if (!detail) {
    notifications.error(
      library.detailState.value.error || t('app.scriptManager.messages.loadDetailFailed'),
    )
  }
  return detail
}

const openReadonly = async (script: ScriptSummary) => {
  const detail = await loadScriptDetail(script)
  if (!detail) return
  readonlyDetail.value = detail
  readonlyDialogVisible.value = true
}

const startCreate = () => {
  editorMode.value = 'create'
  editingScriptId.value = ''
  form.value = {
    name: '',
    description: '',
    interactive: false,
    content: '#!/bin/bash\n\nset -euo pipefail\n',
    timeoutSeconds: 300,
  }
  baseline.value = JSON.stringify(form.value)
  formReturnFocusSelector.value = '[data-ui="script-create"]'
  formDialogVisible.value = true
}

const startEdit = async (script: ScriptSummary) => {
  if (!script.capabilities.canUpdate) return
  const detail = await loadScriptDetail(script)
  if (!detail) return
  editorMode.value = 'edit'
  editingScriptId.value = detail.scriptId
  fillForm(detail)
  formReturnFocusSelector.value = '[data-ui="table"]'
  formDialogVisible.value = true
}

const startClone = async (script: ScriptSummary) => {
  if (!script.capabilities.canClone) return
  const detail = await loadScriptDetail(script)
  if (!detail) return
  editorMode.value = 'clone'
  editingScriptId.value = ''
  fillForm(detail, t('app.scriptManager.cloneName', { name: detail.name }))
  formReturnFocusSelector.value = '[data-ui="table"]'
  formDialogVisible.value = true
}

const closeForm = async () => {
  if (!(await confirmDiscard())) return
  formDialogVisible.value = false
  editingScriptId.value = ''
}

const save = async () => {
  if (!form.value.name.trim())
    return notifications.warning(t('app.scriptManager.messages.nameRequired'))
  if (!form.value.content.trim())
    return notifications.warning(t('app.scriptManager.messages.contentRequired'))
  try {
    const payload = {
      name: form.value.name,
      description: form.value.description || undefined,
      interactive: form.value.interactive,
      content: form.value.content,
      timeoutSeconds: Number(form.value.timeoutSeconds),
    }
    const detail =
      editorMode.value === 'edit' && editingScriptId.value
        ? await library.updateScript(editingScriptId.value, {
            expectedRevision: library.detail.value?.revision ?? 0,
            ...payload,
          })
        : await library.createScript(payload)
    baseline.value = JSON.stringify(form.value)
    formDialogVisible.value = false
    library.detail.value = detail
    notifications.success(t('app.scriptManager.messages.saved'))
  } catch (error) {
    notifications.error(
      error instanceof Error ? error.message : t('app.scriptManager.messages.saveFailed'),
    )
  }
}

const remove = async (script: ScriptSummary) => {
  if (!script.capabilities.canRemove) return
  confirmationReturnFocusSelector.value = '[data-ui="table"]'
  const confirmed = await showConfirmation(
    t('app.scriptManager.delete.message', { name: script.name }),
    t('app.scriptManager.delete.title'),
    t('app.scriptManager.delete.confirm'),
    t('common.cancel'),
    'danger',
  )
  if (!confirmed) return
  try {
    await library.removeScript(script.scriptId)
    notifications.success(t('app.scriptManager.messages.removed'))
  } catch (error) {
    notifications.error(
      error instanceof Error ? error.message : t('app.scriptManager.messages.deleteFailed'),
    )
  }
}

const openRun = async (script: ScriptSummary) => {
  if (!script.capabilities.canRun) return
  const detail = await loadScriptDetail(script)
  if (!detail) return
  runScript.value = detail
  runNodeId.value = onlineNodeOptions.value[0]?.value ?? ''
  runTimeoutSeconds.value = detail.executionDefaults.timeoutSeconds
  stopCountdown()
  runDialogVisible.value = true
}

const submitRun = async () => {
  if (!runScript.value || !runNodeId.value) return
  try {
    await library.startRun(
      runScript.value.scriptId,
      runNodeId.value,
      Number(runTimeoutSeconds.value),
    )
  } catch (error) {
    notifications.error(
      error instanceof Error ? error.message : t('app.scriptManager.messages.runFailed'),
    )
  }
}

const closeRun = async () => {
  const run = library.currentRun.value
  if (
    (!run && runScript.value && library.isActionPending(`run:${runScript.value.scriptId}`)) ||
    (run && library.isActionPending(`dismiss:${run.runId}`))
  )
    return
  if (run && library.isRunActive.value) {
    confirmationReturnFocusSelector.value = '[data-ui="run-dialog"]'
    const confirmed = await showConfirmation(
      t('app.scriptManager.runDialog.closeMessage'),
      t('app.scriptManager.runDialog.closeTitle'),
      t('app.scriptManager.runDialog.cancelAndClose'),
      t('common.cancel'),
      'danger',
    )
    if (!confirmed) return
  }
  try {
    runTerminal.value?.close()
    stopCountdown()
    if (run) await library.dismissRun()
    runDialogVisible.value = false
    runScript.value = null
  } catch (error) {
    notifications.error(
      error instanceof Error ? error.message : t('app.scriptManager.messages.dismissFailed'),
    )
  }
}

/** 将控制事件映射到当前 Dialog 状态，不保存终端转录。 */
const handleTerminalControl = (message: ScriptRunTerminalServerMessage) => {
  const run = library.currentRun.value
  if (!run) return
  if (message.kind === 'started') {
    run.status = 'running'
    const seconds = message.payload.timeoutSeconds
    remainingSeconds.value = seconds > 0 ? seconds : null
    if (countdownTimer !== null) window.clearInterval(countdownTimer)
    if (remainingSeconds.value !== null)
      countdownTimer = window.setInterval(() => {
        if (remainingSeconds.value !== null && remainingSeconds.value > 0)
          remainingSeconds.value -= 1
      }, 1000)
  }
  if (message.kind === 'exited') {
    run.status = message.payload.status
    run.exitCode = message.payload.exitCode ?? undefined
    stopCountdown()
  }
  if (message.kind === 'error') {
    stopCountdown()
    run.status = 'failed'
    run.errorSummary = message.payload.message || t('app.scriptManager.runDialog.disconnected')
  }
}

const handleTerminalDisconnect = () => {
  const run = library.currentRun.value
  if (run && library.isRunActive.value) {
    stopCountdown()
    run.status = 'failed'
    run.errorSummary = t('app.scriptManager.runDialog.disconnected')
  }
}

const rowActions = (script: ScriptSummary): ScriptMenuAction[] => [
  {
    label: t('app.scriptManager.actions.run'),
    icon: 'play',
    disabled: !script.capabilities.canRun,
    tooltip: !script.capabilities.canRun
      ? t('app.scriptManager.messages.actionUnavailable')
      : undefined,
    handler: () => void openRun(script),
  },
  {
    label: t('app.scriptManager.actions.clone'),
    icon: 'copy',
    disabled: !script.capabilities.canClone,
    handler: () => void startClone(script),
  },
  {
    label: t('app.scriptManager.actions.edit'),
    icon: 'edit',
    disabled: !script.capabilities.canUpdate,
    handler: () => void startEdit(script),
  },
  {
    label: t('common.delete'),
    icon: 'trash',
    className: 'danger',
    disabled: !script.capabilities.canRemove,
    handler: () => void remove(script),
  },
]

watch(
  [isDirty, () => library.saveState.value.refreshing],
  ([dirty, saving]) => {
    if (!props.windowId) return
    windowStore.updateWindowRuntimeState(props.windowId, {
      dirty,
      busy: saving,
      allowsNodeSwitch: !dirty && !saving,
      blockLevel: saving ? 'busy' : dirty ? 'dirty' : 'open',
      blockReason: saving
        ? t('app.scriptManager.guardSaving')
        : dirty
          ? t('app.scriptManager.guardDirty')
          : t('app.scriptManager.guardOpen'),
    })
  },
  { immediate: true },
)

const beforeUnload = (event: BeforeUnloadEvent) => {
  if (!isDirty.value) return
  event.preventDefault()
}
window.addEventListener('beforeunload', beforeUnload)
onBeforeUnmount(() => window.removeEventListener('beforeunload', beforeUnload))
onBeforeUnmount(() => {
  stopCountdown()
})
</script>

<template>
  <div class="script-library" data-page="script-library" data-vust-app="script-library">
    <div class="toolbar" data-ui="toolbar">
      <div class="search-field">
        <VustInput
          id="script-library-search"
          v-model="library.filters.keyword"
          name="scriptLibrarySearch"
          class="search-input"
          :aria-label="t('app.scriptManager.search')"
          :placeholder="t('app.scriptManager.searchPlaceholder')"
        />
        <button
          v-if="library.filters.keyword"
          type="button"
          class="search-clear"
          :aria-label="t('app.scriptManager.clearSearch')"
          @click="library.filters.keyword = ''"
        >
          <VustIcon name="x" :size="14" />
        </button>
      </div>
      <div class="toolbar-actions">
        <VustButton type="primary" data-ui="script-create" @click="startCreate">
          {{ t('app.scriptManager.actions.create') }}
        </VustButton>
      </div>
    </div>

    <VustAlert
      v-if="library.listState.value.warning"
      data-ui="alert"
      type="warning"
      :title="t('app.scriptManager.messages.refreshWarning')"
      :description="library.listState.value.warning"
      show-icon
    />
    <VustAlert
      v-if="library.listState.value.error"
      data-ui="alert"
      type="error"
      :title="t('app.scriptManager.messages.loadFailed')"
      :description="library.listState.value.error"
      show-icon
    />

    <div class="table-shell" data-slot="content">
      <VustTable
        data-ui="table"
        tabindex="-1"
        :data="library.scripts.value"
        :columns="columns"
        row-key="scriptId"
        border
      >
        <template #name="{ row }: { row: ScriptSummary }">
          <button
            type="button"
            class="name-button"
            :aria-label="t('app.scriptManager.actions.viewSource', { name: row.name })"
            @click="openReadonly(row)"
          >
            {{ row.name }}
          </button>
        </template>
        <template #interactive="{ row }: { row: ScriptSummary }">
          <VustTag :type="row.interactive ? 'warning' : 'default'">
            {{
              row.interactive
                ? t('app.scriptManager.interactive.yes')
                : t('app.scriptManager.interactive.no')
            }}
          </VustTag>
        </template>
        <template #description="{ row }: { row: ScriptSummary }">
          <VustTooltip v-if="row.description" :text="row.description" position="top" :delay="300">
            <span class="description-text">{{ row.description }}</span>
          </VustTooltip>
          <span v-else class="muted-text">-</span>
        </template>
        <template #time="{ row }: { row: ScriptSummary }">
          {{ formatTime(row.updatedAt) }}
        </template>
        <template #actions="{ row }: { row: ScriptSummary }">
          <VustActionMenu :label="t('common.actions')" :actions="rowActions(row)" />
        </template>
        <template #empty>
          <VustEmpty
            :description="
              library.filters.keyword.trim()
                ? t('app.scriptManager.empty.filtered')
                : t('app.scriptManager.empty.library')
            "
          />
        </template>
      </VustTable>
      <VustLoading :loading="library.listState.value.initialLoading" cover />
    </div>

    <VustPagination
      v-if="library.total.value > library.filters.pageSize"
      data-ui="pagination"
      :current-page="library.filters.page"
      :total-pages="totalPages"
      @page-change="(page) => (library.filters.page = page)"
    />

    <ApplicationDialog
      :visible="readonlyDialogVisible"
      data-ui="script-detail-dialog"
      :title="readonlyDetail?.name ?? t('app.scriptManager.dialog.viewTitle')"
      width="min(980px, 92vw)"
      :close-on-click-overlay="false"
      return-focus-selector="[data-ui='table']"
      @close="readonlyDialogVisible = false"
    >
      <div v-if="readonlyDetail" class="detail-dialog" data-slot="body">
        <div class="detail-meta">
          <VustTag :type="readonlyDetail.interactive ? 'warning' : 'default'">
            {{
              readonlyDetail.interactive
                ? t('app.scriptManager.interactive.script')
                : t('app.scriptManager.interactive.nonInteractive')
            }}
          </VustTag>
          <span>{{ formatTime(readonlyDetail.updatedAt) }}</span>
          <span v-if="readonlyDetail.description">{{ readonlyDetail.description }}</span>
        </div>
        <div class="editor-shell readonly-editor">
          <label id="script-readonly-source-label" class="editor-label">
            {{ t('app.scriptManager.fields.source') }}
          </label>
          <MonacoEditor
            id="script-library-readonly-source"
            :model-value="readonlyDetail.source.content"
            language="shell"
            file-path="script.sh"
            :document-key="`readonly:${readonlyDetail.scriptId}:${readonlyDetail.revision}`"
            read-only
            wheel-focus-on-click
            :aria-labelledby="'script-readonly-source-label'"
            :minimap="false"
            :fixed-overflow-widgets="false"
          />
        </div>
      </div>
      <template #footer>
        <VustButton @click="readonlyDialogVisible = false">{{ t('common.close') }}</VustButton>
      </template>
    </ApplicationDialog>

    <ApplicationDialog
      :visible="formDialogVisible"
      data-ui="script-form-dialog"
      :title="formTitle"
      width="min(980px, 92vw)"
      :close-on-click-overlay="false"
      :close-disabled="savePending"
      initial-focus-selector="#script-library-name"
      :return-focus-selector="formReturnFocusSelector"
      @close="closeForm"
    >
      <div class="form-dialog" data-slot="body">
        <div class="script-form-stack" data-ui="form">
          <div data-slot="form-name">
            <VustFormItem
              :label="t('app.scriptManager.fields.name')"
              for="script-library-name"
              required
            >
              <VustInput
                id="script-library-name"
                v-model="form.name"
                name="scriptName"
                :maxlength="80"
              />
            </VustFormItem>
          </div>
          <div class="form-options-row" data-slot="form-options">
            <div class="interactive-inline-field">
              <VustTooltip :text="t('app.scriptManager.interactive.hint')" position="top">
                <VustCheckbox
                  id="script-library-interactive"
                  v-model="form.interactive"
                  name="scriptInteractive"
                >
                  {{ t('app.scriptManager.interactive.checkbox') }}
                </VustCheckbox>
              </VustTooltip>
            </div>
            <div class="timeout-inline-field">
              <label class="inline-field-label" for="script-library-timeout">
                {{ t('app.scriptManager.fields.timeout') }}
              </label>
              <VustInput
                id="script-library-timeout"
                v-model="form.timeoutSeconds"
                class="timeout-input"
                name="scriptTimeoutSeconds"
                type="number"
                :min="1"
                :max="86400"
              />
            </div>
          </div>
          <div class="editor-shell form-editor" data-slot="form-source">
            <label id="script-source-label" class="editor-label">
              {{ t('app.scriptManager.fields.source') }}
            </label>
            <MonacoEditor
              id="script-library-source"
              v-model="form.content"
              name="scriptSource"
              language="shell"
              file-path="script.sh"
              :document-key="`form:${editorMode}:${editingScriptId || 'new'}`"
              :aria-labelledby="'script-source-label'"
              :minimap="false"
              wheel-focus-on-click
              @save="save"
            />
          </div>
          <div data-slot="form-description">
            <VustFormItem
              :label="t('app.scriptManager.fields.description')"
              for="script-library-description"
            >
              <VustInput
                id="script-library-description"
                v-model="form.description"
                name="scriptDescription"
                type="textarea"
                :rows="2"
                :maxlength="500"
              />
            </VustFormItem>
          </div>
        </div>
      </div>
      <template #footer>
        <VustButton :disabled="savePending" @click="closeForm">{{ t('common.cancel') }}</VustButton>
        <VustButton type="primary" :loading="library.isActionPending('save')" @click="save">
          {{ t('common.save') }}
        </VustButton>
      </template>
    </ApplicationDialog>

    <ApplicationDialog
      :visible="runDialogVisible"
      data-ui="run-dialog"
      :title="t('app.scriptManager.runDialog.title', { name: runScript?.name ?? '' })"
      width="min(760px, 92vw)"
      :close-on-click-overlay="false"
      :close-disabled="runDialogBusy"
      initial-focus-selector="#script-run-node"
      return-focus-selector="[data-ui='table']"
      @close="closeRun"
    >
      <div class="run-dialog" data-slot="body">
        <template v-if="!library.currentRun.value">
          <VustAlert
            v-if="library.nodesState.value.error"
            type="error"
            :title="t('app.scriptManager.messages.loadNodesFailed')"
            :description="library.nodesState.value.error"
            show-icon
          />
          <div class="run-form">
            <VustFormItem
              :label="t('app.scriptManager.fields.node')"
              for="script-run-node"
              required
            >
              <VustSelect
                id="script-run-node"
                v-model="runNodeId"
                name="scriptRunNode"
                :options="onlineNodeOptions"
              />
            </VustFormItem>
            <VustFormItem
              :label="t('app.scriptManager.fields.timeout')"
              for="script-run-timeout"
              required
            >
              <VustInput
                id="script-run-timeout"
                v-model="runTimeoutSeconds"
                name="scriptRunTimeout"
                type="number"
                :min="1"
                :max="86400"
              />
            </VustFormItem>
          </div>
        </template>
        <template v-else>
          <div class="run-status" data-ui="run-status">
            <span>{{ library.currentRun.value.nodeName }}</span>
            <span v-if="remainingSeconds !== null" class="remaining-time">{{
              t('app.scriptManager.runDialog.remaining', { seconds: remainingSeconds })
            }}</span>
            <VustTag :type="statusTag(library.currentRun.value.status)">
              {{ statusLabel(library.currentRun.value.status) }}
            </VustTag>
          </div>
          <VustAlert
            v-if="library.currentRun.value.errorSummary"
            type="error"
            :title="statusLabel(library.currentRun.value.status)"
            :description="library.currentRun.value.errorSummary"
            show-icon
          />
          <ScriptRunTerminal
            ref="runTerminal"
            :run-id="library.currentRun.value.runId"
            @control="handleTerminalControl"
            @disconnected="handleTerminalDisconnect"
          />
        </template>
      </div>
      <template #footer>
        <template v-if="!library.currentRun.value">
          <VustButton :disabled="runDialogBusy" @click="closeRun">
            {{ t('common.cancel') }}
          </VustButton>
          <VustButton
            type="primary"
            :disabled="!runNodeId"
            :loading="runScript ? library.isActionPending(`run:${runScript.scriptId}`) : false"
            @click="submitRun"
          >
            {{ t('app.scriptManager.actions.run') }}
          </VustButton>
        </template>
        <VustButton
          v-else
          :type="library.isRunActive.value ? 'danger' : undefined"
          :disabled="runDialogBusy"
          :loading="
            library.currentRun.value
              ? library.isActionPending(`dismiss:${library.currentRun.value.runId}`)
              : false
          "
          @click="closeRun"
        >
          {{
            library.isRunActive.value
              ? t('app.scriptManager.runDialog.cancelAndClose')
              : t('common.close')
          }}
        </VustButton>
      </template>
    </ApplicationDialog>

    <ApplicationConfirmationDialog
      :visible="confirmationState.visible"
      :title="confirmationState.title"
      :message="confirmationState.message"
      :confirm-text="confirmationState.confirmText"
      :cancel-text="confirmationState.cancelText"
      :type="confirmationState.type"
      :return-focus-selector="confirmationReturnFocusSelector"
      dialog-ui="script-confirmation-dialog"
      @confirm="handleConfirmationResponse(true)"
      @cancel="handleConfirmationResponse(false)"
    />
  </div>
</template>

<style scoped>
.script-library {
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
.toolbar-actions,
.detail-meta,
.run-status {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
}

.toolbar {
  flex-shrink: 0;
  justify-content: space-between;
  flex-wrap: wrap;
}

.search-field {
  position: relative;
  width: min(360px, 100%);
}

.search-input {
  width: 100%;
}

.search-field :deep(.vl-input) {
  padding-right: 36px;
}

.search-clear {
  position: absolute;
  top: 50%;
  right: var(--vdl-space-2);
  width: 24px;
  height: 24px;
  padding: 0;
  border: 0;
  border-radius: var(--vdl-radius-sm);
  color: var(--vdl-text-muted);
  background: transparent;
  cursor: pointer;
  transform: translateY(-50%);
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.search-clear:hover {
  color: var(--vdl-text-primary);
  background: var(--vdl-bg-hover);
}

.search-clear:focus-visible {
  outline: none;
  box-shadow: var(--vdl-focus-ring);
}

.table-shell {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: auto;
  border: 1px solid var(--vdl-border-subtle);
  border-radius: var(--vdl-radius-lg);
  background: var(--vdl-bg-panel);
}

.name-button {
  max-width: 100%;
  padding: 0;
  overflow: hidden;
  border: 0;
  color: var(--vdl-primary);
  background: transparent;
  font: inherit;
  font-weight: 600;
  text-align: left;
  text-overflow: ellipsis;
  white-space: nowrap;
  cursor: pointer;
}

.name-button:hover {
  text-decoration: underline;
}

.name-button:focus-visible {
  outline: none;
  box-shadow: var(--vdl-focus-ring);
  border-radius: var(--vdl-radius-sm);
}

.description-text {
  display: block;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.muted-text,
.detail-meta,
.remaining-time {
  color: var(--vdl-text-muted);
}

.detail-dialog,
.form-dialog,
.run-dialog {
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
}

.detail-meta {
  flex-wrap: wrap;
}

.script-form-stack {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
}

.form-options-row {
  min-width: 0;
  display: flex;
  align-items: center;
  gap: var(--vdl-space-6);
}

.interactive-inline-field,
.timeout-inline-field {
  min-width: 0;
  display: flex;
  align-items: center;
}

.timeout-inline-field {
  gap: var(--vdl-space-2);
}

.inline-field-label {
  flex-shrink: 0;
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-body-sm);
  font-weight: 600;
}

.timeout-input {
  width: 160px;
}

.editor-shell {
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  border: 1px solid var(--vdl-border-subtle);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-panel);
}

.readonly-editor {
  height: min(52vh, 520px);
  min-height: 300px;
}

.form-editor {
  height: calc(50vh - 220px);
  min-height: 320px;
  flex-shrink: 0;
}

.editor-label {
  flex-shrink: 0;
  padding: var(--vdl-space-2) var(--vdl-space-3);
  border-bottom: 1px solid var(--vdl-border-subtle);
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-body-sm);
  font-weight: 600;
}

.run-form {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(160px, 220px);
  gap: var(--vdl-space-3);
}

.run-status {
  justify-content: space-between;
  min-width: 0;
  padding-bottom: var(--vdl-space-2);
  border-bottom: 1px solid var(--vdl-border-subtle);
}

.run-dialog {
  box-sizing: border-box;
  width: 100%;
}

@media (max-width: 680px) {
  .script-library {
    padding: var(--vdl-space-2);
  }

  .toolbar,
  .toolbar-actions {
    width: 100%;
  }

  .search-field {
    width: 100%;
  }

  .toolbar-actions {
    justify-content: flex-end;
  }

  .run-form {
    grid-template-columns: 1fr;
  }

  .form-options-row {
    flex-direction: column;
    align-items: stretch;
    gap: var(--vdl-space-3);
  }

  .timeout-input {
    flex: 1;
    width: auto;
  }

  .readonly-editor {
    min-height: 240px;
  }

  .form-editor {
    height: calc(85vh - 280px);
    min-height: 240px;
  }
}
</style>
