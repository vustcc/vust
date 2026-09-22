<script setup lang="ts">
/** Docker Compose 项目管理页：查询、配置、部署进度与节点变化均在模块内处理。 */
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { load as parseYaml, YAMLException } from 'js-yaml'
import { useDockerStore } from '@/stores/docker'
import { useNodeStore } from '@/stores/node'
import { useConfirmationModalStore } from '@/stores/confirmation-modal'
import type * as dockerType from '@/api/interface/docker'
import MonacoEditor from '@/components/editor/MonacoEditor.vue'
import VustTable, { type VustTableColumn } from '@/components/ui/VustTable.vue'
import {
  VustActionMenu,
  VustAlert,
  VustButton,
  VustCheckbox,
  VustDialog,
  VustDrawer,
  VustEmpty,
  VustFormItem,
  VustInput,
  VustLoading,
  VustPagination,
  VustSelect,
  VustTag,
} from '@/components/ui'
import DockerProjectDetailDrawer from './DockerProjectDetailDrawer.vue'
import DockerProjectDeploymentProgress from './DockerProjectDeploymentProgress.vue'
import DockerContentWorkspace from './DockerContentWorkspace.vue'

const { t } = useI18n()
const store = useDockerStore()
const nodeStore = useNodeStore()
const confirmationStore = useConfirmationModalStore()

const keyword = ref('')
const managementKind = ref<dockerType.DockerProjectManagementKind | ''>('')
const runtimeState = ref<dockerType.DockerProjectRuntimeState | ''>('')
const detailVisible = ref(false)
const detailProjectName = ref('')
const viewMode = ref<'list' | 'create' | 'create-progress'>('list')
const createSubmitting = ref(false)
const createError = ref('')
const configurationVisible = ref(false)
const configurationProjectName = ref('')
const configurationYaml = ref('')
const configurationError = ref('')
const deploymentVisible = ref(false)
const deploymentProjectName = ref('')
const pullImages = ref(false)
const composeExample = 'services:\n  app:\n    image: nginx:latest\n'
const createForm = reactive({
  name: '',
  composeYaml: '',
})

const columns = computed<VustTableColumn[]>(() => [
  { label: t('app.docker.projects.columns.name'), minWidth: 220, slot: 'name' },
  { label: t('app.docker.projects.columns.management'), width: 140, slot: 'management' },
  { label: t('app.docker.projects.columns.status'), width: 130, align: 'center', slot: 'status' },
  {
    label: t('app.docker.projects.columns.services'),
    width: 100,
    align: 'center',
    prop: 'serviceCount',
  },
  {
    label: t('app.docker.projects.columns.runningTotal'),
    width: 140,
    align: 'center',
    slot: 'runningTotal',
  },
  { label: t('app.docker.projects.columns.actions'), width: 110, align: 'center', slot: 'actions' },
])
const managementOptions = computed(() => [
  { label: t('app.docker.projects.filters.allManagement'), value: '' },
  { label: t('app.docker.projects.management.custom'), value: 'custom' },
  { label: t('app.docker.projects.management.suite'), value: 'suite' },
  { label: t('app.docker.projects.management.system'), value: 'system' },
])
const runtimeOptions = computed(() => [
  { label: t('app.docker.projects.filters.allStatus'), value: '' },
  { label: t('app.docker.projects.status.running'), value: 'running' },
  { label: t('app.docker.projects.status.partial'), value: 'partial' },
  { label: t('app.docker.projects.status.stopped'), value: 'stopped' },
  { label: t('app.docker.projects.status.unknown'), value: 'unknown' },
])
const totalPages = computed(() =>
  Math.max(1, Math.ceil(store.projectTotal / store.projectPageSize)),
)
const hasFilters = computed(() =>
  Boolean(keyword.value || managementKind.value || runtimeState.value),
)
const createBusy = computed(() => createSubmitting.value || store.projectMutationLoading)

const loadProjects = (page = store.projectPage) =>
  store.fetchComposeProjects({
    keyword: keyword.value.trim() || undefined,
    managementKind: managementKind.value || undefined,
    runtimeState: runtimeState.value || undefined,
    page,
    pageSize: store.projectPageSize,
  })

/** 使用本地解析器即时检查 YAML 语法；Compose 语义仍由 Agent 校验。 */
const validateYamlSyntax = (value: string) => {
  if (!value.trim()) return ''
  try {
    parseYaml(value)
    return ''
  } catch (error) {
    if (!(error instanceof YAMLException)) return 'Invalid YAML syntax'
    if (!error.mark) return `YAML syntax error: ${error.reason}`
    const line = error.mark.line + 1
    const column = error.mark.column + 1
    return `YAML syntax error at line ${line}, column ${column}: ${error.reason}`
  }
}

const createYamlError = computed(() => validateYamlSyntax(createForm.composeYaml))
const configurationYamlError = computed(() => validateYamlSyntax(configurationYaml.value))

/** 接收创建编辑器内容，并清除已经失效的服务端错误。 */
const updateCreateYaml = (value: string) => {
  createForm.composeYaml = value
  createError.value = ''
}

/** 接收配置编辑器内容，并清除已经失效的服务端错误。 */
const updateConfigurationYaml = (value: string) => {
  configurationYaml.value = value
  configurationError.value = ''
}

const openDetail = async (name: string) => {
  detailProjectName.value = name
  detailVisible.value = true
  await store.fetchComposeProjectDetail(name)
}
const closeDetail = () => {
  detailVisible.value = false
  detailProjectName.value = ''
  store.clearComposeProjectDetail()
}
const openCreate = () => {
  createError.value = ''
  Object.assign(createForm, {
    name: '',
    composeYaml: '',
  })
  viewMode.value = 'create'
}
/** 放弃当前创建表单并恢复项目列表。 */
const closeCreate = () => {
  if (createBusy.value) return
  createError.value = ''
  Object.assign(createForm, {
    name: '',
    composeYaml: '',
  })
  viewMode.value = 'list'
}
const fillComposeExample = async () => {
  if (createForm.composeYaml.trim()) {
    const confirmed = await confirmationStore.showConfirmation(
      t('app.docker.projects.create.exampleConfirm'),
      t('app.docker.projects.create.exampleConfirmTitle'),
      t('app.docker.projects.create.fillExample'),
      t('common.cancel'),
    )
    if (!confirmed) return
  }
  createForm.composeYaml = composeExample
}
const submitCreate = async () => {
  if (createSubmitting.value) return
  createError.value = ''
  if (createYamlError.value) return
  const payload = {
    name: createForm.name.trim(),
    composeYaml: createForm.composeYaml,
  }
  createSubmitting.value = true
  try {
    const validation = await store.validateComposeYaml(payload.composeYaml)
    if (!validation) {
      createError.value = store.projectConfigurationError || t('common.unknownError')
      return
    }
    if (!validation.valid) {
      createError.value = validation.error || t('app.docker.projects.configuration.invalid')
      return
    }
    if (!(await store.createComposeProject(payload))) return
    viewMode.value =
      store.projectDeploymentProgress?.operation === 'create' ? 'create-progress' : 'list'
  } finally {
    createSubmitting.value = false
  }
}

/** 打开当前部署进度；创建任务使用内容区，其他任务沿用 Dialog。 */
const openDeploymentProgress = () => {
  if (!store.projectDeploymentProgress) return
  store.openProjectDeploymentProgress()
  if (store.projectDeploymentProgress.operation === 'create') {
    viewMode.value = 'create-progress'
  }
}

/** 将创建任务转入后台或关闭已完成任务，并恢复项目列表。 */
const leaveCreateProgress = () => {
  store.closeProjectDeploymentProgress()
  viewMode.value = 'list'
}

const openConfiguration = async (name: string) => {
  store.clearComposeProjectConfiguration()
  configurationProjectName.value = name
  configurationYaml.value = ''
  configurationError.value = ''
  configurationVisible.value = true
  if (await store.fetchComposeProjectConfiguration(name)) {
    configurationYaml.value = store.projectConfiguration?.composeYaml || ''
  }
}
const closeConfiguration = () => {
  configurationVisible.value = false
  configurationProjectName.value = ''
  configurationYaml.value = ''
  configurationError.value = ''
  store.clearComposeProjectConfiguration()
}
const saveConfiguration = async () => {
  if (!store.projectConfiguration) return
  configurationError.value = ''
  if (configurationYamlError.value) return
  const validation = await store.validateComposeYaml(configurationYaml.value)
  if (!validation?.valid) {
    configurationError.value =
      validation?.error ||
      store.projectConfigurationError ||
      t('app.docker.projects.configuration.invalid')
    return
  }
  const saved = await store.saveComposeProjectConfiguration(
    configurationProjectName.value,
    configurationYaml.value,
    store.projectConfiguration.revision,
  )
  if (saved) {
    closeConfiguration()
    await loadProjects()
  } else {
    configurationError.value = store.projectConfigurationError || t('common.unknownError')
  }
}

const openDeployment = (name: string) => {
  deploymentProjectName.value = name
  pullImages.value = false
  deploymentVisible.value = true
}
const submitDeployment = async () => {
  if (await store.redeployComposeProject(deploymentProjectName.value, pullImages.value)) {
    deploymentVisible.value = false
  }
}
const removeProject = async (name: string) => {
  const confirmed = await confirmationStore.showConfirmation(
    t('app.docker.projects.remove.confirm', { name }),
    t('app.docker.projects.remove.title'),
    t('common.delete'),
    t('common.cancel'),
  )
  if (confirmed) await store.removeComposeProject(name)
}

const rowActions = (project: dockerType.DockerProjectSummary) => {
  const actions: Array<{
    label: string
    handler: () => void
    disabled?: boolean
    className?: string
  }> = [{ label: t('app.docker.projects.actions.detail'), handler: () => openDetail(project.name) }]
  if (project.management.readOnly) return actions
  actions.push(
    {
      label: t('common.start'),
      handler: async () => {
        await store.runComposeProjectLifecycle(project.name, 'start')
      },
      disabled: !project.capabilities.canStart,
    },
    {
      label: t('common.stop'),
      handler: async () => {
        await store.runComposeProjectLifecycle(project.name, 'stop')
      },
      disabled: !project.capabilities.canStop,
    },
    {
      label: t('common.restart'),
      handler: async () => {
        await store.runComposeProjectLifecycle(project.name, 'restart')
      },
      disabled: !project.capabilities.canRestart,
    },
    {
      label: t('app.docker.projects.actions.editCompose'),
      handler: () => openConfiguration(project.name),
      disabled: !project.capabilities.canEditConfiguration,
    },
    {
      label: t('app.docker.projects.actions.redeploy'),
      handler: () => openDeployment(project.name),
      disabled: !project.capabilities.canRedeploy,
    },
    {
      label: t('common.delete'),
      handler: () => removeProject(project.name),
      disabled: !project.capabilities.canRemove,
      className: 'btn-delete',
    },
  )
  return actions
}

const statusTagType = (state: dockerType.DockerProjectRuntimeState) => {
  if (state === 'running') return 'success'
  if (state === 'partial') return 'warning'
  if (state === 'stopped') return 'info'
  return 'default'
}
const handleVisibility = () => {
  if (document.hidden) {
    store.stopProjectOperationPolling()
  } else {
    void store.recoverActiveComposeDeployment()
  }
}

watch([keyword, managementKind, runtimeState], () => void loadProjects(1))
watch(
  [() => store.projectDeploymentProgress, () => store.projectDeploymentProgressVisible],
  ([progress, visible]) => {
    if (!progress && viewMode.value === 'create-progress') {
      viewMode.value = 'list'
      return
    }
    if (progress?.operation === 'create' && visible && viewMode.value === 'list') {
      viewMode.value = 'create-progress'
    }
  },
  { immediate: true },
)
watch(
  () => nodeStore.currentNodeId,
  () => {
    closeDetail()
    closeConfiguration()
    void Promise.all([loadProjects(1), store.recoverActiveComposeDeployment()])
  },
)
onMounted(() => {
  void Promise.all([loadProjects(1), store.recoverActiveComposeDeployment()])
  document.addEventListener('visibilitychange', handleVisibility)
})
onUnmounted(() => {
  document.removeEventListener('visibilitychange', handleVisibility)
})
</script>

<template>
  <div class="project-page" data-page="docker-projects">
    <template v-if="viewMode === 'list'">
      <div class="project-toolbar" data-ui="toolbar">
        <div class="toolbar-filters">
          <VustInput
            id="docker-project-search"
            v-model="keyword"
            name="dockerProjectSearch"
            :placeholder="t('app.docker.projects.filters.searchPlaceholder')"
          />
          <VustSelect
            id="docker-project-management-filter"
            v-model="managementKind"
            name="dockerProjectManagementFilter"
            :options="managementOptions"
          />
          <VustSelect
            id="docker-project-status-filter"
            v-model="runtimeState"
            name="dockerProjectStatusFilter"
            :options="runtimeOptions"
          />
        </div>
        <div class="toolbar-actions">
          <VustButton
            v-if="store.projectDeploymentProgress && !store.projectDeploymentProgressVisible"
            type="info"
            size="small"
            data-ui="restore-deployment-progress"
            @click="openDeploymentProgress"
          >
            {{
              t('app.docker.projects.deploymentProgress.restore', {
                percent: store.projectDeploymentProgress.progressPercent,
              })
            }}
          </VustButton>
          <VustButton type="secondary" :loading="store.projectListLoading" @click="loadProjects()">
            {{ t('common.refresh') }}
          </VustButton>
          <VustButton type="primary" data-ui="project-create-button" @click="openCreate">
            {{ t('app.docker.projects.actions.create') }}
          </VustButton>
        </div>
      </div>

      <VustAlert
        v-if="store.projectListError && store.composeProjects.length"
        type="warning"
        :title="t('app.docker.projects.refreshFailed')"
        :description="store.projectListError"
        show-icon
        data-ui="refresh-warning"
      />
      <div class="project-content" data-slot="content">
        <VustAlert
          v-if="
            store.projectListError && !store.composeProjects.length && !store.projectListLoading
          "
          type="error"
          :title="t('app.docker.projects.loadFailed')"
          :description="store.projectListError"
          show-icon
        />
        <VustTable
          v-if="store.composeProjects.length"
          :data="store.composeProjects"
          :columns="columns"
          border
          data-ui="table"
        >
          <template #name="{ row }: { row: dockerType.DockerProjectSummary }">
            <button class="project-name" type="button" @click="openDetail(row.name)">
              {{ row.name }}
            </button>
          </template>
          <template #management="{ row }: { row: dockerType.DockerProjectSummary }">
            <VustTag :type="row.management.kind === 'custom' ? 'default' : 'primary'">
              {{ t(`app.docker.projects.management.${row.management.kind}`) }}
            </VustTag>
          </template>
          <template #status="{ row }: { row: dockerType.DockerProjectSummary }">
            <VustTag :type="statusTagType(row.runtimeState)">
              {{ t(`app.docker.projects.status.${row.runtimeState}`) }}
            </VustTag>
          </template>
          <template #runningTotal="{ row }: { row: dockerType.DockerProjectSummary }">
            {{ row.containerStates.running }}/{{ row.containerStates.total }}
          </template>
          <template #actions="{ row }: { row: dockerType.DockerProjectSummary }">
            <VustActionMenu
              :label="t('app.docker.projects.actions.menu')"
              :actions="rowActions(row)"
            />
          </template>
        </VustTable>
        <VustEmpty
          v-else-if="!store.projectListLoading && !store.projectListError"
          :description="
            hasFilters ? t('app.docker.projects.filteredEmpty') : t('app.docker.projects.empty')
          "
        />
        <VustLoading :loading="store.projectListLoading && !store.composeProjects.length" cover />
      </div>
      <div
        v-if="store.projectTotal > store.projectPageSize"
        class="project-pagination"
        data-ui="pagination"
      >
        <VustPagination
          :current-page="store.projectPage"
          :total-pages="totalPages"
          @page-change="loadProjects"
        />
      </div>
    </template>

    <DockerContentWorkspace
      v-else-if="viewMode === 'create'"
      :title="t('app.docker.projects.create.title')"
      :back-label="t('common.back')"
      :back-disabled="createBusy"
      return-focus-selector="[data-ui='project-create-button']"
      data-ui="project-create-view"
      @back="closeCreate"
    >
      <div class="project-form">
        <VustAlert v-if="createError" type="error" :title="createError" show-icon />
        <VustFormItem
          :label="t('app.docker.projects.create.name')"
          for="docker-project-name"
          required
        >
          <VustInput id="docker-project-name" v-model="createForm.name" name="projectName" />
        </VustFormItem>
        <div class="compose-field" data-ui="compose-configuration-field">
          <div class="compose-field-header">
            <div id="docker-project-compose-label" class="compose-field-label">
              <span class="required-mark" aria-hidden="true">*</span>
              {{ t('app.docker.projects.create.compose') }}
            </div>
            <VustButton
              type="secondary"
              size="small"
              data-ui="fill-compose-example"
              @click="fillComposeExample"
            >
              {{ t('app.docker.projects.create.fillExample') }}
            </VustButton>
          </div>
          <div class="compose-editor" data-ui="compose-editor">
            <MonacoEditor
              id="docker-project-compose"
              :model-value="createForm.composeYaml"
              name="composeYaml"
              language="yaml"
              :minimap="false"
              aria-labelledby="docker-project-compose-label"
              @update:model-value="updateCreateYaml"
            />
          </div>
          <h4
            v-if="createYamlError"
            class="compose-yaml-error"
            role="alert"
            data-ui="compose-yaml-error"
          >
            {{ createYamlError }}
          </h4>
        </div>
      </div>
      <template #footer>
        <VustButton
          type="primary"
          :loading="createBusy"
          :disabled="Boolean(createYamlError) || createBusy"
          data-ui="project-create-submit"
          @click="submitCreate"
        >
          {{ t('app.docker.projects.actions.submit') }}
        </VustButton>
      </template>
    </DockerContentWorkspace>

    <DockerContentWorkspace
      v-else-if="store.projectDeploymentProgress"
      :title="t('app.docker.projects.deploymentProgress.title')"
      return-focus-selector="[data-ui='restore-deployment-progress']"
      data-ui="project-create-progress-view"
    >
      <DockerProjectDeploymentProgress
        :task="store.projectDeploymentProgress"
        :refresh-error="store.projectDeploymentProgressError"
      />
      <template #footer>
        <VustButton data-ui="project-create-progress-back" @click="leaveCreateProgress">
          {{
            store.projectDeploymentProgress.status === 'queued' ||
            store.projectDeploymentProgress.status === 'running'
              ? t('app.docker.projects.deploymentProgress.background')
              : t('app.docker.projects.actions.backToList')
          }}
        </VustButton>
      </template>
    </DockerContentWorkspace>

    <DockerProjectDetailDrawer
      v-model="detailVisible"
      :project-name="detailProjectName"
      @close="closeDetail"
      @configure="openConfiguration"
      @redeploy="openDeployment"
    />

    <VustDrawer
      v-model="configurationVisible"
      :title="t('app.docker.projects.configuration.title', { name: configurationProjectName })"
      width="820px"
      data-ui="project-configuration-drawer"
      @close="closeConfiguration"
    >
      <div class="configuration-content" data-slot="body">
        <VustAlert
          v-if="configurationError || store.projectConfigurationError"
          type="error"
          :title="configurationError || store.projectConfigurationError || ''"
          show-icon
        />
        <VustAlert
          type="info"
          :title="t('app.docker.projects.configuration.saveOnlyTitle')"
          :description="t('app.docker.projects.configuration.saveOnlyDescription')"
          show-icon
        />
        <div
          v-if="store.projectConfiguration"
          class="configuration-compose-field"
          data-ui="compose-configuration-field"
        >
          <div class="compose-editor configuration-editor" data-ui="compose-editor">
            <MonacoEditor
              :model-value="configurationYaml"
              language="yaml"
              :minimap="false"
              :aria-label="t('app.docker.projects.configuration.editorLabel')"
              @update:model-value="updateConfigurationYaml"
            />
          </div>
          <h4
            v-if="configurationYamlError"
            class="compose-yaml-error"
            role="alert"
            data-ui="compose-yaml-error"
          >
            {{ configurationYamlError }}
          </h4>
        </div>
        <VustLoading
          :loading="store.projectConfigurationLoading && !store.projectConfiguration"
          cover
        />
      </div>
      <template #footer>
        <VustButton @click="closeConfiguration">{{ t('common.cancel') }}</VustButton>
        <VustButton
          type="primary"
          :loading="store.projectMutationLoading"
          :disabled="!store.projectConfiguration || Boolean(configurationYamlError)"
          @click="saveConfiguration"
          >{{ t('common.save') }}</VustButton
        >
      </template>
    </VustDrawer>

    <VustDialog
      :visible="deploymentVisible"
      :title="t('app.docker.projects.deployment.title')"
      width="520px"
      data-ui="project-deployment-dialog"
      @close="deploymentVisible = false"
    >
      <div class="dialog-content" data-slot="body">
        <VustAlert
          type="warning"
          :title="t('app.docker.projects.deployment.recreateTitle')"
          :description="t('app.docker.projects.deployment.recreateDescription')"
          show-icon
        />
        <VustCheckbox
          id="docker-project-pull-images"
          v-model="pullImages"
          name="pullImages"
          :aria-label="t('app.docker.projects.deployment.pullImages')"
          >{{ t('app.docker.projects.deployment.pullImages') }}</VustCheckbox
        >
      </div>
      <template #footer>
        <VustButton @click="deploymentVisible = false">{{ t('common.cancel') }}</VustButton>
        <VustButton
          type="warning"
          :loading="store.projectMutationLoading"
          @click="submitDeployment"
          >{{ t('app.docker.projects.actions.redeploy') }}</VustButton
        >
      </template>
    </VustDialog>

    <VustDialog
      :visible="
        Boolean(store.projectDeploymentProgress) &&
        store.projectDeploymentProgressVisible &&
        store.projectDeploymentProgress?.operation !== 'create'
      "
      :title="t('app.docker.projects.deploymentProgress.title')"
      width="760px"
      :close-on-click-overlay="false"
      data-ui="project-deployment-progress-dialog"
      @close="store.closeProjectDeploymentProgress"
    >
      <DockerProjectDeploymentProgress
        v-if="store.projectDeploymentProgress"
        :task="store.projectDeploymentProgress"
        :refresh-error="store.projectDeploymentProgressError"
        data-slot="body"
      />
      <template #footer>
        <VustButton @click="store.closeProjectDeploymentProgress">{{
          store.projectDeploymentProgress?.status === 'queued' ||
          store.projectDeploymentProgress?.status === 'running'
            ? t('app.docker.projects.deploymentProgress.background')
            : t('common.close')
        }}</VustButton>
      </template>
    </VustDialog>
  </div>
</template>

<style scoped>
.project-page {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
  padding-top: var(--vdl-space-3);
}
.project-toolbar,
.toolbar-filters,
.toolbar-actions {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-3);
}
.project-toolbar {
  justify-content: space-between;
  flex-wrap: wrap;
  padding: var(--vdl-space-3) var(--vdl-space-4);
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-panel);
}
.toolbar-filters {
  flex: 1;
  min-width: 0;
}
.project-content,
.configuration-content {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: auto;
}
.project-name {
  display: block;
  padding: 0;
  border: 0;
  color: var(--vdl-text-link);
  background: transparent;
  cursor: pointer;
  font: inherit;
  font-weight: var(--vdl-font-weight-medium);
}
.project-pagination {
  display: flex;
  justify-content: flex-end;
  flex-shrink: 0;
}
.project-form,
.configuration-content,
.dialog-content {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-4);
}
:deep([data-ui='project-deployment-progress-dialog'] .vl-dialog-body) {
  scrollbar-gutter: stable;
}
.compose-editor {
  height: 360px;
  min-height: 240px;
  overflow: hidden;
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
}
.compose-field,
.configuration-compose-field,
.compose-field-header {
  display: flex;
}
.compose-field,
.configuration-compose-field {
  flex-direction: column;
  gap: var(--vdl-space-2);
}
.compose-field-header {
  align-items: center;
  gap: var(--vdl-space-2);
}
.compose-field-label {
  color: var(--vdl-text-primary);
  font-size: var(--vdl-font-body-sm);
  font-weight: var(--vdl-font-weight-medium);
}
.compose-yaml-error {
  margin: 0;
  color: var(--vdl-danger);
  font-size: var(--vdl-font-caption);
  font-weight: 500;
  line-height: 1.4;
  overflow-wrap: anywhere;
}
.required-mark {
  color: var(--vdl-danger);
}
@media (max-width: 860px) {
  .project-toolbar,
  .toolbar-filters {
    align-items: stretch;
    flex-direction: column;
  }
  .toolbar-actions {
    justify-content: flex-end;
  }
}
</style>
