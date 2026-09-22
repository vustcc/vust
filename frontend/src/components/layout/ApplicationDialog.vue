<script lang="ts">
interface HostLockState {
  count: number
  overflow: string
}

const hostLocks = new WeakMap<HTMLElement, HostLockState>()
const activeDialogs: symbol[] = []

function lockHost(host: HTMLElement) {
  const current = hostLocks.get(host)
  if (current) {
    current.count += 1
    return
  }

  hostLocks.set(host, {
    count: 1,
    overflow: host.style.overflow,
  })
  host.style.overflow = 'hidden'
}

function unlockHost(host: HTMLElement) {
  const current = hostLocks.get(host)
  if (!current) return

  current.count -= 1
  if (current.count > 0) return

  host.style.overflow = current.overflow
  hostLocks.delete(host)
}
</script>

<script setup lang="ts">
import {
  computed,
  nextTick,
  onBeforeUnmount,
  onMounted,
  ref,
  useAttrs,
  watch,
  type ComponentPublicInstance,
} from 'vue'
import { useI18n } from 'vue-i18n'
import VustIcon from '@/components/icons/VustIcon.vue'
import { VustGlassSurface } from '@/components/ui'

/**
 * 应用窗口级对话框。
 *
 * 组件会从自身锚点查找最近的 `.window-content`，并将遮罩 Teleport 到该内容区，
 * 因而不会覆盖可拖动的 `.window-header`，也不会影响其它应用窗口。找不到窗口内容区时，
 * 对话框保留在当前位置渲染，不会回退到 `body`。
 *
 * @example
 * ```vue
 * <ApplicationDialog
 *   :visible="formVisible"
 *   title="新建任务"
 *   initial-focus-selector="#task-name"
 *   return-focus-selector="[data-ui='create-task']"
 *   @close="formVisible = false"
 * >
 *   <VustInput id="task-name" />
 *   <template #footer>...</template>
 * </ApplicationDialog>
 * ```
 */
defineOptions({ name: 'ApplicationDialog', inheritAttrs: false })

interface Props {
  /** 是否显示对话框。 */
  visible: boolean
  /** 对话框标题，同时用于无障碍标题关联。 */
  title: string
  /** 对话框宽度，默认 `500px`，最大不会超过宿主内容区。 */
  width?: string
  /** 点击遮罩是否请求关闭，默认启用。 */
  closeOnClickOverlay?: boolean
  /** 是否禁用关闭按钮、Escape 和遮罩关闭，适用于提交或上传期间。 */
  closeDisabled?: boolean
  /** 打开后需要聚焦的对话框内部元素选择器。 */
  initialFocusSelector?: string
  /** 关闭后用于恢复焦点的宿主内容区元素选择器。 */
  returnFocusSelector?: string
}

const props = withDefaults(defineProps<Props>(), {
  width: '500px',
  closeOnClickOverlay: true,
  closeDisabled: false,
  initialFocusSelector: '',
  returnFocusSelector: '',
})

const emit = defineEmits<{
  /** 用户通过关闭按钮、Escape 或遮罩请求关闭。 */
  close: []
}>()

const { t } = useI18n()
const attrs = useAttrs()
const anchorElement = ref<HTMLElement | null>(null)
const dialogElement = ref<HTMLElement | ComponentPublicInstance | null>(null)
const hostElement = ref<HTMLElement | null>(null)
const titleId = `application-dialog-title-${Math.random().toString(36).slice(2)}`
const dialogId = Symbol('application-dialog')
const teleportTarget = computed(() => hostElement.value)

let mounted = false
let active = false
let previousFocus: HTMLElement | null = null

const focusableSelector = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',')

/** 获取玻璃表面的实际对话框元素。 */
function getDialogElement() {
  const current = dialogElement.value
  if (current instanceof HTMLElement) return current
  return current?.$el instanceof HTMLElement ? current.$el : null
}

/** 获取当前对话框内可参与键盘循环的元素。 */
function getFocusableElements() {
  const dialog = getDialogElement()
  if (!dialog) return []
  return Array.from(dialog.querySelectorAll<HTMLElement>(focusableSelector)).filter(
    (element) => !element.hidden && element.getAttribute('aria-hidden') !== 'true',
  )
}

/** 关闭对话框；禁用关闭时忽略所有常规关闭入口。 */
function requestClose() {
  if (!props.closeDisabled) emit('close')
}

/** 处理遮罩点击，避免正文点击误触关闭。 */
function handleOverlayClick() {
  if (props.closeOnClickOverlay) requestClose()
}

/** 将 Tab 键限制在当前窗口级对话框内。 */
function handleKeydown(event: KeyboardEvent) {
  if (!props.visible || activeDialogs.at(-1) !== dialogId) return

  if (event.key === 'Escape') {
    event.preventDefault()
    requestClose()
    return
  }

  if (event.key !== 'Tab') return

  const focusable = getFocusableElements()
  if (focusable.length === 0) {
    event.preventDefault()
    getDialogElement()?.focus()
    return
  }

  const first = focusable[0]
  const last = focusable[focusable.length - 1]
  const current = document.activeElement

  if (event.shiftKey && (current === first || !getDialogElement()?.contains(current))) {
    event.preventDefault()
    last.focus()
  } else if (!event.shiftKey && current === last) {
    event.preventDefault()
    first.focus()
  }
}

/** 激活焦点管理和宿主内容区滚动锁。 */
async function activateDialog() {
  if (active) return
  active = true
  activeDialogs.push(dialogId)
  previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null

  if (hostElement.value) lockHost(hostElement.value)
  document.addEventListener('keydown', handleKeydown)

  await nextTick()
  const dialog = getDialogElement()
  const requestedFocus = props.initialFocusSelector
    ? dialog?.querySelector<HTMLElement>(props.initialFocusSelector)
    : null
  requestedFocus?.focus()
  if (requestedFocus) return

  const firstFocusable = getFocusableElements()[0]
  ;(firstFocusable ?? dialog)?.focus()
}

/** 释放宿主锁并把焦点还给触发入口。 */
function deactivateDialog(restoreFocus = true) {
  if (!active) return
  active = false
  const stackIndex = activeDialogs.lastIndexOf(dialogId)
  if (stackIndex >= 0) activeDialogs.splice(stackIndex, 1)
  document.removeEventListener('keydown', handleKeydown)
  if (hostElement.value) unlockHost(hostElement.value)

  if (restoreFocus) {
    const selectorTarget = props.returnFocusSelector
      ? hostElement.value?.querySelector<HTMLElement>(props.returnFocusSelector)
      : null
    const focusTarget = selectorTarget ?? (previousFocus?.isConnected ? previousFocus : null)
    focusTarget?.focus()
  }
  previousFocus = null
}

watch(
  () => props.visible,
  (visible) => {
    if (!mounted) return
    if (visible) void activateDialog()
    else deactivateDialog()
  },
)

onMounted(() => {
  hostElement.value = anchorElement.value?.closest<HTMLElement>('.window-content') ?? null
  mounted = true
  if (props.visible) void activateDialog()
})

onBeforeUnmount(() => {
  deactivateDialog(false)
})
</script>

<template>
  <span ref="anchorElement" class="application-dialog-anchor" data-ui="application-dialog-anchor" />
  <Teleport :to="teleportTarget" :disabled="!hostElement">
    <Transition name="application-dialog-fade">
      <div
        v-if="visible"
        class="application-dialog-overlay"
        data-ui="application-dialog-overlay"
        data-slot="overlay"
        @click.self="handleOverlayClick"
      >
        <VustGlassSurface
          ref="dialogElement"
          v-bind="attrs"
          class="application-dialog"
          :style="{ width }"
          role="dialog"
          aria-modal="true"
          :aria-labelledby="titleId"
          tabindex="-1"
        >
          <header class="application-dialog__header" data-slot="header">
            <h2 :id="titleId" class="application-dialog__title">{{ title }}</h2>
            <button
              class="application-dialog__close"
              type="button"
              :aria-label="t('common.close')"
              :disabled="closeDisabled"
              @click="requestClose"
            >
              <VustIcon name="x" :size="16" />
            </button>
          </header>

          <div class="application-dialog__body" data-slot="body">
            <slot />
          </div>

          <footer v-if="$slots.footer" class="application-dialog__footer" data-slot="footer">
            <slot name="footer" />
          </footer>
        </VustGlassSurface>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.application-dialog-anchor {
  display: none;
}

.application-dialog-overlay {
  position: absolute;
  inset: 0;
  z-index: var(--vdl-z-index-modal);
  display: flex;
  align-items: center;
  justify-content: center;
  box-sizing: border-box;
  padding: var(--vdl-space-4);
  background: var(--vdl-bg-backdrop);
}

.application-dialog {
  display: flex;
  flex-direction: column;
  max-width: 100%;
  max-height: 100%;
  overflow: hidden;
  border: 1px solid var(--vdl-border-subtle);
  border-radius: var(--vdl-radius-lg);
  box-shadow: var(--vdl-shadow-window);
}

.application-dialog__header {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-4) var(--vdl-space-5);
  border-bottom: 1px solid var(--vdl-border-subtle);
}

.application-dialog__title {
  margin: 0;
  color: var(--vdl-text-primary);
  font-size: var(--vdl-font-title);
  font-weight: 600;
}

.application-dialog__close {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  padding: 0;
  color: var(--vdl-text-secondary);
  cursor: pointer;
  background: transparent;
  border: 0;
  border-radius: var(--vdl-radius-md);
}

.application-dialog__close:hover:not(:disabled) {
  color: var(--vdl-text-primary);
  background: var(--vdl-bg-hover);
}

.application-dialog__close:focus-visible {
  outline: none;
  box-shadow: var(--vdl-focus-ring);
}

.application-dialog__close:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}

.application-dialog__body {
  flex: 1 1 auto;
  min-height: 0;
  padding: var(--vdl-space-5);
  overflow: auto;
}

.application-dialog__footer {
  display: flex;
  flex: 0 0 auto;
  justify-content: flex-end;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-4) var(--vdl-space-5);
  border-top: 1px solid var(--vdl-border-subtle);
}

.application-dialog-fade-enter-active,
.application-dialog-fade-leave-active {
  transition: opacity 160ms ease;
}

.application-dialog-fade-enter-from,
.application-dialog-fade-leave-to {
  opacity: 0;
}
</style>
