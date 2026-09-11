import { defineComponent, nextTick } from 'vue'
import { createPinia } from 'pinia'
import { createI18n } from 'vue-i18n'
import { mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useNodeStore } from '@/stores/node'
import { useWindowManagerStore } from '@/stores/window-manager'
import zh from '@/locales/zh'
import en from '@/locales/en'

const fetchAppsMock = vi.hoisted(() => vi.fn())

vi.mock('@/api/modules/apps', () => ({
  appsApi: {
    fetchApps: fetchAppsMock,
  },
}))

vi.mock('@/api/modules/desktop', () => ({
  desktopApi: {
    fetchDesktopApps: vi.fn().mockResolvedValue({ success: false, data: null }),
    saveDesktopApps: vi.fn().mockResolvedValue({ success: true, data: null }),
  },
}))

const Harness = defineComponent({
  setup() {
    return { windowStore: useWindowManagerStore() }
  },
  template: '<div />',
})

beforeEach(() => {
  localStorage.clear()
  fetchAppsMock.mockReset()
  fetchAppsMock.mockResolvedValue({ success: false, data: null })
})

describe('终端窗口节点绑定', () => {
  it('固定节点 payload，标题只显示可国际化的应用名', async () => {
    const pinia = createPinia()
    const i18n = createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })
    const wrapper = mount(Harness, {
      global: {
        plugins: [pinia, i18n],
      },
    })
    const nodeStore = useNodeStore(pinia)
    nodeStore.setNodes([
      { id: 'node-a', name: '节点 A', address: '', status: 'online', tags: [] },
      { id: 'node-b', name: '节点 B', address: '', status: 'online', tags: [] },
    ])
    nodeStore.setCurrentNode('node-a')

    wrapper.vm.windowStore.openWindow('terminal')
    const terminalWindow = wrapper.vm.windowStore.openWindows[0]
    expect(terminalWindow?.payload).toEqual({ nodeId: 'node-a', nodeName: '节点 A' })
    expect(terminalWindow?.title).toBe('终端')
    expect(wrapper.vm.windowStore.checkBeforeNodeSwitch().allowed).toBe(false)

    nodeStore.setCurrentNode('node-b')
    expect(terminalWindow?.payload).toEqual({ nodeId: 'node-a', nodeName: '节点 A' })

    i18n.global.locale.value = 'en'
    await nextTick()
    expect(terminalWindow?.title).toBe('Terminal')
    wrapper.unmount()
  })
})

describe('套件窗口节点切换守卫', () => {
  it('固化代理 Web 套件的节点上下文，并在窗口关闭前阻止切换', async () => {
    fetchAppsMock.mockResolvedValue({
      success: true,
      data: [
        {
          appId: 'suite-a-dashboard',
          i18nTitleKey: '',
          title: '节点 A 套件',
          icon: 'package',
          defaultWidth: 960,
          defaultHeight: 640,
          minWidth: 640,
          minHeight: 420,
          hidden: false,
          sortOrder: 100,
          sourceType: 'suite',
          suiteInstanceId: 'suite-instance-a',
          nodeId: 'node-a',
          appEntryId: 'dashboard',
          entryType: 'proxied_web',
          entryTarget: '/api/v1/suites/proxy/suite-instance-a/dashboard/',
        },
      ],
    })

    const pinia = createPinia()
    const i18n = createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })
    const wrapper = mount(Harness, { global: { plugins: [pinia, i18n] } })
    const nodeStore = useNodeStore(pinia)
    nodeStore.setNodes([
      { id: 'node-a', name: '节点 A', address: '', status: 'online', tags: [] },
      { id: 'node-b', name: '节点 B', address: '', status: 'online', tags: [] },
    ])
    nodeStore.setCurrentNode('node-a')
    await wrapper.vm.windowStore.loadAppCatalog()

    wrapper.vm.windowStore.openWindow('suite-a-dashboard')
    const suiteWindow = wrapper.vm.windowStore.openWindows[0]
    expect(suiteWindow?.appId).toBe('suite-web-app')
    expect(suiteWindow?.title).toBe('节点 A 套件')
    expect(suiteWindow?.nodeContext).toEqual({
      nodeId: 'node-a',
      nodeScope: 'current-node',
      nodeSwitchPolicy: 'block-while-open',
    })

    let guard = wrapper.vm.windowStore.checkBeforeNodeSwitch()
    expect(guard.allowed).toBe(false)
    expect(guard.blockers).toMatchObject([
      {
        windowId: 'suite-a-dashboard',
        title: '节点 A 套件',
        level: 'open',
      },
    ])

    wrapper.vm.windowStore.minimizeWindow('suite-a-dashboard')
    expect(wrapper.vm.windowStore.checkBeforeNodeSwitch().allowed).toBe(false)

    wrapper.vm.windowStore.closeWindow('suite-a-dashboard')
    guard = wrapper.vm.windowStore.checkBeforeNodeSwitch()
    expect(guard).toEqual({ allowed: true, blockers: [] })
    wrapper.unmount()
  })

  it('保持套件中心和显式节点应用可切换，并继续阻止 Compose 套件窗口', async () => {
    fetchAppsMock.mockResolvedValue({
      success: true,
      data: [
        {
          appId: 'suite-a-compose',
          i18nTitleKey: '',
          title: '节点 A Compose',
          icon: 'package',
          defaultWidth: 960,
          defaultHeight: 640,
          minWidth: 640,
          minHeight: 420,
          hidden: false,
          sortOrder: 100,
          sourceType: 'suite',
          suiteInstanceId: 'suite-instance-a',
          nodeId: 'node-a',
          appEntryId: 'compose',
          entryType: 'compose_detail',
          entryTarget: 'suite-project-a',
        },
      ],
    })

    const pinia = createPinia()
    const i18n = createI18n({ legacy: false, locale: 'zh', messages: { zh, en } })
    const wrapper = mount(Harness, { global: { plugins: [pinia, i18n] } })
    const nodeStore = useNodeStore(pinia)
    nodeStore.setNodes([
      { id: 'node-a', name: '节点 A', address: '', status: 'online', tags: [] },
      { id: 'node-b', name: '节点 B', address: '', status: 'online', tags: [] },
    ])
    nodeStore.setCurrentNode('node-a')
    await wrapper.vm.windowStore.loadAppCatalog()

    wrapper.vm.windowStore.openWindow('suite-center')
    wrapper.vm.windowStore.openWindow('runtime-log')
    expect(wrapper.vm.windowStore.checkBeforeNodeSwitch().allowed).toBe(true)

    wrapper.vm.windowStore.openWindow('suite-a-compose')
    const dockerWindow = wrapper.vm.windowStore.openWindows.find(
      (item) => item.appId === 'docker-manager',
    )
    expect(dockerWindow?.payload).toEqual({
      composeProject: 'suite-project-a',
      nodeId: 'node-a',
    })
    expect(wrapper.vm.windowStore.checkBeforeNodeSwitch().allowed).toBe(false)
    wrapper.unmount()
  })
})
