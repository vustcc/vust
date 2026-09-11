<script setup lang="ts">
import { ref } from 'vue'
import { useWindowManagerStore } from '../stores/window-manager'
import type { AppId } from '@/apps/registry'
import ApplicationWindow from '../components/layout/ApplicationWindow.vue'
import AppContextMenu from '../components/AppContextMenu.vue'
import AppIcon from '../components/icons/AppIcon.vue'

const store = useWindowManagerStore()
const desktopContextMenu = ref({
  visible: false,
  x: 0,
  y: 0,
  target: null as AppId | null,
})
const backgroundContextMenu = ref({
  visible: false,
  x: 0,
  y: 0,
})
const dragState = ref({
  draggingId: null as AppId | null,
  startX: 0,
  startY: 0,
  initialIconX: 0,
  initialIconY: 0,
})

const HEADER_HEIGHT = 40
const GRID_SIZE_X = 100
const GRID_SIZE_Y = 100
const DESKTOP_PADDING = 30
const DESKTOP_ICON_WIDTH = 90
const DESKTOP_ICON_HEIGHT = 80

/**
 * @description 将桌面图标坐标限制在可视桌面区域内。
 * @param x 原始 X 坐标。
 * @param y 原始 Y 坐标。
 */
function clampDesktopIconPosition(x: number, y: number) {
  const maxX = Math.max(DESKTOP_PADDING, window.innerWidth - DESKTOP_ICON_WIDTH - DESKTOP_PADDING)
  const maxY = Math.max(
    DESKTOP_PADDING,
    window.innerHeight - HEADER_HEIGHT - DESKTOP_ICON_HEIGHT - DESKTOP_PADDING,
  )

  return {
    x: Math.max(DESKTOP_PADDING, Math.min(x, maxX)),
    y: Math.max(DESKTOP_PADDING, Math.min(y, maxY)),
  }
}

/**
 * @description 按自动排序使用的桌面网格吸附图标坐标。
 * @param x 原始 X 坐标。
 * @param y 原始 Y 坐标。
 */
function snapDesktopIconPosition(x: number, y: number) {
  const snappedX = DESKTOP_PADDING + Math.round((x - DESKTOP_PADDING) / GRID_SIZE_X) * GRID_SIZE_X
  const snappedY = DESKTOP_PADDING + Math.round((y - DESKTOP_PADDING) / GRID_SIZE_Y) * GRID_SIZE_Y

  return clampDesktopIconPosition(snappedX, snappedY)
}

// 桌面区域禁用原生右键菜单，避免影响应用窗口内部操作。
/**
 * @description 点击桌面图标，打开或聚焦对应的应用窗口。
 * @param id 目标应用 ID。
 */
function openDesktopApp(id: AppId) {
  if (dragState.value.draggingId) return
  closeDesktopContextMenu()
  closeBackgroundContextMenu()
  store.openWindow(id)
}

function handleDesktopIconContextMenu(event: MouseEvent, id: AppId) {
  event.preventDefault()
  event.stopPropagation()
  closeBackgroundContextMenu()
  desktopContextMenu.value = {
    visible: true,
    x: event.clientX,
    y: event.clientY,
    target: id,
  }
}

function closeDesktopContextMenu() {
  desktopContextMenu.value.visible = false
  desktopContextMenu.value.target = null
}

function handleDesktopContextMenuDelete() {
  const target = desktopContextMenu.value.target
  if (target) {
    store.removeDesktopShortcut(target)
  }
  closeDesktopContextMenu()
}

function handleBackgroundContextMenu(event: MouseEvent) {
  event.preventDefault()
  closeDesktopContextMenu()
  backgroundContextMenu.value = {
    visible: true,
    x: event.clientX,
    y: event.clientY,
  }
}

function closeBackgroundContextMenu() {
  backgroundContextMenu.value.visible = false
}

function handleAutoArrange() {
  store.autoArrangeDesktopShortcuts()
  closeBackgroundContextMenu()
}

function handleDragStart(event: DragEvent, id: AppId) {
  const shortcut = store.desktopShortcuts.find((s) => s.appId === id)
  if (!shortcut) return

  dragState.value.draggingId = id
  dragState.value.startX = event.clientX
  dragState.value.startY = event.clientY
  dragState.value.initialIconX = shortcut.x
  dragState.value.initialIconY = shortcut.y

  store.beginDesktopReorder()
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/plain', id)
    // 隐藏默认的拖拽预览图，因为我们要自定义位置
    const img = new Image()
    img.src = 'data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7'
    event.dataTransfer.setDragImage(img, 0, 0)
  }
}

function handleDragOver(event: DragEvent) {
  const draggingId = dragState.value.draggingId
  if (!draggingId) return
  event.preventDefault()

  const dx = event.clientX - dragState.value.startX
  const dy = event.clientY - dragState.value.startY

  const { x: newX, y: newY } = clampDesktopIconPosition(
    dragState.value.initialIconX + dx,
    dragState.value.initialIconY + dy,
  )

  store.updateDesktopShortcutPosition(draggingId, newX, newY)
}

function handleLayerDrop(event: DragEvent) {
  const draggingId = dragState.value.draggingId
  if (!draggingId) return
  event.preventDefault()

  const dx = event.clientX - dragState.value.startX
  const dy = event.clientY - dragState.value.startY

  const { x: newX, y: newY } = snapDesktopIconPosition(
    dragState.value.initialIconX + dx,
    dragState.value.initialIconY + dy,
  )

  // 目标格子已有图标时交换位置，符合桌面图标拖拽排序习惯。
  const occupiedShortcut = store.desktopShortcuts.find(
    (s) => s.appId !== draggingId && s.x === newX && s.y === newY,
  )

  if (occupiedShortcut) {
    store.updateDesktopShortcutPosition(
      occupiedShortcut.appId,
      dragState.value.initialIconX,
      dragState.value.initialIconY,
    )
  }
  store.updateDesktopShortcutPosition(draggingId, newX, newY)
}

function handleDragEnd() {
  dragState.value.draggingId = null
  store.finishDesktopReorder()
}

/**
 * @file DesktopView.vue
 * @description 桌面背景视图，包含应用图标和所有活动的窗口实例。
 */
</script>

<template>
  <div class="desktop-container" data-ui="desktop-container" @click="closeBackgroundContextMenu">
    <div
      class="icon-layer"
      data-ui="icon-layer"
      @contextmenu.prevent="handleBackgroundContextMenu"
      @dragover="handleDragOver"
      @drop="handleLayerDrop"
    >
      <div
        v-for="shortcut in store.desktopShortcuts"
        :key="shortcut.appId"
        class="desktop-icon"
        :class="{
          'is-dragging': dragState.draggingId === shortcut.appId,
        }"
        :style="{
          left: shortcut.x + 'px',
          top: shortcut.y + 'px',
        }"
        data-ui="desktop-icon"
        :data-slot="shortcut.appId"
        @dblclick="openDesktopApp(shortcut.appId)"
        @contextmenu.prevent.stop="handleDesktopIconContextMenu($event, shortcut.appId)"
        @dragstart="handleDragStart($event, shortcut.appId)"
        @dragover="handleDragOver"
        @dragend="handleDragEnd"
        draggable="true"
      >
        <div class="icon-image" data-slot="icon-image">
          <AppIcon
            :name="store.availableApps[shortcut.appId]?.icon || shortcut.appId"
            :size="40"
            :label="
              store.availableApps[shortcut.appId]
                ? store.resolveAppTitle(store.availableApps[shortcut.appId])
                : shortcut.appId
            "
            :decorative="false"
          />
        </div>
        <span class="icon-label" data-slot="icon-label">{{
          store.availableApps[shortcut.appId]
            ? store.resolveAppTitle(store.availableApps[shortcut.appId])
            : shortcut.appId
        }}</span>
      </div>
    </div>

    <ApplicationWindow v-for="window in store.openWindows" :key="window.id" :window-data="window" />

    <AppContextMenu
      :visible="desktopContextMenu.visible"
      :x="desktopContextMenu.x"
      :y="desktopContextMenu.y"
      @close="closeDesktopContextMenu"
    >
      <button
        type="button"
        class="context-menu-btn"
        data-ui="delete-shortcut-btn"
        @click="handleDesktopContextMenuDelete"
      >
        {{ $t('desktop.header.contextMenu.deleteShortcut') }}
      </button>
    </AppContextMenu>

    <AppContextMenu
      :visible="backgroundContextMenu.visible"
      :x="backgroundContextMenu.x"
      :y="backgroundContextMenu.y"
      @close="closeBackgroundContextMenu"
    >
      <button
        type="button"
        class="context-menu-btn"
        data-ui="auto-arrange-btn"
        @click="handleAutoArrange"
      >
        {{ $t('desktop.background.contextMenu.autoArrange') }}
      </button>
    </AppContextMenu>
  </div>
</template>

<style scoped>
.desktop-container {
  /* 确保桌面内容区填满父容器 */
  width: 100%;
  height: 100%;
  padding: 0; /* 必须是 0，确保窗口 top: 0 贴合 Header 底部 */
  margin: 0; /* 必须是 0 */
  position: relative; /* 必须设置为 relative 以承载 absolute 的窗口 */
}

/* 图标层容器样式，用于提供图标距离顶部的间距，同时防止外边距合并 */
.icon-layer {
  position: absolute; /* 使用 absolute 而不是 relative 确保它不影响兄弟元素 (窗口) */
  top: 0;
  left: 0;
  width: 100%;
  height: 100%;
  z-index: 1; /* 确保图标可以被点击 */
}

.desktop-icon {
  position: absolute;
  width: 90px;
  cursor: pointer;
  text-align: center;
  user-select: none;
  transition:
    background-color 0.1s,
    transform 0.05s;
  border-radius: 8px;
}

.desktop-icon.is-dragging {
  opacity: 0.55;
}

.desktop-icon:hover {
  background-color: var(--vdl-bg-hover);
  outline: 1px solid var(--vdl-border-brand);
}

.icon-image {
  display: flex;
  justify-content: center;
  color: var(--vdl-desktop-icon-text);
  line-height: 1;
  margin-bottom: 5px;
}

.icon-label {
  display: inline-block;
  max-width: 84px;
  padding: 2px 6px;
  color: var(--vdl-desktop-icon-text);
  font-size: 13px;
  line-height: 1.25;
  overflow-wrap: anywhere;
  text-shadow: var(--vdl-desktop-icon-shadow);
}
</style>
