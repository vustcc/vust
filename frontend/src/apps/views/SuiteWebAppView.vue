<script setup lang="ts">
/**
 * @file SuiteWebAppView.vue
 * @description Compose 套件 Web 入口承载组件，负责以应用窗口形态加载代理 iframe。
 */

import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  createSuiteHostBridge,
  SUITE_MESSAGE_TYPES,
  type SuiteHostBridge,
  type SuiteNotificationPayload,
} from '@vustcc/suite-sdk'
import { VustButton } from '@/components/ui'
import VustIcon from '@/components/icons/VustIcon.vue'
import { useToastStore, type ToastType } from '@/stores/toast'
import { useThemeStore } from '@/stores/theme'
import { useWindowManagerStore } from '@/stores/window-manager'

const props = defineProps<{
  isMaximized?: boolean
  windowId?: string
  payload?: Record<string, unknown>
}>()

const { t, locale } = useI18n()
const themeStore = useThemeStore()
const windowManagerStore = useWindowManagerStore()
const toastStore = useToastStore()

const iframeKey = ref(0)
const iframeRef = ref<HTMLIFrameElement | null>(null)
const failed = ref(false)
// 套件声明 window capability 后，主控优先依赖 SDK 聚焦消息，避免透明层吃掉第一次点击。
const supportsWindowFocus = ref(false)
// 套件声明 notification capability 后，主控才承载该套件的统一弹窗通知。
const supportsNotification = ref(false)
// 套件声明 navigation capability 后，主控才执行套件发起的导航请求。
const supportsNavigation = ref(false)
let suiteBridge: SuiteHostBridge | null = null

const suiteUrl = computed(() => {
  const value = props.payload?.url
  return typeof value === 'string' ? value.trim() : ''
})

const isWindowFocused = computed(() => {
  if (!props.windowId) return false
  const windowInstance = windowManagerStore.openWindows.find((item) => item.id === props.windowId)
  return !!windowInstance && windowInstance.zIndex === windowManagerStore.maxZIndex
})

const shouldShowFocusCatcher = computed(() => {
  // 未接入 SDK 聚焦消息时，未聚焦窗口用透明层接住第一次点击并置顶。
  return (
    !!props.windowId &&
    !!suiteUrl.value &&
    !failed.value &&
    !isWindowFocused.value &&
    !supportsWindowFocus.value
  )
})

function focusSuiteWindow() {
  if (!props.windowId || isWindowFocused.value) return
  windowManagerStore.focusWindow(props.windowId)
}

function reloadSuite() {
  if (!suiteUrl.value) {
    failed.value = true
    return
  }
  failed.value = false
  iframeKey.value += 1
}

function handleLoaded() {
  failed.value = false
  suiteBridge?.sendTheme()
  suiteBridge?.sendLocale()
}

function handleLoadFailed() {
  failed.value = true
}

function resolveSuiteLocale() {
  return locale.value === 'en' ? 'en-US' : 'zh-CN'
}

function normalizeToastType(type: unknown): ToastType {
  return type === 'success' || type === 'error' || type === 'warning' || type === 'info'
    ? type
    : 'info'
}

function handleSuiteNotification(payload: unknown) {
  if (!supportsNotification.value || !payload || typeof payload !== 'object') return

  const notification = payload as Partial<SuiteNotificationPayload>
  const title = typeof notification.title === 'string' ? notification.title.trim() : ''
  const message = typeof notification.message === 'string' ? notification.message.trim() : ''
  if (!title && !message) return

  const duration =
    typeof notification.duration === 'number' &&
    Number.isFinite(notification.duration) &&
    notification.duration > 0
      ? notification.duration
      : undefined

  toastStore.showToast(
    normalizeToastType(notification.type),
    message || title,
    duration,
    title || undefined,
  )
}

function handleSuiteNavigation(payload: unknown) {
  if (!supportsNavigation.value || !payload || typeof payload !== 'object') return

  const navigation = payload as {
    target?: unknown
    value?: unknown
    payload?: unknown
  }
  if (navigation.target !== 'app' || typeof navigation.value !== 'string') return

  const appId = navigation.value.trim()
  if (!appId) return
  const appPayload =
    navigation.payload && typeof navigation.payload === 'object'
      ? (navigation.payload as Record<string, unknown>)
      : {}
  windowManagerStore.openWindowWithPayload(appId, appPayload)
}

watch(
  suiteUrl,
  () => {
    reloadSuite()
  },
  { immediate: true },
)

watch(
  iframeKey,
  async () => {
    // iframe 重建后需要重新绑定主控桥接，确保主题和语言状态能同步到套件。
    suiteBridge?.destroy()
    suiteBridge = null
    supportsWindowFocus.value = false
    supportsNotification.value = false
    supportsNavigation.value = false
    await nextTick()
    suiteBridge = createSuiteHostBridge({
      iframe: () => iframeRef.value,
      theme: () => themeStore.currentTheme,
      locale: resolveSuiteLocale,
      onReady: (payload) => {
        supportsWindowFocus.value = payload.capabilities.includes('window')
        supportsNotification.value = payload.capabilities.includes('notification')
        supportsNavigation.value = payload.capabilities.includes('navigation')
      },
      onMessage: (message) => {
        // 套件 iframe 内部交互通过 SDK 消息回传，主控据此把窗口置顶。
        if (message.type === SUITE_MESSAGE_TYPES.suiteWindowFocus) {
          focusSuiteWindow()
        }
        if (message.type === SUITE_MESSAGE_TYPES.suiteNotificationShow) {
          handleSuiteNotification(message.payload)
        }
        if (message.type === SUITE_MESSAGE_TYPES.suiteNavigationOpen) {
          handleSuiteNavigation(message.payload)
        }
      },
    })
    suiteBridge.sendTheme()
    suiteBridge.sendLocale()
  },
  { immediate: true },
)

watch(
  () => themeStore.currentTheme,
  () => {
    suiteBridge?.sendTheme()
  },
)

watch(
  () => locale.value,
  () => {
    suiteBridge?.sendLocale()
  },
)

onBeforeUnmount(() => {
  suiteBridge?.destroy()
  suiteBridge = null
})
</script>

<template>
  <div class="suite-web-app" data-page="suite-web-app">
    <!-- 套件自身负责页面工具区；主控只保留承载和异常重试能力。 -->
    <div class="suite-web-content" data-slot="content">
      <iframe
        v-if="suiteUrl"
        ref="iframeRef"
        :key="iframeKey"
        :src="suiteUrl"
        class="suite-web-frame"
        referrerpolicy="same-origin"
        data-native-context-menu
        @load="handleLoaded"
        @error="handleLoadFailed"
      ></iframe>

      <!-- 未接入 SDK window capability 的套件无法上报 iframe 内点击，透明层用于第一次点击聚焦。 -->
      <div
        v-if="shouldShowFocusCatcher"
        class="suite-focus-catcher"
        data-slot="focus-catcher"
        @pointerdown.prevent.stop="focusSuiteWindow"
      ></div>

      <div v-if="failed || !suiteUrl" class="suite-web-overlay" data-slot="empty">
        <VustIcon name="warning" :size="32" />
        <p>{{ suiteUrl ? t('app.suiteWebApp.loadFailed') : t('app.suiteWebApp.missingUrl') }}</p>
        <VustButton v-if="suiteUrl" size="small" @click="reloadSuite">
          <VustIcon name="refresh" :size="14" />
          {{ t('app.suiteWebApp.refresh') }}
        </VustButton>
      </div>
    </div>
  </div>
</template>

<style scoped>
.suite-web-app {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  background: var(--vdl-bg-panel);
}

.suite-web-content {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: hidden;
  background: var(--vdl-bg-base);
}

.suite-web-frame {
  width: 100%;
  height: 100%;
  display: block;
  border: 0;
  background: var(--vdl-bg-panel);
}

.suite-focus-catcher {
  position: absolute;
  inset: 0;
  z-index: 1;
  background: transparent;
}

.suite-web-overlay {
  position: absolute;
  inset: 0;
  z-index: 2;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-5);
  color: var(--vdl-text-muted);
  text-align: center;
  background: var(--vdl-bg-panel);
}

.suite-web-overlay p {
  max-width: 360px;
  margin: 0;
  color: var(--vdl-text-secondary);
  font-size: 13px;
  line-height: 1.6;
}
</style>
