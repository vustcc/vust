<script setup lang="ts">
/** Docker Compose 项目部署任务的通用进度展示。 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type * as dockerType from '@/api/interface/docker'
import { VustAlert, VustTag } from '@/components/ui'

const props = defineProps<{
  task: dockerType.DockerProjectTask
  refreshError?: string | null
}>()

const { t } = useI18n()

const progressItems = computed(() => props.task.progressItems ?? [])
const progressPhases = computed(() => {
  const phases: dockerType.DockerProjectProgressPhase[] = ['pulling', 'applying']
  return phases
    .map((phase) => ({
      phase,
      items: progressItems.value.filter((item) => item.phase === phase),
    }))
    .filter((group) => group.items.length)
})
const stageSteps = computed(() => [
  { key: 'preparing', label: t('app.docker.projects.deploymentProgress.stageSteps.preparing') },
  { key: 'pulling', label: t('app.docker.projects.deploymentProgress.stageSteps.pulling') },
  { key: 'applying', label: t('app.docker.projects.deploymentProgress.stageSteps.applying') },
  { key: 'completed', label: t('app.docker.projects.deploymentProgress.stageSteps.completed') },
])

/** 返回任务状态对应的标签语义。 */
function operationStatusType(status: dockerType.DockerProjectTaskStatus) {
  if (status === 'succeeded') return 'success'
  if (status === 'failed') return 'danger'
  if (status === 'cancelled') return 'warning'
  return 'info'
}

/** 返回进度条目状态对应的标签语义。 */
function progressStatusType(status: dockerType.DockerProjectProgressStatus) {
  if (status === 'done') return 'success'
  if (status === 'warning') return 'warning'
  if (status === 'error') return 'danger'
  return 'info'
}

/** 将 Docker Compose 进度动作转换为本地化文案。 */
function progressActionLabel(action: string) {
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

function progressItemChildren(parentId: string) {
  return progressItems.value.filter((item) => item.parentId === parentId)
}

function progressRootItems(items: dockerType.DockerProjectTaskProgressItem[]) {
  const ids = new Set(items.map((item) => item.id))
  return items.filter((item) => !item.parentId || !ids.has(item.parentId))
}

function formatProgressBytes(value: number) {
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

function progressBytesLabel(item: dockerType.DockerProjectTaskProgressItem) {
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

function progressItemPercent(item: dockerType.DockerProjectTaskProgressItem) {
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

function progressItemPercentLabel(item: dockerType.DockerProjectTaskProgressItem) {
  const percent = progressItemPercent(item)
  return percent === undefined ? '' : `${percent}%`
}

function progressPhaseCompleted(phase: dockerType.DockerProjectProgressPhase) {
  const items = progressItems.value.filter((item) => item.phase === phase)
  return (
    items.length > 0 && items.every((item) => item.status === 'done' || item.status === 'warning')
  )
}

/** 计算部署阶段在步骤轨道上的视觉状态。 */
function deploymentStageState(key: string) {
  const operation = props.task
  if (key === 'completed') {
    if (operation.status === 'failed' || operation.status === 'cancelled') return 'error'
    return operation.status === 'succeeded' ? 'done' : 'pending'
  }
  if (operation.status === 'failed' || operation.status === 'cancelled') {
    return operation.stage === key ? 'error' : 'done'
  }
  if (key === 'pulling' && progressPhaseCompleted('pulling')) return 'done'
  if (key === 'applying' && progressPhaseCompleted('applying')) return 'done'
  if (key === 'preparing' && progressItems.value.length) return 'done'
  const order = ['preparing', 'pulling', 'applying']
  const currentStage = operation.stage === 'validating' ? 'preparing' : operation.stage
  const stepIndex = order.indexOf(key)
  const currentIndex = order.indexOf(currentStage)
  if (key === 'pulling' && currentIndex > stepIndex && !operation.pullImages) return 'skipped'
  if (stepIndex < currentIndex || operation.status === 'succeeded') return 'done'
  if (stepIndex === currentIndex) return 'active'
  return 'pending'
}
</script>

<template>
  <div class="deployment-progress" data-ui="project-deployment-progress">
    <VustAlert
      v-if="refreshError"
      type="warning"
      :title="t('app.docker.projects.deploymentProgress.refreshFailed')"
      :description="refreshError"
      show-icon
    />
    <div class="deployment-summary" data-ui="deployment-summary">
      <div class="deployment-identity" data-ui="deployment-identity">
        <strong data-ui="deployment-project-name">{{ task.projectName }}</strong>
        <div class="deployment-meta">
          <span>{{
            t(`app.docker.projects.deploymentProgress.operations.${task.operation}`)
          }}</span>
          <VustTag :type="operationStatusType(task.status)">
            {{ t(`app.docker.projects.deploymentProgress.statuses.${task.status}`) }}
          </VustTag>
        </div>
      </div>
    </div>
    <div class="progress-row" data-ui="deployment-overall-progress">
      <div class="progress-track" aria-hidden="true">
        <div class="progress-value" :style="{ width: `${task.progressPercent}%` }" />
      </div>
      <span>{{ task.progressPercent }}%</span>
    </div>
    <div class="deployment-stage-track" data-ui="deployment-stage-track">
      <div
        v-for="step in stageSteps"
        :key="step.key"
        class="deployment-stage-step"
        :class="`is-${deploymentStageState(step.key)}`"
      >
        <span class="deployment-stage-dot" aria-hidden="true" />
        <span>{{ step.label }}</span>
      </div>
    </div>
    <div
      v-if="task.progressMode === 'text'"
      class="deployment-compatibility"
      data-ui="deployment-progress-compatibility"
    >
      {{ t('app.docker.projects.deploymentProgress.compatibilityText') }}
    </div>
    <div
      v-else-if="task.progressMode === 'unavailable'"
      class="deployment-compatibility"
      data-ui="deployment-progress-compatibility"
    >
      {{ t('app.docker.projects.deploymentProgress.compatibilityUnavailable') }}
    </div>
    <div class="deployment-details" data-ui="deployment-progress-list">
      <div v-if="progressPhases.length" class="deployment-phase-list">
        <div
          v-for="group in progressPhases"
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
                  <VustTag :type="progressStatusType(item.status)">
                    {{ t(`app.docker.projects.deploymentProgress.itemStatuses.${item.status}`) }}
                  </VustTag>
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
                >
                  {{ progressItemPercentLabel(item) }}
                </span>
              </div>
              <div v-if="item.details" class="deployment-item-details">{{ item.details }}</div>
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
                  <span v-if="progressBytesLabel(child)" class="deployment-bytes">
                    {{ progressBytesLabel(child) }}
                  </span>
                  <span
                    v-if="progressItemPercentLabel(child)"
                    data-ui="deployment-progress-layer-percent"
                  >
                    {{ progressItemPercentLabel(child) }}
                  </span>
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
      v-if="task.errorSummary"
      type="error"
      :title="t('app.docker.projects.deploymentProgress.failedTitle')"
      :description="task.errorSummary"
      show-icon
    />
    <VustAlert
      v-if="task.cleanupWarning"
      type="warning"
      :title="t('app.docker.projects.deploymentProgress.cleanupWarning')"
      :description="task.cleanupWarning"
      show-icon
    />
  </div>
</template>

<style scoped>
.deployment-progress,
.deployment-phase-list,
.deployment-phase,
.deployment-item-list,
.deployment-details {
  display: flex;
  flex-direction: column;
  min-height: 0;
}
.deployment-progress {
  gap: var(--vdl-space-4);
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
  overflow: hidden;
  color: var(--vdl-text-primary);
  font-size: var(--vdl-font-subtitle);
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
  right: 50%;
  left: 0;
}
.deployment-stage-step::after {
  right: 0;
  left: 50%;
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
.deployment-item-details,
.deployment-child-copy span,
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
@media (max-width: 860px) {
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
