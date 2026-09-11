<script setup lang="ts">
/**
 * @file FileEditorView.vue
 * @description 固定节点上的可靠多标签文件编辑器。
 */
import { computed, nextTick, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { storeToRefs } from 'pinia'
import EditorMenuBar from '@/apps/views/file-editor/EditorMenuBar.vue'
import MonacoEditor from '@/components/editor/MonacoEditor.vue'
import { useFileEditor, type EditorTab } from '@/composables/useFileEditor'
import { useFileEditorPreferencesStore } from '@/stores/file-editor-preferences'
import { useWindowManagerStore } from '@/stores/window-manager'
import { useNodeStore } from '@/stores/node'
import { VustButton, VustTag, VustDialog, VustTooltip } from '@/components/ui'
import VustIcon from '@/components/icons/VustIcon.vue'

const props = defineProps<{
  windowId?: string
  payload?: {
    path?: string
    nodeId?: string
  }
}>()

const { t } = useI18n()
const windowStore = useWindowManagerStore()
const nodeStore = useNodeStore()
const editorPreferencesStore = useFileEditorPreferencesStore()
const { wordWrap, fontSize, highlightAmbiguousUnicode, minimap, stickyScroll, renderWhitespace } =
  storeToRefs(editorPreferencesStore)
const targetNodeId = ref(props.payload?.nodeId || nodeStore.currentNodeId)
const { loadFileContent, saveFileContent } = useFileEditor(targetNodeId)

const tabs = ref<EditorTab[]>([])
const activePath = ref('')
const editorRef = ref<{ disposeDocument: (documentKey: string) => void } | null>(null)

const confirmDialogVisible = ref(false)
const pendingPath = ref('')
const confirmationKind = ref<'close' | 'reload'>('close')

const activeTab = computed(() => tabs.value.find((t) => t.path === activePath.value))
const dirtyTabs = computed(() => tabs.value.filter((tab) => tab.isDirty))
const editorScopeId = computed(() => props.windowId || 'default')
const editorPanelId = computed(() => `file-editor-${editorScopeId.value}-panel`)
const editorInputId = computed(() => `file-editor-${editorScopeId.value}-content`)
const confirmationTitle = computed(() =>
  confirmationKind.value === 'close'
    ? t('app.fileEditor.closeUnsavedTitle')
    : t('app.fileEditor.reloadUnsavedTitle'),
)
const confirmationMessage = computed(() =>
  confirmationKind.value === 'close'
    ? t('app.fileEditor.closeUnsavedMessage')
    : t('app.fileEditor.reloadUnsavedMessage'),
)
const confirmationLabel = computed(() =>
  confirmationKind.value === 'close'
    ? t('app.fileEditor.closeDirectly')
    : t('app.fileEditor.reloadDirectly'),
)

const tabDomId = (tab: EditorTab) =>
  `file-editor-${editorScopeId.value}-tab-${Math.max(0, tabs.value.indexOf(tab))}`

const isLoadBusy = (tab: EditorTab) =>
  tab.loadState === 'initialLoading' || tab.loadState === 'refreshing'

const isSaveBusy = (tab: EditorTab) => tab.saveState === 'saving' || tab.saveState === 'reconciling'

const statusType = (tab: EditorTab) => {
  if (
    tab.saveState === 'conflict' ||
    tab.saveState === 'failed' ||
    tab.loadState === 'initialError'
  ) {
    return 'danger' as const
  }
  if (tab.isDirty || tab.loadState === 'stale' || tab.durability === 'uncertain') {
    return 'warning' as const
  }
  if (isLoadBusy(tab) || isSaveBusy(tab)) return 'info' as const
  return 'success' as const
}

const statusLabel = (tab: EditorTab) => {
  if (tab.saveState === 'saving') return t('app.fileEditor.saving')
  if (tab.saveState === 'reconciling') return t('app.fileEditor.reconciling')
  if (tab.saveState === 'conflict') return t('app.fileEditor.conflict')
  if (tab.saveState === 'failed') return t('app.fileEditor.saveFailedShort')
  if (tab.loadState === 'initialLoading') return t('app.fileEditor.loading')
  if (tab.loadState === 'refreshing') return t('app.fileEditor.refreshing')
  if (tab.loadState === 'initialError') return t('app.fileEditor.loadFailedShort')
  if (tab.loadState === 'stale') return t('app.fileEditor.stale')
  if (tab.durability === 'uncertain') return t('app.fileEditor.durabilityWarning')
  return tab.isDirty ? t('app.fileEditor.unsaved') : t('app.fileEditor.saved')
}

const openTab = async (path: string) => {
  if (!path) return
  const existingTab = tabs.value.find((t) => t.path === path)
  if (existingTab) {
    activePath.value = path
    return
  }

  const name = path.split('/').filter(Boolean).pop() || path
  const newTab: EditorTab = {
    path,
    name,
    documentKey: `${targetNodeId.value}:${path}`,
    content: '',
    originalContent: '',
    revision: '',
    isDirty: false,
    fileSize: 0,
    loadState: 'idle',
    saveState: 'idle',
    loadSequence: 0,
    saveSequence: 0,
  }
  tabs.value.push(newTab)
  activePath.value = path

  const reactiveTab = tabs.value.find((tab) => tab.path === path)
  if (reactiveTab) {
    await loadFileContent(reactiveTab)
  }
}

const closeTab = (path: string) => {
  const tab = tabs.value.find((t) => t.path === path)
  if (!tab) return

  if (tab.isDirty) {
    pendingPath.value = path
    confirmationKind.value = 'close'
    confirmDialogVisible.value = true
    return
  }

  performClose(path)
}

const performClose = (path: string) => {
  const index = tabs.value.findIndex((t) => t.path === path)
  if (index === -1) return

  editorRef.value?.disposeDocument(tabs.value[index].documentKey)
  tabs.value.splice(index, 1)

  if (activePath.value === path) {
    if (tabs.value.length > 0) {
      activePath.value = tabs.value[Math.max(0, index - 1)].path
    } else {
      activePath.value = ''
    }
  }
}

const handleConfirm = async () => {
  const path = pendingPath.value
  const kind = confirmationKind.value
  confirmDialogVisible.value = false
  pendingPath.value = ''
  if (!path) return
  if (kind === 'close') {
    performClose(path)
    return
  }
  const tab = tabs.value.find((item) => item.path === path)
  if (tab) await loadFileContent(tab, { discardLocalChanges: true })
}

const handleCancelConfirmation = () => {
  confirmDialogVisible.value = false
  pendingPath.value = ''
}

const handleContentChange = (content: string, documentKey: string) => {
  const tab = tabs.value.find((item) => item.documentKey === documentKey)
  if (!tab) return
  tab.content = content
  tab.isDirty = tab.content !== tab.originalContent
  tab.durability = undefined
  if (tab.saveState === 'failed') {
    tab.saveState = 'idle'
    tab.saveError = undefined
  }
}

const handleSave = async (tab?: EditorTab) => {
  const targetTab = tab || activeTab.value
  if (!targetTab) return
  await saveFileContent(targetTab)
}

const handleSaveByDocumentKey = async (documentKey: string) => {
  const tab = tabs.value.find((item) => item.documentKey === documentKey)
  if (tab) await handleSave(tab)
}

const reloadTab = async (tab?: EditorTab) => {
  const targetTab = tab || activeTab.value
  if (!targetTab || isSaveBusy(targetTab)) return
  if (targetTab.isDirty) {
    pendingPath.value = targetTab.path
    confirmationKind.value = 'reload'
    confirmDialogVisible.value = true
    return
  }
  await loadFileContent(targetTab, { discardLocalChanges: true })
}

const adjustFont = (delta: number) => {
  fontSize.value = Math.min(24, Math.max(10, fontSize.value + delta))
}

/** 将编辑器字号恢复为默认值。 */
const resetFont = () => {
  fontSize.value = 14
}

const handleTabKeydown = async (event: KeyboardEvent, tab: EditorTab) => {
  const currentIndex = tabs.value.indexOf(tab)
  if (currentIndex < 0) return
  let nextIndex = currentIndex
  if (event.key === 'ArrowLeft')
    nextIndex = (currentIndex - 1 + tabs.value.length) % tabs.value.length
  else if (event.key === 'ArrowRight') nextIndex = (currentIndex + 1) % tabs.value.length
  else if (event.key === 'Home') nextIndex = 0
  else if (event.key === 'End') nextIndex = tabs.value.length - 1
  else return
  event.preventDefault()
  activePath.value = tabs.value[nextIndex].path
  await nextTick()
  document.getElementById(tabDomId(tabs.value[nextIndex]))?.focus()
}

watch(
  () => props.payload?.path,
  (newPath) => {
    if (newPath) {
      void openTab(newPath)
    }
  },
  { immediate: true },
)

watch(
  dirtyTabs,
  (items) => {
    if (!props.windowId) return
    windowStore.updateWindowRuntimeState(props.windowId, {
      dirty: items.length > 0,
      allowsNodeSwitch: false,
      blockLevel: items.length > 0 ? 'dirty' : 'open',
      blockReason:
        items.length > 0
          ? t('app.fileEditor.guardDirty', { count: items.length })
          : t('app.fileEditor.guardOpen'),
    })
  },
  { immediate: true },
)
</script>

<template>
  <div
    class="file-editor-app"
    data-page="file-editor"
    data-vust-app="file-editor"
    :data-node-id="targetNodeId"
  >
    <EditorMenuBar
      :menu-id="`file-editor-${editorScopeId}`"
      :word-wrap="wordWrap"
      :minimap="minimap"
      :sticky-scroll="stickyScroll"
      :highlight-ambiguous-unicode="highlightAmbiguousUnicode"
      :render-whitespace="renderWhitespace"
      :font-size="fontSize"
      :reload-disabled="!activeTab || isLoadBusy(activeTab) || isSaveBusy(activeTab)"
      :save-disabled="
        !activeTab ||
        !activeTab.isDirty ||
        isLoadBusy(activeTab) ||
        isSaveBusy(activeTab) ||
        activeTab.saveState === 'conflict' ||
        activeTab.capabilities?.canWrite === false
      "
      @reload="reloadTab()"
      @save="handleSave()"
      @update:word-wrap="wordWrap = $event"
      @update:minimap="minimap = $event"
      @update:sticky-scroll="stickyScroll = $event"
      @update:highlight-ambiguous-unicode="highlightAmbiguousUnicode = $event"
      @update:render-whitespace="renderWhitespace = $event"
      @adjust-font="adjustFont"
      @reset-font="resetFont"
      @reset-preferences="editorPreferencesStore.resetPreferences()"
    />

    <!-- Tab 栏 -->
    <div
      v-if="tabs.length > 0"
      class="tabs-header"
      data-ui="editor-tabs"
      data-slot="tabs"
      role="tablist"
      :aria-label="t('app.fileEditor.openFiles')"
    >
      <div
        v-for="tab in tabs"
        :key="tab.path"
        class="editor-tab"
        :class="{ 'is-active': activePath === tab.path }"
      >
        <button
          :id="tabDomId(tab)"
          type="button"
          class="tab-select"
          role="tab"
          :aria-selected="activePath === tab.path"
          :aria-controls="editorPanelId"
          :tabindex="activePath === tab.path ? 0 : -1"
          @click="activePath = tab.path"
          @keydown="handleTabKeydown($event, tab)"
        >
          <span class="tab-status" :class="{ 'is-dirty': tab.isDirty }" aria-hidden="true"></span>
          <VustTooltip
            class="tab-name-tooltip"
            :text="tab.path"
            :disabled="activePath === tab.path"
            position="bottom"
          >
            <span class="tab-name">{{ tab.name }}</span>
          </VustTooltip>
        </button>
        <button
          type="button"
          class="tab-close"
          :aria-label="t('app.fileEditor.closeTab', { name: tab.name })"
          @click.stop="closeTab(tab.path)"
        >
          <VustIcon name="error" :size="14" />
        </button>
      </div>
    </div>

    <!-- 编辑器主体 -->
    <div
      :id="editorPanelId"
      class="editor-body"
      data-ui="editor-area"
      data-slot="content"
      role="tabpanel"
      :aria-labelledby="activeTab ? tabDomId(activeTab) : undefined"
    >
      <template v-if="activeTab">
        <div v-if="activeTab.loadState === 'initialLoading'" class="state-overlay">
          {{ t('app.fileEditor.loading') }}
        </div>
        <div v-else-if="activeTab.loadState === 'initialError'" class="state-overlay error">
          {{ activeTab.loadError }}
        </div>
        <MonacoEditor
          v-else
          ref="editorRef"
          :model-value="activeTab.content"
          :document-key="activeTab.documentKey"
          :file-path="activeTab.path"
          :read-only="activeTab.capabilities?.canWrite === false"
          :word-wrap="wordWrap"
          :minimap="minimap"
          :sticky-scroll="stickyScroll"
          :highlight-ambiguous-unicode="highlightAmbiguousUnicode"
          :render-whitespace="renderWhitespace"
          :font-size="fontSize"
          :id="editorInputId"
          name="fileContent"
          :aria-label="t('app.fileEditor.editorLabel', { path: activeTab.path })"
          @change="handleContentChange"
          @save="handleSaveByDocumentKey"
        />
      </template>
      <div v-else class="empty-state">
        <VustIcon name="file" :size="48" class="empty-icon" />
        <p>{{ t('app.fileEditor.selectFile') }}</p>
      </div>
    </div>

    <div class="editor-status-bar" data-ui="editor-status-bar" data-slot="status-bar">
      <span
        v-if="activeTab"
        class="current-path"
        data-slot="path"
        dir="ltr"
        :title="activeTab.path"
      >
        {{ activeTab.path }}
      </span>
      <span v-else class="status-bar-spacer"></span>
      <VustTag
        v-if="activeTab"
        data-slot="file-status"
        :type="statusType(activeTab)"
        :title="activeTab.refreshWarning || activeTab.saveError"
      >
        {{ statusLabel(activeTab) }}
      </VustTag>
    </div>

    <!-- 关闭确认对话框 -->
    <VustDialog
      :visible="confirmDialogVisible"
      :title="confirmationTitle"
      data-ui="editor-confirmation"
      @close="handleCancelConfirmation"
    >
      <div class="confirmation-message" data-slot="message">{{ confirmationMessage }}</div>
      <div class="confirmation-actions" data-slot="actions">
        <VustButton @click="handleCancelConfirmation">{{ t('confirmation.cancel') }}</VustButton>
        <VustButton
          :type="confirmationKind === 'close' ? 'danger' : 'primary'"
          @click="handleConfirm"
        >
          {{ confirmationLabel }}
        </VustButton>
      </div>
    </VustDialog>
  </div>
</template>

<style scoped>
.file-editor-app {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--vdl-bg-base);
  overflow: hidden;
}

.tabs-header {
  display: flex;
  background: var(--vdl-bg-panel);
  border-bottom: 1px solid var(--vdl-border-subtle);
  overflow-x: auto;
  overflow-y: hidden;
  height: 40px;
  flex-shrink: 0;
}

.tabs-header::-webkit-scrollbar {
  height: 0;
}

.editor-tab {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
  padding: 0 var(--vdl-space-3);
  height: 100%;
  min-width: 120px;
  max-width: 200px;
  background: var(--vdl-bg-muted);
  border-right: 1px solid var(--vdl-border-subtle);
  cursor: pointer;
  user-select: none;
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-body-sm);
  transition: all 0.2s ease;
}

.tab-select {
  all: unset;
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
  flex: 1;
  min-width: 0;
  height: 100%;
  cursor: pointer;
}

.tab-select:focus-visible,
.tab-close:focus-visible {
  outline: 2px solid var(--vdl-primary);
  outline-offset: -2px;
}

.editor-tab:hover {
  background: var(--vdl-bg-hover);
  color: var(--vdl-text-primary);
}

.editor-tab.is-active {
  background: var(--vdl-bg-canvas);
  color: var(--vdl-text-primary);
  border-bottom: 2px solid var(--vdl-primary);
}

.tab-status {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: transparent;
  flex-shrink: 0;
}

.tab-status.is-dirty {
  background: var(--vdl-warning);
}

.tab-name {
  display: block;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.tab-name-tooltip {
  flex: 1;
  min-width: 0;
  overflow: hidden;
}

.tab-close {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  border-radius: var(--vdl-radius-sm);
  color: var(--vdl-text-muted);
  padding: 0;
  border: 0;
  background: transparent;
  cursor: pointer;
  opacity: 0;
  transition:
    opacity 0.2s,
    background 0.2s;
}

.editor-tab:hover .tab-close,
.editor-tab.is-active .tab-close {
  opacity: 1;
}

.tab-close:hover {
  background: var(--vdl-bg-input);
  color: var(--vdl-text-primary);
}

.empty-tabs-placeholder {
  display: flex;
  align-items: center;
  padding: 0 var(--vdl-space-4);
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-body-sm);
}

.editor-status-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-3);
  min-height: 30px;
  padding: 0 var(--vdl-space-3);
  background: var(--vdl-bg-panel);
  border-top: 1px solid var(--vdl-border-subtle);
  flex-shrink: 0;
}

.current-path {
  min-width: 0;
  font-size: var(--vdl-font-caption);
  font-family: var(--vdl-font-mono);
  color: var(--vdl-text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  text-align: left;
}

.status-bar-spacer {
  min-width: 0;
}

.editor-body {
  flex: 1;
  position: relative;
  background: var(--vdl-bg-canvas);
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.state-overlay {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--vdl-bg-canvas);
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-body);
  z-index: 10;
}

.state-overlay.error {
  color: var(--vdl-danger);
}

.empty-state {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--vdl-space-4);
  color: var(--vdl-text-muted);
}

.empty-icon {
  opacity: 0.5;
}

.confirmation-message {
  padding: var(--vdl-space-5) 0;
}

.confirmation-actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--vdl-space-3);
  margin-top: var(--vdl-space-5);
}
</style>
