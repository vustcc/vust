<script setup lang="ts">
import { VustButton, VustSelect } from '@/components/ui'
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useToastStore } from '@/stores/toast'
import { buildTerminalTheme, useThemeStore } from '@/stores/theme'
import { useDockerTerminalWs } from './useDockerTerminalWs'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { useI18n } from 'vue-i18n'
import '@xterm/xterm/css/xterm.css'

type ShellType = 'bash' | 'sh'

const props = defineProps<{
  containerId: string | null
  nodeId: string
  active?: boolean
}>()

const emit = defineEmits<{
  (e: 'activeChange', value: boolean): void
}>()

const toastStore = useToastStore()
const themeStore = useThemeStore()
const { t } = useI18n()
const {
  connected,
  connecting,
  sessionId,
  lastError,
  lastTerminalMessage,
  openSession,
  writeInput,
  resizeTerminal,
  closeSession,
} = useDockerTerminalWs(computed(() => props.nodeId))

const selectedShell = ref<ShellType>('bash')
const actualShell = ref<ShellType | null>(null)
const terminalHost = ref<HTMLDivElement | null>(null)

let xterm: Terminal | null = null
let fitAddon: FitAddon | null = null
let resizeObserver: ResizeObserver | null = null
let dataDisposable: { dispose: () => void } | null = null
let resizeDisposable: { dispose: () => void } | null = null

const shellOptions = [
  { label: 'bash', value: 'bash' },
  { label: 'sh', value: 'sh' },
]

const statusHint = computed(() => {
  if (!props.containerId) return t('app.docker.containers.terminalPanel.selectContainerFirst')
  if (connecting.value) return t('app.docker.containers.terminalPanel.connecting')
  if (sessionId.value && connected.value) {
    return t('app.docker.containers.terminalPanel.connectedWithShell', {
      shell: actualShell.value || 'bash',
    })
  }
  if (lastError.value) return lastError.value
  return t('app.docker.containers.terminalPanel.disconnected')
})

const isConnected = computed(() => !!sessionId.value && connected.value)
const hasActiveTerminalSession = computed(() => connecting.value || isConnected.value)

const mountTerminal = () => {
  if (!terminalHost.value || xterm) return
  xterm = new Terminal({
    cursorBlink: false,
    disableStdin: true,
    fontSize: 13,
    lineHeight: 1.2,
    convertEol: false,
    scrollback: 5000,
    theme: buildTerminalTheme(themeStore.currentTheme),
  })
  fitAddon = new FitAddon()
  xterm.loadAddon(fitAddon)
  xterm.open(terminalHost.value)
  fitAddon.fit()

  dataDisposable = xterm.onData((data) => {
    writeInput(data)
  })
  resizeDisposable = xterm.onResize(({ cols, rows }) => {
    resizeTerminal(cols, rows)
  })

  resizeObserver = new ResizeObserver(() => {
    if (!fitAddon || !xterm) return
    fitAddon.fit()
    resizeTerminal(xterm.cols, xterm.rows)
  })
  resizeObserver.observe(terminalHost.value)
}

const resetTerminalView = () => {
  if (!xterm) return
  xterm.clear()
  xterm.write('\x1b[2J\x1b[H')
}

const connectTerminal = () => {
  if (!props.containerId) {
    toastStore.error(t('app.docker.containers.terminalPanel.missingContainerId'))
    return
  }
  if (!xterm || !fitAddon) return
  fitAddon.fit()
  resetTerminalView()
  openSession({
    container_id: props.containerId,
    shell: selectedShell.value,
    cols: xterm.cols || 80,
    rows: xterm.rows || 24,
  })
}

const disconnectTerminal = () => {
  closeSession()
}

const preventInactiveFocus = (event: MouseEvent) => {
  if (isConnected.value) return
  event.preventDefault()
  event.stopPropagation()
  xterm?.blur()
}

watch(isConnected, (interactive) => {
  if (!xterm) return
  xterm.options.disableStdin = !interactive
  xterm.options.cursorBlink = interactive
  if (interactive) {
    requestAnimationFrame(() => xterm?.focus())
  } else {
    xterm.blur()
  }
})

watch(
  () => themeStore.currentTheme,
  (theme) => {
    if (xterm) xterm.options.theme = buildTerminalTheme(theme)
  },
)

watch(
  () => props.active,
  (active) => {
    if (active) return
    closeSession()
  },
)

watch(
  hasActiveTerminalSession,
  (value) => {
    emit('activeChange', value)
  },
  { immediate: true },
)

watch(
  () => props.containerId,
  (next, prev) => {
    if (prev && prev !== next) {
      closeSession()
      actualShell.value = null
      if (xterm) {
        resetTerminalView()
      }
    }
  },
)

watch(
  () => lastTerminalMessage.value,
  (message) => {
    if (!message || !xterm) return
    if (message.kind === 'terminalStarted') {
      actualShell.value = message.payload.shell
      if (message.payload.shell !== selectedShell.value) {
        toastStore.info(
          t('app.docker.containers.terminalPanel.shellFallback', {
            shell: message.payload.shell,
          }),
        )
      }
      return
    }
    if (message.kind === 'terminalOutput') {
      if (sessionId.value && sessionId.value !== message.payload.session_id) return
      xterm.write(message.payload.data)
      return
    }
    if (message.kind === 'terminalExit') {
      if (sessionId.value && sessionId.value !== message.payload.session_id) return
      const code = message.payload.exit_code
      xterm.writeln(
        `\r\n${t('app.docker.containers.terminalPanel.processExited', {
          code: code === null || code === undefined ? '-' : String(code),
        })}`,
      )
      closeSession()
      return
    }
    if (message.kind === 'terminalError') {
      toastStore.error(
        message.payload.message || t('app.docker.containers.terminalPanel.terminalError'),
      )
      xterm.writeln(
        `\r\n${t('app.docker.containers.terminalPanel.errorLine', {
          message: message.payload.message || t('app.docker.containers.terminalPanel.unknownError'),
        })}`,
      )
    }
  },
)

onMounted(() => {
  mountTerminal()
})

onBeforeUnmount(() => {
  closeSession()
  dataDisposable?.dispose()
  resizeDisposable?.dispose()
  resizeObserver?.disconnect()
  xterm?.dispose()
  xterm = null
  fitAddon = null
})
</script>

<template>
  <div class="container-terminal-panel">
    <div class="terminal-toolbar">
      <div class="terminal-actions">
        <span
          class="terminal-status-dot"
          :class="{ 'is-online': isConnected }"
          :title="statusHint"
        />
        <VustSelect
          id="docker-terminal-shell"
          v-model="selectedShell"
          class="terminal-shell"
          size="small"
          :disabled="!!sessionId"
          :options="shellOptions"
        />
        <VustButton
          type="primary"
          size="small"
          :disabled="!props.containerId || !!sessionId || connecting"
          @click="connectTerminal"
        >
          {{ t('app.docker.containers.terminalPanel.connect') }}
        </VustButton>
        <VustButton size="small" :disabled="!sessionId" @click="disconnectTerminal">
          {{ t('app.docker.containers.terminalPanel.disconnect') }}
        </VustButton>
      </div>
    </div>
    <div
      class="terminal-host-container"
      :class="{ 'is-inactive': !isConnected }"
      :aria-disabled="!isConnected"
      @mousedown.capture="preventInactiveFocus"
    >
      <div ref="terminalHost" class="terminal-host" data-native-context-menu />
      <div v-if="!isConnected" class="terminal-inactive-state" data-slot="terminal-state">
        {{ statusHint }}
      </div>
    </div>
  </div>
</template>

<style scoped>
.container-terminal-panel {
  display: flex;
  flex-direction: column;
  flex: 1;
  height: 100%;
  min-height: 0;
  padding: var(--vdl-space-4);
  box-sizing: border-box;
  gap: var(--vdl-space-3);
}

.terminal-toolbar {
  display: flex;
  justify-content: flex-end;
  align-items: center;
  gap: var(--vdl-space-4);
  flex-wrap: wrap;
}

.terminal-actions {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
  flex-wrap: wrap;
}

.terminal-status-dot {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background: var(--vdl-danger);
  box-shadow: 0 0 0 2px var(--vdl-bg-muted);
}

.terminal-status-dot.is-online {
  background: var(--vdl-success);
  box-shadow: 0 0 0 2px var(--vdl-bg-muted);
}

.terminal-shell {
  width: 80px;
  min-width: 80px;
}

.terminal-host-container {
  position: relative;
  flex: 1;
  min-height: 0;
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  overflow: hidden;
  background: var(--vdl-bg-canvas);
  padding: var(--vdl-space-2);
  box-sizing: border-box;
}

.terminal-host {
  width: 100%;
  height: 100%;
  box-sizing: border-box;
}

.terminal-host-container.is-inactive .terminal-host {
  opacity: 0.55;
}

.terminal-inactive-state {
  position: absolute;
  top: 50%;
  left: 50%;
  z-index: 2;
  max-width: calc(100% - var(--vdl-space-8));
  padding: var(--vdl-space-2) var(--vdl-space-3);
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-panel);
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-body-sm);
  text-align: center;
  pointer-events: none;
  transform: translate(-50%, -50%);
}

:deep(.xterm) {
  height: 100%;
  padding: 0;
}

:deep(.xterm-viewport) {
  background-color: var(--vdl-bg-canvas) !important;
}
</style>
