<script setup lang="ts">
/** 一次性脚本运行终端；转录仅存在于组件内存。 */
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import type { ScriptRunTerminalServerMessage } from '@/api/generated/scripts'
import { buildTerminalTheme, useThemeStore } from '@/stores/theme'
import '@xterm/xterm/css/xterm.css'

const props = defineProps<{ runId: string }>()
const emit = defineEmits<{ control: [message: ScriptRunTerminalServerMessage]; disconnected: [] }>()
const theme = useThemeStore()
const host = ref<HTMLDivElement | null>(null)
let terminal: Terminal | null = null
let fit: FitAddon | null = null
let socket: WebSocket | null = null
let observer: ResizeObserver | null = null
let intentionalClose = false
let disconnectReported = false

const sendControl = (type: string, payload?: Record<string, number>) => {
  if (socket?.readyState === WebSocket.OPEN)
    socket.send(JSON.stringify({ type, ...(payload ? { payload } : {}) }))
}
const resize = () => {
  if (!host.value?.clientWidth || !host.value.clientHeight || !fit) return
  fit.fit()
  sendControl('resize', { cols: terminal?.cols || 80, rows: terminal?.rows || 24 })
}
const close = () => {
  intentionalClose = true
  sendControl('close')
  socket?.close(1000)
}
defineExpose({ close })
const closeForPageUnload = () => {
  close()
  void fetch(`/api/v1/script-runs/${encodeURIComponent(props.runId)}`, {
    method: 'DELETE',
    keepalive: true,
  })
}
window.addEventListener('pagehide', closeForPageUnload)

watch(
  () => theme.currentTheme,
  (value) => {
    if (terminal) terminal.options.theme = buildTerminalTheme(value)
  },
)
onMounted(async () => {
  terminal = new Terminal({
    cursorBlink: true,
    convertEol: false,
    fontFamily:
      getComputedStyle(document.documentElement).getPropertyValue('--vdl-font-mono').trim() ||
      'monospace',
    theme: buildTerminalTheme(theme.currentTheme),
  })
  fit = new FitAddon()
  terminal.loadAddon(fit)
  terminal.open(host.value!)
  await nextTick()
  fit.fit()
  terminal.onData((data) => {
    if (socket?.readyState !== WebSocket.OPEN) return
    const bytes = new TextEncoder().encode(data)
    for (let offset = 0; offset < bytes.length; offset += 16384)
      socket.send(bytes.slice(offset, offset + 16384))
  })
  const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:'
  socket = new WebSocket(
    `${protocol}//${location.host}/api/v1/script-runs/${encodeURIComponent(props.runId)}/ws`,
  )
  socket.binaryType = 'arraybuffer'
  socket.onopen = () =>
    sendControl('start', { cols: terminal?.cols || 80, rows: terminal?.rows || 24 })
  socket.onmessage = (event) => {
    if (typeof event.data === 'string') {
      try {
        const message = JSON.parse(event.data) as ScriptRunTerminalServerMessage
        emit('control', message)
        if (message.kind === 'started') terminal?.focus()
        if (message.kind === 'exited' || message.kind === 'error') disconnectReported = true
      } catch {
        /* 非法控制帧由服务端断开 */
      }
    } else {
      terminal?.write(new Uint8Array(event.data as ArrayBuffer))
    }
  }
  const reportDisconnect = () => {
    if (!intentionalClose && !disconnectReported) {
      disconnectReported = true
      emit('disconnected')
    }
  }
  socket.onclose = reportDisconnect
  socket.onerror = reportDisconnect
  observer = new ResizeObserver(() => resize())
  observer.observe(host.value!)
})
onBeforeUnmount(() => {
  window.removeEventListener('pagehide', closeForPageUnload)
  close()
  observer?.disconnect()
  terminal?.dispose()
  socket = null
})
</script>

<template>
  <div
    ref="host"
    class="script-run-terminal"
    data-ui="script-run-terminal"
    data-slot="terminal"
    @keydown.stop
  />
</template>

<style scoped>
.script-run-terminal {
  box-sizing: border-box;
  width: 100%;
  height: min(52vh, 480px);
  min-height: 300px;
  padding: var(--vdl-space-2);
  overflow: hidden;
  border: 1px solid var(--vdl-border-subtle);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-canvas);
}
.script-run-terminal :deep(.xterm) {
  height: 100%;
}
@media (max-width: 680px) {
  .script-run-terminal {
    min-height: 240px;
    height: 48vh;
  }
}
</style>
