import { defineComponent } from 'vue'
import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { DiskInventory, DiskOperation } from '@/api/generated/disks'
import { disksApi } from '@/api/modules/disks'
import { useDiskManager } from '@/composables/useDiskManager'
import en from '@/locales/en'
import zh from '@/locales/zh'

vi.mock('@/api/modules/disks', () => ({
  disksApi: {
    inventory: vi.fn(),
    detail: vi.fn(),
    createOperation: vi.fn(),
    operation: vi.fn(),
    cancelOperation: vi.fn(),
  },
}))

const api = vi.mocked(disksApi)
const response = <T>(data: T) => ({ success: true, code: 200, message: '', data })
const inventory = (deviceName: string): DiskInventory => ({
  node: { nodeId: 'local', nodeName: 'Local' },
  status: 'ready',
  collectedAt: '2026-07-16T00:00:00Z',
  sourceStatuses: [],
  warnings: [],
  disks: [
    {
      diskId: `disk-${deviceName}`,
      deviceName,
      fingerprint: 'a'.repeat(64),
      sizeBytes: 1024,
      identityConfidence: 'durable',
      topologyStatus: 'blank',
      ownership: 'unknown',
      protectionReasons: [],
      capabilities: {
        canCreatePartition: true,
        canEraseAndCreatePartition: true,
        canViewDetail: true,
      },
    },
  ],
})
const operation = (): DiskOperation => ({
  operationId: 'operation-1',
  nodeId: 'local',
  kind: 'createPartition',
  status: 'queued',
  target: {},
  completedSteps: 0,
  totalSteps: 5,
  createdAt: '2026-07-16T00:00:00Z',
  updatedAt: '2026-07-16T00:00:00Z',
  capabilities: { canCancel: true },
})
const deferred = <T>() => {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => (resolve = done))
  return { promise, resolve }
}
const Harness = defineComponent({ setup: () => useDiskManager('local'), template: '<div />' })
const mountHarness = () =>
  mount(Harness, {
    global: { plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })] },
  })

describe('useDiskManager', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    sessionStorage.clear()
    api.inventory.mockResolvedValue(response(inventory('sda')))
  })
  afterEach(() => {
    vi.useRealTimers()
    vi.clearAllMocks()
  })

  it('旧刷新响应不能覆盖最新设备事实', async () => {
    const oldRequest = deferred<ReturnType<typeof response<DiskInventory>>>()
    const newRequest = deferred<ReturnType<typeof response<DiskInventory>>>()
    api.inventory
      .mockImplementationOnce(() => oldRequest.promise)
      .mockImplementationOnce(() => newRequest.promise)
    const wrapper = mountHarness()
    const latest = wrapper.vm.refresh()
    newRequest.resolve(response(inventory('sdb')))
    await latest
    oldRequest.resolve(response(inventory('sda')))
    await flushPromises()
    expect(wrapper.vm.inventory?.disks[0]?.deviceName).toBe('sdb')
    wrapper.unmount()
  })

  it('重复提交只创建一个磁盘 operation', async () => {
    api.createOperation.mockResolvedValue(response(operation()))
    const wrapper = mountHarness()
    await flushPromises()
    const request = {
      kind: 'createPartition' as const,
      diskId: 'disk-sda',
      expectedFingerprint: 'a'.repeat(64),
    }
    await Promise.all([wrapper.vm.submit(request), wrapper.vm.submit(request)])
    expect(api.createOperation).toHaveBeenCalledTimes(1)
    wrapper.unmount()
  })
})
