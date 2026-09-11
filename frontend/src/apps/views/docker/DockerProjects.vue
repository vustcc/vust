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

const { t } = useI18n()
const store = useDockerStore()
const nodeStore = useNodeStore()
const confirmationStore = useConfirmationModalStore()

const keyword = ref('')
const managementKind = ref<dockerType.DockerProjectManagementKind | ''>('')
const runtimeState = ref<dockerType.DockerProjectRuntimeState | ''>('')
const detailVisible = ref(false)
const detailProjectName = ref('')
const createVisible = ref(false)
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
const deploymentProgressItems = computed(() => store.projectDeploymentProgress?.progressItems ?? [])
const deploymentProgressPhases = computed(() => {
  const phases: dockerType.DockerProjectProgressPhase[] = ['pulling', 'applying']
  return phases
    .map((phase) => ({
      phase,
      items: deploymentProgressItems.value.filter((item) => item.phase === phase),
    }))
    .filter((group) => group.items.length)
})
const deploymentStageSteps = computed(() => [
  { key: 'preparing', label: t('app.docker.projects.deploymentProgress.stageSteps.preparing') },
  { key: 'pulling', label: t('app.docker.projects.deploymentProgress.stageSteps.pulling') },
  { key: 'applying', label: t('app.docker.projects.deploymentProgress.stageSteps.applying') },
  { key: 'completed', label: t('app.docker.projects.deploymentProgress.stageSteps.completed') },
])

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
  createVisible.value = true
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
  createError.value = ''
  if (createYamlError.value) return
  const validation = await store.validateComposeYaml(createForm.composeYaml)
  if (!validation) {
    createError.value = store.projectConfigurationError || t('common.unknownError')
    return
  }
  if (!validation.valid) {
    createError.value = validation.error || t('app.docker.projects.configuration.invalid')
    return
  }
  if (
    await store.createComposeProject({
      name: createForm.name.trim(),
      composeYaml: createForm.composeYaml,
    })
  ) {
    createVisible.value = false
  }
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
const operationStatusType = (status: dockerType.DockerProjectTaskStatus) => {
  if (status === 'succeeded') return 'success'
  if (status === 'failed') return 'danger'
  if (status === 'cancelled') return 'warning'
  return 'info'
}
const progressStatusType = (status: dockerType.DockerProjectProgressStatus) => {
  if (status === 'done') return 'success'
  if (status === 'warning') return 'warning'
  if (status === 'error') return 'danger'
  return 'info'
}
const progressActionLabel = (action: string) => {
  const key = action.trim().toLowerCase().replaceAll(' ', '_')
  const knownActions = new Set([
    'creating',
    'starting',
    'started',
    'waiting',
    'healthy',
    'running',
    'created',
    'stopping',
    'stopped',
    'removing',
    'removed',
    'building',
    'built',
    'pulling',
    'pulled',
    'downloading',
    'download_complete',
  ])
  return knownActions.has(key) ? t(`app.docker.projects.deploymentProgress.actions.${key}`) : action
}
const progressItemChildren = (parentId: string) =>
  deploymentProgressItems.value.filter((item) => item.parentId === parentId)
const progressRootItems = (items: dockerType.DockerProjectTaskProgressItem[]) => {
  const ids = new Set(items.map((item) => item.id))
  return items.filter((item) => !item.parentId || !ids.has(item.parentId))
}
const formatProgressBytes = (value: number) => {
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']
  let amount = Math.max(0, value)
  let unit = 0
  while (amount >= 1024 && unit < units.length - 1) {
    amount /= 1024
    unit += 1
  }
  const digits = unit === 0 || amount >= 100 ? 0 : 1
  return `${amount.toFixed(digits)} ${units[unit]}`
}
const progressBytesLabel = (item: dockerType.DockerProjectTaskProgressItem) => {
  if (
    typeof item.currentBytes === 'number' &&
    Number.isFinite(item.currentBytes) &&
    typeof item.totalBytes === 'number' &&
    Number.isFinite(item.totalBytes) &&
    item.totalBytes > 0
  ) {
    return `${formatProgressBytes(item.currentBytes)} / ${formatProgressBytes(item.totalBytes)}`
  }
  return ''
}
const progressItemPercent = (item: dockerType.DockerProjectTaskProgressItem) => {
  if (typeof item.percent === 'number' && Number.isFinite(item.percent)) return item.percent
  const children = progressItemChildren(item.id)
  const byteMetrics = children
    .filter(
      (child) =>
        typeof child.currentBytes === 'number' &&
        Number.isFinite(child.currentBytes) &&
        typeof child.totalBytes === 'number' &&
        Number.isFinite(child.totalBytes),
    )
    .reduce(
      (metrics, child) => ({
        current: metrics.current + Math.min(child.currentBytes ?? 0, child.totalBytes ?? 0),
        total: metrics.total + (child.totalBytes ?? 0),
      }),
      { current: 0, total: 0 },
    )
  if (byteMetrics.total) return Math.round((byteMetrics.current / byteMetrics.total) * 100)
  const percentages = children
    .map((child) => child.percent)
    .filter((value): value is number => typeof value === 'number' && Number.isFinite(value))
  if (!percentages.length) return undefined
  return Math.round(percentages.reduce((sum, value) => sum + value, 0) / percentages.length)
}
const progressItemPercentLabel = (item: dockerType.DockerProjectTaskProgressItem) => {
  const percent = progressItemPercent(item)
  return percent === undefined ? '' : `${percent}%`
}
const progressPhaseCompleted = (phase: dockerType.DockerProjectProgressPhase) => {
  const items = deploymentProgressItems.value.filter((item) => item.phase === phase)
  return (
    items.length > 0 && items.every((item) => item.status === 'done' || item.status === 'warning')
  )
}
const deploymentStageState = (key: string) => {
  const operation = store.projectDeploymentProgress
  if (!operation) return 'pending'
  if (key === 'completed') {
    if (operation.status === 'failed' || operation.status === 'cancelled') return 'error'
    return operation.status === 'succeeded' ? 'done' : 'pending'
  }
  if (operation.status === 'failed' || operation.status === 'cancelled') {
    return operation.stage === key ? 'error' : 'done'
  }
  if (key === 'pulling' && progressPhaseCompleted('pulling')) return 'done'
  if (key === 'applying' && progressPhaseCompleted('applying')) return 'done'
  if (key === 'preparing' && deploymentProgressItems.value.length) return 'done'
  const order = ['preparing', 'pulling', 'applying']
  const currentStage = operation.stage === 'validating' ? 'preparing' : operation.stage
  const stepIndex = order.indexOf(key)
  const currentIndex = order.indexOf(currentStage)
  if (key === 'pulling' && currentIndex > stepIndex && !operation.pullImages) return 'skipped'
  if (stepIndex < currentIndex || operation.status === 'succeeded') return 'done'
  if (stepIndex === currentIndex) return 'active'
  return 'pending'
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
          @click="store.openProjectDeploymentProgress"
        >
          {{
            t('app.docker.projects.deploymentProgress.restore', {
              percent: store.projectDeploymentProgress.progressPercent,
            })
          }}
        </VustButton>
        <VustButton type="secondary" :loading="store.projectListLoading" @click="loadProjects()">{{
          t('common.refresh')
        }}</VustButton>
        <VustButton type="primary" @click="openCreate">{{
          t('app.docker.projects.actions.create')
        }}</VustButton>
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
        v-if="store.projectListError && !store.composeProjects.length && !store.projectListLoading"
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
          <VustTag :type="row.management.kind === 'custom' ? 'default' : 'primary'">{{
            t(`app.docker.projects.management.${row.management.kind}`)
          }}</VustTag>
        </template>
        <template #status="{ row }: { row: dockerType.DockerProjectSummary }">
          <VustTag :type="statusTagType(row.runtimeState)">{{
            t(`app.docker.projects.status.${row.runtimeState}`)
          }}</VustTag>
        </template>
        <template #runningTotal="{ row }: { row: dockerType.DockerProjectSummary }"
          >{{ row.containerStates.running }}/{{ row.containerStates.total }}</template
        >
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

    <VustDialog
      :visible="createVisible"
      :title="t('app.docker.projects.create.title')"
      width="820px"
      :close-on-click-overlay="!store.projectMutationLoading"
      data-ui="project-create-dialog"
      @close="createVisible = false"
    >
      <div class="project-form" data-slot="body">
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
        <VustButton :disabled="store.projectMutationLoading" @click="createVisible = false">{{
          t('common.cancel')
        }}</VustButton>
        <VustButton
          type="primary"
          :loading="store.projectMutationLoading"
          :disabled="Boolean(createYamlError)"
          @click="submitCreate"
          >{{ t('app.docker.projects.actions.submit') }}</VustButton
        >
      </template>
    </VustDialog>

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
      :visible="Boolean(store.projectDeploymentProgress) && store.projectDeploymentProgressVisible"
      :title="t('app.docker.projects.deploymentProgress.title')"
      width="760px"
      :close-on-click-overlay="false"
      data-ui="project-deployment-progress-dialog"
      @close="store.closeProjectDeploymentProgress"
    >
      <div v-if="store.projectDeploymentProgress" class="deployment-progress" data-slot="body">
        <VustAlert
          v-if="store.projectDeploymentProgressError"
          type="warning"
          :title="t('app.docker.projects.deploymentProgress.refreshFailed')"
          :description="store.projectDeploymentProgressError"
          show-icon
        />
        <div class="deployment-summary" data-ui="deployment-summary">
          <div class="deployment-identity" data-ui="deployment-identity">
            <strong data-ui="deployment-project-name">{{
              store.projectDeploymentProgress.projectName
            }}</strong>
            <div class="deployment-meta">
              <span>{{
                t(
                  `app.docker.projects.deploymentProgress.operations.${store.projectDeploymentProgress.operation}`,
                )
              }}</span>
              <VustTag :type="operationStatusType(store.projectDeploymentProgress.status)">{{
                t(
                  `app.docker.projects.deploymentProgress.statuses.${store.projectDeploymentProgress.status}`,
                )
              }}</VustTag>
            </div>
          </div>
        </div>
        <div class="progress-row" data-ui="deployment-overall-progress">
          <div class="progress-track" aria-hidden="true">
            <div
              class="progress-value"
              :style="{ width: `${store.projectDeploymentProgress.progressPercent}%` }"
            />
          </div>
          <span>{{ store.projectDeploymentProgress.progressPercent }}%</span>
        </div>
        <div class="deployment-stage-track" data-ui="deployment-stage-track">
          <div
            v-for="step in deploymentStageSteps"
            :key="step.key"
            class="deployment-stage-step"
            :class="`is-${deploymentStageState(step.key)}`"
          >
            <span class="deployment-stage-dot" aria-hidden="true" />
            <span>{{ step.label }}</span>
          </div>
        </div>
        <div
          v-if="store.projectDeploymentProgress.progressMode === 'text'"
          class="deployment-compatibility"
          data-ui="deployment-progress-compatibility"
        >
          {{ t('app.docker.projects.deploymentProgress.compatibilityText') }}
        </div>
        <div
          v-else-if="store.projectDeploymentProgress.progressMode === 'unavailable'"
          class="deployment-compatibility"
          data-ui="deployment-progress-compatibility"
        >
          {{ t('app.docker.projects.deploymentProgress.compatibilityUnavailable') }}
        </div>
        <div class="deployment-details" data-ui="deployment-progress-list">
          <div v-if="deploymentProgressPhases.length" class="deployment-phase-list">
            <div
              v-for="group in deploymentProgressPhases"
              :key="group.phase"
              class="deployment-phase"
              :data-slot="`deployment-phase-${group.phase}`"
            >
              <div class="deployment-phase-title" data-ui="deployment-phase-title">
                {{ t(`app.docker.projects.deploymentProgress.phases.${group.phase}`) }}
              </div>
              <div class="deployment-item-list">
                <div
                  v-for="item in progressRootItems(group.items)"
                  :key="item.id"
                  class="deployment-item"
                  :class="`is-${item.status}`"
                  data-ui="deployment-progress-item"
                >
                  <div class="deployment-item-main">
                    <div class="deployment-item-identity">
                      <strong data-ui="deployment-progress-item-label">{{ item.label }}</strong>
                    </div>
                    <div class="deployment-item-state">
                      <VustTag :type="progressStatusType(item.status)">{{
                        t(`app.docker.projects.deploymentProgress.itemStatuses.${item.status}`)
                      }}</VustTag>
                    </div>
                  </div>
                  <div
                    v-if="progressItemPercent(item) !== undefined || item.status === 'working'"
                    class="deployment-item-progress"
                  >
                    <div class="progress-track" aria-hidden="true">
                      <div
                        class="progress-value"
                        :class="{ 'is-indeterminate': progressItemPercent(item) === undefined }"
                        :style="
                          progressItemPercent(item) === undefined
                            ? undefined
                            : { width: `${progressItemPercent(item)}%` }
                        "
                      />
                    </div>
                    <span
                      v-if="progressItemPercentLabel(item)"
                      data-ui="deployment-progress-item-percent"
                      >{{ progressItemPercentLabel(item) }}</span
                    >
                  </div>
                  <div v-if="item.details" class="deployment-item-details">
                    {{ item.details }}
                  </div>
                  <div
                    v-if="progressItemChildren(item.id).length"
                    class="deployment-child-list"
                    data-slot="deployment-progress-children"
                  >
                    <div
                      v-for="child in progressItemChildren(item.id)"
                      :key="child.id"
                      class="deployment-child-item"
                      :class="`is-${child.status}`"
                      data-ui="deployment-progress-layer"
                    >
                      <div class="deployment-child-copy">
                        <strong data-ui="deployment-progress-layer-label">{{ child.label }}</strong>
                        <span>{{ progressActionLabel(child.action) }}</span>
                      </div>
                      <span v-if="progressBytesLabel(child)" class="deployment-bytes">{{
                        progressBytesLabel(child)
                      }}</span>
                      <span
                        v-if="progressItemPercentLabel(child)"
                        data-ui="deployment-progress-layer-percent"
                        >{{ progressItemPercentLabel(child) }}</span
                      >
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>
          <div v-else class="deployment-empty-progress">
            {{ t('app.docker.projects.deploymentProgress.waitingForDetails') }}
          </div>
        </div>
        <VustAlert
          v-if="store.projectDeploymentProgress.errorSummary"
          type="error"
          :title="t('app.docker.projects.deploymentProgress.failedTitle')"
          :description="store.projectDeploymentProgress.errorSummary"
          show-icon
        />
        <VustAlert
          v-if="store.projectDeploymentProgress.cleanupWarning"
          type="warning"
          :title="t('app.docker.projects.deploymentProgress.cleanupWarning')"
          :description="store.projectDeploymentProgress.cleanupWarning"
          show-icon
        />
      </div>
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
.dialog-content,
.deployment-progress {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-4);
}
.deployment-progress {
  min-height: 0;
}
.deployment-summary {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--vdl-space-5);
}
.deployment-identity {
  display: flex;
  align-items: center;
  min-width: 0;
  gap: var(--vdl-space-2);
}
.deployment-identity > strong {
  flex: 1;
  min-width: 0;
  font-size: var(--vdl-font-subtitle);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.deployment-meta {
  display: flex;
  align-items: center;
  flex-shrink: 0;
  gap: var(--vdl-space-2);
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-body-sm);
}
.progress-row {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-3);
}
.progress-row > span {
  width: 44px;
  flex-shrink: 0;
  font-variant-numeric: tabular-nums;
  text-align: right;
}
.progress-track {
  flex: 1;
  height: 8px;
  overflow: hidden;
  border-radius: var(--vdl-radius-pill);
  background: var(--vdl-bg-muted);
}
.progress-value {
  height: 100%;
  border-radius: inherit;
  background: var(--vdl-primary);
  transition: width 180ms ease;
}
.progress-value.is-indeterminate {
  width: 38%;
  animation: deployment-progress-indeterminate 1.2s ease-in-out infinite;
}
.deployment-stage-track {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  padding: var(--vdl-space-1) 0;
}
.deployment-stage-step {
  position: relative;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--vdl-space-1);
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-caption);
  text-align: center;
}
.deployment-stage-step::before,
.deployment-stage-step::after {
  position: absolute;
  top: 6px;
  height: 1px;
  background: var(--vdl-border-default);
  content: '';
}
.deployment-stage-step::before {
  left: 0;
  right: 50%;
}
.deployment-stage-step::after {
  left: 50%;
  right: 0;
}
.deployment-stage-step:first-child::before,
.deployment-stage-step:last-child::after {
  display: none;
}
.deployment-stage-dot {
  z-index: 1;
  width: 12px;
  height: 12px;
  border: 2px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-pill);
  background: var(--vdl-bg-panel);
}
.deployment-stage-step.is-active {
  color: var(--vdl-text-primary);
  font-weight: var(--vdl-font-weight-medium);
}
.deployment-stage-step.is-active .deployment-stage-dot {
  border-color: var(--vdl-primary);
  background: var(--vdl-primary);
}
.deployment-stage-step.is-done .deployment-stage-dot {
  border-color: var(--vdl-success);
  background: var(--vdl-success);
}
.deployment-stage-step.is-error .deployment-stage-dot {
  border-color: var(--vdl-danger);
  background: var(--vdl-danger);
}
.deployment-stage-step.is-skipped {
  opacity: 0.6;
}
.deployment-compatibility {
  padding: var(--vdl-space-2) var(--vdl-space-3);
  border-left: 2px solid var(--vdl-warning);
  color: var(--vdl-text-secondary);
  background: var(--vdl-bg-muted);
  font-size: var(--vdl-font-body-sm);
}
.deployment-details,
.deployment-phase-list,
.deployment-phase,
.deployment-item-list {
  display: flex;
  flex-direction: column;
  min-height: 0;
}
.deployment-phase-list {
  gap: var(--vdl-space-4);
}
.deployment-phase {
  gap: var(--vdl-space-2);
}
.deployment-phase-title {
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-body-sm);
  font-weight: var(--vdl-font-weight-medium);
}
.deployment-item-list {
  border-top: 1px solid var(--vdl-border-subtle);
}
.deployment-item {
  padding: var(--vdl-space-3) 0;
  border-bottom: 1px solid var(--vdl-border-subtle);
}
.deployment-item-main,
.deployment-item-state,
.deployment-item-progress,
.deployment-child-item {
  display: flex;
  align-items: center;
}
.deployment-item-main {
  justify-content: space-between;
  gap: var(--vdl-space-4);
}
.deployment-item-identity {
  display: flex;
  flex-direction: column;
  min-width: 0;
  gap: 2px;
}
.deployment-item-identity strong,
.deployment-child-copy strong {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.deployment-item-identity span,
.deployment-child-copy span,
.deployment-item-details,
.deployment-bytes {
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-caption);
}
.deployment-item-state {
  min-width: 64px;
  flex-shrink: 0;
  justify-content: flex-end;
  gap: var(--vdl-space-3);
}
.deployment-bytes {
  font-family: var(--vdl-font-mono);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
.deployment-item-progress {
  gap: var(--vdl-space-2);
  margin-top: var(--vdl-space-2);
  font-size: var(--vdl-font-caption);
  font-variant-numeric: tabular-nums;
}
.deployment-item-progress > span {
  width: 36px;
  color: var(--vdl-text-muted);
  text-align: right;
}
.deployment-item-details {
  margin-top: var(--vdl-space-2);
  overflow-wrap: anywhere;
}
.deployment-child-list {
  margin-top: var(--vdl-space-2);
  padding-left: var(--vdl-space-3);
  border-left: 1px solid var(--vdl-border-default);
}
.deployment-child-item {
  min-height: 28px;
  gap: var(--vdl-space-3);
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-caption);
}
.deployment-child-copy {
  display: flex;
  flex: 1;
  min-width: 0;
  gap: var(--vdl-space-2);
}
.deployment-child-copy strong {
  max-width: 45%;
}
.deployment-child-item > span:last-child {
  min-width: 36px;
  text-align: right;
}
.deployment-empty-progress {
  padding: var(--vdl-space-5);
  border: 1px dashed var(--vdl-border-default);
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-body-sm);
  text-align: center;
}
@keyframes deployment-progress-indeterminate {
  0% {
    transform: translateX(-110%);
  }
  100% {
    transform: translateX(300%);
  }
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
  .deployment-summary {
    gap: var(--vdl-space-3);
  }
  .deployment-item-main {
    align-items: flex-start;
    flex-direction: column;
    gap: var(--vdl-space-2);
  }
  .deployment-item-state {
    width: 100%;
    justify-content: space-between;
  }
  .deployment-child-copy {
    flex-direction: column;
    gap: 0;
  }
}
@media (prefers-reduced-motion: reduce) {
  .progress-value {
    transition: none;
  }
  .progress-value.is-indeterminate {
    animation: none;
  }
}
</style>
