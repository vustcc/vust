/**
 * @file useDockerInstall.ts
 * @description Docker 一键安装 composable，封装专用安装接口、模拟日志、健康检查探活等完整流程。
 *
 * 职责：
 * - 确认弹窗交互
 * - 注册全局操作锁（防止并发）
 * - 模拟日志滚动（提升等待体验）
 * - 通过 Docker 专用安装 API 执行官方仓库安装流程
 * - 安装完成后多轮渐进式健康检查
 * - 安装控制台开关管理
 */

import { nextTick, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { dockerApi } from '@/api/modules/docker'
import { useConfirmationModalStore } from '@/stores/confirmation-modal'
import { useNodeStore } from '@/stores/node'
import { useWindowManagerStore } from '@/stores/window-manager'

/** composable 入参 */
export interface UseDockerInstallOptions {
  /**
   * Docker 可用性检测回调，安装完成后用于探活。
   * 返回 true 表示 Docker 已可用。
   */
  fetchDockerAvailability: () => Promise<boolean>
}

/**
 * Docker 一键安装 composable。
 *
 * 封装从用户确认 → 脚本执行 → 模拟日志 → 健康检查的完整流程，
 * 供 DockerInstallConsole 组件消费。
 */
export function useDockerInstall(options: UseDockerInstallOptions) {
  const { fetchDockerAvailability } = options

  const { t } = useI18n()
  const modalStore = useConfirmationModalStore()
  const nodeStore = useNodeStore()
  const windowStore = useWindowManagerStore()

  // --------------- 状态 ---------------

  /** 是否正在安装中 */
  const isInstallingDocker = ref(false)
  /** 是否显示安装控制台 */
  const showInstallConsole = ref(false)
  /** 安装日志行 */
  const installLogs = ref<string[]>([])
  /** 安装结果：null=进行中, true=成功, false=失败 */
  const installSuccess = ref<boolean | null>(null)
  /** 模拟日志定时器句柄 */
  let simTimer: number | null = null

  // --------------- 内部工具 ---------------

  /**
   * 滚动终端控制台到底部。
   */
  const scrollToTerminalBottom = () => {
    nextTick(() => {
      const el = document.querySelector('.terminal-console')
      if (el) {
        el.scrollTop = el.scrollHeight
      }
    })
  }

  // --------------- 核心流程 ---------------

  /**
   * 启动 Docker 一键安装流程。
   *
   * 1. 弹出确认对话框
   * 2. 注册全局操作锁
   * 3. 启动模拟日志定时器
   * 4. 调用 Docker 专用安装 API
   * 5. 安装成功后启动多轮健康检查
   *
   * @returns 安装完成后是否成功
   */
  const startInstallDocker = async (): Promise<boolean> => {
    const confirmed = await modalStore.showConfirmation(
      t('app.docker.install.confirmMessage'),
      t('app.docker.install.confirmTitle'),
      t('app.docker.install.confirmOk'),
      t('app.docker.install.confirmCancel'),
    )
    if (!confirmed) return false

    const targetNodeId = nodeStore.currentNodeId || 'local'
    const operationId = `docker-install:${targetNodeId}:${Date.now()}`
    showInstallConsole.value = true
    isInstallingDocker.value = true
    windowStore.registerGlobalOperation({
      operationId,
      nodeId: targetNodeId,
      sourceAppId: 'docker-manager',
      title: t('app.docker.install.operationTitle'),
      cancellable: false,
      reason: t('app.docker.install.operationBusy'),
    })
    installSuccess.value = null
    installLogs.value = [
      t('app.scriptManager.console.initChannel'),
      t('app.scriptManager.console.delivering'),
    ]
    scrollToTerminalBottom()

    // 1. 开启实时拟真伪日志滚动定时器，防止干等体验
    const simulatedLogs = [
      { delay: 1500, text: t('app.docker.install.simulatedLogs.precheck') },
      { delay: 3500, text: t('app.docker.install.simulatedLogs.environmentPassed') },
      { delay: 6000, text: t('app.docker.install.simulatedLogs.repository') },
      {
        delay: 10000,
        text: t('app.docker.install.simulatedLogs.aptUpdate'),
      },
      {
        delay: 22000,
        text: t('app.docker.install.simulatedLogs.pullingPackages'),
      },
      {
        delay: 38000,
        text: t('app.docker.install.simulatedLogs.unpacking'),
      },
      {
        delay: 52000,
        text: t('app.docker.install.simulatedLogs.registeringSystemd'),
      },
      { delay: 65000, text: t('app.docker.install.simulatedLogs.activating') },
      {
        delay: 75000,
        text: t('app.docker.install.simulatedLogs.finalCheck'),
      },
    ]

    if (simTimer) clearInterval(simTimer)
    const startTime = Date.now()
    simTimer = window.setInterval(() => {
      if (!isInstallingDocker.value) {
        if (simTimer) clearInterval(simTimer)
        return
      }
      const elapsed = Date.now() - startTime
      simulatedLogs.forEach((item) => {
        if (elapsed >= item.delay && !installLogs.value.includes(item.text)) {
          installLogs.value.push(item.text)
          scrollToTerminalBottom()
        }
      })
    }, 1000)

    let success = false

    try {
      const res = await dockerApi.forNode(targetNodeId).installDocker(600)
      // 停止伪日志定时器
      if (simTimer) {
        clearInterval(simTimer)
        simTimer = null
      }

      if (res.data) {
        const payload = res.data

        // 清理原先只有两句的伪日志提示，直接追加真实日志以确保信息真实与精准
        installLogs.value.push(t('app.docker.install.simulatedLogs.realOutput'))

        if (payload.stdout) {
          payload.stdout.split('\n').forEach((l) => {
            if (l.trim()) installLogs.value.push(`[STDOUT] ${l}`)
          })
        }
        if (payload.stderr) {
          payload.stderr.split('\n').forEach((l) => {
            if (l.trim()) installLogs.value.push(`[STDERR] ${l}`)
          })
        }
        scrollToTerminalBottom()

        if (payload.exitCode === 0) {
          // 开启多轮退避探活自愈机制，彻底避开 Timing Race
          installLogs.value.push(t('app.docker.install.simulatedLogs.socketWait'))
          scrollToTerminalBottom()

          // A. 静默缓冲等待 3.5 秒
          await new Promise((resolve) => setTimeout(resolve, 3500))

          // B. 启动最大 5 轮，每次间隔 1.5 秒的渐进式可用性探活
          let checkSuccess = false
          for (let attempt = 1; attempt <= 5; attempt++) {
            installLogs.value.push(t('app.docker.install.simulatedLogs.healthCheck', { attempt }))
            scrollToTerminalBottom()
            const available = await fetchDockerAvailability()
            if (available) {
              checkSuccess = true
              installLogs.value.push(t('app.docker.install.simulatedLogs.healthSuccess'))
              scrollToTerminalBottom()
              break
            }
            await new Promise((resolve) => setTimeout(resolve, 1500))
          }

          if (checkSuccess) {
            installSuccess.value = true
            success = true
            installLogs.value.push(t('app.scriptManager.console.execDone'))
            scrollToTerminalBottom()
          } else {
            installSuccess.value = false
            installLogs.value.push(t('app.docker.install.simulatedLogs.healthFailed'))
            scrollToTerminalBottom()
          }
        } else {
          installSuccess.value = false
          installLogs.value.push(
            t('app.scriptManager.console.exitCode', { code: String(payload.exitCode) }),
          )
          scrollToTerminalBottom()
        }
      }
    } catch (err) {
      if (simTimer) {
        clearInterval(simTimer)
        simTimer = null
      }
      installSuccess.value = false
      const errMsg =
        err instanceof Error ? err.message : t('app.scriptManager.messages.networkTimeout')
      installLogs.value.push(t('app.scriptManager.console.commError', { error: errMsg }))
      scrollToTerminalBottom()
    } finally {
      isInstallingDocker.value = false
      windowStore.finishGlobalOperation(operationId)
    }

    return success
  }

  return {
    /** 是否正在安装 */
    isInstallingDocker,
    /** 是否显示控制台 */
    showInstallConsole,
    /** 安装日志行 */
    installLogs,
    /** 安装结果 */
    installSuccess,
    /** 启动安装 */
    startInstallDocker,
  }
}
