<script setup lang="ts">
import { computed, ref, watchEffect, onUnmounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { useWindowManagerStore, type WindowInstance } from '../../stores/window-manager'
import { useToastStore } from '../../stores/toast'
import AppIcon from '../icons/AppIcon.vue'

const props = defineProps<{
  windowData: WindowInstance // 从 Store 传递进来的窗口数据
}>()

const store = useWindowManagerStore()
const toastStore = useToastStore()
const { t } = useI18n()
const isDragging = ref(false)
const isResizing = ref(false)

// --- 窗口控制函数 ---
const handleClose = () => {
  const guard = store.checkBeforeCloseWindow(props.windowData.id)
  if (!guard.allowed) {
    const reason = guard.blockers.map((item) => item.reason).join('；')
    toastStore.error(reason || t('guard.closeWindowBlocked'))
    return
  }
  store.closeWindow(props.windowData.id)
}
const handleMinimize = () => store.minimizeWindow(props.windowData.id)
const handleMaximize = () => store.toggleMaximize(props.windowData.id)
const handleFocus = () => store.focusWindow(props.windowData.id)

function shouldAllowNativeContextMenu(event: MouseEvent) {
  if (event.shiftKey || event.altKey) return true
  if (props.windowData.contextMenuPolicy === 'native') return true
  const target = event.target as Element | null
  if (!target) return false
  if (
    target.closest(
      '[data-native-context-menu], input, textarea, select, [contenteditable="true"], .monaco-editor, .xterm, iframe',
    )
  ) {
    return true
  }
  return false
}

function handleWindowContextMenu(event: MouseEvent) {
  const target = event.target as HTMLElement
  const inContent = !!target?.closest('.window-content')
  if (inContent && shouldAllowNativeContextMenu(event)) return
  event.preventDefault()
}

// --- 拖动逻辑所需的状态 ---
let mouseStartX = 0 // 鼠标按下时的初始X坐标 (Viewport 绝对坐标)
let mouseStartY = 0 // 鼠标按下时的初始Y坐标 (Viewport 绝对坐标)
let windowStartX = 0 // 鼠标按下时，窗口的初始X存储位置 (Store 中的 positionX)
let windowStartY = 0 // 鼠标按下时，窗口的初始Y存储位置 (Store 中的 positionY)
let windowStartWidth = 0 // 鼠标按下时，窗口的初始宽度
let windowStartHeight = 0 // 鼠标按下时，窗口的初始高度
let resizeDirection: ResizeDirection | null = null
let latestClientX = 0
let latestClientY = 0
let pendingAnimationFrame: number | null = null

const DEFAULT_MIN_WIDTH = 300
const DEFAULT_MIN_HEIGHT = 200
// --- 布局常量 ---
// 这些数值需与 theme.css 中的 --vdl-header-height 和 --vdl-window-header-height 保持一致（去掉 px）
const HEADER_HEIGHT = 40
const WINDOW_HEADER_HEIGHT = 30

type ResizeDirection = 'n' | 's' | 'e' | 'w' | 'ne' | 'nw' | 'se' | 'sw'

const isWindowOperating = computed(() => store.activeWindowOperation !== null)

function clearPendingAnimationFrame() {
  if (pendingAnimationFrame === null) return
  cancelAnimationFrame(pendingAnimationFrame)
  pendingAnimationFrame = null
}

function scheduleWindowUpdate(callback: () => void) {
  if (pendingAnimationFrame !== null) return
  pendingAnimationFrame = requestAnimationFrame(() => {
    pendingAnimationFrame = null
    callback()
  })
}

function applyDragPosition() {
  if (!isDragging.value) return

  // 计算鼠标移动的增量 (Delta)
  const deltaX = latestClientX - mouseStartX
  const deltaY = latestClientY - mouseStartY

  // 计算新位置（暂未应用限制）
  let newX = windowStartX + deltaX
  let newY = windowStartY + deltaY

  // --- 边界限制计算 ---
  // 获取当前视口尺寸
  const viewportWidth = window.innerWidth
  const viewportHeight = window.innerHeight

  // 左右边界最小保留宽度
  const MARGIN_SAFE_WIDTH = 140

  // 桌面工作区高度 (视口 - 顶部栏)
  const desktopAreaHeight = viewportHeight - HEADER_HEIGHT

  // Y 轴限制
  // 顶部边界：不能超过 Header (newY >= 0)
  const minY = 0
  // 底部边界：标题栏底部不能超出视口底部
  const maxY = desktopAreaHeight - WINDOW_HEADER_HEIGHT

  // X 轴限制

  // 左边界 (minX)：窗口的左边缘可以向左移动，直到窗口的右边缘距离视口左边缘至少 MARGIN_SAFE_WIDTH。
  // newX + windowWidth >= MARGIN_SAFE_WIDTH => newX >= MARGIN_SAFE_WIDTH - windowWidth
  const minX = MARGIN_SAFE_WIDTH - props.windowData.width

  // 右边界 (maxX)：窗口的左边缘可以向右移动，直到窗口的左边缘距离视口右边缘至少 MARGIN_SAFE_WIDTH。
  // newX <= viewportWidth - MARGIN_SAFE_WIDTH
  const maxX = viewportWidth - MARGIN_SAFE_WIDTH

  // 应用限制
  if (newY < minY) newY = minY
  if (newY > maxY) newY = maxY
  if (newX < minX) newX = minX // 应用新的左边界限制
  if (newX > maxX) newX = maxX // 应用右边界限制

  if (newX !== props.windowData.positionX || newY !== props.windowData.positionY) {
    store.updatePosition(props.windowData.id, newX, newY)
  }
}

function applyResize() {
  if (!isResizing.value) return

  const deltaX = latestClientX - mouseStartX
  const deltaY = latestClientY - mouseStartY

  let newWidth = windowStartWidth
  let newHeight = windowStartHeight
  let newX = windowStartX
  let newY = windowStartY

  const viewportWidth = window.innerWidth
  const viewportHeight = window.innerHeight
  const desktopAreaHeight = viewportHeight - HEADER_HEIGHT
  const minWidth = Math.max(props.windowData.minWidth, DEFAULT_MIN_WIDTH)
  const minHeight = Math.max(props.windowData.minHeight, DEFAULT_MIN_HEIGHT)

  const rightEdge = windowStartX + windowStartWidth
  const bottomEdge = windowStartY + windowStartHeight

  if (resizeDirection?.includes('e')) {
    const maxWidth = Math.max(minWidth, viewportWidth - windowStartX)
    newWidth = Math.min(maxWidth, windowStartWidth + deltaX)
  }
  if (resizeDirection?.includes('s')) {
    const maxHeight = Math.max(minHeight, desktopAreaHeight - windowStartY)
    newHeight = Math.min(maxHeight, windowStartHeight + deltaY)
  }
  if (resizeDirection?.includes('w')) {
    const nextX = Math.min(Math.max(0, windowStartX + deltaX), rightEdge - minWidth)
    newX = nextX
    newWidth = rightEdge - nextX
  }
  if (resizeDirection?.includes('n')) {
    const nextY = Math.min(Math.max(0, windowStartY + deltaY), bottomEdge - minHeight)
    newY = nextY
    newHeight = bottomEdge - nextY
  }

  newWidth = Math.max(minWidth, newWidth)
  newHeight = Math.max(minHeight, newHeight)

  if (newWidth !== props.windowData.width || newHeight !== props.windowData.height) {
    store.updateSize(props.windowData.id, newWidth, newHeight)
  }
  if (newX !== props.windowData.positionX || newY !== props.windowData.positionY) {
    store.updatePosition(props.windowData.id, newX, newY)
  }
}

/**
 * @description 鼠标按下时，开始拖动
 * @param event 鼠标事件
 */
const startDrag = (event: MouseEvent) => {
  // 全屏状态下禁止拖动
  if (props.windowData.isMaximized) return

  event.preventDefault()
  isDragging.value = true
  handleFocus() // 拖动即聚焦

  // 1. 记录鼠标按下时的绝对位置
  mouseStartX = event.clientX
  mouseStartY = event.clientY

  // 2. 记录窗口当前的存储位置
  windowStartX = props.windowData.positionX
  windowStartY = props.windowData.positionY
  latestClientX = event.clientX
  latestClientY = event.clientY
  store.beginWindowOperation(props.windowData.id, 'drag')

  // 确保事件监听器附加到 document
  document.addEventListener('mousemove', dragMove)
  document.addEventListener('mouseup', dragEnd)
}

/**
 * @description 鼠标移动时，更新窗口位置，并应用边界限制
 * @param event 鼠标事件
 */
const dragMove = (event: MouseEvent) => {
  if (!isDragging.value) return
  latestClientX = event.clientX
  latestClientY = event.clientY
  scheduleWindowUpdate(applyDragPosition)
}

/**
 * @description 鼠标松开时，停止拖动
 */
const dragEnd = () => {
  clearPendingAnimationFrame()
  applyDragPosition()
  isDragging.value = false
  store.finishWindowOperation(props.windowData.id)
  document.removeEventListener('mousemove', dragMove)
  document.removeEventListener('mouseup', dragEnd)
}

/**
 * @description 鼠标按下时，开始改变窗口大小
 * @param event 鼠标事件
 */
const startResize = (event: MouseEvent, direction: ResizeDirection) => {
  if (props.windowData.isMaximized) return

  event.preventDefault()
  event.stopPropagation()
  isResizing.value = true
  handleFocus()
  resizeDirection = direction

  mouseStartX = event.clientX
  mouseStartY = event.clientY
  windowStartWidth = props.windowData.width
  windowStartHeight = props.windowData.height
  windowStartX = props.windowData.positionX
  windowStartY = props.windowData.positionY
  latestClientX = event.clientX
  latestClientY = event.clientY
  store.beginWindowOperation(props.windowData.id, 'resize')

  document.addEventListener('mousemove', resizeMove)
  document.addEventListener('mouseup', resizeEnd)
}

/**
 * @description 鼠标移动时，更新窗口尺寸，并应用边界限制
 * @param event 鼠标事件
 */
const resizeMove = (event: MouseEvent) => {
  if (!isResizing.value) return
  latestClientX = event.clientX
  latestClientY = event.clientY
  scheduleWindowUpdate(applyResize)
}

/**
 * @description 鼠标松开时，停止改变窗口大小
 */
const resizeEnd = () => {
  clearPendingAnimationFrame()
  applyResize()
  isResizing.value = false
  resizeDirection = null
  store.finishWindowOperation(props.windowData.id)
  document.removeEventListener('mousemove', resizeMove)
  document.removeEventListener('mouseup', resizeEnd)
}

// 确保初始位置应用到 CSS
const windowStyle = ref({
  transform: `translate(${props.windowData.positionX}px, ${props.windowData.positionY}px)`,
  zIndex: props.windowData.zIndex,
  width: `${props.windowData.width}px`,
  height: `${props.windowData.height}px`,
})

// 监听位置和 Z-index 变化
watchEffect(() => {
  windowStyle.value.transform = `translate(${props.windowData.positionX}px, ${props.windowData.positionY}px)`
  windowStyle.value.zIndex = props.windowData.zIndex
  // 让 windowStyle 总是反映 props.windowData 中存储的原始尺寸，
  // 最大化时的覆盖行为完全交给 CSS 类 `app-window-maximized`。
  windowStyle.value.width = `${props.windowData.width}px`
  windowStyle.value.height = `${props.windowData.height}px`

  // 在拖动时，禁用文本选择
  if (isDragging.value) {
    document.body.style.userSelect = 'none'
    document.body.style.cursor = 'grabbing'
  } else if (isResizing.value) {
    document.body.style.userSelect = 'none'
    document.body.style.cursor = 'se-resize'
  } else {
    document.body.style.userSelect = 'auto'
    document.body.style.cursor = 'default'
  }
})

onUnmounted(() => {
  // 组件卸载时移除可能残留的 document 事件监听器，并恢复 body 样式
  clearPendingAnimationFrame()
  store.finishWindowOperation(props.windowData.id)
  document.removeEventListener('mousemove', dragMove)
  document.removeEventListener('mouseup', dragEnd)
  document.removeEventListener('mousemove', resizeMove)
  document.removeEventListener('mouseup', resizeEnd)
  document.body.style.userSelect = 'auto'
  document.body.style.cursor = 'default'
})

/**
 * @file ApplicationWindow.vue
 * @description 通用应用窗口容器，提供拖动、控制按钮和动态内容渲染。
 */
</script>

<template>
  <div
    v-show="!windowData.isMinimized"
    class="window-container"
    :class="{
      'window-focused': windowData.zIndex === store.maxZIndex,
      'window-maximized': windowData.isMaximized,
      'window-operating': isWindowOperating,
      'window-dragging': isDragging,
      'window-resizing': isResizing,
    }"
    :style="windowStyle"
    @mousedown="handleFocus"
    @contextmenu.capture="handleWindowContextMenu"
  >
    <div
      class="window-header"
      data-slot="header"
      @mousedown="startDrag"
      @dblclick="handleMaximize"
      @contextmenu.prevent
    >
      <div class="window-controls">
        <button
          class="control-btn btn-close"
          @click.stop="handleClose"
          :title="t('common.close')"
        ></button>
        <button
          class="control-btn btn-minimize"
          @click.stop="handleMinimize"
          :title="t('common.minimize')"
        ></button>
        <button
          class="control-btn btn-maximize"
          @click.stop="handleMaximize"
          :title="t('common.maximize')"
        ></button>
      </div>

      <span class="window-title">
        <AppIcon :name="windowData.icon" :size="16" :label="windowData.title" />
        <span>{{ windowData.title }}</span>
      </span>
    </div>

    <div class="window-content" data-slot="content">
      <component
        :is="windowData.component"
        :is-maximized="windowData.isMaximized"
        :window-id="windowData.id"
        :app-id="windowData.appId"
        :payload="windowData.payload"
      />
      <div
        v-if="isWindowOperating"
        class="window-operation-shield"
        data-slot="operation-shield"
      ></div>
    </div>
    <div
      v-if="!windowData.isMaximized"
      class="window-resizer resizer-n"
      @mousedown="startResize($event, 'n')"
    ></div>
    <div
      v-if="!windowData.isMaximized"
      class="window-resizer resizer-s"
      @mousedown="startResize($event, 's')"
    ></div>
    <div
      v-if="!windowData.isMaximized"
      class="window-resizer resizer-e"
      @mousedown="startResize($event, 'e')"
    ></div>
    <div
      v-if="!windowData.isMaximized"
      class="window-resizer resizer-w"
      @mousedown="startResize($event, 'w')"
    ></div>
    <div
      v-if="!windowData.isMaximized"
      class="window-resizer resizer-ne"
      @mousedown="startResize($event, 'ne')"
    ></div>
    <div
      v-if="!windowData.isMaximized"
      class="window-resizer resizer-nw"
      @mousedown="startResize($event, 'nw')"
    ></div>
    <div
      v-if="!windowData.isMaximized"
      class="window-resizer resizer-se"
      @mousedown="startResize($event, 'se')"
    ></div>
    <div
      v-if="!windowData.isMaximized"
      class="window-resizer resizer-sw"
      @mousedown="startResize($event, 'sw')"
    ></div>
  </div>
</template>

<style scoped>
.window-container {
  position: absolute;
  min-width: 300px;
  min-height: 200px;
  background-color: var(--vdl-bg-panel);
  border-radius: var(--vdl-radius-lg);
  box-shadow: var(--vdl-shadow-window);
  border: 1px solid var(--vdl-border-default);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  transition: box-shadow 0.1s;
}

.window-dragging,
.window-resizing {
  transition: none;
  will-change: transform, width, height;
}

.window-operating :deep(iframe) {
  pointer-events: none;
}

/* 焦点状态 */
.window-focused {
  border-color: var(--vdl-border-brand);
  box-shadow: var(--vdl-shadow-brand), var(--vdl-shadow-window);
}

/* 标题栏样式 */
.window-header {
  height: var(--vdl-window-header-height);
  flex-shrink: 0;
  background-color: var(--vdl-bg-muted);
  border-bottom: 1px solid var(--vdl-border-subtle);
  display: flex;
  align-items: center;
  user-select: none;
  cursor: grab; /* 拖动光标 */
  padding: 0 var(--vdl-space-2);
  position: relative; /* 确保标题栏可以拖动 */
}

.window-title {
  position: absolute;
  left: 50%;
  transform: translateX(-50%);
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--vdl-font-body-sm);
  color: var(--vdl-text-primary);
  font-weight: 500;
}

/* 窗口控制按钮容器 */
.window-controls {
  display: flex;
  gap: 6px;
  z-index: 10; /* 确保按钮在标题上方 */
}

/* 窗口控制按钮 (仿 macOS 红黄绿点) */
.control-btn {
  width: 12px;
  height: 12px;
  border-radius: 50%;
  border: none;
  cursor: pointer;
  padding: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  position: relative;
  transition: filter 0.2s;
}

.control-btn:active {
  filter: brightness(0.8);
}

.control-btn::after {
  content: '';
  width: 8px;
  height: 8px;
  opacity: 0;
  transition: opacity 0.2s;
  background-size: contain;
  background-repeat: no-repeat;
  background-position: center;
}

/* 当鼠标悬停在按钮组上时显示图标 */
.window-controls:hover .control-btn::after {
  opacity: 1;
}

.btn-close {
  background-color: #ff5f56; /* 红色 */
  border: 0.5px solid #e0443e;
}
.btn-close::after {
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='%234c0000' stroke-width='5' stroke-linecap='round' stroke-linejoin='round'%3E%3Cline x1='18' y1='6' x2='6' y2='18'%3E%3C/line%3E%3Cline x1='6' y1='6' x2='18' y2='18'%3E%3C/line%3E%3C/svg%3E");
}

.btn-minimize {
  background-color: #ffbd2e; /* 黄色 */
  border: 0.5px solid #d8a226;
}
.btn-minimize::after {
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='%23995700' stroke-width='5' stroke-linecap='round' stroke-linejoin='round'%3E%3Cline x1='5' y1='12' x2='19' y2='12'%3E%3C/line%3E%3C/svg%3E");
}

.btn-maximize {
  background-color: #27c93f; /* 绿色 */
  border: 0.5px solid #1aab29;
}
.btn-maximize::after {
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='%23006400' stroke-width='5' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpolyline points='15 3 21 3 21 9'%3E%3C/polyline%3E%3Cpolyline points='9 21 3 21 3 15'%3E%3C/polyline%3E%3Cline x1='21' y1='3' x2='14' y2='10'%3E%3C/line%3E%3Cline x1='3' y1='21' x2='10' y2='14'%3E%3C/line%3E%3C/svg%3E");
}

/* 如果是全屏状态，绿色按钮显示恢复图标 */
.window-maximized .btn-maximize::after {
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='%23006400' stroke-width='5' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpolyline points='4 14 10 14 10 20'%3E%3C/polyline%3E%3Cpolyline points='20 10 14 10 14 4'%3E%3C/polyline%3E%3Cline x1='14' y1='10' x2='21' y2='3'%3E%3C/line%3E%3Cline x1='3' y1='21' x2='10' y2='14'%3E%3C/line%3E%3C/svg%3E");
}

/* 窗口内容 */
.window-content {
  position: relative;
  flex-grow: 1;
  overflow: auto; /* 允许应用内容滚动 */
  /* 确保应用组件可以撑满内容区 */
  display: flex;
  flex-direction: column;
}

.window-operation-shield {
  position: absolute;
  inset: 0;
  z-index: 20;
  background: transparent;
  cursor: inherit;
}

.window-resizer {
  position: absolute;
  z-index: 5;
}

.resizer-n,
.resizer-s {
  left: 8px;
  right: 8px;
  height: 6px;
}

.resizer-e,
.resizer-w {
  top: 8px;
  bottom: 8px;
  width: 6px;
}

.resizer-n {
  top: 0;
  cursor: n-resize;
}

.resizer-s {
  bottom: 0;
  cursor: s-resize;
}

.resizer-e {
  right: 0;
  cursor: e-resize;
}

.resizer-w {
  left: 0;
  cursor: w-resize;
}

.resizer-ne,
.resizer-nw,
.resizer-se,
.resizer-sw {
  width: 12px;
  height: 12px;
}

.resizer-ne {
  top: 0;
  right: 0;
  cursor: ne-resize;
}

.resizer-nw {
  top: 0;
  left: 0;
  cursor: nw-resize;
}

.resizer-se {
  right: 0;
  bottom: 0;
  cursor: se-resize;
}

.resizer-sw {
  left: 0;
  bottom: 0;
  cursor: sw-resize;
}

/* 全屏样式 */
.window-maximized {
  top: 0 !important;
  left: 0 !important;
  transform: none !important;
  width: 100vw !important;
  height: calc(100vh - var(--vdl-header-height)) !important;
  border-radius: 0;
}
</style>
