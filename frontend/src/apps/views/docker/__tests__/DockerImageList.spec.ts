import { nextTick, reactive } from 'vue'
import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import en from '@/locales/en'
import zh from '@/locales/zh'
import DockerImageList from '../DockerImageList.vue'

const state = vi.hoisted(() => ({
  docker: {} as Record<string, unknown>,
  node: {} as Record<string, unknown>,
  toast: {
    success: vi.fn(),
    info: vi.fn(),
  },
  loadImage: vi.fn(),
}))

vi.mock('@/stores/docker', () => ({ useDockerStore: () => state.docker }))
vi.mock('@/stores/node', () => ({ useNodeStore: () => state.node }))
vi.mock('@/stores/toast', () => ({ useToastStore: () => state.toast }))
vi.mock('@/api/modules/docker', () => ({
  dockerApi: {
    forNode: vi.fn(() => ({ loadImage: state.loadImage })),
  },
}))

const mountView = () =>
  mount(DockerImageList, {
    global: {
      plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })],
    },
  })

const selectFile = async (wrapper: ReturnType<typeof mountView>, file: File) => {
  const input = wrapper.find('[data-ui="image-import-file-input"]').element as HTMLInputElement
  Object.defineProperty(input, 'files', { configurable: true, value: [file] })
  input.dispatchEvent(new Event('change'))
  await nextTick()
}

const openImportView = async (wrapper: ReturnType<typeof mountView>) => {
  await wrapper.find('[data-ui="image-import-button"]').trigger('click')
  await nextTick()
}

describe('DockerImageList image archive import', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
    state.docker = reactive({
      imagesList: [],
      imageListLoading: false,
      imageListError: '',
      imageDeleteLoadingId: '',
      fetchImagesList: vi.fn().mockResolvedValue(true),
      fetchOverviewData: vi.fn().mockResolvedValue({}),
      handleDeleteImage: vi.fn(),
    })
    state.node = reactive({
      currentNodeId: 'node-a',
      nodes: [
        { id: 'node-a', name: '边缘节点 A' },
        { id: 'node-b', name: '边缘节点 B' },
      ],
    })
    state.loadImage.mockReset()
    state.toast.success.mockReset()
    state.toast.info.mockReset()
  })

  it('在内容区打开导入视图并隐藏镜像列表', async () => {
    const wrapper = mountView()
    await openImportView(wrapper)

    const importView = wrapper.find('[data-ui="image-import-view"]')
    expect(importView.text()).toContain('目标节点：边缘节点 A')
    expect(importView.text()).toContain('10 GB')
    expect(importView.text()).toContain('zstd')
    expect(
      importView.find('[data-ui="image-import-file-name"] input').attributes('aria-label'),
    ).toBe('请选择镜像归档文件')
    expect(wrapper.find('[data-ui="image-import-button"]').exists()).toBe(false)
    expect(wrapper.find('[data-ui="image-refresh-button"]').exists()).toBe(false)
    expect(document.body.querySelector('[data-ui="image-import-dialog"]')).toBeNull()
    wrapper.unmount()
  })

  it('立即拒绝非法后缀、空文件和超过 10 GB 的文件', async () => {
    const wrapper = mountView()
    await openImportView(wrapper)

    await selectFile(wrapper, new File(['bad'], 'image.zip'))
    expect(wrapper.find('[data-ui="image-import-error"]').text()).toContain('请选择支持的')

    await selectFile(wrapper, new File([], 'image.tar'))
    expect(wrapper.find('[data-ui="image-import-error"]').text()).toContain('不能为空')

    const oversized = new File(['archive'], 'image.TAR.ZST')
    Object.defineProperty(oversized, 'size', { value: 10_000_000_001 })
    await selectFile(wrapper, oversized)
    expect(wrapper.find('[data-ui="image-import-error"]').text()).toContain('不能超过 10 GB')
    expect(state.loadImage).not.toHaveBeenCalled()
    wrapper.unmount()
  })

  it('先显示上传百分比，传输完成后切换载入状态并刷新镜像数据', async () => {
    let resolveRequest: ((value: unknown) => void) | undefined
    state.loadImage.mockImplementation(
      (_formData: FormData, options: { onUploadProgress: (event: unknown) => void }) =>
        new Promise((resolve) => {
          resolveRequest = resolve
          options.onUploadProgress({ loaded: 50, total: 100 })
        }),
    )
    const wrapper = mountView()
    await openImportView(wrapper)
    await selectFile(wrapper, new File(['archive'], 'bundle.tar.gz'))
    await wrapper.find('[data-ui="image-import-submit"]').trigger('click')
    await nextTick()
    const progress = wrapper.find('[data-ui="image-import-progress"]')
    expect(progress.text()).toContain('50%')
    expect(progress.find('[role="progressbar"]').attributes('aria-valuenow')).toBe('50')

    const options = state.loadImage.mock.calls[0]?.[1] as {
      onUploadProgress: (event: unknown) => void
    }
    options.onUploadProgress({ loaded: 100, total: 100 })
    await nextTick()
    expect(progress.text()).toContain('正在载入镜像，请勿关闭窗口')
    expect(progress.find('[role="progressbar"]').attributes('aria-valuenow')).toBeUndefined()

    resolveRequest?.({ success: true, data: 'Loaded image', message: '' })
    await flushPromises()
    expect(wrapper.find('[data-ui="image-import-view"]').exists()).toBe(false)
    expect(wrapper.find('[data-ui="image-import-button"]').exists()).toBe(true)
    expect(state.toast.success).toHaveBeenCalledWith('镜像归档已成功导入。')
    expect(state.docker.fetchImagesList).toHaveBeenCalledTimes(2)
    expect(state.docker.fetchOverviewData).toHaveBeenCalledOnce()
    wrapper.unmount()
  })

  it('导入失败时保留文件和弹窗以便重试', async () => {
    state.loadImage.mockResolvedValue({
      success: false,
      message:
        'Docker image import failed: Error unpacking image mssql:latest: mismatched image rootfs and manifest layers',
    })
    const wrapper = mountView()
    await openImportView(wrapper)
    await selectFile(wrapper, new File(['archive'], 'bundle.txz'))
    await wrapper.find('[data-ui="image-import-submit"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-ui="image-import-view"]').exists()).toBe(true)
    expect(wrapper.find('[data-ui="image-import-file-meta"]').text()).toContain('bundle.txz')
    expect(wrapper.find('[data-ui="image-import-error"]').text()).toContain(
      'mismatched image rootfs and manifest layers',
    )
    wrapper.unmount()
  })

  it('主动取消、节点切换和组件卸载都会中止上传', async () => {
    const signals: AbortSignal[] = []
    state.loadImage.mockImplementation(
      (_formData: FormData, options: { signal: AbortSignal }) =>
        new Promise((_resolve, reject) => {
          signals.push(options.signal)
          options.signal.addEventListener('abort', () =>
            reject(new DOMException('Aborted', 'AbortError')),
          )
        }),
    )
    const wrapper = mountView()
    await openImportView(wrapper)
    await selectFile(wrapper, new File(['archive'], 'bundle.tbz2'))
    await wrapper.find('[data-ui="image-import-submit"]').trigger('click')
    await nextTick()
    expect(wrapper.find('[data-ui="workspace-header-back"]').text()).toBe('返回')
    expect(wrapper.find('[data-ui="image-import-back"]').exists()).toBe(false)
    await wrapper.find('[data-ui="workspace-header-back"]').trigger('click')
    await flushPromises()
    expect(signals[0]?.aborted).toBe(true)
    expect(state.toast.info).toHaveBeenCalledWith('已取消镜像导入。')

    await openImportView(wrapper)
    await selectFile(wrapper, new File(['archive'], 'bundle.tar'))
    await wrapper.find('[data-ui="image-import-submit"]').trigger('click')
    await nextTick()
    state.node.currentNodeId = 'node-b'
    await flushPromises()
    expect(signals[1]?.aborted).toBe(true)
    expect(state.toast.info).toHaveBeenCalledWith('目标节点已切换，原节点的镜像导入已取消。')

    state.node.currentNodeId = 'node-a'
    await nextTick()
    await openImportView(wrapper)
    await selectFile(wrapper, new File(['archive'], 'bundle.tzst'))
    await wrapper.find('[data-ui="image-import-submit"]').trigger('click')
    await nextTick()
    wrapper.unmount()
    await flushPromises()
    expect(signals[2]?.aborted).toBe(true)
  })
})
