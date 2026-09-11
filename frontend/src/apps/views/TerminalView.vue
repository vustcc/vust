<script setup lang="ts">
/**
 * @file TerminalView.vue
 * @description 固定节点宿主机 PTY 会话界面，展示服务端确认的访问与会话状态。
 */

import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import type { TerminalServerMessage } from '@/api/generated'
import { useTerminalSession, type TerminalSessionState } from '@/composables/useTerminalSession'
import { useThemeStore, buildTerminalTheme } from '@/stores/theme'
import { useWindowManagerStore } from '@/stores/window-manager'
import { VustAlert, VustButton, VustLoading, VustTag } from '@/components/ui'
import '@xterm/xterm/css/xterm.css'

const props = defineProps<{
  windowId?: string
  payload?: Record<string, unknown>
}>()

const { t } = useI18n()
const themeStore = useThemeStore()
const windowStore = useWindowManagerStore()
const terminalHost = ref<HTMLDivElement | null>(null)
const hasTranscript = ref(false)

const targetNodeId =
  typeof props.payload?.nodeId === 'string' && props.payload.nodeId ? props.payload.nodeId : 'local'
const targetNodeName =
  typeof props.payload?.nodeName === 'string' && props.payload.nodeName
    ? props.payload.nodeName
    : targetNodeId

let xterm: Terminal | null = null
let fitAddon: FitAddon | null = null
let resizeObserver: ResizeObserver | null = null
let resizeFrame: number | null = null

/** 控制事件只更新页面状态，不写入终端转录。 */
const handleControl = (message: TerminalServerMessage) => {
  if (message.kind === 'started') {
    xterm?.focus()
  }
}

const terminalSession = useTerminalSession(targetNodeId, {
  onOutput(bytes) {
    hasTranscript.value = true
    xterm?.write(bytes)
  },
  onControl: handleControl,
})

const {
  access,
  accessState,
  sessionState,
  sessionError,
  exitResult,
  idleWarningSeconds,
  canConnect,
  checkAccess,
  connect,
  writeInput,
  resize,
  disconnect,
  cancel,
} = terminalSession

const activeStates: TerminalSessionState[] = [
  'connecting',
  'starting',
  'active',
  'idleWarning',
  'closing',
]
const hasActiveSession = computed(() => activeStates.includes(sessionState.value))
const isConnecting = computed(() => ['connecting', 'starting'].includes(sessionState.value))

const visibleStatus = computed(() => {
  if (accessState.value === 'checking') return 'checking'
  if (accessState.value === 'failed') return 'accessFailed'
  if (accessState.value === 'unavailable') return 'unavailable'
  return sessionState.value
})

const statusLabel = computed(() => t(`app.terminal.status.${visibleStatus.value}`))
const statusType = computed<'default' | 'info' | 'success' | 'warning' | 'danger'>(() => {
  if (['active'].includes(visibleStatus.value)) return 'success'
  if (['connecting', 'starting', 'checking', 'closing'].includes(visibleStatus.value)) return 'info'
  if (['idleWarning', 'unavailable'].includes(visibleStatus.value)) return 'warning'
  if (['failed', 'accessFailed'].includes(visibleStatus.value)) return 'danger'
  return 'default'
})

const sessionErrorText = computed(() =>
  sessionError.value ? t(`app.terminal.errors.${sessionError.value.code}`) : '',
)
const sessionResultText = computed(() => {
  if (!exitResult.value) return ''
  return exitResult.value.exitCode === null
    ? t('app.terminal.sessionEnded')
    : t('app.terminal.sessionEndedWithCode', { code: exitResult.value.exitCode })
})
const inactiveStatusText = computed(
  () => sessionErrorText.value || sessionResultText.value || statusLabel.value,
)
const terminalBackground = computed(
  () => buildTerminalTheme(themeStore.currentTheme)?.background || 'var(--vdl-bg-canvas)',
)
const unavailableText = computed(() => {
  const code = access.value?.unavailableReason
  return code ? t(`app.terminal.errors.${code}`) : t('app.terminal.accessUnavailable')
})

/** 解析 VDL 等宽字体 Token 后交给 Canvas 渲染器。 */
const resolveMonospaceFont = () => {
  const token = getComputedStyle(document.documentElement)
    .getPropertyValue('--vdl-font-mono')
    .trim()
  return token || 'monospace'
}

/** 合并窗口尺寸变化，并忽略隐藏或零尺寸状态。 */
const scheduleFit = () => {
  if (resizeFrame !== null) cancelAnimationFrame(resizeFrame)
  resizeFrame = requestAnimationFrame(() => {
    resizeFrame = null
    const host = terminalHost.value
    if (!host || host.clientWidth === 0 || host.clientHeight === 0 || !fitAddon) return
    try {
      fitAddon.fit()
    } catch {
      // 窗口布局切换期间由下一次 ResizeObserver 回调重新计算。
    }
  })
}

/** 清空旧会话并以当前可见网格创建全新 PTY。 */
const startConnection = async () => {
  if (!canConnect.value) return
  xterm?.reset()
  xterm?.clear()
  hasTranscript.value = false
  await nextTick()
  scheduleFit()
  connect(xterm?.cols || 80, xterm?.rows || 24)
}

/** 清理当前转录，但不改变会话生命周期。 */
const clearTranscript = () => {
  xterm?.clear()
  hasTranscript.value = false
}

watch(
  () => themeStore.currentTheme,
  (theme) => {
    if (xterm) xterm.options.theme = buildTerminalTheme(theme)
  },
)

watch(
  hasActiveSession,
  (active) => {
    if (!props.windowId) return
    windowStore.updateWindowRuntimeState(props.windowId, {
      activeSession: active,
      allowsNodeSwitch: false,
      blockLevel: active ? 'active' : 'open',
      blockReason: active ? t('app.terminal.guardActive') : t('app.terminal.guardOpen'),
      blockReasonKey: active ? 'app.terminal.guardActive' : 'app.terminal.guardOpen',
    })
  },
  { immediate: true },
)

onMounted(() => {
  if (!terminalHost.value) return
  xterm = new Terminal({
    cursorBlink: true,
    disableStdin: true,
    fontFamily: resolveMonospaceFont(),
    fontSize: 13,
    scrollback: 5_000,
    theme: buildTerminalTheme(themeStore.currentTheme),
  })
  fitAddon = new FitAddon()
  xterm.loadAddon(fitAddon)
  xterm.open(terminalHost.value)
  xterm.textarea?.setAttribute(
    'aria-label',
    t('app.terminal.terminalLabel', { node: targetNodeName }),
  )
  xterm.onData(writeInput)
  xterm.onResize(({ cols, rows }) => resize(cols, rows))
  resizeObserver = new ResizeObserver(scheduleFit)
  resizeObserver.observe(terminalHost.value)
  scheduleFit()
  void checkAccess()
})

watch(sessionState, (state) => {
  if (xterm) xterm.options.disableStdin = !['active', 'idleWarning'].includes(state)
})

onBeforeUnmount(() => {
  resizeObserver?.disconnect()
  if (resizeFrame !== null) cancelAnimationFrame(resizeFrame)
  xterm?.dispose()
  xterm = null
  fitAddon = null
})
</script>

<template>
  <div
    class="terminal-app"
    data-page="terminal"
    data-ui="terminal-workspace"
    :style="{ '--terminal-background': terminalBackground }"
  >
    <div class="terminal-toolbar" data-ui="terminal-toolbar" data-slot="toolbar">
      <div class="terminal-context" data-slot="target">
        <span class="context-label">{{ t('app.terminal.targetNode') }}</span>
        <strong class="context-value">{{ targetNodeName }}</strong>
        <VustTag :type="statusType" data-ui="terminal-status">{{ statusLabel }}</VustTag>
        <span
          v-if="idleWarningSeconds !== null"
          class="context-notice context-notice-warning"
          data-ui="terminal-idle-warning"
        >
          {{ t('app.terminal.idleWarning', { seconds: idleWarningSeconds }) }}
        </span>
        <span
          v-else-if="sessionError?.recoverable && sessionErrorText"
          class="context-notice context-notice-error"
          data-ui="terminal-session-error"
        >
          {{ sessionErrorText }}
        </span>
        <span v-if="sessionResultText" class="session-result" data-slot="session-result">
          {{ sessionResultText }}
        </span>
      </div>
      <div class="terminal-actions" data-slot="actions">
        <VustButton size="small" type="secondary" data-ui="terminal-clear" @click="clearTranscript">
          {{ t('app.terminal.clear') }}
        </VustButton>
        <VustButton
          v-if="isConnecting"
          size="small"
          type="secondary"
          data-ui="terminal-cancel"
          @click="cancel"
        >
          {{ t('common.cancel') }}
        </VustButton>
        <VustButton
          v-else-if="sessionState === 'active' || sessionState === 'idleWarning'"
          size="small"
          type="danger"
          data-ui="terminal-disconnect"
          @click="disconnect"
        >
          {{ t('app.terminal.disconnect') }}
        </VustButton>
        <VustButton
          v-else
          size="small"
          type="primary"
          :disabled="!canConnect"
          data-ui="terminal-connect"
          @click="startConnection"
        >
          {{ hasTranscript ? t('app.terminal.reconnect') : t('app.terminal.connect') }}
        </VustButton>
      </div>
    </div>

    <div class="sr-only" role="status" aria-live="polite" data-slot="announcements">
      <span>{{ statusLabel }}</span>
      <span v-if="idleWarningSeconds !== null">
        {{ t('app.terminal.idleWarning', { seconds: idleWarningSeconds }) }}
      </span>
      <span v-if="sessionErrorText">{{ sessionErrorText }}</span>
      <span v-if="exitResult">
        {{
          exitResult.exitCode === null
            ? t('app.terminal.sessionEnded')
            : t('app.terminal.sessionEndedWithCode', { code: exitResult.exitCode })
        }}
      </span>
    </div>

    <div class="terminal-stage" data-slot="canvas">
      <div
        ref="terminalHost"
        class="terminal-canvas"
        role="application"
        :aria-label="t('app.terminal.terminalLabel', { node: targetNodeName })"
        data-ui="terminal-canvas"
        data-native-context-menu
      ></div>

      <div
        v-if="accessState === 'checking'"
        class="terminal-overlay"
        aria-busy="true"
        data-ui="terminal-checking"
      >
        <VustLoading
          class="terminal-checking-loading"
          :loading="true"
          :text="t('app.terminal.checkingAccess')"
        />
      </div>
      <div
        v-else-if="accessState === 'failed'"
        class="terminal-overlay"
        data-ui="terminal-access-failed"
      >
        <VustAlert type="error" :title="t('app.terminal.accessFailed')" show-icon />
        <VustButton type="secondary" @click="checkAccess">{{ t('common.retry') }}</VustButton>
      </div>
      <div
        v-else-if="accessState === 'unavailable'"
        class="terminal-overlay"
        data-ui="terminal-unavailable"
      >
        <VustAlert type="warning" :title="unavailableText" show-icon />
      </div>
      <div
        v-else-if="sessionState === 'ended' || sessionState === 'failed'"
        class="terminal-overlay terminal-inactive-overlay"
        data-ui="terminal-inactive"
      >
        <VustTag :type="statusType">{{ statusLabel }}</VustTag>
        <p>{{ inactiveStatusText }}</p>
        <VustButton
          type="primary"
          :disabled="!canConnect"
          data-ui="terminal-reconnect"
          @click="startConnection"
        >
          {{ t('app.terminal.reconnect') }}
        </VustButton>
      </div>
      <div
        v-else-if="!hasTranscript && !hasActiveSession"
        class="terminal-overlay"
        data-ui="terminal-ready"
      >
        <p>{{ t('app.terminal.readyHint', { node: targetNodeName }) }}</p>
        <VustButton type="primary" :disabled="!canConnect" @click="startConnection">
          {{ t('app.terminal.connect') }}
        </VustButton>
      </div>
    </div>
  </div>
</template>

<style scoped>
.terminal-app {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  background: var(--vdl-bg-canvas);
  color: var(--vdl-text-primary);
}

.terminal-toolbar {
  min-height: 44px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-2) var(--vdl-space-3);
  border-bottom: 1px solid var(--vdl-border-subtle);
  background: var(--vdl-bg-surface);
}

.terminal-context,
.terminal-actions {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
}

.terminal-context {
  min-width: 0;
}

.context-label,
.session-result {
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-body-sm);
}

.context-value {
  max-width: 240px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-family: var(--vdl-font-mono);
  font-size: var(--vdl-font-body-sm);
}

.context-notice {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--vdl-font-body-sm);
}

.context-notice-warning {
  color: var(--vdl-warning);
}

.context-notice-error {
  color: var(--vdl-danger);
}

.terminal-stage {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: hidden;
  padding: var(--vdl-space-3);
  background: var(--terminal-background);
}

.terminal-canvas {
  width: 100%;
  height: 100%;
  min-height: 0;
  overflow: hidden;
  background: var(--terminal-background);
}

.terminal-overlay {
  position: absolute;
  inset: var(--vdl-space-3);
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--vdl-space-4);
  padding: var(--vdl-space-5);
  text-align: center;
  background: var(--vdl-bg-canvas);
}

.terminal-overlay > :deep(*) {
  max-width: 520px;
}

.terminal-overlay p {
  margin: 0;
  color: var(--vdl-text-secondary);
  line-height: 1.6;
}

.terminal-checking-loading {
  width: 240px;
  max-width: 100%;
}

.terminal-checking-loading :deep(.vl-loading-text) {
  white-space: nowrap;
}

.sr-only {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}

:deep(.xterm) {
  height: 100%;
  background: var(--terminal-background);
}

:deep(.xterm-viewport) {
  overflow-y: auto;
  background: var(--terminal-background) !important;
}

:deep(.xterm-screen) {
  background: var(--terminal-background);
}

@media (max-width: 700px) {
  .terminal-toolbar {
    align-items: flex-start;
    flex-wrap: wrap;
  }

  .terminal-context {
    width: 100%;
  }

  .terminal-actions {
    width: 100%;
    justify-content: flex-end;
  }

  .context-value {
    max-width: 160px;
  }
}
</style>
