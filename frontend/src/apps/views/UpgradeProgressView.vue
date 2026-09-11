<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { upgradesApi } from '@/api/modules/upgrades'
import type { UpgradePlanDetail } from '@/api/modules/upgrades'
import { useToastStore } from '@/stores/toast'
import http from '@/api'
import { resetAuthState } from '@/router'
import { useWindowManagerStore } from '@/stores/window-manager'
import { VustTag } from '@/components/ui'
import { getLoginEntryPath } from '@/utils/login-entry'

const router = useRouter()
const route = useRoute()
const { t } = useI18n()
const toastStore = useToastStore()
const windowStore = useWindowManagerStore()

const upgradePlan = ref<UpgradePlanDetail | null>(null)
const isLoading = ref(true)
const isCancelling = ref(false)
const isRedirecting = ref(false)
const UPGRADE_PLAN_ID_KEY = 'vust.activeUpgradePlanId'
const UPGRADE_PLAN_DETAIL_KEY = 'vust.activeUpgradePlanDetail'
let timerId: number | null = null
let missingPlanCount = 0

const routePlanId = computed(() => {
  const value = route.query.planId
  if (typeof value === 'string' && value.trim()) return value.trim()
  return window.sessionStorage.getItem(UPGRADE_PLAN_ID_KEY)
})

const isDisplayablePlan = (plan: UpgradePlanDetail) => {
  return ['pending', 'running', 'succeeded', 'failed', 'canceled'].includes(plan.plan.status)
}

const leaveUpgradePage = () => {
  if (isRedirecting.value) return
  window.sessionStorage.removeItem(UPGRADE_PLAN_ID_KEY)
  window.sessionStorage.removeItem(UPGRADE_PLAN_DETAIL_KEY)
  isRedirecting.value = true
  if (timerId !== null) {
    window.clearInterval(timerId)
    timerId = null
  }
  resetAuthState()
  window.location.replace(getLoginEntryPath())
}

const cachePlan = (plan: UpgradePlanDetail) => {
  window.sessionStorage.setItem(UPGRADE_PLAN_ID_KEY, plan.plan.planId)
  window.sessionStorage.setItem(UPGRADE_PLAN_DETAIL_KEY, JSON.stringify(plan))
}

const hydrateCachedPlan = () => {
  const raw = window.sessionStorage.getItem(UPGRADE_PLAN_DETAIL_KEY)
  if (!raw) return
  try {
    const cached = JSON.parse(raw) as UpgradePlanDetail
    if (isDisplayablePlan(cached)) {
      upgradePlan.value = cached
      isLoading.value = false
    }
  } catch {
    window.sessionStorage.removeItem(UPGRADE_PLAN_DETAIL_KEY)
  }
}

const fetchStatus = async () => {
  try {
    const planId = routePlanId.value
    const response = planId ? await upgradesApi.detail(planId) : await upgradesApi.latestPlan()
    if (response.success && response.data) {
      if (isDisplayablePlan(response.data)) {
        upgradePlan.value = response.data
        cachePlan(response.data)
        missingPlanCount = 0
      } else {
        missingPlanCount += 1
      }
    } else {
      missingPlanCount += 1
    }
    if (!upgradePlan.value && missingPlanCount >= 3) {
      leaveUpgradePage()
    }
  } catch (error) {
    console.error('Failed to fetch latest upgrade plan', error)
    if (!upgradePlan.value) {
      missingPlanCount += 1
      if (missingPlanCount >= 3) {
        leaveUpgradePage()
      }
    }
  } finally {
    isLoading.value = false
  }
}

const cancelUpgrade = async () => {
  const planId = upgradePlan.value?.plan.planId
  if (!planId) return
  isCancelling.value = true
  try {
    const response = await upgradesApi.cancelPlan(planId)
    if (response.success) {
      toastStore.success(t('upgradeProgress.notifications.cancelSuccess'))
      upgradePlan.value = response.data ?? null
    } else {
      toastStore.error(response.message || t('upgradeProgress.notifications.cancelFailed'))
    }
  } catch (error) {
    console.error('Failed to cancel upgrade', error)
    toastStore.error(t('upgradeProgress.notifications.cancelFailed'))
  } finally {
    isCancelling.value = false
  }
}

const handleFinishAction = async () => {
  if (timerId !== null) {
    window.clearInterval(timerId)
    timerId = null
  }
  const status = upgradePlan.value?.plan.status
  if (status === 'succeeded') {
    try {
      await http.post('/auth/logout')
    } catch (error) {
      console.error('Logout request failed', error)
    }

    window.sessionStorage.removeItem(UPGRADE_PLAN_ID_KEY)
    window.sessionStorage.removeItem(UPGRADE_PLAN_DETAIL_KEY)
    resetAuthState()

    const windowsToClose = [...windowStore.openWindows]
    windowsToClose.forEach((w) => windowStore.closeWindow(w.id))

    window.location.replace(getLoginEntryPath())
  } else {
    window.sessionStorage.removeItem(UPGRADE_PLAN_ID_KEY)
    window.sessionStorage.removeItem(UPGRADE_PLAN_DETAIL_KEY)
    router.push('/')
  }
}

const getFinishActionLabel = () => {
  const status = upgradePlan.value?.plan.status
  if (status === 'succeeded') {
    return t('upgradeProgress.labels.reLogin')
  }
  return t('upgradeProgress.labels.returnToSystem')
}

// 升级完成状态计算
const isFinished = computed(() => {
  const status = upgradePlan.value?.plan.status
  return status === 'succeeded' || status === 'failed' || status === 'canceled'
})

const getTargetStatus = (target: { status: string; subStatus?: string | null }) => {
  if (target.status === 'running' && target.subStatus) {
    return target.subStatus
  }
  return target.status
}

const parseDownloadingPercent = (status: string) => {
  if (!status.startsWith('downloading:')) return null
  const rawPercent = status.split(':')[1]?.replace('%', '')
  const percent = Number(rawPercent)
  if (!Number.isFinite(percent)) return null
  return Math.max(0, Math.min(100, percent))
}

const getTargetProgress = (target: { status: string; subStatus?: string | null }) => {
  const status = getTargetStatus(target)
  const downloadingPercent = parseDownloadingPercent(status)
  if (downloadingPercent !== null) {
    return Math.round(10 + downloadingPercent * 0.4)
  }

  switch (status) {
    case 'pending':
    case 'deferred':
      return 0
    case 'running':
      return 10
    case 'downloading':
      return 20
    case 'downloaded':
      return 50
    case 'verifying':
      return 60
    case 'staging':
      return 70
    case 'prepared':
      return 80
    case 'applying':
      return 85
    case 'restarting':
    case 'restart_scheduled':
      return 90
    case 'succeeded':
    case 'failed':
    case 'canceled':
      return 100
    default:
      return target.status === 'running' ? 10 : 0
  }
}

// 升级进度百分比计算：按每个 target 的阶段进度求平均。
const progressPercent = computed(() => {
  if (!upgradePlan.value || upgradePlan.value.targets.length === 0) return 0
  const total = upgradePlan.value.targets.reduce(
    (sum, target) => sum + getTargetProgress(target),
    0,
  )
  return Math.round(total / upgradePlan.value.targets.length)
})

// 升级对象排序：如果有主控（controller），其卡片始终置顶于第一位呈现
const sortedTargets = computed(() => {
  if (!upgradePlan.value || !upgradePlan.value.targets) return []
  return [...upgradePlan.value.targets].sort((a, b) => {
    if (a.targetType === 'controller' && b.targetType !== 'controller') return -1
    if (a.targetType !== 'controller' && b.targetType === 'controller') return 1
    return 0
  })
})

const pageTitle = computed(() => {
  switch (upgradePlan.value?.plan.status) {
    case 'pending':
      return t('upgradeProgress.title.preparing')
    case 'running':
      return t('upgradeProgress.title.running')
    case 'succeeded':
      return t('upgradeProgress.title.succeeded')
    case 'failed':
      return t('upgradeProgress.title.failed')
    case 'canceled':
      return t('upgradeProgress.title.canceled')
    default:
      return isLoading.value
        ? t('upgradeProgress.title.fetching')
        : t('upgradeProgress.title.default')
  }
})

const planStatusLabel = computed(() => {
  switch (upgradePlan.value?.plan.status) {
    case 'pending':
      return t('upgradeProgress.status.preparing')
    case 'running':
      return t('upgradeProgress.status.executing')
    case 'succeeded':
      return t('upgradeProgress.status.finished')
    case 'failed':
      return t('upgradeProgress.status.failed')
    case 'canceled':
      return t('upgradeProgress.status.canceled')
    default:
      return t('upgradeProgress.status.unknown')
  }
})

const getStatusType = (status: string) => {
  switch (status) {
    case 'succeeded':
      return 'success'
    case 'failed':
      return 'danger'
    case 'running':
    case 'downloading':
    case 'verifying':
    case 'staging':
    case 'applying':
    case 'restarting':
      return 'info'
    case 'deferred':
      return 'warning'
    default:
      return 'default'
  }
}

const getStatusLabel = (status: string) => {
  if (status.startsWith('downloading:')) {
    const percent = status.split(':')[1] || '0%'
    return t('upgradeProgress.status.downloadingPercent', { percent })
  }
  switch (status) {
    case 'pending':
      return t('upgradeProgress.status.pending')
    case 'running':
      return t('upgradeProgress.status.running')
    case 'downloading':
      return t('upgradeProgress.status.downloading')
    case 'downloaded':
      return t('upgradeProgress.status.downloaded')
    case 'verifying':
      return t('upgradeProgress.status.verifying')
    case 'staging':
      return t('upgradeProgress.status.staging')
    case 'applying':
      return t('upgradeProgress.status.applying')
    case 'restarting':
    case 'restart_scheduled':
      return t('upgradeProgress.status.restarting')
    case 'succeeded':
      return t('upgradeProgress.status.succeeded')
    case 'failed':
      return t('upgradeProgress.status.failed')
    case 'deferred':
      return t('upgradeProgress.status.deferred')
    case 'canceled':
      return t('upgradeProgress.status.canceled')
    default:
      return status
  }
}

const shortNodeId = (nodeId?: string | null) => {
  if (!nodeId) return ''
  return nodeId.length > 12 ? `${nodeId.slice(0, 8)}...${nodeId.slice(-4)}` : nodeId
}

const getTargetName = (target: { targetType: string; nodeId?: string | null }) => {
  if (target.targetType === 'controller') return t('upgradeProgress.targetType.controllerNode')
  if (!target.nodeId) return t('upgradeProgress.targetType.unknownNode')
  return upgradePlan.value?.nodeNames?.[target.nodeId] || shortNodeId(target.nodeId)
}

const getEventTargetName = (targetId?: string) => {
  if (!targetId) return ''
  const target = upgradePlan.value?.targets.find((t) => t.targetId === targetId)
  if (!target) return ''
  return getTargetName(target)
}

onMounted(() => {
  hydrateCachedPlan()
  void fetchStatus()
  timerId = window.setInterval(() => {
    void fetchStatus()
  }, 3000)
})

onUnmounted(() => {
  if (timerId !== null) {
    window.clearInterval(timerId)
  }
})
</script>

<template>
  <div class="upgrade-container" data-page="upgrade-progress">
    <div class="upgrade-card">
      <!-- 头部：标题与主状态 -->
      <header class="upgrade-header">
        <div class="brand">
          <span
            class="pulse-dot"
            :class="{ 'pulse-active': upgradePlan?.plan.status === 'running' }"
          ></span>
          <h1>{{ pageTitle }}</h1>
        </div>
        <div v-if="upgradePlan" class="plan-info-header">
          <span class="target-ver-badge"
            >{{ t('upgradeProgress.labels.targetVersion') }}:
            {{ upgradePlan.plan.targetVersion }}</span
          >
          <span :class="['status-tag', getStatusType(upgradePlan.plan.status)]">
            {{ planStatusLabel }}
          </span>
        </div>
      </header>

      <!-- 未加载完成骨架屏 -->
      <div v-if="isLoading && !upgradePlan" class="loading-state">
        <div class="spinner"></div>
        <p>{{ t('upgradeProgress.title.fetching') }}</p>
      </div>

      <!-- 升级信息主体 -->
      <div v-else-if="upgradePlan" class="upgrade-body">
        <!-- 进度条 -->
        <section class="progress-section">
          <div class="progress-meta">
            <span>
              {{ t('upgradeProgress.labels.totalProgress') }}
              ({{ t('upgradeProgress.labels.targetCount', { count: upgradePlan.targets.length }) }})
            </span>
            <span class="percent-text">{{ progressPercent }}%</span>
          </div>
          <div class="progress-bar-wrapper">
            <div class="progress-bar" :style="{ width: `${progressPercent}%` }"></div>
          </div>
        </section>

        <!-- 节点列表 -->
        <section class="targets-section">
          <h3>{{ t('upgradeProgress.labels.targetDetail') }}</h3>
          <div class="targets-grid">
            <div
              v-for="target in sortedTargets"
              :key="target.targetId"
              class="target-item-card"
              data-ui="upgrade-target-card"
            >
              <div class="target-meta">
                <span class="target-type-badge" :class="target.targetType">
                  {{
                    target.targetType === 'controller'
                      ? t('upgradeProgress.targetType.controller')
                      : t('upgradeProgress.targetType.agent')
                  }}
                </span>
                <span class="target-name">
                  {{ getTargetName(target) }}
                </span>
              </div>
              <div v-if="target.nodeId" class="target-node-id code-text">
                {{ shortNodeId(target.nodeId) }}
              </div>
              <div class="target-details">
                <div class="detail-row">
                  <span class="label">{{ t('upgradeProgress.labels.currentVersion') }}:</span>
                  <span class="val code-text">{{
                    target.status === 'succeeded'
                      ? target.targetVersion
                      : target.currentVersion || t('upgradeProgress.status.unknown')
                  }}</span>
                </div>
                <div class="detail-row">
                  <span class="label">{{ t('upgradeProgress.labels.targetVersion') }}:</span>
                  <span class="val code-text">{{ target.targetVersion }}</span>
                </div>
                <div class="detail-row status-row">
                  <span class="label">{{ t('upgradeProgress.labels.currentStatus') }}:</span>
                  <span :class="['status-badge-text', getStatusType(getTargetStatus(target))]">
                    {{ getStatusLabel(getTargetStatus(target)) }}
                  </span>
                </div>
              </div>
              <!-- 错误详情回显 -->
              <div v-if="target.errorDetail" class="error-box">
                {{ target.errorDetail }}
              </div>
            </div>
          </div>
        </section>

        <!-- 升级日志 -->
        <section class="events-section">
          <h3>{{ t('upgradeProgress.labels.activityLog') }}</h3>
          <div class="events-timeline">
            <div v-for="event in upgradePlan.events" :key="event.eventId" class="timeline-item">
              <span class="event-time code-text">
                {{ new Date(event.createdAt).toLocaleTimeString() }}
              </span>
              <span class="event-marker"></span>
              <div
                class="event-desc"
                style="display: flex; align-items: center; gap: var(--vdl-space-2); flex-wrap: wrap"
              >
                <VustTag v-if="getEventTargetName(event.targetId)" type="info" size="small">
                  {{ getEventTargetName(event.targetId) }}
                </VustTag>
                <span class="event-msg">{{ event.message }}</span>
                <span class="event-type-label code-text">[{{ event.eventType }}]</span>
              </div>
            </div>
            <div v-if="upgradePlan.events.length === 0" class="no-events">
              {{ t('upgradeProgress.labels.noLogs') }}
            </div>
          </div>
        </section>

        <!-- 底部控制面板 -->
        <footer class="upgrade-actions">
          <button
            v-if="upgradePlan.plan.status === 'running'"
            class="btn btn-danger"
            :disabled="isCancelling"
            @click="cancelUpgrade"
          >
            {{
              isCancelling
                ? t('upgradeProgress.labels.cancelling')
                : t('upgradeProgress.labels.cancelUpgrade')
            }}
          </button>
          <button v-if="isFinished" class="btn btn-primary" @click="handleFinishAction">
            {{ getFinishActionLabel() }}
          </button>
        </footer>
      </div>

      <!-- 无有效计划时离开升级专用页面 -->
      <div v-else class="loading-state">
        <div class="spinner"></div>
        <p>
          {{
            isRedirecting
              ? t('upgradeProgress.labels.leavingPage')
              : t('upgradeProgress.labels.confirmingStatus')
          }}
        </p>
      </div>
    </div>
  </div>
</template>

<style scoped>
.upgrade-container {
  height: 100vh;
  min-height: 0;
  display: flex;
  justify-content: center;
  align-items: center;
  background: var(--vdl-bg-canvas);
  color: var(--vdl-text-primary);
  font-family: var(--vdl-font-family);
  padding: var(--vdl-space-6);
}

.upgrade-card {
  width: 100%;
  max-width: 860px;
  max-height: calc(100vh - var(--vdl-space-6) * 2);
  min-height: 0;
  background: var(--vdl-bg-glass);
  border: 1px solid var(--vdl-border-default);
  box-shadow: var(--vdl-shadow-window);
  backdrop-filter: blur(16px);
  border-radius: var(--vdl-radius-lg);
  padding: var(--vdl-space-8);
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-6);
}

.upgrade-header {
  flex-shrink: 0;
  display: flex;
  justify-content: space-between;
  align-items: center;
  border-bottom: 1px solid var(--vdl-border-subtle);
  padding-bottom: var(--vdl-space-5);
}

.upgrade-body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-5);
}

.brand {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-3);
}

.brand h1 {
  color: var(--vdl-primary);
  font-size: var(--vdl-font-metric);
  font-weight: 600;
  letter-spacing: 0;
  margin: 0;
}

.pulse-dot {
  width: 10px;
  height: 10px;
  background-color: var(--vdl-danger);
  border-radius: var(--vdl-radius-pill);
  position: relative;
}

.pulse-active {
  background-color: var(--vdl-success);
}

.pulse-active::after {
  content: '';
  position: absolute;
  top: 0;
  left: 0;
  width: 100%;
  height: 100%;
  border-radius: var(--vdl-radius-pill);
  border: 2px solid var(--vdl-success);
  animation: pulse-ring 1.8s infinite ease-out;
}

@keyframes pulse-ring {
  0% {
    transform: scale(0.8);
    opacity: 1;
  }
  100% {
    transform: scale(2.4);
    opacity: 0;
  }
}

.plan-info-header {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-3);
}

.target-ver-badge {
  background: var(--vdl-info-soft);
  color: var(--vdl-primary);
  font-size: var(--vdl-font-body-sm);
  padding: var(--vdl-space-1) var(--vdl-space-3);
  border-radius: var(--vdl-radius-sm);
  border: 1px solid var(--vdl-border-brand);
  font-family: var(--vdl-font-mono);
}

.status-tag {
  font-size: var(--vdl-font-caption);
  font-weight: 700;
  padding: var(--vdl-space-1) var(--vdl-space-3);
  border: 1px solid transparent;
  border-radius: var(--vdl-radius-sm);
  letter-spacing: 0.5px;
}

.status-tag.success {
  background: var(--vdl-success-soft);
  color: var(--vdl-success);
  border-color: color-mix(in srgb, var(--vdl-success) 36%, transparent);
}

.status-tag.danger {
  background: var(--vdl-danger-soft);
  color: var(--vdl-danger);
  border-color: var(--vdl-border-danger);
}

.status-tag.info {
  background: var(--vdl-info-soft);
  color: var(--vdl-primary);
  border-color: var(--vdl-border-brand);
}

.status-tag.warning {
  background: var(--vdl-warning-soft);
  color: var(--vdl-warning);
  border-color: color-mix(in srgb, var(--vdl-warning) 36%, transparent);
}

.status-tag.default {
  background: var(--vdl-neutral-soft);
  color: var(--vdl-text-muted);
  border-color: var(--vdl-border-default);
}

.loading-state,
.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: calc(var(--vdl-space-8) * 2) 0;
  gap: var(--vdl-space-4);
  color: var(--vdl-text-muted);
}

.spinner {
  width: 40px;
  height: 40px;
  border: 3px solid var(--vdl-info-soft);
  border-top-color: var(--vdl-primary);
  border-radius: var(--vdl-radius-pill);
  animation: spin 1s infinite linear;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

.progress-section {
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-2);
}

.progress-meta {
  display: flex;
  justify-content: space-between;
  align-items: center;
  font-size: var(--vdl-font-body);
  color: var(--vdl-text-muted);
}

.percent-text {
  font-size: var(--vdl-font-title-lg);
  font-weight: 700;
  color: var(--vdl-primary);
}

.progress-bar-wrapper {
  height: 10px;
  background: var(--vdl-bg-muted);
  border-radius: var(--vdl-radius-pill);
  overflow: hidden;
  border: 1px solid var(--vdl-border-subtle);
}

.progress-bar {
  height: 100%;
  background: var(--vdl-primary);
  border-radius: var(--vdl-radius-pill);
  transition: width 0.4s cubic-bezier(0.4, 0, 0.2, 1);
}

.targets-section,
.events-section {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
}

.targets-section {
  flex: 1 1 260px;
  min-height: 180px;
}

.events-section {
  flex-shrink: 0;
}

.targets-section h3,
.events-section h3 {
  font-size: var(--vdl-font-subtitle);
  font-weight: 600;
  color: var(--vdl-text-primary);
  margin: 0;
}

.targets-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
  gap: var(--vdl-space-4);
  min-height: 0;
  overflow-y: auto;
  padding-right: var(--vdl-space-1);
  scrollbar-gutter: stable;
}

.target-item-card {
  background: var(--vdl-bg-card);
  border: 1px solid var(--vdl-border-subtle);
  border-radius: var(--vdl-radius-md);
  padding: var(--vdl-space-4);
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
}

.target-meta {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
  border-bottom: 1px solid var(--vdl-border-subtle);
  padding-bottom: var(--vdl-space-2);
}

.target-type-badge {
  font-size: var(--vdl-font-caption);
  font-weight: 600;
  padding: 2px var(--vdl-space-2);
  border-radius: var(--vdl-radius-xs);
}

.target-type-badge.controller {
  background: color-mix(in srgb, var(--vdl-accent) 16%, transparent);
  color: var(--vdl-accent);
}

.target-type-badge.agent {
  background: var(--vdl-info-soft);
  color: var(--vdl-primary);
}

.target-name {
  font-size: var(--vdl-font-body-sm);
  font-weight: 600;
  color: var(--vdl-text-secondary);
}

.target-node-id {
  font-size: var(--vdl-font-caption);
  color: var(--vdl-text-subtle);
}

.target-details {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-2);
  font-size: var(--vdl-font-caption);
}

.detail-row {
  display: flex;
  justify-content: space-between;
}

.detail-row .label {
  color: var(--vdl-text-muted);
}

.detail-row .val {
  color: var(--vdl-text-secondary);
}

.status-badge-text {
  font-weight: 600;
}

.status-badge-text.success {
  color: var(--vdl-success);
}

.status-badge-text.danger {
  color: var(--vdl-danger);
}

.status-badge-text.info {
  color: var(--vdl-primary);
}

.status-badge-text.warning {
  color: var(--vdl-warning);
}

.status-badge-text.default {
  color: var(--vdl-text-muted);
}

.error-box {
  background: var(--vdl-danger-soft);
  border: 1px solid var(--vdl-border-danger);
  border-radius: var(--vdl-radius-sm);
  padding: var(--vdl-space-2);
  font-size: var(--vdl-font-caption);
  color: var(--vdl-danger);
  word-break: break-all;
}

.events-timeline {
  max-height: 180px;
  overflow-y: auto;
  background: var(--vdl-bg-panel);
  border: 1px solid var(--vdl-border-subtle);
  border-radius: var(--vdl-radius-md);
  padding: var(--vdl-space-4);
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
}

.timeline-item {
  display: flex;
  align-items: flex-start;
  gap: var(--vdl-space-3);
  font-size: var(--vdl-font-body-sm);
}

.event-time {
  color: var(--vdl-text-muted);
  min-width: 70px;
  font-size: var(--vdl-font-caption);
}

.event-marker {
  width: 6px;
  height: 6px;
  background-color: var(--vdl-primary);
  border-radius: var(--vdl-radius-pill);
  margin-top: 6px;
  flex-shrink: 0;
}

.event-desc {
  display: flex;
  gap: var(--vdl-space-2);
  align-items: center;
  flex-wrap: wrap;
}

.event-msg {
  color: var(--vdl-text-secondary);
}

.event-type-label {
  color: var(--vdl-text-subtle);
  font-size: var(--vdl-font-caption);
}

.no-events {
  text-align: center;
  padding: var(--vdl-space-5) 0;
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-body-sm);
}

.upgrade-actions {
  flex-shrink: 0;
  display: flex;
  justify-content: flex-end;
  border-top: 1px solid var(--vdl-border-subtle);
  padding-top: var(--vdl-space-5);
}

.btn {
  font-family: inherit;
  font-size: var(--vdl-font-body);
  font-weight: 600;
  padding: var(--vdl-space-3) var(--vdl-space-6);
  border-radius: var(--vdl-radius-md);
  border: 1px solid transparent;
  cursor: pointer;
  transition:
    background-color 0.2s,
    border-color 0.2s,
    color 0.2s,
    opacity 0.2s;
}

.btn-primary {
  background: var(--vdl-primary);
  color: var(--vdl-text-inverse);
}

.btn-primary:not(:disabled):hover {
  background: var(--vdl-primary-hover);
}

.btn-primary:not(:disabled):active {
  background: var(--vdl-primary-active);
}

.btn-danger {
  background: var(--vdl-danger-soft);
  color: var(--vdl-danger);
  border-color: var(--vdl-border-danger);
}

.btn-danger:not(:disabled):hover {
  background: var(--vdl-danger);
  color: var(--vdl-text-on-danger);
}

.btn-danger:not(:disabled):active {
  opacity: 0.85;
}

.btn:focus-visible {
  outline: none;
  box-shadow: var(--vdl-focus-ring);
}

.btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
  box-shadow: none;
}

.code-text {
  font-family: var(--vdl-font-mono);
}

@media (prefers-reduced-motion: reduce) {
  .pulse-active::after,
  .spinner {
    animation: none;
  }

  .progress-bar,
  .btn {
    transition: none;
  }
}
</style>
