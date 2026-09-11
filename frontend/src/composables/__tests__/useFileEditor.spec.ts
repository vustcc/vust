import { defineComponent, reactive, ref } from 'vue'
import { createI18n } from 'vue-i18n'
import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useFileEditor, type EditorTab } from '@/composables/useFileEditor'
import zh from '@/locales/zh'
import type { FileDocument, FileSaveResult } from '@/api/generated'

const api = vi.hoisted(() => ({
  readFile: vi.fn(),
  writeFile: vi.fn(),
  forNode: vi.fn(),
}))
const notifications = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  warning: vi.fn(),
}))

vi.mock('@/api/modules/fs', () => ({ fsApi: { forNode: api.forNode } }))
vi.mock('@/stores/toast', () => ({ useToastStore: () => notifications }))

const capabilities: FileDocument['capabilities'] = {
  canOpen: true,
  canRead: true,
  canWrite: true,
  canCreateChild: false,
  canRename: true,
  canCopy: true,
  canRemove: true,
  canUpload: false,
  canDownload: true,
}

const document = (content: string, revision = 'revision-1'): FileDocument => ({
  path: '/tmp/file.txt',
  content,
  encoding: 'utf8',
  sizeBytes: content.length,
  revision,
  loadedAt: '2026-07-16T00:00:00.000Z',
  capabilities,
})

const response = <T>(data: T) => ({ success: true, code: 200, message: '', data })

const tab = (): EditorTab => ({
  path: '/tmp/file.txt',
  name: 'file.txt',
  documentKey: 'node-a:/tmp/file.txt',
  content: '',
  originalContent: '',
  revision: '',
  isDirty: false,
  fileSize: 0,
  loadState: 'idle',
  saveState: 'idle',
  loadSequence: 0,
  saveSequence: 0,
})

const deferred = <T>() => {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((complete) => {
    resolve = complete
  })
  return { promise, resolve }
}

const Harness = defineComponent({
  setup() {
    const nodeId = ref('node-a')
    const editorTab = reactive(tab())
    return { editorTab, ...useFileEditor(nodeId) }
  },
  template: '<div />',
})

const mountHarness = () =>
  mount(Harness, {
    global: {
      plugins: [createI18n({ legacy: false, locale: 'zh', messages: { zh } })],
    },
  })

describe('useFileEditor', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    api.forNode.mockReturnValue({ readFile: api.readFile, writeFile: api.writeFile })
    api.readFile.mockResolvedValue(response(document('initial')))
  })

  it('刷新失败时保留已有内容并进入 stale', async () => {
    const wrapper = mountHarness()
    await wrapper.vm.loadFileContent(wrapper.vm.editorTab)
    api.readFile.mockResolvedValueOnce({
      success: false,
      code: 503,
      message: 'node unavailable',
      data: null,
    })

    await wrapper.vm.loadFileContent(wrapper.vm.editorTab, { discardLocalChanges: true })

    expect(wrapper.vm.editorTab.content).toBe('initial')
    expect(wrapper.vm.editorTab.loadState).toBe('stale')
    expect(wrapper.vm.editorTab.refreshWarning).toBe('node unavailable')
  })

  it('revision 冲突保留本地内容并禁止重复保存', async () => {
    const wrapper = mountHarness()
    await wrapper.vm.loadFileContent(wrapper.vm.editorTab)
    wrapper.vm.editorTab.content = 'local change'
    wrapper.vm.editorTab.isDirty = true
    api.writeFile.mockResolvedValue({
      success: false,
      code: 409,
      message: 'file changed',
      errorCode: 'FILE_CHANGED',
      data: null,
    })

    await wrapper.vm.saveFileContent(wrapper.vm.editorTab)
    await wrapper.vm.saveFileContent(wrapper.vm.editorTab)

    expect(wrapper.vm.editorTab.content).toBe('local change')
    expect(wrapper.vm.editorTab.saveState).toBe('conflict')
    expect(api.writeFile).toHaveBeenCalledTimes(1)
  })

  it('保存期间继续输入时只提交快照并保持新输入为 dirty', async () => {
    const wrapper = mountHarness()
    await wrapper.vm.loadFileContent(wrapper.vm.editorTab)
    wrapper.vm.editorTab.content = 'saved snapshot'
    wrapper.vm.editorTab.isDirty = true
    const pending = deferred<ReturnType<typeof response<FileSaveResult>>>()
    api.writeFile.mockReturnValueOnce(pending.promise)

    const saving = wrapper.vm.saveFileContent(wrapper.vm.editorTab)
    wrapper.vm.editorTab.content = 'newer input'
    pending.resolve(
      response({ document: document('saved snapshot', 'revision-2'), durability: 'durable' }),
    )
    await saving

    expect(api.writeFile).toHaveBeenCalledWith(
      expect.objectContaining({ content: 'saved snapshot', expectedRevision: 'revision-1' }),
    )
    expect(wrapper.vm.editorTab.originalContent).toBe('saved snapshot')
    expect(wrapper.vm.editorTab.content).toBe('newer input')
    expect(wrapper.vm.editorTab.isDirty).toBe(true)
  })

  it('网络错误后读取到保存快照时核对为成功', async () => {
    const wrapper = mountHarness()
    await wrapper.vm.loadFileContent(wrapper.vm.editorTab)
    wrapper.vm.editorTab.content = 'saved after disconnect'
    wrapper.vm.editorTab.isDirty = true
    api.writeFile.mockRejectedValueOnce(new Error('connection reset'))
    api.readFile.mockResolvedValueOnce(response(document('saved after disconnect', 'revision-2')))

    await wrapper.vm.saveFileContent(wrapper.vm.editorTab)
    await flushPromises()

    expect(wrapper.vm.editorTab.saveState).toBe('idle')
    expect(wrapper.vm.editorTab.isDirty).toBe(false)
    expect(wrapper.vm.editorTab.revision).toBe('revision-2')
    expect(notifications.success).toHaveBeenCalled()
  })
})
