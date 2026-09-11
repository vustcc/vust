<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { VustTableColumn } from '@/components/ui/VustTable.vue'
import {
  VustActionMenu,
  VustAlert,
  VustButton,
  VustDescriptions,
  VustDialog,
  VustDrawer,
  VustEmpty,
  VustFormItem,
  VustInput,
  VustLoading,
  VustSelect,
  VustTable,
  VustTag,
} from '@/components/ui'
import type {
  DockerVolumeContainerReference,
  DockerVolumeManagementKind,
  DockerVolumeSummary,
} from '@/api/interface/docker'
import { useConfirmationModalStore } from '@/stores/confirmation-modal'
import { useDockerStore } from '@/stores/docker'
import { useNodeStore } from '@/stores/node'
import { formatKeyValue, parseKeyValueLines } from '@/utils/docker-format'
import { formatDateTime } from '@/utils/time'

defineOptions({ inheritAttrs: false })

/** Docker 卷列表、本地卷创建和按需详情管理页。 */
const { t } = useI18n()
const store = useDockerStore()
const nodeStore = useNodeStore()
const modalStore = useConfirmationModalStore()

const search = ref('')
const managementFilter = ref<'' | DockerVolumeManagementKind>('')
const createVisible = ref(false)
const advancedVisible = ref(false)
const createError = ref('')
const detailVisible = ref(false)
const selectedSummary = ref<DockerVolumeSummary | null>(null)
const form = ref({ name: '', options: '', labels: '' })

const columns = computed<VustTableColumn[]>(() => [
  { label: t('app.docker.volumes.columns.name'), minWidth: 230, slot: 'name' },
  { label: t('app.docker.volumes.columns.management'), width: 150, slot: 'management' },
  {
    label: t('app.docker.volumes.columns.createdAt'),
    width: 180,
    slot: 'createdAt',
    align: 'center',
  },
  { label: t('app.docker.volumes.columns.actions'), width: 110, align: 'center', slot: 'actions' },
])

const referenceColumns = computed<VustTableColumn[]>(() => [
  { label: t('app.docker.volumes.detail.containerName'), minWidth: 170, slot: 'name' },
  {
    label: t('app.docker.volumes.detail.containerState'),
    width: 110,
    slot: 'state',
    align: 'center',
  },
  { label: t('app.docker.volumes.detail.destination'), minWidth: 180, slot: 'destination' },
  {
    label: t('app.docker.volumes.detail.accessMode'),
    width: 100,
    slot: 'accessMode',
    align: 'center',
  },
])

const managementOptions = computed(() => [
  { label: t('app.docker.volumes.filters.allManagement'), value: '' },
  ...(['suite', 'compose', 'custom'] as const).map((value) => ({
    label: t(`app.docker.volumes.management.${value}`),
    value,
  })),
])

const filteredVolumes = computed(() => {
  const keyword = search.value.trim().toLowerCase()
  return store.volumes.filter((volume) => {
    if (keyword && !volume.name.toLowerCase().includes(keyword)) return false
    if (managementFilter.value && volume.management.kind !== managementFilter.value) return false
    return true
  })
})

const currentDetail = computed(() => store.volumeDetail)
const currentSummary = computed(() => currentDetail.value?.summary ?? selectedSummary.value)

const detailItems = computed(() => {
  const summary = currentSummary.value
  if (!summary) return []
  return [
    { label: t('app.docker.volumes.detail.name'), value: summary.name },
    { label: t('app.docker.volumes.detail.management'), value: managementLabel(summary) },
    { label: t('app.docker.volumes.detail.createdAt'), value: formatDateTime(summary.createdAt) },
    { label: t('app.docker.volumes.detail.scope'), value: currentDetail.value?.scope || '-' },
    {
      label: t('app.docker.volumes.detail.referenceCount'),
      value: currentDetail.value ? String(currentDetail.value.referencedContainers.length) : '-',
    },
    {
      label: t('app.docker.volumes.detail.mountpoint'),
      value: currentDetail.value?.mountpoint || '-',
      span: 2,
    },
    {
      label: t('app.docker.volumes.detail.options'),
      value: formatKeyValue(currentDetail.value?.options),
      span: 2,
    },
    {
      label: t('app.docker.volumes.detail.labels'),
      value: formatKeyValue(currentDetail.value?.labels),
      span: 2,
    },
  ]
})

/** 返回卷归属的本地化名称。 */
function managementLabel(volume: DockerVolumeSummary): string {
  const label = t(`app.docker.volumes.management.${volume.management.kind}`)
  return volume.management.ownerName ? `${label} · ${volume.management.ownerName}` : label
}

/** 返回卷归属标签的固定 VDL 语义色。 */
function managementTagType(kind: DockerVolumeManagementKind): 'primary' | 'warning' | 'info' {
  if (kind === 'custom') return 'primary'
  if (kind === 'compose') return 'info'
  return 'warning'
}

/** 返回容器状态标签的语义色。 */
function stateTagType(state: string): 'success' | 'warning' | 'danger' | 'default' {
  if (state === 'running') return 'success'
  if (state === 'paused' || state === 'restarting') return 'warning'
  if (state === 'exited' || state === 'dead') return 'danger'
  return 'default'
}

/** 返回容器状态的本地化名称。 */
function stateLabel(state: string): string {
  const knownStates = ['created', 'running', 'paused', 'restarting', 'removing', 'exited', 'dead']
  return knownStates.includes(state)
    ? t(`app.docker.volumes.detail.states.${state}`)
    : t('app.docker.volumes.detail.states.unknown')
}

/** 打开并重置本地卷创建对话框。 */
function openCreate(): void {
  form.value = { name: '', options: '', labels: '' }
  advancedVisible.value = false
  createError.value = ''
  store.volumeCreateError = null
  createVisible.value = true
}

/** 在未提交时关闭创建对话框。 */
function closeCreate(): void {
  if (store.volumeCreateLoading) return
  createVisible.value = false
  createError.value = ''
  store.volumeCreateError = null
}

/** 解析高级键值配置并提供字段级错误。 */
function parseFormMap(value: string, label: string): Record<string, string> | undefined {
  const parsed = parseKeyValueLines(value)
  if (parsed === null) throw new Error(t('app.docker.volumes.create.invalidKeyValue', { label }))
  return Object.keys(parsed).length ? parsed : undefined
}

/** 校验并提交本地卷创建请求。 */
async function submitCreate(): Promise<void> {
  createError.value = ''
  const name = form.value.name.trim()
  if (!/^[A-Za-z0-9][A-Za-z0-9_.-]{1,254}$/.test(name)) {
    createError.value = t('app.docker.volumes.create.invalidName')
    return
  }
  try {
    const options = parseFormMap(form.value.options, t('app.docker.volumes.create.options'))
    const labels = parseFormMap(form.value.labels, t('app.docker.volumes.create.labels'))
    const reserved = Object.keys(labels ?? {}).find(
      (key) => key.startsWith('vust.') || key.startsWith('com.docker.compose.'),
    )
    if (reserved) {
      createError.value = t('app.docker.volumes.create.reservedLabel', { key: reserved })
      return
    }
    if (await store.createVolume({ name, options, labels })) closeCreate()
  } catch (error) {
    createError.value = error instanceof Error ? error.message : t('common.unknownError')
  }
}

/** 打开卷详情并按需加载引用容器。 */
async function openDetail(volume: DockerVolumeSummary): Promise<void> {
  store.clearVolumeDetail()
  selectedSummary.value = volume
  detailVisible.value = true
  await store.fetchVolumeDetail(volume.name)
}

/** 关闭详情并使未完成的请求失效。 */
function closeDetail(): void {
  detailVisible.value = false
  selectedSummary.value = null
  store.clearVolumeDetail()
}

/** 删除允许管理的数据卷。 */
async function deleteVolume(volume: DockerVolumeSummary): Promise<void> {
  if (!volume.capabilities.canRemove) return
  const confirmed = await modalStore.showConfirmation(
    t(
      volume.management.kind === 'custom'
        ? 'app.docker.messages.deleteVolumeConfirm'
        : 'app.docker.messages.deleteManagedVolumeConfirm',
      { name: volume.name },
    ),
    t('app.docker.messages.deleteConfirmTitle'),
    t('app.docker.messages.deleteAction'),
    t('confirmation.cancel'),
  )
  if (!confirmed) return
  const deleted = await store.removeVolume(volume)
  if (deleted && selectedSummary.value?.name === volume.name) closeDetail()
}

/** 生成卷行操作。 */
function rowActions(volume: DockerVolumeSummary) {
  return [
    {
      label: t('app.docker.volumes.actions.view'),
      handler: () => void openDetail(volume),
    },
    {
      label: t('app.docker.volumes.actions.delete'),
      handler: () => void deleteVolume(volume),
      class: 'app-btn-delete',
      disabled: !volume.capabilities.canRemove,
    },
  ]
}

watch(
  () => nodeStore.currentNodeId,
  (_nodeId, previousNodeId) => {
    if (previousNodeId !== undefined) closeDetail()
    void store.fetchVolumes()
  },
  { immediate: true },
)
</script>

<template>
  <div class="volume-page" data-page="docker-volumes">
    <div class="volume-toolbar" data-ui="toolbar">
      <div class="toolbar-filters" data-slot="filters">
        <label class="sr-only" for="docker-volume-search">
          {{ t('app.docker.volumes.filters.searchPlaceholder') }}
        </label>
        <VustInput
          id="docker-volume-search"
          v-model="search"
          class="search-input"
          :placeholder="t('app.docker.volumes.filters.searchPlaceholder')"
        />
        <VustSelect v-model="managementFilter" class="filter-select" :options="managementOptions" />
      </div>
      <div class="toolbar-actions" data-slot="actions">
        <VustButton :loading="store.volumeListLoading" @click="store.fetchVolumes">
          {{ t('common.refresh') }}
        </VustButton>
        <VustButton type="primary" @click="openCreate">
          {{ t('app.docker.volumes.actions.create') }}
        </VustButton>
      </div>
    </div>

    <VustAlert
      v-if="store.volumeListError"
      type="warning"
      :title="t('app.docker.volumes.refreshFailed')"
      :description="store.volumeListError"
      show-icon
      data-ui="volume-list-error"
    />
    <VustAlert
      v-if="store.volumeWarnings.length"
      type="warning"
      :title="t('app.docker.volumes.daemonWarnings')"
      :description="store.volumeWarnings.join('; ')"
      show-icon
      data-ui="volume-daemon-warnings"
    />

    <div class="volume-table-shell" data-ui="table">
      <VustTable v-if="filteredVolumes.length" :data="filteredVolumes" :columns="columns" border>
        <template #name="{ row }: { row: DockerVolumeSummary }">
          <button class="volume-name" type="button" @click="openDetail(row)">
            {{ row.name }}
          </button>
        </template>
        <template #management="{ row }: { row: DockerVolumeSummary }">
          <VustTag :type="managementTagType(row.management.kind)" size="small">
            {{ managementLabel(row) }}
          </VustTag>
        </template>
        <template #createdAt="{ row }: { row: DockerVolumeSummary }">
          {{ formatDateTime(row.createdAt) }}
        </template>
        <template #actions="{ row }: { row: DockerVolumeSummary }">
          <VustActionMenu
            :label="t('app.docker.volumes.actions.menu')"
            :disabled="store.volumeDeleteLoadingName === row.name"
            :actions="rowActions(row)"
          />
        </template>
      </VustTable>
      <VustEmpty
        v-else-if="!store.volumeListLoading"
        :description="
          search || managementFilter
            ? t('app.docker.volumes.filteredEmpty')
            : t('app.docker.volumes.empty')
        "
      />
      <VustLoading :loading="store.volumeListLoading && !store.volumes.length" cover />
    </div>

    <VustDialog
      :visible="createVisible"
      :title="t('app.docker.volumes.create.title')"
      width="620px"
      :close-on-click-overlay="!store.volumeCreateLoading"
      data-ui="volume-create-dialog"
      @close="closeCreate"
    >
      <div class="create-form" data-slot="body">
        <VustAlert
          v-if="createError || store.volumeCreateError"
          type="error"
          :title="createError || store.volumeCreateError || ''"
          show-icon
        />
        <VustFormItem
          :label="t('app.docker.volumes.create.name')"
          for="docker-volume-name"
          required
        >
          <VustInput
            id="docker-volume-name"
            v-model="form.name"
            :placeholder="t('app.docker.volumes.create.namePlaceholder')"
          />
        </VustFormItem>
        <VustButton type="secondary" size="small" @click="advancedVisible = !advancedVisible">
          {{
            advancedVisible
              ? t('app.docker.volumes.create.hideAdvanced')
              : t('app.docker.volumes.create.showAdvanced')
          }}
        </VustButton>
        <div v-if="advancedVisible" class="advanced-fields" data-slot="advanced">
          <VustFormItem
            :label="t('app.docker.volumes.create.options')"
            :hint="t('app.docker.volumes.create.keyValueHint')"
            for="docker-volume-options"
          >
            <VustInput
              id="docker-volume-options"
              v-model="form.options"
              type="textarea"
              :rows="4"
            />
          </VustFormItem>
          <VustFormItem
            :label="t('app.docker.volumes.create.labels')"
            :hint="t('app.docker.volumes.create.labelHint')"
            for="docker-volume-labels"
          >
            <VustInput id="docker-volume-labels" v-model="form.labels" type="textarea" :rows="4" />
          </VustFormItem>
        </div>
      </div>
      <template #footer>
        <VustButton :disabled="store.volumeCreateLoading" @click="closeCreate">
          {{ t('common.cancel') }}
        </VustButton>
        <VustButton type="primary" :loading="store.volumeCreateLoading" @click="submitCreate">
          {{ t('app.docker.volumes.actions.submit') }}
        </VustButton>
      </template>
    </VustDialog>

    <VustDrawer
      v-model="detailVisible"
      :title="t('app.docker.volumes.detail.title')"
      width="760px"
      data-ui="volume-detail-drawer"
      @close="closeDetail"
    >
      <div class="detail-content" data-slot="detail">
        <VustAlert
          v-if="store.volumeDetailError"
          type="warning"
          :title="t('app.docker.volumes.detail.loadFailed')"
          :description="store.volumeDetailError"
          show-icon
        />
        <VustDescriptions v-if="currentSummary" :items="detailItems" :column="2" border />

        <div class="detail-block" data-slot="references">
          <div class="block-title">{{ t('app.docker.volumes.detail.references') }}</div>
          <VustTable
            v-if="currentDetail?.referencedContainers.length"
            :data="currentDetail.referencedContainers"
            :columns="referenceColumns"
            border
          >
            <template #name="{ row }: { row: DockerVolumeContainerReference }">
              <span class="container-name">{{ row.name }}</span>
            </template>
            <template #state="{ row }: { row: DockerVolumeContainerReference }">
              <VustTag :type="stateTagType(row.state)" size="small">{{
                stateLabel(row.state)
              }}</VustTag>
            </template>
            <template #destination="{ row }: { row: DockerVolumeContainerReference }">
              <span class="mono-text">{{ row.destination || '-' }}</span>
            </template>
            <template #accessMode="{ row }: { row: DockerVolumeContainerReference }">
              {{
                row.readOnly
                  ? t('app.docker.volumes.detail.readOnlyMode')
                  : t('app.docker.volumes.detail.readWriteMode')
              }}
            </template>
          </VustTable>
          <VustEmpty
            v-else-if="!store.volumeDetailLoading"
            :description="t('app.docker.volumes.detail.noReferences')"
          />
        </div>
        <VustLoading :loading="store.volumeDetailLoading && !currentDetail" cover />
      </div>
    </VustDrawer>
  </div>
</template>

<style scoped>
.volume-page {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
  padding-top: var(--vdl-space-3);
}
.volume-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-4);
  padding: var(--vdl-space-3) var(--vdl-space-4);
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-panel);
}
.toolbar-filters,
.toolbar-actions {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
}
.search-input {
  width: 240px;
}
.filter-select {
  width: 150px;
}
.volume-table-shell {
  position: relative;
  flex: 1;
  min-height: 260px;
  overflow: auto;
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-panel);
}
.volume-name {
  padding: 0;
  border: 0;
  background: transparent;
  color: var(--vdl-primary);
  font: inherit;
  font-weight: 600;
  cursor: pointer;
  word-break: break-all;
}
.volume-name:hover {
  text-decoration: underline;
}
.mono-text {
  font-family: var(--vdl-font-mono);
  word-break: break-all;
}
.create-form,
.advanced-fields,
.detail-content,
.detail-block {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
}
.block-title {
  color: var(--vdl-text-primary);
  font-weight: 600;
}
.container-name {
  font-weight: 600;
}
.sr-only {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}
@media (max-width: 980px) {
  .volume-toolbar {
    align-items: stretch;
    flex-direction: column;
  }
  .toolbar-filters,
  .toolbar-actions {
    flex-wrap: wrap;
  }
}
@media (max-width: 680px) {
  .search-input,
  .filter-select {
    width: 100%;
  }
  .toolbar-filters,
  .toolbar-actions {
    align-items: stretch;
    flex-direction: column;
  }
}
</style>
