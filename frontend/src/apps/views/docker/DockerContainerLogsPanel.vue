<script setup lang="ts">
import {
  VustAlert,
  VustButton,
  VustEmpty,
  VustInput,
  VustLoading,
  VustTabs,
  VustTag,
} from '@/components/ui'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useContainerLogs, type LogMode } from './composables/useContainerLogs'
import { ansiToHtml } from '@/utils/ansi'

/**
 * @file DockerContainerLogsPanel.vue
 * @description 容器日志面板组件，嵌入在详情视图中使用。
 * 内部逻辑委托给 `useContainerLogs` composable。
 */

const props = defineProps<{
  containerId: string | null
  containerName?: string | null
  nodeId: string
  active?: boolean
}>()

const emit = defineEmits<{
  (e: 'activeChange', value: boolean): void
}>()

const { t } = useI18n()

// --- Composable ---

/** 将 props 映射为响应式 ref / computed，驱动 composable */
const containerIdRef = computed(() => props.containerId)
const nodeIdRef = computed(() => props.nodeId)
const activeRef = computed(() => props.active ?? false)

const {
  logLines,
  isLoading,
  logMode,
  logModeTabs,
  statusText,
  autoScrollEl,
  realtimeSubscribed,
  logError,
  isPaused,
  pausedLineCount,
  loadLatestLogs,
  switchMode,
  togglePaused,
  clearLogs,
} = useContainerLogs({
  containerId: containerIdRef,
  nodeId: nodeIdRef,
  active: activeRef,
})

const keyword = ref('')
const visibleLines = computed(() => {
  const query = keyword.value.trim().toLowerCase()
  return query
    ? logLines.value.filter((line) => line.toLowerCase().includes(query))
    : logLines.value
})
const logHtml = computed(() => visibleLines.value.map(ansiToHtml).join('\n'))

// --- Panel-specific Logic ---

/**
 * 处理日志模式切换。
 */
const handleModeChange = (mode: LogMode) => {
  switchMode(mode)
}

const normalizeMode = (value: unknown): LogMode => (value === 'latest' ? 'latest' : 'realtime')

/**
 * 刷新最新日志（仅 latest 模式下可用）。
 */
const refreshLatest = () => {
  if (logMode.value === 'latest') {
    void loadLatestLogs()
  }
}

/**
 * 将 realtimeSubscribed 状态同步到父组件的 activeChange 事件。
 */
watch(
  realtimeSubscribed,
  (value) => {
    emit('activeChange', value)
  },
  { immediate: true },
)
</script>

<template>
  <div class="container-logs-panel" data-ui="container-runtime-logs">
    <div class="logs-header" data-ui="toolbar">
      <div class="logs-title">
        <span class="logs-name">{{ t('app.docker.containers.logsPanel.title') }}</span>
        <span class="logs-subtitle">{{ props.containerName || props.containerId || '' }}</span>
      </div>
      <div class="logs-actions">
        <VustTag
          :type="logError ? 'danger' : realtimeSubscribed ? 'success' : 'info'"
          effect="plain"
        >
          {{ statusText }}
        </VustTag>
        <VustInput
          id="docker-container-log-search"
          v-model="keyword"
          name="docker-container-log-search"
          :placeholder="t('app.docker.containers.logsPanel.searchPlaceholder')"
          clearable
          class="log-search"
        />
        <VustTabs
          :model-value="logMode"
          class="log-modes"
          :tabs="logModeTabs"
          @update:model-value="(value) => handleModeChange(normalizeMode(value))"
        />
        <VustButton
          v-if="logMode === 'realtime'"
          size="small"
          type="secondary"
          @click="togglePaused"
        >
          {{
            isPaused
              ? t('app.docker.containers.logsPanel.resume', { count: pausedLineCount })
              : t('app.docker.containers.logsPanel.pause')
          }}
        </VustButton>
        <VustButton
          v-if="logMode === 'latest'"
          size="small"
          :loading="isLoading"
          @click="refreshLatest"
        >
          {{ t('common.refresh') }}
        </VustButton>
        <VustButton size="small" type="secondary" @click="clearLogs">
          {{ t('app.docker.containers.logsPanel.clear') }}
        </VustButton>
      </div>
    </div>

    <VustAlert
      v-if="logError"
      type="error"
      :title="t('app.docker.containers.logsPanel.fetchError')"
      :description="logError"
      data-ui="container-log-error"
    />

    <div class="logs-body" data-slot="log-output">
      <VustLoading :loading="isLoading" cover />
      <div ref="autoScrollEl" class="log-scroll" data-native-context-menu>
        <pre v-if="visibleLines.length" class="log-content" v-html="logHtml"></pre>
        <VustEmpty
          v-else
          :description="
            keyword
              ? t('app.docker.containers.logsPanel.filteredEmpty')
              : t('app.docker.containers.logsPanel.empty')
          "
        />
      </div>
    </div>
  </div>
</template>

<style scoped>
.container-logs-panel {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
  flex: 1;
  height: 100%;
  min-height: 0;
  padding: var(--vdl-space-4);
  box-sizing: border-box;
}

.logs-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-2);
  flex-wrap: wrap;
}

.logs-title {
  display: flex;
  align-items: baseline;
  gap: var(--vdl-space-3);
  flex-wrap: wrap;
}

.logs-name {
  font-weight: 700;
  color: var(--vdl-primary);
  font-size: var(--vdl-font-subtitle);
}

.logs-subtitle {
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-body-sm);
  word-break: break-all;
}

.logs-actions {
  display: flex;
  gap: var(--vdl-space-2);
  align-items: center;
  flex-wrap: wrap;
}

.log-search {
  width: 220px;
}

.log-modes {
  width: auto;
}

.log-modes :deep(.vl-tabs-nav) {
  border-bottom: none;
  height: 32px;
}

.log-modes :deep(.vl-tabs-item) {
  height: 32px;
  font-size: var(--vdl-font-body-sm);
}

.logs-body {
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-canvas);
  color: var(--vdl-text-secondary);
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
  overflow: hidden;
  position: relative;
}

.log-content {
  margin: 0;
  padding: var(--vdl-space-4);
  font-family: var(--vdl-font-mono);
  font-size: var(--vdl-font-code);
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-all;
}

.log-scroll {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
  overflow: auto;
  scrollbar-width: thin;
}
</style>
