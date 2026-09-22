import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import FileManagerView from '../FileManagerView.vue'
import zh from '@/locales/zh'
import type { FileListPage, FsEntry } from '@/api/interface/fs'

const api = vi.hoisted(() => ({
  home: vi.fn(),
  listEntries: vi.fn(),
  forNode: vi.fn(),
  openWindowWithPayload: vi.fn(),
}))
const operations = vi.hoisted(() => ({
  createFile: vi.fn(),
  mkdir: vi.fn(),
  renamePath: vi.fn(),
  removePath: vi.fn(),
  runPathTask: vi.fn(),
  startUpload: vi.fn(),
  cancelUpload: vi.fn(),
  dismissUpload: vi.fn(),
  uploadActive: { __v_isRef: true, value: false },
  uploadTask: {
    visible: false,
    kind: 'files',
    displayName: '',
    targetDirectory: '',
    status: 'completed',
    totalFiles: 0,
    completedFiles: 0,
    failedFiles: 0,
    totalBytes: 0,
    transferredBytes: 0,
    progressPercent: 0,
    currentFile: '',
    failures: [],
    errorSummary: '',
  },
}))
const windowRuntime = vi.hoisted(() => ({ updateWindowRuntimeState: vi.fn() }))
vi.mock('@/api/modules/fs', () => ({ fsApi: { forNode: api.forNode } }))
vi.mock('@/stores/node', () => ({
  useNodeStore: () => ({ currentNodeId: 'global-node' }),
}))
vi.mock('@/stores/window-manager', () => ({
  useWindowManagerStore: () => ({
    updateWindowRuntimeState: windowRuntime.updateWindowRuntimeState,
    openWindowWithPayload: api.openWindowWithPayload,
  }),
}))
vi.mock('@/stores/toast', () => ({
  useToastStore: () => ({ error: vi.fn(), success: vi.fn() }),
}))
vi.mock('@/composables/useFileOperations', () => ({
  useFileOperations: () => ({
    createFile: operations.createFile,
    mkdir: operations.mkdir,
    removePath: operations.removePath,
    renamePath: operations.renamePath,
    runPathTask: operations.runPathTask,
    downloadFile: vi.fn(),
    uploadTask: operations.uploadTask,
    uploadActive: operations.uploadActive,
    startUpload: operations.startUpload,
    cancelUpload: operations.cancelUpload,
    dismissUpload: operations.dismissUpload,
    resumeActiveTasks: vi.fn().mockResolvedValue(undefined),
    resumeActiveTransfers: vi.fn().mockResolvedValue(undefined),
  }),
}))

const response = <T>(data: T) => ({ success: true, code: 200, message: '', data })

const entry = (name: string, path: string, kind: FsEntry['kind'] = 'file'): FsEntry => ({
  name,
  path,
  kind,
  sizeBytes: 1,
  revision: `revision-${name}`,
  management: { kind: 'custom' },
  capabilities: {
    canOpen: true,
    canRead: true,
    canWrite: true,
    canCreateChild: false,
    canRename: true,
    canCopy: true,
    canRemove: true,
    canUpload: false,
    canDownload: true,
  },
})

const page = (path: string, entries: FsEntry[]): FileListPage => ({
  path,
  entries,
  page: 1,
  pageSize: 50,
  total: entries.length,
  counts: {
    fileCount: entries.length,
    directoryCount: 0,
    symlinkCount: 0,
    otherCount: 0,
  },
  loadedAt: '2026-07-16T00:00:00.000Z',
})

const deferred = <T>() => {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((complete) => {
    resolve = complete
  })
  return { promise, resolve }
}

describe('FileManagerView', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    api.forNode.mockReturnValue({ home: api.home, listEntries: api.listEntries })
    api.home.mockResolvedValue(response({ path: '/home' }))
    api.listEntries.mockResolvedValue(response(page('/home', [])))
    operations.createFile.mockResolvedValue(true)
    operations.mkdir.mockResolvedValue(true)
    operations.renamePath.mockResolvedValue(true)
    operations.removePath.mockResolvedValue(true)
    operations.runPathTask.mockResolvedValue(true)
    operations.startUpload.mockResolvedValue(false)
    operations.uploadActive.value = false
    Object.assign(operations.uploadTask, {
      visible: false,
      kind: 'files',
      displayName: '',
      targetDirectory: '',
      status: 'completed',
      totalFiles: 0,
      completedFiles: 0,
      failedFiles: 0,
      totalBytes: 0,
      transferredBytes: 0,
      progressPercent: 0,
      currentFile: '',
      failures: [],
      errorSummary: '',
    })
  })

  it('固定使用窗口节点并让最新路径请求获胜', async () => {
    const wrapper = mount(FileManagerView, {
      props: { payload: { nodeId: 'node-a' } },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })
    await flushPromises()
    expect(api.forNode).toHaveBeenCalledWith('node-a')
    expect(wrapper.get('[data-page="file-manager"]').attributes('data-node-id')).toBe('node-a')

    const slow = deferred<ReturnType<typeof response<FileListPage>>>()
    const fast = deferred<ReturnType<typeof response<FileListPage>>>()
    api.listEntries.mockImplementation(({ path }: { path: string }) => {
      if (path === '/slow') return slow.promise
      if (path === '/fast') return fast.promise
      return Promise.resolve(response(page(path, [])))
    })
    const input = wrapper.get('input[name="fileManagerPath"]')
    await input.setValue('/slow')
    await input.trigger('keyup.enter')
    await input.setValue('/fast')
    await input.trigger('keyup.enter')
    fast.resolve(response(page('/fast', [entry('fast.txt', '/fast/fast.txt')])))
    await flushPromises()
    slow.resolve(response(page('/slow', [entry('slow.txt', '/slow/slow.txt')])))
    await flushPromises()

    expect(wrapper.text()).toContain('fast.txt')
    expect(wrapper.text()).not.toContain('slow.txt')
    wrapper.unmount()
  })

  it('打开文件编辑器时保留可响应语言变化的标题键', async () => {
    api.listEntries.mockResolvedValue(
      response(page('/home', [entry('notes.txt', '/home/notes.txt')])),
    )
    const wrapper = mount(FileManagerView, {
      props: { payload: { nodeId: 'node-a' } },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })
    await flushPromises()

    await wrapper.get('.entry-name').trigger('dblclick')

    expect(api.openWindowWithPayload).toHaveBeenCalledWith(
      'file-editor',
      { path: '/home/notes.txt', nodeId: 'node-a' },
      {
        title: '文件编辑',
        i18nTitleKey: 'app.fileEditor.appName',
      },
    )
    wrapper.unmount()
  })

  it('后台刷新失败保留已有目录数据', async () => {
    api.listEntries.mockResolvedValue(
      response(page('/home', [entry('retained.txt', '/home/retained.txt')])),
    )
    const wrapper = mount(FileManagerView, {
      props: { payload: { nodeId: 'node-a' } },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })
    await flushPromises()
    api.listEntries.mockResolvedValue({
      success: false,
      code: 503,
      message: 'node unavailable',
      data: null,
    })
    await wrapper.get('[data-ui="file-refresh"]').trigger('click')
    await flushPromises()

    expect(wrapper.text()).toContain('retained.txt')
    expect(wrapper.text()).toContain('node unavailable')
    wrapper.unmount()
  })

  it('取消目录删除确认时不调用删除接口', async () => {
    const directory = entry('archive', '/home/archive', 'directory')
    api.listEntries.mockResolvedValue(response(page('/home', [directory])))
    const wrapper = mount(FileManagerView, {
      props: { payload: { nodeId: 'node-a' } },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })
    await flushPromises()

    const deletion = (
      wrapper.vm as unknown as { handleDelete: (item: FsEntry) => Promise<void> }
    ).handleDelete(directory)
    await flushPromises()

    const confirmationDialog = wrapper.get('[data-ui="file-confirmation-dialog"]')
    expect(confirmationDialog.text()).toContain('/home/archive')
    const cancelButton = confirmationDialog
      .findAll('button')
      .find((button) => button.text() === '取消')
    expect(cancelButton).toBeDefined()
    await cancelButton!.trigger('click')
    await deletion
    expect(operations.removePath).not.toHaveBeenCalled()
    wrapper.unmount()
  })

  it('确认单项删除时保留路径、递归参数和 revision', async () => {
    const file = entry('report.txt', '/home/report.txt')
    api.listEntries.mockResolvedValue(response(page('/home', [file])))
    const wrapper = mount(FileManagerView, {
      props: { payload: { nodeId: 'node-a' } },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })
    await flushPromises()

    const deletion = (
      wrapper.vm as unknown as { handleDelete: (item: FsEntry) => Promise<void> }
    ).handleDelete(file)
    await flushPromises()
    const confirmButton = wrapper
      .get('[data-ui="file-confirmation-dialog"]')
      .findAll('button')
      .find((button) => button.text() === '确认删除')
    expect(confirmButton).toBeDefined()
    await confirmButton!.trigger('click')
    await deletion
    await flushPromises()

    expect(operations.removePath).toHaveBeenCalledWith(
      '/home/report.txt',
      true,
      'revision-report.txt',
    )
    wrapper.unmount()
  })

  it('批量删除只使用确认弹窗打开时的路径快照', async () => {
    const wrapper = mount(FileManagerView, {
      props: { payload: { nodeId: 'node-a' } },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })
    await flushPromises()
    const view = wrapper.vm as unknown as {
      toggleSelection: (path: string, checked: boolean) => void
      handleBatchDelete: () => Promise<void>
    }
    view.toggleSelection('/home/a', true)
    view.toggleSelection('/home/b', true)

    const deletion = view.handleBatchDelete()
    await flushPromises()
    view.toggleSelection('/home/c', true)
    const confirmButton = wrapper
      .get('[data-ui="file-confirmation-dialog"]')
      .findAll('button')
      .find((button) => button.text() === '确认删除')
    expect(confirmButton).toBeDefined()
    await confirmButton!.trigger('click')
    await deletion
    await flushPromises()

    expect(operations.runPathTask).toHaveBeenCalledWith(
      'remove',
      [{ path: '/home/a' }, { path: '/home/b' }],
      true,
    )
    wrapper.unmount()
  })

  it('通过“新建”菜单打开文件和文件夹表单', async () => {
    const wrapper = mount(FileManagerView, {
      attachTo: document.body,
      props: { payload: { nodeId: 'node-a' } },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })
    await flushPromises()

    expect(wrapper.find('[data-ui="file-operation-dialog"]').exists()).toBe(false)
    const createButton = wrapper.findAll('button').find((button) => button.text().trim() === '新建')
    expect(createButton).toBeDefined()
    await createButton!.trigger('click')
    await flushPromises()

    const menuItems = Array.from(
      document.body.querySelectorAll<HTMLButtonElement>('button'),
    ).filter((button) => ['新建文件', '新建文件夹'].includes(button.textContent?.trim() || ''))
    expect(menuItems.map((button) => button.textContent?.trim())).toEqual([
      '新建文件',
      '新建文件夹',
    ])

    menuItems[0]!.click()
    await flushPromises()
    expect(wrapper.get('[data-ui="file-operation-dialog"]').text()).toContain('新建文件')
    expect(document.activeElement?.getAttribute('name')).toBe('fileManagerDialogInput')
    wrapper.unmount()
  })

  it('创建失败时保留表单，成功后关闭并刷新列表', async () => {
    operations.createFile.mockResolvedValueOnce(false).mockResolvedValueOnce(true)
    const wrapper = mount(FileManagerView, {
      props: { payload: { nodeId: 'node-a' } },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })
    await flushPromises()

    const view = wrapper.vm as unknown as {
      openDialog: (type: 'file') => void
    }
    view.openDialog('file')
    await flushPromises()
    const input = wrapper.get('input[name="fileManagerDialogInput"]')
    await input.setValue('draft.txt')

    const confirm = () =>
      wrapper
        .get('[data-ui="file-operation-dialog"]')
        .findAll('button')
        .find((button) => button.text() === '确定')!

    await confirm().trigger('click')
    await flushPromises()
    expect(operations.createFile).toHaveBeenLastCalledWith('/home/draft.txt', '')
    expect(wrapper.find('[data-ui="file-operation-dialog"]').exists()).toBe(true)
    expect(input.element.value).toBe('draft.txt')

    await confirm().trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-ui="file-operation-dialog"]').exists()).toBe(false)
    expect(api.listEntries).toHaveBeenCalledTimes(2)
    wrapper.unmount()
  })

  it('提供多文件与保留相对路径的文件夹上传入口', async () => {
    const wrapper = mount(FileManagerView, {
      props: { payload: { nodeId: 'node-a' } },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })
    await flushPromises()

    expect(wrapper.get('input[name="fileManagerUploadFiles"]').attributes()).toHaveProperty(
      'multiple',
    )
    const folderInput = wrapper.get('input[name="fileManagerUploadFolder"]')
    expect(folderInput.attributes()).toHaveProperty('multiple')
    expect(folderInput.attributes()).toHaveProperty('webkitdirectory')

    const file = new File(['content'], 'guide.md')
    Object.defineProperty(file, 'webkitRelativePath', { value: 'docs/guide.md' })
    const input = { files: [file], value: 'selected' } as unknown as HTMLInputElement
    await (
      wrapper.vm as unknown as {
        runSelectedUpload: (input: HTMLInputElement, kind: 'folder') => Promise<void>
      }
    ).runSelectedUpload(input, 'folder')

    expect(operations.startUpload).toHaveBeenCalledWith('/home', {
      kind: 'folder',
      displayName: 'docs',
      files: [{ file, relativePath: 'docs/guide.md' }],
    })
    expect(input.value).toBe('')
    wrapper.unmount()
  })

  it('在窗口级对话框展示可访问上传进度并用上传原因阻止关闭窗口', async () => {
    operations.uploadActive.value = true
    Object.assign(operations.uploadTask, {
      visible: true,
      kind: 'folder',
      displayName: 'docs',
      targetDirectory: '/home',
      status: 'uploading',
      totalFiles: 4,
      completedFiles: 1,
      failedFiles: 1,
      totalBytes: 8,
      transferredBytes: 4,
      progressPercent: 50,
      currentFile: 'docs/report.txt',
      failures: [{ path: 'docs/failed.txt', message: 'conflict' }],
      errorSummary: '',
    })
    const wrapper = mount(FileManagerView, {
      props: { payload: { nodeId: 'node-a' }, windowId: 'file-window' },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })
    await flushPromises()

    const dialog = wrapper.element.querySelector<HTMLElement>('[data-ui="file-upload-dialog"]')
    expect(dialog).not.toBeNull()
    const progress = dialog!.querySelector<HTMLElement>('[role="progressbar"]')
    expect(progress?.getAttribute('aria-valuenow')).toBe('50')
    expect(dialog!.textContent).toContain('已处理 2 / 4 个文件')

    dialog!.querySelector<HTMLButtonElement>('.application-dialog__close')?.click()
    await flushPromises()
    expect(operations.dismissUpload).not.toHaveBeenCalled()
    expect(wrapper.find('[data-ui="file-upload-dialog"]').exists()).toBe(true)

    dialog!.querySelector<HTMLButtonElement>('[data-ui="file-upload-cancel"]')?.click()
    await flushPromises()
    expect(operations.cancelUpload).toHaveBeenCalledTimes(1)
    expect(windowRuntime.updateWindowRuntimeState).toHaveBeenCalledWith(
      'file-window',
      expect.objectContaining({
        busy: true,
        blockLevel: 'busy',
        blockReason: '文件正在上传，请完成或取消上传后再关闭窗口或切换节点。',
      }),
    )
    wrapper.unmount()
  })

  it('上传终态允许从模态框关闭结果', async () => {
    Object.assign(operations.uploadTask, {
      visible: true,
      displayName: 'report.txt',
      targetDirectory: '/home',
      status: 'completed',
      totalFiles: 1,
      completedFiles: 1,
      totalBytes: 8,
      transferredBytes: 8,
      progressPercent: 100,
    })
    const wrapper = mount(FileManagerView, {
      props: { payload: { nodeId: 'node-a' } },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
      },
    })
    await flushPromises()

    const dismiss = wrapper.get<HTMLButtonElement>('[data-ui="file-upload-dismiss"]')
    await dismiss.trigger('click')
    await flushPromises()

    expect(operations.dismissUpload).toHaveBeenCalledTimes(1)
    wrapper.unmount()
  })
})
