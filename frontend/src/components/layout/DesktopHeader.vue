<script setup lang="ts">
import { computed, ref, onMounted, onUnmounted } from 'vue'
import { useRouter } from 'vue-router'
import { resetAuthState } from '@/router'
import { getLoginEntryPath } from '@/utils/login-entry'
import { useWindowManagerStore, type WindowInstance } from '../../stores/window-manager'
import type { AppId } from '@/apps/registry'
import { useToastStore } from '../../stores/toast'
import { useNotificationCenterStore } from '../../stores/notification-center'
import { useConfirmationModalStore } from '../../stores/confirmation-modal'
import VustTooltip from '../ui/VustTooltip.vue'
import AppContextMenu from '../AppContextMenu.vue'
import AppIcon from '../icons/AppIcon.vue'
import VustIcon from '../icons/VustIcon.vue'
import { VustGlassSurface } from '@/components/ui'
import http from '@/api'
import { useI18n } from 'vue-i18n'
import { useNodeStore } from '../../stores/node'

const { t } = useI18n()
const store = useWindowManagerStore()
const toastStore = useToastStore()
const notificationCenterStore = useNotificationCenterStore()
const confirmationModal = useConfirmationModalStore()
const router = useRouter()
const appLibraryOpen = ref(false)
const hoveredLibraryAppId = ref<AppId | null>(null)
const focusedLibraryAppId = ref<AppId | null>(null)
const libraryContextMenu = ref({
  visible: false,
  x: 0,
  y: 0,
  target: null as AppId | null,
})

// --- 顶层工作节点上下文切换 ---
const nodeStore = useNodeStore()
const currentNode = computed(() => {
  return nodeStore.nodes.find((n) => n.id === nodeStore.currentNodeId) || null
})
const currentNodeLabel = computed(() => {
  if (currentNode.value) {
    return currentNode.value.id === 'local' ? t('desktop.header.localNode') : currentNode.value.name
  }
  return nodeStore.currentNodeId || t('desktop.header.nodePlaceholder')
})
const currentNodeAddress = computed(() => currentNode.value?.address || t('common.status.unknown'))

function buildCloseBlockMessage() {
  const blockers = store.checkBeforeCloseAll().blockers
  if (blockers.length === 0) return ''
  const details = blockers
    .slice(0, 5)
    .map((item) => `${item.title}: ${item.reason}`)
    .join('；')
  const restCount = Math.max(0, blockers.length - 5)
  const restText = restCount > 0 ? t('desktop.header.closeBlockedRest', { count: restCount }) : ''
  return t('desktop.header.closeBlocked', { details, rest: restText })
}

function handleBeforeUnload(event: BeforeUnloadEvent) {
  if (!buildCloseBlockMessage()) return
  event.preventDefault()
  return ''
}

onMounted(() => {
  nodeStore.startAutoRefresh()
  notificationCenterStore.startPolling()
  window.addEventListener('beforeunload', handleBeforeUnload)
})

onUnmounted(() => {
  nodeStore.stopAutoRefresh()
  notificationCenterStore.stopPolling()
  window.removeEventListener('beforeunload', handleBeforeUnload)
})

const appList = computed(() => {
  const ids =
    store.appCatalogOrder.length > 0
      ? store.appCatalogOrder
      : (Object.keys(store.availableApps) as AppId[])
  return ids
    .map((id) => {
      const config = store.availableApps[id]
      if (!config || config.hidden) return null
      return {
        id,
        title: store.resolveAppTitle(config),
        icon: config.icon,
        sourceType: config.sourceType ?? 'builtin',
      }
    })
    .filter(
      (
        item,
      ): item is {
        id: AppId
        title: string
        icon: string
        sourceType: 'builtin' | 'suite'
      } => item !== null,
    )
})

const builtinAppList = computed(() => appList.value.filter((app) => app.sourceType !== 'suite'))
const suiteAppList = computed(() => appList.value.filter((app) => app.sourceType === 'suite'))

/** @description 清理当前会话并返回登录页。 */
async function handleLogout() {
  const closeBlockMessage = buildCloseBlockMessage()
  if (closeBlockMessage) {
    toastStore.error(closeBlockMessage)
    return
  }

  const confirmed = await confirmationModal.showConfirmation(
    t('desktop.header.logoutConfirmMessage'),
    t('desktop.header.logoutConfirmTitle'),
    t('desktop.header.logout'),
    t('confirmation.cancel'),
  )
  if (!confirmed) return

  await http.post('/auth/logout')
  notificationCenterStore.stopPolling(true)
  resetAuthState()

  const windowsToClose = [...store.openWindows]
  windowsToClose.forEach((w) => store.closeWindow(w.id))
  window.location.replace(getLoginEntryPath())
}

/** 打开或聚焦个人通知中心。 */
function openNotificationCenter() {
  store.openWindow('notification-center')
}

const unreadBadge = computed(() => {
  const count = notificationCenterStore.unreadCount
  if (count <= 0) return ''
  return count > 99 ? '99+' : String(count)
})

/**
 * @description 返回到桌面视图，并确保所有窗口被最小化。
 */
function handleReturnToDesktop() {
  // 最小化所有未最小化的窗口
  store.openWindows.forEach((w) => {
    if (!w.isMinimized) {
      store.minimizeWindow(w.id)
    }
  })

  // 导航到桌面背景路由
  if (router.currentRoute.value.name !== 'desktop') {
    router.push({ name: 'desktop' })
  }
}

/**
 * @description 点击打开的应用图标时，聚焦、恢复或最小化窗口。
 * @param id 窗口的唯一 ID。
 */
function handleAppIconClick(id: string) {
  const openWindowsArray = store.openWindows as WindowInstance[]

  const instance = openWindowsArray.find((w: WindowInstance) => w.id === id)
  if (!instance) return

  if (instance.isMinimized) {
    // 1. 如果窗口最小化，则恢复并聚焦
    instance.isMinimized = false
    store.focusWindow(id)
  } else {
    // 2. 如果窗口可见（未最小化）
    if (instance.zIndex === store.maxZIndex) {
      // 2a. 如果窗口当前已是焦点窗口，则最小化
      store.minimizeWindow(id)
    } else {
      // 2b. 如果窗口可见但未聚焦，则聚焦（提到最前）
      store.focusWindow(id)
    }
  }
}

function toggleAppLibrary() {
  if (appLibraryOpen.value) {
    closeAppLibrary()
    return
  }
  appLibraryOpen.value = true
}

function closeAppLibrary() {
  appLibraryOpen.value = false
  hoveredLibraryAppId.value = null
  focusedLibraryAppId.value = null
  closeLibraryContextMenu()
}

function handleOverlayBackgroundClick() {
  closeAppLibrary()
}

function handleLibraryAppClick(id: AppId) {
  store.openWindow(id)
  closeAppLibrary()
  closeLibraryContextMenu()
}

function openLibraryContextMenu(event: MouseEvent, id: AppId) {
  event.preventDefault()
  libraryContextMenu.value = {
    visible: true,
    x: event.clientX,
    y: event.clientY,
    target: id,
  }
}

function handleAddToDesktop() {
  const target = libraryContextMenu.value.target
  if (target) {
    store.addDesktopShortcut(target)
    toastStore.info(t('notification.addedToDesktop'))
  }
  closeLibraryContextMenu()
}

function closeLibraryContextMenu() {
  libraryContextMenu.value.visible = false
  libraryContextMenu.value.target = null
}

function isAppOnDesktop(id: AppId | null): boolean {
  if (!id) return false
  return store.desktopShortcuts.some((s) => s.appId === id)
}

/**
 * @description 判断应用库条目是否应显示交互态玻璃材质。
 * @param id 目标应用 ID。
 */
function isLibraryGlassActive(id: AppId) {
  return hoveredLibraryAppId.value === id || focusedLibraryAppId.value === id
}

/**
 * @file DesktopHeader.vue
 * @description 桌面布局的上部菜单栏组件。
 */
</script>

<template>
  <VustGlassSurface
    class="header-content"
    data-ui="desktop-header"
    data-slot="header"
    @contextmenu.prevent
  >
    <div class="header-center">
      <div class="nav-home" :class="{ 'nav-home-separated': store.openWindows.length > 0 }">
        <VustTooltip :text="$t('desktop.header.showDesktop')" position="bottom">
          <button class="desktop-btn" @click="handleReturnToDesktop">
            <VustIcon name="desktop" :size="18" />
          </button>
        </VustTooltip>
        <VustTooltip :text="$t('desktop.header.appLibrary')" position="bottom">
          <button class="library-btn" @click="toggleAppLibrary">
            <VustIcon name="app-library" :size="18" />
          </button>
        </VustTooltip>
      </div>

      <div
        class="nav-apps-group"
        :class="{ 'nav-apps-group-separated': store.openWindows.length > 0 }"
      >
        <VustTooltip
          v-for="window in store.openWindows"
          :key="window.id"
          :text="window.title"
          position="bottom"
        >
          <div
            class="nav-app"
            @click="handleAppIconClick(window.id)"
            :class="{
              'app-active': !window.isMinimized,
              'app-minimized': window.isMinimized,
            }"
          >
            <AppIcon
              class="nav-app-icon"
              :name="window.icon"
              :size="16"
              :label="window.title"
              :decorative="false"
            />
          </div>
        </VustTooltip>
      </div>
    </div>

    <div class="header-space" data-ui="node-context-space">
      <div class="node-selector-container" data-ui="node-context-display">
        <!-- 节点胶囊 -->
        <div
          class="node-selector-capsule"
          :class="{ 'node-unavailable': nodeStore.currentNodeUnavailable }"
          data-ui="node-selector-capsule"
        >
          <span class="status-dot-green"></span>
          <span class="node-label">{{ currentNodeLabel }}</span>
          <span class="node-address mono">({{ currentNodeAddress }})</span>
        </div>
      </div>
    </div>

    <div class="header-operate">
      <VustTooltip :text="$t('desktop.header.notifications')" position="bottom">
        <button
          type="button"
          class="notification-center-btn"
          data-ui="desktop-notification-button"
          :aria-label="$t('desktop.header.notifications')"
          @click="openNotificationCenter"
        >
          <AppIcon
            :name="store.availableApps['notification-center']?.icon || 'notification-center'"
            :size="18"
            :label="$t('desktop.header.notifications')"
            :decorative="false"
          />
          <span v-if="unreadBadge" class="notification-badge" data-slot="unread-badge">
            {{ unreadBadge }}
          </span>
        </button>
      </VustTooltip>
      <VustTooltip :text="$t('desktop.header.logout')" position="bottom">
        <button
          type="button"
          class="logout-btn"
          data-ui="desktop-logout-button"
          :aria-label="$t('desktop.header.logout')"
          @click="handleLogout"
        >
          <VustIcon name="logout" :size="18" />
        </button>
      </VustTooltip>
    </div>
    <Teleport to="body">
      <div
        v-if="appLibraryOpen"
        class="library-panel-overlay"
        @click="handleOverlayBackgroundClick"
      >
        <VustGlassSurface class="library-panel-wrapper" data-ui="app-library-panel">
          <div class="library-panel-header" data-slot="header" @click.stop>
            <div>{{ $t('desktop.header.appLibrary') }}</div>
          </div>
          <div class="library-scroll" data-ui="app-library-groups">
            <section class="library-section" data-ui="builtin-app-group">
              <div class="library-section-title">{{ $t('desktop.header.builtinApps') }}</div>
              <div class="library-grid">
                <VustGlassSurface
                  v-for="app in builtinAppList"
                  :key="app.id"
                  as="button"
                  profile="control"
                  :glass="isLibraryGlassActive(app.id) ? undefined : false"
                  type="button"
                  class="library-item"
                  :class="{ 'is-glass-idle': !isLibraryGlassActive(app.id) }"
                  @mouseenter="hoveredLibraryAppId = app.id"
                  @mouseleave="hoveredLibraryAppId = null"
                  @focus="focusedLibraryAppId = app.id"
                  @blur="focusedLibraryAppId = null"
                  @click.stop="handleLibraryAppClick(app.id)"
                  @contextmenu.prevent.stop="openLibraryContextMenu($event, app.id)"
                >
                  <AppIcon
                    class="library-icon"
                    :name="app.icon || app.id"
                    :size="30"
                    :label="app.title"
                    :decorative="false"
                  />
                  <span class="library-title">{{ app.title }}</span>
                </VustGlassSurface>
              </div>
            </section>
            <section
              v-if="suiteAppList.length > 0"
              class="library-section"
              data-ui="suite-app-group"
            >
              <div class="library-section-title">{{ $t('desktop.header.suiteApps') }}</div>
              <div class="library-grid">
                <VustGlassSurface
                  v-for="app in suiteAppList"
                  :key="app.id"
                  as="button"
                  profile="control"
                  :glass="isLibraryGlassActive(app.id) ? undefined : false"
                  type="button"
                  class="library-item"
                  :class="{ 'is-glass-idle': !isLibraryGlassActive(app.id) }"
                  @mouseenter="hoveredLibraryAppId = app.id"
                  @mouseleave="hoveredLibraryAppId = null"
                  @focus="focusedLibraryAppId = app.id"
                  @blur="focusedLibraryAppId = null"
                  @click.stop="handleLibraryAppClick(app.id)"
                  @contextmenu.prevent.stop="openLibraryContextMenu($event, app.id)"
                >
                  <AppIcon
                    class="library-icon"
                    :name="app.icon || app.id"
                    :size="30"
                    :label="app.title"
                    :decorative="false"
                  />
                  <span class="library-title">{{ app.title }}</span>
                </VustGlassSurface>
              </div>
            </section>
          </div>
        </VustGlassSurface>
      </div>
    </Teleport>
    <AppContextMenu
      :visible="libraryContextMenu.visible"
      :x="libraryContextMenu.x"
      :y="libraryContextMenu.y"
      @close="closeLibraryContextMenu"
    >
      <button
        type="button"
        class="context-menu-btn"
        data-ui="add-to-desktop-btn"
        :disabled="isAppOnDesktop(libraryContextMenu.target)"
        :title="
          $t(
            isAppOnDesktop(libraryContextMenu.target)
              ? 'desktop.header.contextMenu.alreadyOnDesktop'
              : 'desktop.header.contextMenu.addToDesktop',
          )
        "
        @click="handleAddToDesktop"
      >
        {{ $t('desktop.header.contextMenu.addToDesktop') }}
      </button>
    </AppContextMenu>
  </VustGlassSurface>
</template>
<style scoped>
.header-content {
  height: var(--vdl-header-height);
  flex-shrink: 0;
  border-width: 0 0 1px;
  border-color: var(--vdl-glass-border);
  border-radius: 0;
  color: var(--vdl-text-primary);
  padding: 0 var(--vdl-space-3);
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: var(--vdl-font-body);
  user-select: none;
  z-index: var(--vdl-z-index-header);
  position: relative;
}

/* Center 区域 (flex: none) */
.header-center {
  display: flex;
  align-items: center;
  padding: 2px 4px;
  border: 1px solid var(--vdl-border-subtle);
  border-radius: 5px;
  z-index: 1;
  position: relative;
}

/* 桌面按钮容器 */
.nav-home {
  display: flex;
}

/* 仅当有应用打开时，nav-home 添加分隔符和左侧间隙 */
.nav-home.nav-home-separated {
  border-right: 1px solid var(--vdl-border-subtle);
  padding-right: 4px;
}

/* --- 应用图标组 Wrapper --- */
.nav-apps-group {
  display: flex;
  align-items: center;
  gap: 3px; /* 控制应用图标之间的间距 */
}

/* 仅当有应用打开时，nav-apps-group 添加分隔符右侧间隙 */
.nav-apps-group.nav-apps-group-separated {
  padding-left: 4px;
}

/* 桌面按钮 */
.desktop-btn {
  background: none;
  border: 1px solid transparent;
  color: inherit;
  font-size: 18px;
  cursor: pointer;
  padding: 4px;
  border-radius: 4px;
  transition: background-color 0.2s;
  line-height: 1; /* 确保图标对齐 */
}

.desktop-btn:hover {
  background-color: var(--vdl-bg-hover);
  border-color: var(--vdl-glass-border-hover);
}

.desktop-btn:focus-visible,
.library-btn:focus-visible,
.nav-app:focus-visible,
.logout-btn:focus-visible,
.notification-center-btn:focus-visible,
.library-item:focus-visible {
  border-color: var(--vdl-glass-border-hover);
  outline: none;
  box-shadow: var(--vdl-focus-ring);
}

.library-btn {
  background: none;
  border: 1px solid transparent;
  color: inherit;
  font-size: 18px;
  cursor: pointer;
  padding: 4px;
  border-radius: 4px;
  margin-left: 2px;
  transition: background-color 0.2s;
  line-height: 1;
}

.library-btn:hover {
  background-color: var(--vdl-bg-hover);
  border-color: var(--vdl-glass-border-hover);
}

/* 应用图标 */
.nav-app {
  width: 24px;
  height: 24px;
  border-radius: 4px;
  display: flex;
  justify-content: center;
  align-items: center;
  cursor: pointer;
  position: relative;
  transition: background-color 0.2s;
  border: 1px solid transparent;
}

.nav-app:hover {
  background-color: var(--vdl-bg-hover);
  border-color: var(--vdl-glass-border-hover);
}

.nav-app-icon {
  color: var(--vdl-text-primary);
}

/* 2. Space 区域 (标题) - 使用绝对定位实现固定居中 */
.header-space {
  display: flex;
  justify-content: center;
  align-items: center;

  /* 绝对定位居中 */
  position: absolute;
  left: 50%;
  top: 50%;
  transform: translate(-50%, -50%);

  flex-shrink: 0;
}

.node-selector-container {
  position: relative;
}

.node-selector-capsule {
  display: flex;
  align-items: center;
  gap: 6px;
  background: var(--vdl-glass-control-tint);
  border: 1px solid var(--vdl-glass-border);
  padding: 4px 12px;
  border-radius: var(--vdl-radius-pill);
  color: var(--vdl-primary);
  font-size: 12px;
  font-weight: 600;
  transition: all 0.25s cubic-bezier(0.4, 0, 0.2, 1);
  user-select: none;
}

.node-selector-capsule.clickable {
  cursor: pointer;
}

.node-selector-capsule.clickable:hover {
  background: var(--vdl-bg-hover);
  border-color: var(--vdl-glass-border-hover);
  box-shadow: var(--vdl-shadow-brand);
}

.node-selector-capsule.active-open {
  background: var(--vdl-bg-active);
  border-color: var(--vdl-glass-border-hover);
  box-shadow: var(--vdl-shadow-brand);
}

.node-selector-capsule.node-unavailable {
  border-color: var(--vdl-warning);
  color: var(--vdl-warning);
}

.node-selector-capsule.node-unavailable .status-dot-green {
  background-color: var(--vdl-warning);
  box-shadow: 0 0 6px var(--vdl-warning);
}

.status-dot-green {
  width: 6px;
  height: 6px;
  background-color: var(--vdl-success);
  border-radius: 50%;
  box-shadow: 0 0 6px var(--vdl-success);
  display: inline-block;
  animation: dot-breath 1.8s infinite ease-in-out;
  flex-shrink: 0;
}

@keyframes dot-breath {
  0% {
    opacity: 0.6;
    transform: scale(0.9);
  }
  50% {
    opacity: 1;
    transform: scale(1.15);
  }
  100% {
    opacity: 0.6;
    transform: scale(0.9);
  }
}

.node-address {
  color: var(--vdl-text-muted);
  font-weight: 500;
  opacity: 0.8;
}

.dropdown-arrow {
  font-size: 9px;
  margin-left: 2px;
  color: var(--vdl-text-subtle);
  transition: transform 0.2s ease;
}

.node-selector-capsule.active-open .dropdown-arrow {
  transform: rotate(180deg);
  color: var(--vdl-primary);
}

/* 下拉浮层 */
.node-selector-dropdown {
  position: absolute;
  top: calc(100% + 8px);
  left: 50%;
  transform: translateX(-50%);
  width: 280px;
  background: var(--vdl-bg-panel);
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  box-shadow: 0 10px 30px rgba(0, 0, 0, 0.4);
  backdrop-filter: blur(12px);
  z-index: 1000;
  overflow: hidden;
  animation: fadeInDown 0.15s cubic-bezier(0.16, 1, 0.3, 1) forwards;
}

@keyframes fadeInDown {
  from {
    opacity: 0;
    transform: translate(-50%, -4px);
  }
  to {
    opacity: 1;
    transform: translate(-50%, 0);
  }
}

.dropdown-header {
  padding: 8px 12px;
  font-size: 11px;
  text-transform: uppercase;
  color: var(--vdl-text-subtle);
  border-bottom: 1px solid var(--vdl-border-subtle);
  font-weight: 600;
  letter-spacing: 0.5px;
}

.dropdown-list {
  list-style: none;
  margin: 0;
  padding: 4px 0;
}

.dropdown-item {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 10px 12px;
  cursor: pointer;
  transition: all 0.15s ease;
}

.dropdown-item:hover {
  background: var(--vdl-bg-hover);
}

.dropdown-item.active {
  background: rgba(0, 200, 255, 0.08);
}

.dropdown-item.active .item-name {
  color: var(--vdl-primary);
}

.item-info {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.item-name {
  font-size: 13px;
  font-weight: 500;
  color: var(--vdl-text-primary);
}

.item-addr {
  font-size: 11px;
  color: var(--vdl-text-muted);
}

.item-status {
  display: flex;
  align-items: center;
  gap: 5px;
  font-size: 11px;
  color: var(--vdl-success);
}

/* 3. Operate 区域 (flex: none) */
.header-operate {
  display: flex; /* 使用 Flex 布局排列时间和按钮 */
  align-items: center;
  gap: 10px; /* 时间和按钮之间的间距 */

  padding-right: 5px; /* 靠右侧内边距 */
  font-size: 13px;
  justify-content: flex-end; /* 确保内容靠右 */

  flex-shrink: 0;
  z-index: 1; /* 确保在绝对定位的标题上方 */
}

.logout-btn,
.notification-center-btn {
  background: none;
  border: 1px solid transparent;
  color: inherit;
  font-size: 18px;
  cursor: pointer;
  width: 28px;
  height: 28px;
  border-radius: 4px;
  transition: background-color 0.2s;
  line-height: 1;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  position: relative;
}

.logout-btn:hover,
.notification-center-btn:hover {
  background-color: var(--vdl-bg-hover);
  border-color: var(--vdl-glass-border-hover);
}

.notification-badge {
  position: absolute;
  top: -5px;
  right: -7px;
  min-width: 16px;
  height: 16px;
  padding: 0 4px;
  box-sizing: border-box;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--vdl-radius-pill);
  background: var(--vdl-danger);
  color: var(--vdl-text-on-danger);
  font-size: 10px;
  font-weight: 700;
  line-height: 1;
}

.library-panel-overlay {
  position: fixed;
  inset: 0;
  background: var(--vdl-bg-backdrop);
  display: flex;
  justify-content: center;
  align-items: center;
  z-index: var(--vdl-z-index-popover);
}

.library-panel-wrapper {
  width: calc(100% - 40px);
  height: calc(100% - 40px);
  border-radius: var(--vdl-radius-lg);
  border-color: var(--vdl-glass-border);
  box-shadow: var(--vdl-glass-shadow);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.library-panel-header {
  font-size: 14px;
  font-weight: 600;
  display: flex;
  justify-content: flex-start;
  align-items: center;
  padding: 16px 24px;
  color: var(--vdl-text-primary);
  border-bottom: 1px solid var(--vdl-border-subtle);
  background: color-mix(in srgb, var(--vdl-bg-muted) var(--vdl-glass-header-opacity), transparent);
}

.library-scroll {
  flex: 1;
  padding: 24px 40px;
  overflow-y: auto;
}

.library-section + .library-section {
  margin-top: 20px;
}

.library-section-title {
  color: var(--vdl-text-muted);
  font-size: 12px;
  font-weight: 600;
  margin-bottom: 10px;
}

.library-grid {
  display: grid;
  column-gap: 18px;
  row-gap: 12px;
  grid-template-columns: repeat(auto-fit, 120px);
  justify-content: start;
  align-content: start;
}

.library-item {
  border-color: transparent;
  border-radius: 8px;
  width: 120px;
  height: 120px;
  padding: 12px;
  cursor: pointer;
  display: flex;
  flex-direction: column;
  justify-content: center;
  align-items: center;
  gap: 4px;
  position: relative;
  color: var(--vdl-text-primary);
  transition:
    border-color 0.2s,
    background-color 0.2s;
}

.library-item.library-item.is-glass-idle {
  background-color: transparent;
  background-image: none;
  border-color: transparent;
  -webkit-backdrop-filter: none;
  backdrop-filter: none;
}

.library-item:hover {
  border-color: var(--vdl-glass-border-hover);
}

.library-icon {
  color: var(--vdl-text-primary);
}

.library-title {
  font-size: 12px;
  text-align: center;
  max-width: 100%;
}
</style>
