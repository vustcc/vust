<script setup lang="ts">
import { onUnmounted, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import VustIcon from '@/components/icons/VustIcon.vue'
import { useDockerInstall } from './composables/useDockerInstall'

/**
 * @file DockerInstallConsole.vue
 * @description Docker 未安装提示卡片 + 一键安装控制台浮层。
 *
 * 包含两块 UI：
 * 1. Docker 未检测到时的提示卡片（含安装按钮 & 后台安装通知条）
 * 2. 安装过程的实时终端控制台模态框
 *
 * 通过 `useDockerInstall` composable 管理安装状态与流程。
 */

const props = defineProps<{
  /** Docker 状态码，用于渲染 i18n 状态文本 */
  dockerStatusCode: string
  /** Docker 可用性检测回调，安装完成后用于探活 */
  fetchDockerAvailability: () => Promise<boolean>
}>()

const emit = defineEmits<{
  /** 安装成功后触发，通知父组件刷新 Docker 状态 */
  (e: 'installed'): void
  /** 安装状态改变时触发，同步给父组件以更新窗口运行繁忙状态 */
  (e: 'install-state-change', installing: boolean): void
}>()

const { t } = useI18n()

// --- Composable ---

const { isInstallingDocker, showInstallConsole, installLogs, installSuccess, startInstallDocker } =
  useDockerInstall({
    fetchDockerAvailability: props.fetchDockerAvailability,
  })

// 同步安装状态给父组件
watch(
  isInstallingDocker,
  (val) => {
    emit('install-state-change', val)
  },
  { immediate: true },
)

onUnmounted(() => {
  emit('install-state-change', false)
})

/**
 * 处理安装按钮点击：
 * - 若正在安装中，打开控制台
 * - 否则启动安装流程，成功后触发 installed 事件
 */
const handleInstallClick = async () => {
  if (isInstallingDocker.value) {
    showInstallConsole.value = true
    return
  }
  const success = await startInstallDocker()
  if (success) {
    emit('installed')
  }
}
</script>

<template>
  <div class="wip-placeholder-docker">
    <div
      class="docker-error-card-container"
      style="display: flex; flex-direction: column; align-items: center; gap: 20px"
    >
      <!-- 极速后台安装呼吸胶囊通知条 -->
      <div
        v-if="isInstallingDocker && !showInstallConsole"
        class="install-background-notification"
        @click="showInstallConsole = true"
      >
        <span class="spinner-small"></span>
        <span>{{ t('app.docker.install.backgroundRunning') }}</span>
      </div>

      <div class="docker-error-card">
        <VustIcon class="docker-err-icon" name="error" :size="48" />
        <p class="docker-err-title">{{ t(`app.docker.status.${dockerStatusCode}`) }}</p>
        <p class="docker-err-desc">{{ t('app.docker.install.notDetectedDesc') }}</p>
        <button class="install-docker-btn" @click="handleInstallClick">
          <span v-if="isInstallingDocker" class="spinner-small" style="margin-right: 4px"></span>
          <VustIcon v-else name="play" :size="14" />
          <span>{{
            isInstallingDocker
              ? t('app.docker.install.viewProgress')
              : t('app.docker.install.runBtn')
          }}</span>
        </button>
      </div>
    </div>
  </div>

  <!-- Docker 一键安装实时终端浮层 -->
  <div v-if="showInstallConsole" class="modal-overlay">
    <div class="modal install-console-modal">
      <div class="modal-header">
        <h3>{{ t('app.docker.install.consoleTitle') }}</h3>
        <button class="modal-close" @click="showInstallConsole = false">×</button>
      </div>
      <div class="modal-body console-body-wrapper">
        <div class="terminal-console" data-native-context-menu>
          <div
            v-for="(log, idx) in installLogs"
            :key="idx"
            :class="[
              'log-line',
              {
                'system-log': log.startsWith('[系统') || log.startsWith('[System]'),
                'stdout-log': log.startsWith('[STDOUT]'),
                'stderr-log': log.startsWith('[STDERR]'),
                'error-log': log.startsWith('[错误') || log.startsWith('[Error]'),
              },
            ]"
          >
            {{ log }}
          </div>
        </div>
        <div class="console-footer">
          <span v-if="isInstallingDocker" class="installing-indicator">
            <span class="spinner-small"></span> {{ t('app.docker.install.progressInfo') }}
          </span>
          <span v-else-if="installSuccess === true" class="status-indicator success">
            {{ t('app.docker.install.successInfo') }}
          </span>
          <span v-else-if="installSuccess === false" class="status-indicator failed">
            {{ t('app.docker.install.failedInfo') }}
          </span>
          <button class="btn-close-console" @click="showInstallConsole = false">
            {{
              isInstallingDocker
                ? t('app.docker.install.closeConsoleBackground')
                : t('app.docker.install.closeConsole')
            }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* Docker 未安装精美卡片样式 */
.wip-placeholder-docker {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: 1;
  height: 100%;
  padding: 40px;
  background: radial-gradient(circle at center, rgba(23, 28, 41, 0.4), rgba(13, 17, 23, 0.8));
}

.docker-error-card {
  max-width: 500px;
  background: rgba(22, 27, 34, 0.7);
  border: 1px solid rgba(48, 54, 65, 0.8);
  border-radius: 12px;
  padding: 36px;
  text-align: center;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.3);
  backdrop-filter: blur(8px);
}

.docker-err-icon {
  color: #58a6ff;
  margin-bottom: 20px;
  filter: drop-shadow(0 0 12px rgba(88, 166, 255, 0.4));
}

.docker-err-title {
  font-size: 18px;
  font-weight: 600;
  color: #f0f6fc;
  margin: 0 0 12px 0;
}

.docker-err-desc {
  font-size: 13px;
  color: #8b949e;
  line-height: 1.6;
  margin: 0 0 28px 0;
}

.install-docker-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  background: linear-gradient(135deg, #1f6feb 0%, #388bfd 100%);
  border: none;
  border-radius: 6px;
  padding: 10px 24px;
  color: #ffffff;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  box-shadow: 0 4px 12px rgba(31, 111, 235, 0.25);
  transition: all 0.3s ease;
}

.install-docker-btn:hover:not(:disabled) {
  transform: translateY(-1px);
  box-shadow: 0 6px 20px rgba(31, 111, 235, 0.45);
}

.install-docker-btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

/* 控制台 Terminal Modal 样式 */
.modal-overlay {
  z-index: var(--vdl-z-index-modal, 1000);
  position: fixed;
  inset: 0;
  background-color: var(--vdl-bg-backdrop, rgba(0, 0, 0, 0.5));
  display: flex;
  justify-content: center;
  align-items: center;
}

.modal {
  background: var(--vdl-bg-panel, #161b22);
  border: 1px solid var(--vdl-border-strong, rgba(48, 54, 65, 0.8));
  border-radius: var(--vdl-radius-lg, 12px);
  box-shadow: var(--vdl-shadow-window, 0 8px 32px rgba(0, 0, 0, 0.4));
  overflow: hidden;
}

.modal-header {
  padding: 16px 20px;
  border-bottom: 1px solid var(--vdl-border-subtle, rgba(48, 54, 65, 0.5));
  background: var(--vdl-bg-muted, rgba(22, 27, 34, 0.8));
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.modal-header h3 {
  font-size: 14px;
  font-weight: 600;
  color: var(--vdl-text-primary, #f0f6fc);
  margin: 0;
}

.modal-close {
  background: none;
  border: none;
  color: var(--vdl-text-muted, #8b949e);
  font-size: 18px;
  cursor: pointer;
  padding: 0 4px;
}

.install-console-modal {
  width: 720px !important;
  max-width: 90% !important;
}

.console-body-wrapper {
  display: flex;
  flex-direction: column;
  gap: 16px;
  padding: 0 !important;
}

.terminal-console {
  height: 320px;
  background: #0d1117;
  border-bottom: 1px solid rgba(48, 54, 65, 0.8);
  padding: 16px;
  overflow-y: auto;
  font-family:
    ui-monospace,
    SFMono-Regular,
    SF Mono,
    Menlo,
    Consolas,
    Liberation Mono,
    monospace;
  font-size: 12px;
  line-height: 1.6;
  display: flex;
  flex-direction: column;
  gap: 4px;
  text-align: left;
}

.log-line {
  white-space: pre-wrap;
  word-break: break-all;
}

.system-log {
  color: #58a6ff;
}
.stdout-log {
  color: #c9d1d9;
}
.stderr-log {
  color: #ff7b72;
}
.error-log {
  color: #ff7b72;
  font-weight: bold;
}

.console-footer {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 16px 20px;
}

.installing-indicator {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: #58a6ff;
}

.spinner-small {
  width: 14px;
  height: 14px;
  border: 2px solid rgba(88, 166, 255, 0.2);
  border-top-color: #58a6ff;
  border-radius: 50%;
  animation: spin 1s linear infinite;
}

.status-indicator {
  font-size: 12px;
  font-weight: 500;
}

.status-indicator.success {
  color: #3ecf8e;
}
.status-indicator.failed {
  color: #ff7b72;
}

.btn-close-console {
  background: rgba(33, 38, 45, 0.8);
  border: 1px solid rgba(240, 246, 250, 0.15);
  border-radius: 6px;
  padding: 6px 16px;
  color: #c9d1d9;
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  transition: all 0.2s ease;
}

.btn-close-console:hover:not(:disabled) {
  background: rgba(48, 54, 65, 0.8);
  border-color: rgba(240, 246, 250, 0.3);
}

.btn-close-console:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

/* 极速后台安装呼吸胶囊通知条 */
.install-background-notification {
  display: flex;
  align-items: center;
  gap: 10px;
  background: rgba(31, 111, 235, 0.15);
  border: 1px solid rgba(88, 166, 255, 0.4);
  border-radius: 20px;
  padding: 8px 20px;
  color: #58a6ff;
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  box-shadow: 0 4px 16px rgba(31, 111, 235, 0.15);
  animation: vl-pulse-blue 2s infinite ease-in-out;
  transition: all 0.3s ease;
}

.install-background-notification:hover {
  background: rgba(31, 111, 235, 0.25);
  border-color: #58a6ff;
  transform: translateY(-1px);
  box-shadow: 0 6px 20px rgba(31, 111, 235, 0.3);
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

@keyframes vl-pulse-blue {
  0% {
    box-shadow: 0 0 0 0 rgba(88, 166, 255, 0.4);
    border-color: rgba(88, 166, 255, 0.4);
  }
  50% {
    box-shadow: 0 0 0 8px rgba(88, 166, 255, 0);
    border-color: rgba(88, 166, 255, 0.8);
  }
  100% {
    box-shadow: 0 0 0 0 rgba(88, 166, 255, 0);
    border-color: rgba(88, 166, 255, 0.4);
  }
}
</style>
