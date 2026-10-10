import { defineComponent } from 'vue'
import { createI18n } from 'vue-i18n'
import { mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import zh from '@/locales/zh'
import en from '@/locales/en'
import { useDockerInstall } from '../composables/useDockerInstall'

const mocks = vi.hoisted(() => ({
  installDocker: vi.fn(),
  forNode: vi.fn(),
  showConfirmation: vi.fn(),
  registerGlobalOperation: vi.fn(),
  finishGlobalOperation: vi.fn(),
}))

vi.mock('@/api/modules/docker', () => ({ dockerApi: { forNode: mocks.forNode } }))
vi.mock('@/stores/node', () => ({ useNodeStore: () => ({ currentNodeId: 'node-1' }) }))
vi.mock('@/stores/confirmation-modal', () => ({ useConfirmationModalStore: () => mocks }))
vi.mock('@/stores/window-manager', () => ({ useWindowManagerStore: () => mocks }))

const execution = (exitCode = 0, timedOut = false) => ({
  success: true,
  code: 200,
  message: 'Docker install finished',
  data: { exitCode, timedOut, stdout: 'package output', stderr: '', startedAt: 1, finishedAt: 2 },
})

/** 挂载使用真实语言资源的安装流程，避免失效 key 被测试桩掩盖。 */
function setup(locale = 'zh') {
  const availability = vi.fn().mockResolvedValue(true)
  let install!: ReturnType<typeof useDockerInstall>
  const wrapper = mount(
    defineComponent({
      setup() {
        install = useDockerInstall({ fetchDockerAvailability: availability })
        return () => null
      },
    }),
    { global: { plugins: [createI18n({ legacy: false, locale, messages: { zh, en } })] } },
  )
  return { install, availability, wrapper }
}

describe('Docker 安装流程', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.resetAllMocks()
    mocks.forNode.mockReturnValue({ installDocker: mocks.installDocker })
    mocks.showConfirmation.mockResolvedValue(true)
  })

  afterEach(() => {
    vi.clearAllTimers()
    vi.useRealTimers()
  })

  it.each(['zh', 'en'])('%s 成功日志使用有效的 Docker 翻译', async (locale) => {
    mocks.installDocker.mockResolvedValue(execution())
    const { install, wrapper } = setup(locale)
    const pending = install.startInstallDocker()
    await vi.runAllTimersAsync()
    expect(await pending).toBe(true)
    expect(install.installSuccess.value).toBe(true)
    expect(install.installLogs.value.join('\n')).not.toMatch(/app\.(scriptManager|docker)\./)
    expect(mocks.forNode).toHaveBeenCalledWith('node-1')
    expect(mocks.finishGlobalOperation).toHaveBeenCalledOnce()
    wrapper.unmount()
  })

  it('非零退出码显示真实输出及退出码', async () => {
    const result = execution(100)
    result.data.stderr = 'APT repository unavailable'
    mocks.installDocker.mockResolvedValue(result)
    const { install, availability, wrapper } = setup()
    expect(await install.startInstallDocker()).toBe(false)
    expect(install.installSuccess.value).toBe(false)
    expect(install.installLogs.value).toContain('[STDERR] APT repository unavailable')
    expect(install.installLogs.value.join('\n')).toContain('状态码：100')
    expect(availability).not.toHaveBeenCalled()
    expect(mocks.finishGlobalOperation).toHaveBeenCalledOnce()
    wrapper.unmount()
  })

  it('网络异常使用有效文案并释放操作锁', async () => {
    mocks.installDocker.mockRejectedValue(new Error('connection closed'))
    const { install, wrapper } = setup()
    expect(await install.startInstallDocker()).toBe(false)
    expect(install.installSuccess.value).toBe(false)
    expect(install.isInstallingDocker.value).toBe(false)
    expect(install.installLogs.value.join('\n')).toContain('connection closed')
    expect(install.installLogs.value.join('\n')).not.toContain('app.scriptManager')
    expect(vi.getTimerCount()).toBe(0)
    expect(mocks.finishGlobalOperation).toHaveBeenCalledOnce()
    wrapper.unmount()
  })
})
