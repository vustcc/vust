import { createPinia } from 'pinia'
import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { DiskDetail, DiskInventory } from '@/api/generated/disks'
import { disksApi } from '@/api/modules/disks'
import { VustActionMenu } from '@/components/ui'
import DiskManagerView from '@/apps/views/DiskManagerView.vue'
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
vi.mock('@/stores/window-manager', () => ({
  useWindowManagerStore: () => ({ updateWindowRuntimeState: vi.fn() }),
}))

const inventory: DiskInventory = {
  node: { nodeId: 'local', nodeName: 'Local' },
  status: 'ready',
  collectedAt: '2026-07-16T00:00:00Z',
  sourceStatuses: [],
  warnings: [],
  disks: [
    {
      diskId: 'disk-system',
      deviceName: 'sda',
      fingerprint: 'a'.repeat(64),
      sizeBytes: 1024,
      identityConfidence: 'durable',
      topologyStatus: 'simple',
      ownership: 'system',
      protectionReasons: ['systemDevice'],
      capabilities: {
        canCreatePartition: false,
        canEraseAndCreatePartition: false,
        canViewDetail: true,
      },
    },
  ],
}

const adoptableDiskDetail: DiskDetail = {
  diskId: 'disk-external',
  deviceName: 'sdb',
  fingerprint: '795dfeed3c9ae7c0a4ba3f89416b4de1',
  sizeBytes: 10 * 1024 * 1024 * 1024,
  model: 'QEMU HARDDISK',
  serial: 'drive-scsi1',
  mediaType: 'hdd',
  identityConfidence: 'durable',
  topologyStatus: 'simple',
  ownership: 'external',
  protectionReasons: [],
  capabilities: {
    canCreatePartition: false,
    canEraseAndCreatePartition: true,
    canViewDetail: true,
  },
  eraseConfirmationText: 'ERASE 81 sdb 416b4de1',
  partitions: [
    {
      partitionId: 'part-external',
      deviceName: 'sdb1',
      partUuid: '009ff2bc-aa21-4260-904c-102464f90e0d',
      sizeBytes: 10 * 1024 * 1024 * 1024 - 2 * 1024 * 1024,
      filesystem: {
        filesystemType: 'ext4',
        uuid: '70004d7f-662c-46c0-ac84-b5e76ce2f527',
      },
      mounts: [],
      ownership: 'external',
      protectionReasons: [],
      capabilities: {
        canFormat: true,
        canAdoptFilesystem: true,
        canAdoptMounted: false,
        canMount: false,
        canUnmount: false,
        canChangeMountLocation: false,
        canRemoveManagedVolume: false,
      },
      formatConfirmationText: 'FORMAT sdb1 416b4de1',
    },
  ],
  references: [],
}

const disabledManagedDiskDetail: DiskDetail = {
  ...adoptableDiskDetail,
  ownership: 'managed',
  partitions: [
    {
      ...adoptableDiskDetail.partitions[0],
      ownership: 'managed',
      capabilities: {
        canFormat: true,
        canAdoptFilesystem: false,
        canAdoptMounted: false,
        canMount: true,
        canUnmount: false,
        canChangeMountLocation: false,
        canRemoveManagedVolume: true,
      },
      managedVolume: {
        volumeId: 'volume-managed',
        filesystemUuid: '70004d7f-662c-46c0-ac84-b5e76ce2f527',
        desiredState: 'unmounted',
        runtimeState: 'unmounted',
      },
    },
  ],
}

const mountedExternalDiskDetail: DiskDetail = {
  ...adoptableDiskDetail,
  protectionReasons: ['externalMount', 'externalFstab'],
  partitions: [
    {
      ...adoptableDiskDetail.partitions[0],
      mounts: ['/mnt/ggggg'],
      protectionReasons: ['externalMount', 'externalFstab'],
      capabilities: {
        canFormat: true,
        canAdoptFilesystem: false,
        canAdoptMounted: true,
        canMount: false,
        canUnmount: false,
        canChangeMountLocation: false,
        canRemoveManagedVolume: false,
      },
    },
  ],
  references: [
    { kind: 'mount', summary: '/mnt/ggggg' },
    { kind: 'fstab', summary: '/dev/sdb1' },
  ],
}

describe('DiskManagerView', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    sessionStorage.clear()
    vi.mocked(disksApi.inventory).mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: inventory,
    })
  })

  it('提供稳定页面标记且受保护磁盘没有变更入口', async () => {
    const wrapper = mount(DiskManagerView, {
      props: { payload: { nodeId: 'local' } },
      global: {
        plugins: [createPinia(), createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
      },
    })
    await flushPromises()

    expect(wrapper.attributes('data-page')).toBe('disk-manager')
    expect(wrapper.find('[data-ui="toolbar"]').exists()).toBe(true)
    expect(wrapper.find('[data-ui="table"]').exists()).toBe(true)
    expect(wrapper.find('[data-ui="toolbar"]').text()).not.toContain('Local')
    expect(wrapper.text()).toContain('/dev/sda')
    const menu = wrapper.findComponent(VustActionMenu)
    expect(menu.props('actions').map((action: { label: string }) => action.label)).toEqual([
      '查看详情',
    ])
    wrapper.unmount()
  })

  it('在磁盘详情中可以独立接管未挂载的 ext4 文件系统', async () => {
    vi.mocked(disksApi.inventory).mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: {
        ...inventory,
        disks: [adoptableDiskDetail],
      },
    })
    vi.mocked(disksApi.detail).mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: adoptableDiskDetail,
    })
    const wrapper = mount(DiskManagerView, {
      props: { payload: { nodeId: 'local' } },
      global: {
        plugins: [createPinia(), createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
        stubs: {
          VustDrawer: {
            props: ['modelValue', 'title'],
            template:
              '<section v-if="modelValue" data-test="drawer"><h2>{{ title }}</h2><slot /></section>',
          },
          VustDialog: {
            props: ['visible', 'title'],
            template:
              '<section v-if="visible" data-test="dialog"><h2>{{ title }}</h2><slot /></section>',
          },
        },
      },
    })
    await flushPromises()

    expect(wrapper.find('.table-shell').text()).not.toContain('QEMU HARDDISK')
    const menu = wrapper.findComponent(VustActionMenu)
    await menu.props('actions')[0].handler()
    await flushPromises()
    expect(wrapper.find('[data-test="drawer"]').text()).toContain('QEMU HARDDISK')
    expect(wrapper.find('[data-test="drawer"]').text()).toContain('序列号')
    expect(wrapper.find('[data-test="drawer"]').text()).toContain('drive-scsi1')

    const partitionMenu = wrapper.find('[data-ui="volume-actions"]').findComponent(VustActionMenu)
    const adoptAction = partitionMenu
      .props('actions')
      .find((action: { label: string }) => action.label === '接管文件系统')
    expect(adoptAction).toBeDefined()
    await adoptAction?.handler()
    await flushPromises()

    const dialog = wrapper.find('[data-test="dialog"]')
    expect(dialog.exists()).toBe(true)
    expect(dialog.text()).toContain('接管文件系统')
    expect(dialog.text()).toContain('不格式化、不挂载')
    wrapper.unmount()
  })

  it('为未挂载数据卷提供独立挂载和移除管理入口', async () => {
    vi.mocked(disksApi.inventory).mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: { ...inventory, disks: [disabledManagedDiskDetail] },
    })
    vi.mocked(disksApi.detail).mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: disabledManagedDiskDetail,
    })
    const wrapper = mount(DiskManagerView, {
      props: { payload: { nodeId: 'local' } },
      global: {
        plugins: [createPinia(), createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
        stubs: {
          VustDrawer: {
            props: ['modelValue', 'title'],
            template: '<div v-if="modelValue" data-test="drawer"><slot /></div>',
          },
          VustDialog: {
            props: ['visible', 'title'],
            template: '<div v-if="visible" data-test="dialog"><h2>{{ title }}</h2><slot /></div>',
          },
        },
      },
    })
    await flushPromises()
    await wrapper.findComponent(VustActionMenu).props('actions')[0].handler()
    await flushPromises()

    const partitionMenu = wrapper.find('[data-ui="volume-actions"]').findComponent(VustActionMenu)
    const actionTexts = partitionMenu
      .props('actions')
      .map((action: { label: string }) => action.label)
    expect(actionTexts).toEqual(['格式化分区', '挂载数据卷', '移除数据卷管理'])

    await partitionMenu.get('button').trigger('click')
    await flushPromises()
    const dropdown = document.body.querySelector<HTMLElement>('.vl-dropdown[role="menu"]')
    expect(dropdown).not.toBeNull()

    const mountAction = partitionMenu
      .props('actions')
      .find((action: { label: string }) => action.label === '挂载数据卷')
    await mountAction?.handler()
    await flushPromises()
    expect(wrapper.find('[data-test="dialog"]').text()).toContain('选择数据卷名称')
    wrapper.unmount()
  })

  it('允许显式接管挂载在受管目录下的普通 ext4 数据卷', async () => {
    vi.mocked(disksApi.inventory).mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: { ...inventory, disks: [mountedExternalDiskDetail] },
    })
    vi.mocked(disksApi.detail).mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: mountedExternalDiskDetail,
    })
    const wrapper = mount(DiskManagerView, {
      props: { payload: { nodeId: 'local' } },
      global: {
        plugins: [createPinia(), createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
        stubs: {
          VustDrawer: {
            props: ['modelValue', 'title'],
            template: '<div v-if="modelValue" data-test="drawer"><slot /></div>',
          },
          VustDialog: {
            props: ['visible', 'title'],
            template: '<div v-if="visible" data-test="dialog"><h2>{{ title }}</h2><slot /></div>',
          },
        },
      },
    })
    await flushPromises()
    await wrapper.findComponent(VustActionMenu).props('actions')[0].handler()
    await flushPromises()

    const partitionMenu = wrapper.find('[data-ui="volume-actions"]').findComponent(VustActionMenu)
    const adoptAction = partitionMenu
      .props('actions')
      .find((action: { label: string }) => action.label === '接管当前挂载')
    expect(adoptAction).toBeDefined()
    await adoptAction?.handler()
    await flushPromises()
    expect(wrapper.find('[data-test="dialog"]').text()).toContain(
      '保持当前 ext4 数据卷和挂载路径不变',
    )
    wrapper.unmount()
  })

  it('格式化是独立高风险操作且不要求挂载名称', async () => {
    vi.mocked(disksApi.inventory).mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: { ...inventory, disks: [adoptableDiskDetail] },
    })
    vi.mocked(disksApi.detail).mockResolvedValue({
      success: true,
      code: 200,
      message: '',
      data: adoptableDiskDetail,
    })
    const wrapper = mount(DiskManagerView, {
      props: { payload: { nodeId: 'local' } },
      global: {
        plugins: [createPinia(), createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
        stubs: {
          VustDrawer: {
            props: ['modelValue', 'title'],
            template: '<div v-if="modelValue" data-test="drawer"><slot /></div>',
          },
          VustDialog: {
            props: ['visible', 'title'],
            template: '<div v-if="visible" data-test="dialog"><h2>{{ title }}</h2><slot /></div>',
          },
        },
      },
    })
    await flushPromises()
    await wrapper.findComponent(VustActionMenu).props('actions')[0].handler()
    await flushPromises()

    const partitionMenu = wrapper.find('[data-ui="volume-actions"]').findComponent(VustActionMenu)
    const formatAction = partitionMenu
      .props('actions')
      .find((action: { label: string }) => action.label === '格式化分区')
    expect(formatAction?.className).toBe('app-btn-delete')
    await formatAction?.handler()
    await flushPromises()

    const dialog = wrapper.find('[data-ui="format-form"]')
    expect(dialog.text()).toContain('完成后保持未挂载状态')
    expect(dialog.find('[name="diskMountName"]').exists()).toBe(false)
    expect(dialog.find('[name="diskFormatConfirmation"]').exists()).toBe(true)
    wrapper.unmount()
  })
})
