<script setup lang="ts">
/**
 * @file EditorMenuBar.vue
 * @description 文件编辑器专用菜单栏，提供文件操作与视图配置的标准菜单交互。
 */
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import VustIcon from '@/components/icons/VustIcon.vue'

const props = defineProps<{
  menuId: string
  wordWrap: boolean
  minimap: boolean
  stickyScroll: boolean
  highlightAmbiguousUnicode: boolean
  renderWhitespace: 'none' | 'selection' | 'all'
  fontSize: number
  reloadDisabled: boolean
  saveDisabled: boolean
}>()

const emit = defineEmits<{
  reload: []
  save: []
  'update:wordWrap': [value: boolean]
  'update:minimap': [value: boolean]
  'update:stickyScroll': [value: boolean]
  'update:highlightAmbiguousUnicode': [value: boolean]
  'update:renderWhitespace': [value: 'none' | 'selection' | 'all']
  adjustFont: [delta: number]
  resetFont: []
  resetPreferences: []
}>()

const { t } = useI18n()
const menuBarRef = ref<HTMLElement | null>(null)
const fileTriggerRef = ref<HTMLButtonElement | null>(null)
const viewTriggerRef = ref<HTMLButtonElement | null>(null)
const fileMenuRef = ref<HTMLElement | null>(null)
const viewMenuRef = ref<HTMLElement | null>(null)
const whitespaceMenuTriggerRef = ref<HTMLButtonElement | null>(null)
const whitespaceMenuRef = ref<HTMLElement | null>(null)
const fontMenuTriggerRef = ref<HTMLButtonElement | null>(null)
const fontMenuRef = ref<HTMLElement | null>(null)
const openMenu = ref<'file' | 'view' | null>(null)
const activeTrigger = ref<'file' | 'view'>('file')
const openSubmenu = ref<'whitespace' | 'font' | null>(null)
const rootMenuStyle = ref<Record<string, string>>({})
const submenuStyle = ref<Record<string, string>>({})

const fileMenuId = `${props.menuId}-file-menu`
const viewMenuId = `${props.menuId}-view-menu`
const whitespaceMenuId = `${props.menuId}-whitespace-menu`
const fontMenuId = `${props.menuId}-font-menu`
const whitespaceOptions = ['none', 'selection', 'all'] as const
const whitespaceLabel = (value: (typeof whitespaceOptions)[number]) =>
  t(`app.fileEditor.whitespace${value[0].toUpperCase()}${value.slice(1)}`)
const whitespaceValueLabel = computed(() => whitespaceLabel(props.renderWhitespace))

/** 返回指定顶级菜单的触发按钮。 */
const getTrigger = (menu: 'file' | 'view') =>
  menu === 'file' ? fileTriggerRef.value : viewTriggerRef.value

/** 返回当前顶级菜单浮层。 */
const getOpenMenuElement = () => (openMenu.value === 'file' ? fileMenuRef.value : viewMenuRef.value)

/** 获取菜单内所有可用操作项。 */
const getEnabledItems = (menu: HTMLElement | null) =>
  menu
    ? [...menu.querySelectorAll<HTMLButtonElement>(':scope > .editor-menu-item:not(:disabled)')]
    : []

/** 将浮层限制在当前视口内，并优先显示在触发器下方。 */
const positionRootMenu = () => {
  const trigger = openMenu.value ? getTrigger(openMenu.value) : null
  const menu = getOpenMenuElement()
  if (!trigger || !menu) return
  const triggerRect = trigger.getBoundingClientRect()
  const menuRect = menu.getBoundingClientRect()
  const margin = 8
  const gap = 2
  const left = Math.min(
    Math.max(margin, triggerRect.left),
    Math.max(margin, window.innerWidth - menuRect.width - margin),
  )
  const belowTop = triggerRect.bottom + gap
  const top =
    belowTop + menuRect.height <= window.innerHeight - margin
      ? belowTop
      : Math.max(margin, triggerRect.top - menuRect.height - gap)
  rootMenuStyle.value = { left: `${left}px`, top: `${top}px` }
}

/** 返回指定二级菜单的触发项。 */
const getSubmenuTrigger = (submenu: 'whitespace' | 'font') =>
  submenu === 'whitespace' ? whitespaceMenuTriggerRef.value : fontMenuTriggerRef.value

/** 返回当前二级菜单浮层。 */
const getOpenSubmenuElement = () =>
  openSubmenu.value === 'whitespace' ? whitespaceMenuRef.value : fontMenuRef.value

/** 将二级菜单放在父菜单项侧边，空间不足时向左展开。 */
const positionSubmenu = () => {
  const trigger = openSubmenu.value ? getSubmenuTrigger(openSubmenu.value) : null
  const menu = getOpenSubmenuElement()
  if (!trigger || !menu) return
  const triggerRect = trigger.getBoundingClientRect()
  const menuRect = menu.getBoundingClientRect()
  const margin = 8
  const gap = 2
  const rightLeft = triggerRect.right + gap
  const left =
    rightLeft + menuRect.width <= window.innerWidth - margin
      ? rightLeft
      : Math.max(margin, triggerRect.left - menuRect.width - gap)
  const top = Math.min(
    Math.max(margin, triggerRect.top),
    Math.max(margin, window.innerHeight - menuRect.height - margin),
  )
  submenuStyle.value = { left: `${left}px`, top: `${top}px` }
}

/** 更新所有已打开菜单的位置。 */
const updatePositions = () => {
  positionRootMenu()
  if (openSubmenu.value) positionSubmenu()
}

/** 关闭全部菜单，可选择将焦点返回触发按钮。 */
const closeMenus = (restoreFocus = false) => {
  const trigger = openMenu.value ? getTrigger(openMenu.value) : null
  openMenu.value = null
  openSubmenu.value = null
  if (restoreFocus) nextTick(() => trigger?.focus())
}

/** 打开指定顶级菜单，并按需聚焦首个或末个菜单项。 */
const openRootMenu = async (menu: 'file' | 'view', focusTarget?: 'first' | 'last') => {
  openMenu.value = menu
  openSubmenu.value = null
  await nextTick()
  positionRootMenu()
  if (focusTarget) {
    const items = getEnabledItems(getOpenMenuElement())
    items[focusTarget === 'first' ? 0 : items.length - 1]?.focus()
  }
}

/** 点击顶级菜单时切换其展开状态。 */
const toggleRootMenu = (menu: 'file' | 'view') => {
  activeTrigger.value = menu
  if (openMenu.value === menu) {
    closeMenus()
    return
  }
  void openRootMenu(menu)
}

/** 已有菜单展开时，悬停另一个顶级菜单会直接切换。 */
const handleTriggerMouseenter = (menu: 'file' | 'view') => {
  if (openMenu.value && openMenu.value !== menu) {
    activeTrigger.value = menu
    void openRootMenu(menu)
  }
}

/** 处理顶级菜单栏的方向键与展开操作。 */
const handleTriggerKeydown = (event: KeyboardEvent, menu: 'file' | 'view') => {
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault()
    void openRootMenu(menu, event.key === 'ArrowDown' ? 'first' : 'last')
    return
  }
  if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
    event.preventDefault()
    const nextMenu = menu === 'file' ? 'view' : 'file'
    activeTrigger.value = nextMenu
    getTrigger(nextMenu)?.focus()
    if (openMenu.value) void openRootMenu(nextMenu)
    return
  }
  if (event.key === 'Escape' && openMenu.value) {
    event.preventDefault()
    closeMenus(true)
  }
}

/** 在顶级下拉菜单中循环移动焦点。 */
const handleRootMenuKeydown = (event: KeyboardEvent) => {
  const menu = getOpenMenuElement()
  const items = getEnabledItems(menu)
  const currentIndex = items.indexOf(document.activeElement as HTMLButtonElement)
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault()
    const direction = event.key === 'ArrowDown' ? 1 : -1
    items[(currentIndex + direction + items.length) % items.length]?.focus()
  } else if (event.key === 'Home' || event.key === 'End') {
    event.preventDefault()
    items[event.key === 'Home' ? 0 : items.length - 1]?.focus()
  } else if (event.key === 'Escape') {
    event.preventDefault()
    closeMenus(true)
  } else if (event.key === 'ArrowLeft') {
    event.preventDefault()
    const nextMenu = openMenu.value === 'file' ? 'view' : 'file'
    activeTrigger.value = nextMenu
    void openRootMenu(nextMenu, 'first')
  } else if (
    event.key === 'ArrowRight' &&
    document.activeElement === whitespaceMenuTriggerRef.value
  ) {
    event.preventDefault()
    void openNestedMenu('whitespace', true)
  } else if (event.key === 'ArrowRight' && document.activeElement === fontMenuTriggerRef.value) {
    event.preventDefault()
    void openNestedMenu('font', true)
  } else if (event.key === 'ArrowRight') {
    event.preventDefault()
    const nextMenu = openMenu.value === 'view' ? 'file' : 'view'
    activeTrigger.value = nextMenu
    void openRootMenu(nextMenu, 'first')
  }
}

/** 打开指定二级菜单，并可聚焦第一个可用操作。 */
const openNestedMenu = async (submenu: 'whitespace' | 'font', focusFirst = false) => {
  openSubmenu.value = submenu
  await nextTick()
  positionSubmenu()
  if (focusFirst) getEnabledItems(getOpenSubmenuElement())[0]?.focus()
}

/** 处理二级菜单的键盘导航。 */
const handleSubmenuKeydown = (event: KeyboardEvent) => {
  const submenu = openSubmenu.value
  const items = getEnabledItems(getOpenSubmenuElement())
  const currentIndex = items.indexOf(document.activeElement as HTMLButtonElement)
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault()
    const direction = event.key === 'ArrowDown' ? 1 : -1
    items[(currentIndex + direction + items.length) % items.length]?.focus()
  } else if (event.key === 'Home' || event.key === 'End') {
    event.preventDefault()
    items[event.key === 'Home' ? 0 : items.length - 1]?.focus()
  } else if (event.key === 'ArrowLeft') {
    event.preventDefault()
    openSubmenu.value = null
    nextTick(() => {
      if (submenu) getSubmenuTrigger(submenu)?.focus()
    })
  } else if (event.key === 'Escape') {
    event.preventDefault()
    closeMenus(true)
  }
}

/** 执行菜单动作并关闭全部菜单。 */
const runAction = (action: () => void) => {
  action()
  closeMenus()
}

/** 点击菜单外部时关闭菜单。 */
const handleDocumentPointerdown = (event: PointerEvent) => {
  const target = event.target as Node
  if (
    menuBarRef.value?.contains(target) ||
    fileMenuRef.value?.contains(target) ||
    viewMenuRef.value?.contains(target) ||
    whitespaceMenuRef.value?.contains(target) ||
    fontMenuRef.value?.contains(target)
  ) {
    return
  }
  closeMenus()
}

onMounted(() => {
  document.addEventListener('pointerdown', handleDocumentPointerdown)
  window.addEventListener('resize', updatePositions)
  window.addEventListener('scroll', updatePositions, true)
})

onBeforeUnmount(() => {
  document.removeEventListener('pointerdown', handleDocumentPointerdown)
  window.removeEventListener('resize', updatePositions)
  window.removeEventListener('scroll', updatePositions, true)
})
</script>

<template>
  <div
    ref="menuBarRef"
    class="editor-menu-bar"
    data-ui="editor-menu-bar"
    data-slot="menu-bar"
    role="menubar"
    :aria-label="t('app.fileEditor.menuBar')"
  >
    <button
      ref="fileTriggerRef"
      type="button"
      class="editor-menu-trigger"
      data-ui="editor-file-menu-trigger"
      role="menuitem"
      aria-haspopup="menu"
      :aria-controls="fileMenuId"
      :aria-expanded="openMenu === 'file'"
      :tabindex="activeTrigger === 'file' ? 0 : -1"
      @focus="activeTrigger = 'file'"
      @click.stop="toggleRootMenu('file')"
      @mouseenter="handleTriggerMouseenter('file')"
      @keydown="handleTriggerKeydown($event, 'file')"
    >
      {{ t('app.fileEditor.fileMenu') }}
    </button>
    <button
      ref="viewTriggerRef"
      type="button"
      class="editor-menu-trigger"
      data-ui="editor-view-menu-trigger"
      role="menuitem"
      aria-haspopup="menu"
      :aria-controls="viewMenuId"
      :aria-expanded="openMenu === 'view'"
      :tabindex="activeTrigger === 'view' ? 0 : -1"
      @focus="activeTrigger = 'view'"
      @click.stop="toggleRootMenu('view')"
      @mouseenter="handleTriggerMouseenter('view')"
      @keydown="handleTriggerKeydown($event, 'view')"
    >
      {{ t('app.fileEditor.viewMenu') }}
    </button>
  </div>

  <Teleport to="body">
    <div
      v-if="openMenu === 'file'"
      :id="fileMenuId"
      ref="fileMenuRef"
      class="editor-dropdown-menu"
      data-ui="editor-file-menu"
      role="menu"
      :aria-label="t('app.fileEditor.fileMenu')"
      :style="rootMenuStyle"
      @keydown="handleRootMenuKeydown"
    >
      <button
        type="button"
        class="editor-menu-item"
        data-ui="editor-save"
        role="menuitem"
        :disabled="saveDisabled"
        @click="runAction(() => emit('save'))"
      >
        <VustIcon name="save" :size="15" />
        <span class="editor-menu-label">{{ t('app.fileEditor.save') }}</span>
        <span class="editor-menu-hint">Ctrl+S</span>
      </button>
      <button
        type="button"
        class="editor-menu-item"
        data-ui="editor-reload"
        role="menuitem"
        :disabled="reloadDisabled"
        @click="runAction(() => emit('reload'))"
      >
        <VustIcon name="refresh" :size="15" />
        <span class="editor-menu-label">{{ t('app.fileEditor.reload') }}</span>
      </button>
    </div>

    <div
      v-if="openMenu === 'view'"
      :id="viewMenuId"
      ref="viewMenuRef"
      class="editor-dropdown-menu"
      data-ui="editor-view-menu"
      role="menu"
      :aria-label="t('app.fileEditor.viewMenu')"
      :style="rootMenuStyle"
      @keydown="handleRootMenuKeydown"
    >
      <button
        type="button"
        class="editor-menu-item"
        data-ui="editor-word-wrap"
        role="menuitemcheckbox"
        :aria-checked="wordWrap"
        @mouseenter="openSubmenu = null"
        @click="runAction(() => emit('update:wordWrap', !wordWrap))"
      >
        <span class="editor-menu-icon-slot">
          <VustIcon v-if="wordWrap" name="success" :size="15" />
        </span>
        <span class="editor-menu-label">{{ t('app.fileEditor.wordWrap') }}</span>
      </button>
      <button
        type="button"
        class="editor-menu-item"
        data-ui="editor-minimap"
        role="menuitemcheckbox"
        :aria-checked="minimap"
        @mouseenter="openSubmenu = null"
        @click="runAction(() => emit('update:minimap', !minimap))"
      >
        <span class="editor-menu-icon-slot">
          <VustIcon v-if="minimap" name="success" :size="15" />
        </span>
        <span class="editor-menu-label">{{ t('app.fileEditor.minimap') }}</span>
      </button>
      <button
        type="button"
        class="editor-menu-item"
        data-ui="editor-sticky-scroll"
        role="menuitemcheckbox"
        :aria-checked="stickyScroll"
        @mouseenter="openSubmenu = null"
        @click="runAction(() => emit('update:stickyScroll', !stickyScroll))"
      >
        <span class="editor-menu-icon-slot">
          <VustIcon v-if="stickyScroll" name="success" :size="15" />
        </span>
        <span class="editor-menu-label">{{ t('app.fileEditor.stickyScroll') }}</span>
      </button>
      <button
        type="button"
        class="editor-menu-item"
        data-ui="editor-ambiguous-unicode-highlight"
        role="menuitemcheckbox"
        :aria-checked="highlightAmbiguousUnicode"
        @mouseenter="openSubmenu = null"
        @click="
          runAction(() => emit('update:highlightAmbiguousUnicode', !highlightAmbiguousUnicode))
        "
      >
        <span class="editor-menu-icon-slot">
          <VustIcon v-if="highlightAmbiguousUnicode" name="success" :size="15" />
        </span>
        <span class="editor-menu-label">{{ t('app.fileEditor.highlightAmbiguousUnicode') }}</span>
      </button>
      <div class="editor-menu-separator" role="separator"></div>
      <button
        ref="whitespaceMenuTriggerRef"
        type="button"
        class="editor-menu-item"
        data-ui="editor-whitespace-menu-trigger"
        role="menuitem"
        aria-haspopup="menu"
        :aria-controls="whitespaceMenuId"
        :aria-expanded="openSubmenu === 'whitespace'"
        @click.stop="openNestedMenu('whitespace', true)"
        @mouseenter="openNestedMenu('whitespace')"
      >
        <VustIcon name="code" :size="15" />
        <span class="editor-menu-label">{{ t('app.fileEditor.renderWhitespace') }}</span>
        <span class="editor-menu-value">{{ whitespaceValueLabel }}</span>
        <VustIcon name="chevron-right" :size="14" />
      </button>
      <button
        ref="fontMenuTriggerRef"
        type="button"
        class="editor-menu-item"
        data-ui="editor-font-size-menu-trigger"
        role="menuitem"
        aria-haspopup="menu"
        :aria-controls="fontMenuId"
        :aria-expanded="openSubmenu === 'font'"
        @click.stop="openNestedMenu('font', true)"
        @mouseenter="openNestedMenu('font')"
      >
        <VustIcon name="code" :size="15" />
        <span class="editor-menu-label">{{ t('app.fileEditor.fontSize') }}</span>
        <span class="editor-menu-value">{{
          t('app.fileEditor.fontSizeValue', { size: fontSize })
        }}</span>
        <VustIcon name="chevron-right" :size="14" />
      </button>
      <div class="editor-menu-separator" role="separator"></div>
      <button
        type="button"
        class="editor-menu-item"
        data-ui="editor-reset-view-preferences"
        role="menuitem"
        @mouseenter="openSubmenu = null"
        @click="runAction(() => emit('resetPreferences'))"
      >
        <VustIcon name="restart" :size="15" />
        <span class="editor-menu-label">{{ t('app.fileEditor.resetViewPreferences') }}</span>
      </button>
    </div>

    <div
      v-if="openMenu === 'view' && openSubmenu === 'whitespace'"
      :id="whitespaceMenuId"
      ref="whitespaceMenuRef"
      class="editor-dropdown-menu editor-submenu"
      data-ui="editor-whitespace-menu"
      role="menu"
      :aria-label="t('app.fileEditor.renderWhitespace')"
      :style="submenuStyle"
      @keydown="handleSubmenuKeydown"
    >
      <button
        v-for="option in whitespaceOptions"
        :key="option"
        type="button"
        class="editor-menu-item"
        :data-ui="`editor-whitespace-${option}`"
        role="menuitemradio"
        :aria-checked="renderWhitespace === option"
        @click="runAction(() => emit('update:renderWhitespace', option))"
      >
        <span class="editor-menu-icon-slot">
          <VustIcon v-if="renderWhitespace === option" name="success" :size="15" />
        </span>
        <span class="editor-menu-label">{{ whitespaceLabel(option) }}</span>
      </button>
    </div>

    <div
      v-if="openMenu === 'view' && openSubmenu === 'font'"
      :id="fontMenuId"
      ref="fontMenuRef"
      class="editor-dropdown-menu editor-submenu"
      data-ui="editor-font-size-menu"
      role="menu"
      :aria-label="t('app.fileEditor.fontSize')"
      :style="submenuStyle"
      @keydown="handleSubmenuKeydown"
    >
      <button
        type="button"
        class="editor-menu-item"
        data-ui="editor-font-increase"
        role="menuitem"
        :disabled="fontSize >= 24"
        @click="runAction(() => emit('adjustFont', 1))"
      >
        <VustIcon name="plus" :size="15" />
        <span class="editor-menu-label">{{ t('app.fileEditor.increaseFont') }}</span>
      </button>
      <button
        type="button"
        class="editor-menu-item"
        data-ui="editor-font-decrease"
        role="menuitem"
        :disabled="fontSize <= 10"
        @click="runAction(() => emit('adjustFont', -1))"
      >
        <VustIcon name="minus" :size="15" />
        <span class="editor-menu-label">{{ t('app.fileEditor.decreaseFont') }}</span>
      </button>
      <div class="editor-menu-separator" role="separator"></div>
      <button
        type="button"
        class="editor-menu-item"
        data-ui="editor-font-reset"
        role="menuitem"
        :disabled="fontSize === 14"
        @click="runAction(() => emit('resetFont'))"
      >
        <VustIcon name="restart" :size="15" />
        <span class="editor-menu-label">{{ t('app.fileEditor.resetFont') }}</span>
        <span class="editor-menu-value">{{ t('app.fileEditor.fontSizeValue', { size: 14 }) }}</span>
      </button>
    </div>
  </Teleport>
</template>

<style scoped>
.editor-menu-bar {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-1);
  min-height: 30px;
  padding: 0 var(--vdl-space-2);
  background: var(--vdl-bg-panel);
  border-bottom: 1px solid var(--vdl-border-subtle);
  flex-shrink: 0;
}

.editor-menu-trigger {
  height: 24px;
  padding: 0 var(--vdl-space-2);
  border: 0;
  border-radius: var(--vdl-radius-sm);
  background: transparent;
  color: var(--vdl-text-secondary);
  font: inherit;
  font-size: var(--vdl-font-body-sm);
  cursor: pointer;
}

.editor-menu-trigger:hover,
.editor-menu-trigger[aria-expanded='true'] {
  background: var(--vdl-bg-hover);
  color: var(--vdl-text-primary);
}

.editor-menu-trigger:focus-visible,
.editor-menu-item:focus-visible {
  outline: none;
  box-shadow: var(--vdl-focus-ring);
}

.editor-dropdown-menu {
  position: fixed;
  z-index: calc(var(--vdl-z-index-modal) + 2);
  min-width: 220px;
  padding: var(--vdl-space-1);
  border: 1px solid var(--vdl-border-strong);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-panel);
  box-shadow: var(--vdl-shadow-panel);
}

.editor-submenu {
  min-width: 210px;
}

.editor-menu-item {
  display: grid;
  grid-template-columns: 18px minmax(96px, 1fr) auto auto;
  align-items: center;
  gap: var(--vdl-space-2);
  width: 100%;
  min-height: 30px;
  padding: var(--vdl-space-1) var(--vdl-space-2);
  border: 0;
  border-radius: var(--vdl-radius-sm);
  background: transparent;
  color: var(--vdl-text-secondary);
  font: inherit;
  font-size: var(--vdl-font-body-sm);
  text-align: left;
  cursor: pointer;
}

.editor-menu-item:hover:not(:disabled),
.editor-menu-item:focus-visible:not(:disabled) {
  background: var(--vdl-bg-hover);
  color: var(--vdl-text-primary);
}

.editor-menu-item:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.editor-menu-icon-slot {
  display: inline-flex;
  width: 15px;
  height: 15px;
  align-items: center;
  justify-content: center;
}

.editor-menu-label {
  white-space: nowrap;
}

.editor-menu-hint,
.editor-menu-value {
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-caption);
  white-space: nowrap;
}

.editor-menu-separator {
  height: 1px;
  margin: var(--vdl-space-1) var(--vdl-space-2);
  background: var(--vdl-border-subtle);
}
</style>
