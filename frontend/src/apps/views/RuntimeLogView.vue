<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { RuntimeLogFile, RuntimeLogLine } from '@/api/generated'
import { runtimeLogApi } from '@/api/modules/runtime-log'
import { nodesApi, type NodeSummaryResponse } from '@/api/modules/nodes'
import {
  VustAlert,
  VustButton,
  VustEmpty,
  VustFormItem,
  VustInput,
  VustLoading,
  VustSelect,
  VustTag,
  VustTooltip,
} from '@/components/ui'

const { t } = useI18n()
const nodes = ref<NodeSummaryResponse[]>([])
const files = ref<RuntimeLogFile[]>([])
const lines = ref<RuntimeLogLine[]>([])
const availability = ref('unavailable')
const filesLoading = ref(false)
const filesError = ref('')
const filesLoadedAt = ref<string>()
const queryLoading = ref(false)
const queryError = ref('')
const refreshWarning = ref('')
const queryLoadedAt = ref<string>()
const nextCursor = ref<string>()
const hasMore = ref(false)
const scanTruncated = ref(false)
const filtersExpanded = ref(false)
const truncatedFields = reactive(new Set<string>())
let filesSequence = 0
let querySequence = 0
let filesController: AbortController | undefined
let queryController: AbortController | undefined

const form = reactive({
  service: 'master',
  nodeId: 'local',
  fileId: '',
  level: '',
  target: '',
  keyword: '',
})
const serviceOptions = computed(() => [
  { label: t('app.runtimeLog.services.master'), value: 'master' },
  { label: t('app.runtimeLog.services.agent'), value: 'agent' },
])
const nodeOptions = computed(() => [
  { label: t('app.runtimeLog.localNode'), value: 'local' },
  ...nodes.value
    .filter((node) => node.nodeId !== 'local')
    .map((node) => ({ label: node.name || node.nodeId, value: node.nodeId })),
])
const fileOptions = computed(() =>
  files.value.map((file) => ({
    label: `${file.fileName} · ${formatBytes(file.sizeBytes)}`,
    value: file.fileId,
  })),
)
const levelOptions = [
  { label: 'ALL', value: '' },
  { label: 'ERROR', value: 'ERROR' },
  { label: 'WARN', value: 'WARN' },
  { label: 'INFO', value: 'INFO' },
  { label: 'DEBUG', value: 'DEBUG' },
  { label: 'TRACE', value: 'TRACE' },
]
const selectedFile = computed(() => files.value.find((file) => file.fileId === form.fileId))
const availabilityText = computed(() => t(`app.runtimeLog.availability.${availability.value}`))

function formatBytes(value: number) {
  if (value < 1024) return `${value} B`
  const units = ['KiB', 'MiB', 'GiB']
  let size = value / 1024
  let index = 0
  while (size >= 1024 && index < units.length - 1) {
    size /= 1024
    index += 1
  }
  return `${size.toFixed(size >= 10 ? 0 : 1)} ${units[index]}`
}
function formatTime(value?: string) {
  if (!value) return ''
  const date = new Date(value)
  const pad = (part: number) => String(part).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}
function levelTag(level?: string) {
  const value = level?.toUpperCase()
  return value === 'ERROR'
    ? 'danger'
    : value === 'WARN'
      ? 'warning'
      : value === 'INFO'
        ? 'info'
        : 'default'
}

/** 根据当前网格列宽更新日志字段的截断状态。 */
function updateFieldTruncation(event: PointerEvent, fieldKey: string) {
  const wrapper = event.currentTarget as HTMLElement
  const field = wrapper.querySelector<HTMLElement>('[data-slot="log-field"]')
  if (field && field.scrollWidth > field.clientWidth) truncatedFields.add(fieldKey)
  else truncatedFields.delete(fieldKey)
}
async function loadFiles() {
  const sequence = ++filesSequence
  filesController?.abort()
  queryController?.abort()
  filesController = new AbortController()
  filesLoading.value = true
  filesError.value = ''
  truncatedFields.clear()
  lines.value = []
  nextCursor.value = undefined
  hasMore.value = false
  const snapshot = `${form.service}:${form.nodeId}`
  try {
    const response = await runtimeLogApi.files(
      { service: form.service, nodeId: form.service === 'agent' ? form.nodeId : undefined },
      filesController.signal,
    )
    if (sequence !== filesSequence || snapshot !== `${form.service}:${form.nodeId}`) return
    if (!response.success || !response.data) throw new Error(response.message)
    files.value = response.data.files
    availability.value = response.data.availability
    filesLoadedAt.value = new Date().toISOString()
    form.fileId = files.value[0]?.fileId ?? ''
  } catch (error) {
    if (
      sequence !== filesSequence ||
      (error instanceof DOMException && error.name === 'AbortError')
    )
      return
    files.value = []
    availability.value = 'unavailable'
    filesError.value = error instanceof Error ? error.message : t('app.runtimeLog.filesFailed')
  } finally {
    if (sequence === filesSequence) filesLoading.value = false
  }
}
async function queryLogs(loadMore = false) {
  const file = selectedFile.value
  if (!file || queryLoading.value) return
  const sequence = ++querySequence
  queryController?.abort()
  queryController = new AbortController()
  queryLoading.value = true
  queryError.value = ''
  const hadLines = lines.value.length > 0
  const snapshot = `${form.service}:${form.nodeId}:${file.fileId}:${loadMore ? nextCursor.value : 'latest'}`
  try {
    const response = await runtimeLogApi.query(
      {
        service: form.service,
        nodeId: form.service === 'agent' ? form.nodeId : undefined,
        fileId: file.fileId,
        fileName: file.fileName,
        level: form.level || undefined,
        target: form.target.trim() || undefined,
        keyword: form.keyword.trim() || undefined,
        cursor: loadMore ? nextCursor.value : undefined,
        limit: 200,
      },
      queryController.signal,
    )
    if (
      sequence !== querySequence ||
      !snapshot.startsWith(`${form.service}:${form.nodeId}:${selectedFile.value?.fileId}`)
    )
      return
    if (!response.success || !response.data) {
      if (response.errorCode === 'RUNTIME_LOG_CURSOR_STALE') {
        refreshWarning.value = t('app.runtimeLog.cursorStale')
        await loadFiles()
        return
      }
      throw new Error(response.message)
    }
    if (!loadMore) truncatedFields.clear()
    lines.value = loadMore ? [...lines.value, ...response.data.lines] : response.data.lines
    nextCursor.value = response.data.nextCursor
    hasMore.value = response.data.hasMore
    scanTruncated.value = response.data.scanTruncated
    queryLoadedAt.value = new Date().toISOString()
    refreshWarning.value = ''
  } catch (error) {
    if (
      sequence !== querySequence ||
      (error instanceof DOMException && error.name === 'AbortError')
    )
      return
    const message = error instanceof Error ? error.message : t('app.runtimeLog.queryFailed')
    if (hadLines) refreshWarning.value = message
    else queryError.value = message
  } finally {
    if (sequence === querySequence) queryLoading.value = false
  }
}
watch(
  () => [form.service, form.nodeId],
  () => {
    if (form.service === 'master') form.nodeId = 'local'
    void loadFiles()
  },
)
watch(
  () => form.fileId,
  () => {
    queryController?.abort()
    lines.value = []
    nextCursor.value = undefined
    hasMore.value = false
    queryError.value = ''
    refreshWarning.value = ''
  },
)
onMounted(async () => {
  const response = await nodesApi.list()
  if (response.success && response.data) nodes.value = response.data
  await loadFiles()
})
onBeforeUnmount(() => {
  filesController?.abort()
  queryController?.abort()
})
</script>

<template>
  <div class="runtime-log" data-page="runtime-log" data-vust-app="runtime-log">
    <div class="toolbar" data-ui="toolbar" data-slot="controls">
      <VustFormItem
        class="primary-select"
        :label="t('app.runtimeLog.service')"
        for="runtime-log-service"
      >
        <VustSelect
          id="runtime-log-service"
          v-model="form.service"
          name="runtimeLogService"
          :options="serviceOptions"
        />
      </VustFormItem>
      <VustFormItem class="primary-select" :label="t('app.runtimeLog.node')" for="runtime-log-node">
        <VustSelect
          id="runtime-log-node"
          v-model="form.nodeId"
          name="runtimeLogNode"
          :options="nodeOptions"
          :disabled="form.service === 'master'"
        />
      </VustFormItem>
      <VustFormItem class="file-select" :label="t('app.runtimeLog.file')" for="runtime-log-file">
        <VustSelect
          id="runtime-log-file"
          v-model="form.fileId"
          name="runtimeLogFile"
          :options="fileOptions"
          :disabled="filesLoading || !files.length"
        />
      </VustFormItem>
      <div class="toolbar-actions" data-ui="toolbar-actions" data-slot="actions">
        <VustButton
          :aria-expanded="filtersExpanded"
          aria-controls="runtime-log-advanced-filters"
          @click="filtersExpanded = !filtersExpanded"
          >{{
            filtersExpanded ? t('app.runtimeLog.collapseFilters') : t('app.runtimeLog.moreFilters')
          }}</VustButton
        >
        <VustButton
          type="primary"
          :disabled="!selectedFile"
          :loading="queryLoading && !lines.length"
          @click="queryLogs(false)"
          >{{ t('app.runtimeLog.query') }}</VustButton
        >
        <VustButton :loading="filesLoading" @click="loadFiles">{{
          t('app.runtimeLog.refreshFiles')
        }}</VustButton>
      </div>
    </div>

    <div
      v-if="filtersExpanded"
      id="runtime-log-advanced-filters"
      class="advanced-filters"
      data-ui="filters"
      data-slot="controls"
    >
      <VustFormItem class="level-select" :label="t('app.runtimeLog.level')" for="runtime-log-level">
        <VustSelect
          id="runtime-log-level"
          v-model="form.level"
          name="runtimeLogLevel"
          :options="levelOptions"
        />
      </VustFormItem>
      <VustFormItem
        class="advanced-input"
        :label="t('app.runtimeLog.target')"
        for="runtime-log-target"
      >
        <VustInput
          id="runtime-log-target"
          v-model="form.target"
          name="runtimeLogTarget"
          :aria-label="t('app.runtimeLog.target')"
        />
      </VustFormItem>
      <VustFormItem
        class="advanced-input"
        :label="t('app.runtimeLog.keyword')"
        for="runtime-log-keyword"
      >
        <VustInput
          id="runtime-log-keyword"
          v-model="form.keyword"
          name="runtimeLogKeyword"
          :aria-label="t('app.runtimeLog.keyword')"
          @keyup.enter="queryLogs(false)"
        />
      </VustFormItem>
    </div>

    <VustAlert
      v-if="filesError"
      data-ui="files-error"
      type="error"
      :title="t('app.runtimeLog.filesFailed')"
      :description="filesError"
      show-icon
    />
    <VustAlert
      v-if="refreshWarning"
      data-ui="refresh-warning"
      type="warning"
      :title="t('app.runtimeLog.refreshFailed')"
      :description="refreshWarning"
      show-icon
    />
    <VustAlert
      v-if="queryError"
      data-ui="query-error"
      type="error"
      :title="t('app.runtimeLog.queryFailed')"
      :description="queryError"
      show-icon
    />
    <VustAlert
      v-if="scanTruncated"
      data-ui="scan-truncated"
      type="warning"
      :title="t('app.runtimeLog.scanTruncated')"
      show-icon
    />

    <div class="log-shell" data-ui="runtime-log-lines" data-slot="content">
      <div v-if="lines.length" class="log-lines" role="log" aria-live="polite">
        <div
          v-for="line in lines"
          :key="`${line.offset}:${line.timestamp ?? ''}`"
          class="log-line"
          data-slot="log-line"
        >
          <time>{{ formatTime(line.timestamp) }}</time>
          <div class="level-cell" data-slot="log-level">
            <VustTag :type="levelTag(line.level)">{{
              line.level || t('app.runtimeLog.unparsed')
            }}</VustTag>
          </div>
          <VustTooltip
            class="log-field-tooltip target-tooltip"
            :text="line.target || ''"
            :disabled="!line.target || !truncatedFields.has(`${line.offset}:target`)"
            position="bottom"
            @pointerover.capture="updateFieldTruncation($event, `${line.offset}:target`)"
          >
            <span class="target" data-slot="log-field">{{ line.target || '' }}</span>
          </VustTooltip>
          <VustTooltip
            class="log-field-tooltip"
            :text="line.message"
            :disabled="!truncatedFields.has(`${line.offset}:message`)"
            position="bottom"
            @pointerover.capture="updateFieldTruncation($event, `${line.offset}:message`)"
          >
            <span class="message" data-slot="log-field">{{ line.message }}</span>
          </VustTooltip>
        </div>
        <div class="load-more" data-slot="load-more">
          <VustButton v-if="hasMore" :loading="queryLoading" @click="queryLogs(true)">{{
            t('app.runtimeLog.loadMore')
          }}</VustButton>
        </div>
      </div>
      <VustEmpty
        v-else-if="!filesLoading && !queryLoading"
        :description="
          files.length
            ? queryLoadedAt
              ? t('app.runtimeLog.noMatches')
              : t('app.runtimeLog.selectAndQuery')
            : availabilityText
        "
      />
      <VustLoading :loading="filesLoading || queryLoading" cover />
    </div>
  </div>
</template>

<style scoped>
.runtime-log {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-3);
  background: var(--vdl-bg-canvas);
  color: var(--vdl-text-primary);
}
.toolbar,
.advanced-filters {
  display: flex;
  align-items: flex-end;
  flex-wrap: wrap;
  gap: var(--vdl-space-2);
}
.primary-select {
  flex: 0 0 180px;
  inline-size: 180px;
  max-inline-size: 100%;
}
.file-select {
  flex: 1 1 280px;
  min-inline-size: 240px;
}
.toolbar-actions {
  display: flex;
  align-items: flex-end;
  justify-content: flex-end;
  gap: var(--vdl-space-2);
  margin-block-end: var(--vdl-space-4);
  margin-inline-start: auto;
}
.advanced-filters {
  padding-block: var(--vdl-space-2);
  border-block: 1px solid var(--vdl-border-subtle);
}
.level-select {
  flex: 0 0 180px;
}
.advanced-input {
  flex: 1 1 240px;
}
.log-shell {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: auto;
  border: 1px solid var(--vdl-border-subtle);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-sunken);
}
.log-lines {
  min-width: 760px;
  font-family: var(--vdl-font-mono);
  font-size: var(--vdl-font-caption);
}
.log-line {
  display: grid;
  grid-template-columns: 19ch 74px 25ch minmax(0, 1fr);
  align-items: center;
  gap: var(--vdl-space-4);
  padding: var(--vdl-space-2) var(--vdl-space-3);
  border-bottom: 1px solid var(--vdl-border-subtle);
}
.log-line time,
.target {
  color: var(--vdl-text-muted);
}
.log-line time,
.log-field-tooltip,
.target,
.message {
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.target,
.message {
  display: block;
}
.level-cell {
  display: flex;
  justify-content: center;
}
.load-more {
  display: flex;
  justify-content: center;
  padding: var(--vdl-space-3);
}
@media (max-width: 760px) {
  .toolbar > *,
  .advanced-filters > * {
    flex: 1 1 180px;
  }
  .toolbar-actions {
    flex-basis: 100%;
    margin-inline-start: 0;
  }
}
</style>
