<script setup lang="ts">
/**
 * Docker 管理器主视图 (瘦壳层)
 *
 * 职责：
 * 1. 侧边栏菜单渲染
 * 2. 动态组件切换 (<component :is>)
 * 3. Docker 不可用状态 (使用 DockerInstallConsole 组件)
 * 4. 菜单驱动的数据加载协调 (通知 store 该 fetch/poll 什么)
 *
 * 所有领域数据和业务操作均委托给 useDockerStore()。
 */

import { ref, computed, markRaw, onUnmounted, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useDockerStore } from '@/stores/docker'
import { useNodeStore } from '@/stores/node'
import { useWindowManagerStore } from '@/stores/window-manager'

import DockerOverview from './DockerOverview.vue'
import DockerProjects from './DockerProjects.vue'
import DockerAllContainers from './DockerAllContainers.vue'
import DockerImages from './DockerImages.vue'
import DockerVolumes from './DockerVolumes.vue'
import DockerNetworks from './DockerNetworks.vue'
import DockerInstallConsole from './DockerInstallConsole.vue'
import VustIcon from '@/components/icons/VustIcon.vue'

const props = defineProps<{
  windowId?: string
  payload?: Record<string, unknown>
}>()

const { t } = useI18n()
const store = useDockerStore()
const nodeStore = useNodeStore()
const windowStore = useWindowManagerStore()

// ─── Docker 一键安装状态 (由 DockerInstallConsole 同步，用于更新窗口运行时状态) ───
const isInstallingDocker = ref(false)
const realtimeLogsActive = ref(false)
const containerTerminalActive = ref(false)

// ─── 菜单定义 ───
const menuItems = computed(() => [
  {
    key: 'overview',
    label: t('app.docker.menu.overview'),
    icon: 'chart',
    component: markRaw(DockerOverview),
  },
  {
    key: 'projects',
    label: t('app.docker.menu.projects'),
    icon: 'folder',
    component: markRaw(DockerProjects),
  },
  {
    key: 'containers',
    label: t('app.docker.menu.containers'),
    icon: 'package',
    component: markRaw(DockerAllContainers),
  },
  {
    key: 'images',
    label: t('app.docker.menu.images'),
    icon: 'image',
    component: markRaw(DockerImages),
  },
  {
    key: 'volumes',
    label: t('app.docker.menu.volumes'),
    icon: 'disk',
    component: markRaw(DockerVolumes),
  },
  {
    key: 'networks',
    label: t('app.docker.menu.networks'),
    icon: 'network',
    component: markRaw(DockerNetworks),
  },
])

const activeMenu = ref('overview')
const targetNodeId = computed(() => {
  const payloadNodeId = typeof props.payload?.nodeId === 'string' ? props.payload.nodeId : ''
  return payloadNodeId || nodeStore.currentNodeId || 'local'
})

/** 切换当前激活的菜单项 */
function switchMenu(key: string) {
  activeMenu.value = key
  if (key !== 'containers') {
    store.isContainerDetailActive = false
  }
}

/** 当前激活的动态组件 */
const activeComponent = computed(() => {
  return menuItems.value.find((i) => i.key === activeMenu.value)?.component
})

/** 处理容器创建按钮点击。 */
const handleCreateClick = async () => {
  await store.startContainerCreateFlow()
}

/** 更新实时日志活跃状态 */
const updateRealtimeLogsActive = (value: boolean) => {
  realtimeLogsActive.value = value
}

/** 更新容器终端活跃状态 */
const updateContainerTerminalActive = (value: boolean) => {
  containerTerminalActive.value = value
}

// ─── 窗口运行时状态 management ───

/** 更新窗口运行时状态 (安装进行中、日志/终端活跃) */
const updateDockerWindowRuntimeState = () => {
  if (!props.windowId) return
  const busy = isInstallingDocker.value
  const activeSession = realtimeLogsActive.value || containerTerminalActive.value
  const reason = busy
    ? t('app.docker.install.runtimeBusy')
    : containerTerminalActive.value
      ? t('app.docker.install.runtimeTerminalActive')
      : realtimeLogsActive.value
        ? t('app.docker.install.runtimeLogsActive')
        : t('app.docker.install.runtimeOpen')
  windowStore.updateWindowRuntimeState(props.windowId, {
    busy,
    activeSession,
    allowsNodeSwitch: false,
    blockLevel: busy ? 'busy' : activeSession ? 'active' : 'open',
    blockReason: reason,
  })
}

watch(
  [isInstallingDocker, realtimeLogsActive, containerTerminalActive],
  updateDockerWindowRuntimeState,
  { immediate: true },
)

watch(
  () => store.dockerAvailable,
  (available) => {
    if (available) {
      isInstallingDocker.value = false
    }
  },
)

// ─── 菜单切换数据加载协调 ───

watch(
  activeMenu,
  (newMenu) => {
    if (!store.dockerAvailable) {
      store.isLoading = false
      store.stopOverviewPolling()
      store.stopHistoryPolling()
      store.stopContainerStatsPolling()
      return
    }
    let fetchAction: Promise<void> | undefined

    switch (newMenu) {
      case 'overview':
        void store.refreshOverviewDataIfNeeded().then((state) => {
          if (state === 'cached') void store.fetchOverviewHistoryAll()
        })
        store.startOverviewPolling()
        store.startHistoryPolling()
        store.stopContainerStatsPolling()
        break
      case 'containers':
        void store.fetchContainers()
        store.startContainerStatsPolling(() => activeMenu.value)
        break
      case 'projects':
        store.stopContainerStatsPolling()
        break
      case 'images':
        store.stopContainerStatsPolling()
        break
      case 'volumes':
        store.stopContainerStatsPolling()
        break
      case 'networks':
        store.stopContainerStatsPolling()
        break
      default:
        store.stopContainerStatsPolling()
        break
    }

    if (fetchAction) {
      store.isLoading = true
      fetchAction.then(() => {
        store.isLoading = false
      })
    }

    store.isContainerCreateActive = false
    store.containerStep = 'selectImage'
    store.selectedImageId = null
  },
  { immediate: false },
)

watch(
  activeMenu,
  (menu) => {
    if (menu !== 'overview') {
      store.stopOverviewPolling()
      store.stopHistoryPolling()
    }
  },
  { immediate: true },
)

// ─── 节点切换重新初始化 ───

watch(
  targetNodeId,
  async () => {
    store.stopAllPolling()
    store.isLoading = true
    await store.initialLoad()
    if (store.dockerAvailable && activeMenu.value === 'overview') {
      store.startOverviewPolling()
      store.startHistoryPolling()
    }
    if (store.dockerAvailable && activeMenu.value === 'containers') {
      await store.fetchContainers()
      store.startContainerStatsPolling(() => activeMenu.value)
    }
  },
  { immediate: true },
)

// ─── 清理 ───

onUnmounted(() => {
  store.stopAllPolling()
})
</script>

<template>
  <div class="docker-manager" data-page="docker-manager">
    <div class="sidebar">
      <nav class="nav">
        <div
          v-for="item in menuItems"
          :key="item.key"
          :class="['nav-item', { 'nav-item-active': activeMenu === item.key }]"
          @click="switchMenu(item.key)"
        >
          <VustIcon class="nav-item-icon" :name="item.icon" :size="18" />
          <span class="nav-item-label">{{ item.label }}</span>
        </div>
      </nav>
    </div>

    <div class="content-area">
      <!-- 加载中 -->
      <div v-if="store.isLoading && activeMenu !== 'containers'" class="wip-placeholder">
        <p>{{ t('app.docker.loading') }}</p>
      </div>

      <!-- Docker 不可用 -->
      <DockerInstallConsole
        v-else-if="!store.dockerAvailable"
        :docker-status-code="store.dockerStatusCode"
        :fetch-docker-availability="store.fetchDockerAvailability"
        @installed="store.initialLoad"
        @install-state-change="(val) => (isInstallingDocker = val)"
      />

      <!-- 动态子组件 (过渡期：props 来自 store，events 调 store actions) -->
      <component
        v-else
        :is="activeComponent"
        :containers="store.containers"
        :container-resource-stats="store.containerResourceStats"
        :is-create-active="activeMenu === 'containers' ? store.isContainerCreateActive : false"
        :container-step="store.containerStep"
        :selected-image-id="store.selectedImageId"
        :selected-image-label="store.selectedImageLabel"
        :container-form="store.containerForm"
        :node-id="targetNodeId"
        @action="store.handleContainerAction"
        @create="handleCreateClick"
        @cancel-create="store.cancelContainerCreate"
        @submit-create="store.submitContainerConfig"
        @fetch-container-stats="store.fetchContainerResourceStats"
        @update:container-step="(v: 'selectImage' | 'config') => (store.containerStep = v)"
        @update:selected-image-id="(v: string | null) => (store.selectedImageId = v)"
        @update:container-form="(v: typeof store.containerForm) => (store.containerForm = v)"
        @update:detail-active="(v: boolean) => (store.isContainerDetailActive = v)"
        @logs-active-change="updateRealtimeLogsActive"
        @terminal-active-change="updateContainerTerminalActive"
      />
    </div>
  </div>
</template>

<style scoped>
/* ─── 主布局 ─── */
.docker-manager {
  display: flex;
  height: 100%;
  overflow: hidden;
  background-color: var(--vdl-bg-canvas);
  font-family: var(--vdl-font-family);
  color: var(--vdl-text-primary);
}

.sidebar {
  width: 200px;
  background-color: var(--vdl-bg-panel);
  border-right: 1px solid var(--vdl-border-default);
  display: flex;
  flex-direction: column;
  padding: var(--vdl-space-3) 0;
  flex-shrink: 0;
}

.nav {
  list-style: none;
  padding: 0;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.nav-item {
  height: 48px;
  padding: 0 var(--vdl-space-5);
  display: flex;
  align-items: center;
  gap: var(--vdl-space-3);
  color: var(--vdl-text-secondary);
  cursor: pointer;
  transition: all 0.2s cubic-bezier(0.4, 0, 0.2, 1);
  font-size: var(--vdl-font-body-sm);
  font-weight: 500;
}

.nav-item:hover {
  background-color: var(--vdl-bg-hover);
  color: var(--vdl-text-primary);
}

.nav-item-active {
  background-color: var(--vdl-bg-active);
  color: var(--vdl-primary);
  border-left: 4px solid var(--vdl-primary);
}

.nav-item-active:hover {
  background-color: var(--vdl-bg-active);
  opacity: 0.9;
}

.nav-item-icon {
  margin-right: var(--vdl-space-2);
}

.nav-item-label {
  flex-grow: 1;
}

/* ─── 内容区 ─── */
.content-area {
  flex-grow: 1;
  display: flex;
  flex-direction: column;
  padding: var(--vdl-space-4);
  overflow-y: auto;
  overflow-x: hidden;
  min-height: 0;
}

.wip-placeholder {
  display: flex;
  justify-content: center;
  align-items: center;
  flex-grow: 1;
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-body);
}

/* ─── 弹窗通用 ─── */
.modal-overlay {
  position: fixed;
  inset: 0;
  background-color: var(--vdl-bg-backdrop);
  display: flex;
  justify-content: center;
  align-items: center;
  z-index: var(--vdl-z-index-modal);
  backdrop-filter: blur(4px);
}

.modal {
  background: var(--vdl-bg-panel);
  border: 1px solid var(--vdl-border-strong);
  border-radius: var(--vdl-radius-lg);
  box-shadow: var(--vdl-shadow-window);
  width: 500px;
  max-width: 90%;
  max-height: 80%;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  animation: vl-modal-pop 0.3s cubic-bezier(0.34, 1.56, 0.64, 1);
}

.modal-header {
  padding: var(--vdl-space-4) var(--vdl-space-5);
  border-bottom: 1px solid var(--vdl-border-subtle);
  background: var(--vdl-bg-muted);
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.modal-header h3 {
  margin: 0;
  font-size: var(--vdl-font-subtitle);
  color: var(--vdl-text-primary);
  font-weight: 600;
}

.modal-close {
  background: none;
  border: none;
  font-size: 20px;
  cursor: pointer;
  color: var(--vdl-text-muted);
  transition: color 0.2s;
}

.modal-close:hover {
  color: var(--vdl-text-primary);
}

.modal-body {
  padding: var(--vdl-space-5);
  overflow-y: auto;
}

.modal-footer {
  padding: var(--vdl-space-3) var(--vdl-space-5);
  border-top: 1px solid var(--vdl-border-subtle);
  display: flex;
  justify-content: flex-end;
}

/* ─── 网络详情 ─── */
.detail-grid {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
}

.detail-item {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-1);
}

.detail-label {
  font-weight: 600;
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-caption);
}

.detail-value {
  color: var(--vdl-text-primary);
  word-break: break-all;
  font-size: var(--vdl-font-body-sm);
}

@keyframes vl-modal-pop {
  from {
    transform: scale(0.95);
    opacity: 0;
  }
  to {
    transform: scale(1);
    opacity: 1;
  }
}
</style>
