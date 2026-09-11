import { defineStore } from 'pinia'
import { ref, markRaw, computed, watch, reactive, nextTick } from 'vue'
import { useI18n } from 'vue-i18n'
import { desktopApi } from '@/api/modules/desktop'
import { appsApi } from '@/api/modules/apps'
import { availableApps, type AppConfig, type AppId } from '@/apps/registry'
import { useNodeStore } from '@/stores/node'

// 定义窗口实例的类型
export interface WindowInstance {
  id: string
  /** 应用注册 ID，用于查找节点范围与切换策略。 */
  appId: AppId
  component: ReturnType<typeof markRaw> // 使用 markRaw 包装组件，防止 Vue 深度响应式转换
  title: string
  i18nTitleKey?: string // 存储 i18n key 以便动态更新
  /** VDL 图标名称。 */
  icon: string
  /** 应用窗口内容区右键策略。 */
  contextMenuPolicy: NonNullable<AppConfig['contextMenuPolicy']>
  positionX: number
  positionY: number
  width: number
  height: number
  minWidth: number
  minHeight: number
  isMinimized: boolean
  isMaximized: boolean
  zIndex: number
  payload?: Record<string, unknown>
  /** 窗口创建时固化的节点上下文，避免应用目录刷新后发生语义漂移。 */
  nodeContext: WindowNodeContext
  runtimeState?: WindowRuntimeState
}

export interface WindowNodeContext {
  nodeId?: string
  nodeScope: AppConfig['nodeScope']
  nodeSwitchPolicy: AppConfig['nodeSwitchPolicy']
}

export type NodeSwitchBlockLevel = 'open' | 'dirty' | 'active' | 'busy' | 'unknown'

export interface WindowRuntimeState {
  allowsNodeSwitch?: boolean
  blockLevel?: NodeSwitchBlockLevel
  blockReason?: string
  blockReasonKey?: string
  dirty?: boolean
  activeSession?: boolean
  busy?: boolean
}

export interface WindowGuardBlocker {
  windowId: string
  appId: AppId
  title: string
  level: NodeSwitchBlockLevel
  reason: string
}

export interface WindowGuardResult {
  allowed: boolean
  blockers: WindowGuardBlocker[]
}

export type WindowOperationType = 'drag' | 'resize'

export interface ActiveWindowOperation {
  id: string
  type: WindowOperationType
}

export interface GlobalOperationState {
  operationId: string
  nodeId?: string
  sourceAppId: AppId
  title: string
  busy: boolean
  cancellable: boolean
  reason?: string
  blocksNodeSwitch?: boolean
}

// 定义桌面快捷方式项
export interface DesktopAppShortcut {
  appId: AppId
  x: number
  y: number
}

// 应用组件映射表在独立目录中维护，展示信息由后端 apps 目录覆盖。

const HEADER_HEIGHT = 40 // 头部高度: 40px (vust-header-bar)
const BASE_Z_INDEX = 1000
const GRID_SIZE_X = 100
const GRID_SIZE_Y = 100
const DESKTOP_PADDING = 30
const DESKTOP_ICON_WIDTH = 90
const DESKTOP_ICON_HEIGHT = 80

/**
 * @description 计算应用窗口在 X 轴上的居中位置
 * @param windowWidth 窗口的默认宽度
 * @returns 居中 X 坐标
 */
function calculateCenteredX(windowWidth: number): number {
  // 假设桌面宽度为视口宽度 (window.innerWidth)
  return Math.max(0, window.innerWidth / 2 - windowWidth / 2)
}

/**
 * @description 计算应用窗口在 Y 轴上的居中位置
 * @param windowHeight 窗口的默认高度
 * @returns 居中 Y 坐标
 */
function calculateCenteredY(windowHeight: number): number {
  // 计算【桌面区域】可用的垂直空间
  const availableHeight = window.innerHeight - HEADER_HEIGHT

  // Y 坐标是相对于桌面区域的顶部 (Header 的底部, 即 Y=0)
  const centeredPosition = (availableHeight - windowHeight) / 2

  // 确保 Y 坐标不小于 0 (窗口顶部不能被 Header 遮挡)
  // 如果窗口太高导致 centeredPosition < 0，则返回 0，让窗口从桌面顶部开始显示。
  return Math.max(0, centeredPosition)
}

export const useWindowManagerStore = defineStore('windowManager', () => {
  const { t, locale, messages } = useI18n()
  const nodeStore = useNodeStore()
  // 存储所有打开的窗口实例
  const openWindows = ref<WindowInstance[]>([])
  const globalOperations = ref<GlobalOperationState[]>([])
  const activeWindowOperation = ref<ActiveWindowOperation | null>(null)
  // 使用响应式副本以便后续合并后端应用元数据。
  const availableAppsState = reactive({ ...availableApps }) as Record<AppId, AppConfig>
  const desktopShortcuts = ref<DesktopAppShortcut[]>([])
  const appCatalogOrder = ref<AppId[]>([])
  const hasLoadedDesktopShortcuts = ref(false)
  const isDesktopDragging = ref(false)
  const isApplyingDesktopShortcuts = ref(false)
  let desktopShortcutsLoadPromise: Promise<void> | null = null
  let desktopShortcutsPersistPromise: Promise<void> | null = null
  const defaultDesktopApps: AppId[] = [
    'node-manager',
    'docker-manager',
    'system-monitor',
    'terminal',
    'file-manager',
    'settings',
  ]

  // 侦听语言变化并更新所有已打开窗口的标题与阻止节点切换的描述原因
  watch(locale, () => {
    void loadAppCatalog()
    openWindows.value.forEach((window) => {
      if (window.i18nTitleKey) {
        window.title = t(window.i18nTitleKey)
      }
      if (window.runtimeState && window.runtimeState.blockReasonKey) {
        window.runtimeState.blockReason = t(window.runtimeState.blockReasonKey)
      }
    })
  })

  watch(
    () => nodeStore.currentNodeId,
    () => {
      void refreshDesktopState()
    },
  )

  const maxZIndex = computed(() => {
    return openWindows.value.reduce((max, window) => Math.max(max, window.zIndex), BASE_Z_INDEX)
  })

  if (typeof window !== 'undefined') {
    void loadAppCatalog()
      .catch(() => undefined)
      .finally(() =>
        loadDesktopShortcuts().finally(() => {
          hasLoadedDesktopShortcuts.value = true
        }),
      )
    watch(
      desktopShortcuts,
      () => {
        if (!hasLoadedDesktopShortcuts.value) return
        if (isDesktopDragging.value) return
        if (isApplyingDesktopShortcuts.value) return
        void persistDesktopShortcuts()
      },
      { deep: true },
    )
  }

  type DesktopAppItemPayload = {
    appId: string
    sortOrder: number
    visible: boolean
    x?: number
    y?: number
  }

  function normalizeDesktopAppItems(items: DesktopAppItemPayload[]) {
    const seen = new Set<string>()
    return items.filter((item): item is DesktopAppItemPayload & { appId: AppId } => {
      if (!(item.appId in availableAppsState)) return false
      if (seen.has(item.appId)) return false
      seen.add(item.appId)
      return true
    })
  }

  function buildDesktopItemsPayload() {
    const catalogOrder =
      appCatalogOrder.value.length > 0
        ? appCatalogOrder.value
        : (Object.keys(availableAppsState) as AppId[])

    const shortcutMap = new Map<string, DesktopAppShortcut>()
    desktopShortcuts.value.forEach((s) => {
      shortcutMap.set(s.appId.toString(), s)
    })

    return catalogOrder.map((appId, index) => {
      const shortcut = shortcutMap.get(appId.toString())
      return {
        appId,
        sortOrder: index, // 这里的 sortOrder 在自由布局中意义减弱，但保留 catalog 顺序
        visible: !!shortcut,
        x: shortcut?.x,
        y: shortcut?.y,
      }
    })
  }

  type AppCatalogItem = {
    appId: string
    i18nTitleKey: string
    title?: string
    icon: string
    defaultWidth: number
    defaultHeight: number
    minWidth: number
    minHeight: number
    hidden: boolean
    sourceType?: 'builtin' | 'suite'
    suiteInstanceId?: string
    nodeId?: string
    appEntryId?: string
    entryType?: 'proxied_web' | 'compose_detail'
    entryTarget?: string
  }

  /**
   * @description 将后端应用图标字段规范化为本地图标名称或套件资源 URL。
   * @param appId 应用 ID。
   * @param icon 后端返回的图标名称或资源地址。
   */
  function normalizeAppIconName(appId: AppId, icon: string) {
    const value = icon.trim()
    if (value.startsWith('/') || value.startsWith('http://') || value.startsWith('https://')) {
      return value
    }
    return /^[a-z0-9-]+$/.test(value) ? value : appId
  }

  function resolveAppTitle(config: AppConfig) {
    if (config.i18nTitleKey) return t(config.i18nTitleKey)
    return config.title || ''
  }

  function applyAppCatalog(items: AppCatalogItem[]) {
    const order: AppId[] = []
    const seen = new Set<string>()
    items.forEach((item) => {
      if (seen.has(item.appId)) return
      seen.add(item.appId)
      const appId = item.appId as AppId
      let config = availableAppsState[appId]
      if (!config && item.sourceType === 'suite') {
        const suiteWebApp = availableAppsState['suite-web-app']
        if (!suiteWebApp) return
        config = reactive({
          ...suiteWebApp,
          title: item.title || item.appId,
          i18nTitleKey: '',
          icon: normalizeAppIconName(appId, item.icon || 'package'),
          nodeScope: 'current-node',
          nodeSwitchPolicy: 'block-while-open',
          contextMenuPolicy: 'native',
          sourceType: 'suite',
        }) as AppConfig
        availableAppsState[appId] = config
      }
      if (!config) {
        console.warn(`App component config not found: ${item.appId}`)
        return
      }
      // 后端应用目录只覆盖展示元数据；节点范围与切换策略始终以本地内置注册表为准。
      config.title = item.title || ''
      config.i18nTitleKey = item.i18nTitleKey
      config.icon = normalizeAppIconName(appId, item.icon)
      config.defaultWidth = item.defaultWidth
      config.defaultHeight = item.defaultHeight
      config.minWidth = item.minWidth
      config.minHeight = item.minHeight
      config.hidden = item.hidden
      config.sourceType = item.sourceType ?? 'builtin'
      config.suiteInstanceId = item.suiteInstanceId
      config.nodeId = item.nodeId
      config.appEntryId = item.appEntryId
      config.entryType = item.entryType
      config.entryTarget = item.entryTarget
      order.push(appId)
    })
    appCatalogOrder.value = order
    syncOpenSuiteWindowTitles()
    Object.keys(availableAppsState).forEach((key) => {
      if (!seen.has(key)) {
        const config = availableAppsState[key as AppId]
        if (config) {
          config.hidden = true
        }
      }
    })
  }

  function syncOpenSuiteWindowTitles() {
    openWindows.value.forEach((windowInstance) => {
      const suiteConfig = availableAppsState[windowInstance.id as AppId]
      if (suiteConfig?.sourceType !== 'suite') return
      const title = resolveAppTitle(suiteConfig)
      windowInstance.title = title
      windowInstance.icon = suiteConfig.icon
      windowInstance.payload = {
        ...windowInstance.payload,
        title,
      }
    })
  }

  function getDefaultRuntimeState(
    appConfig: Pick<AppConfig, 'nodeScope' | 'nodeSwitchPolicy'>,
  ): WindowRuntimeState {
    if (appConfig.nodeScope === 'global' || appConfig.nodeSwitchPolicy === 'allow') {
      return { allowsNodeSwitch: true }
    }
    if (appConfig.nodeSwitchPolicy === 'block-while-open') {
      return {
        allowsNodeSwitch: false,
        blockLevel: 'open',
        blockReason: t('guard.nodeAppOpen'),
        blockReasonKey: 'guard.nodeAppOpen',
      }
    }
    return { allowsNodeSwitch: true }
  }

  function resolveRuntimeBlocker(windowInstance: WindowInstance): WindowGuardBlocker | null {
    const runtimeState =
      windowInstance.runtimeState ?? getDefaultRuntimeState(windowInstance.nodeContext)

    const level: NodeSwitchBlockLevel | null =
      runtimeState.blockLevel ??
      (runtimeState.dirty
        ? 'dirty'
        : runtimeState.activeSession
          ? 'active'
          : runtimeState.busy
            ? 'busy'
            : runtimeState.allowsNodeSwitch === false
              ? 'open'
              : null)

    if (!level) return null
    if (runtimeState.allowsNodeSwitch === true && !runtimeState.dirty && !runtimeState.busy) {
      if (!runtimeState.activeSession) return null
    }

    return {
      windowId: windowInstance.id,
      appId: windowInstance.appId,
      title: windowInstance.title,
      level,
      reason:
        runtimeState.blockReason || t('guard.windowUsingNode', { title: windowInstance.title }),
    }
  }

  function findI18nKeyByValue(targetVal: string): string | undefined {
    if (!targetVal) return undefined

    const dicts =
      messages && typeof messages === 'object' && 'value' in messages
        ? (messages.value as Record<string, unknown>)
        : (messages as Record<string, unknown>)
    const currentLocale =
      locale && typeof locale === 'object' && 'value' in locale
        ? (locale.value as string)
        : (locale as string)

    const dict = dicts ? (dicts[currentLocale] as Record<string, unknown>) : undefined
    if (!dict) return undefined

    function search(obj: Record<string, unknown>, currentPath: string): string | undefined {
      for (const key in obj) {
        if (Object.prototype.hasOwnProperty.call(obj, key)) {
          const val = obj[key]
          const newPath = currentPath ? `${currentPath}.${key}` : key
          if (typeof val === 'string') {
            if (val.trim() === targetVal.trim()) {
              return newPath
            }
          } else if (typeof val === 'object' && val !== null) {
            const found = search(val as Record<string, unknown>, newPath)
            if (found) return found
          }
        }
      }
      return undefined
    }

    return search(dict, '')
  }

  /**
   * @description 更新窗口运行态，供节点切换与关闭守卫统一判断。
   * @param id 窗口实例 ID。
   * @param state 运行态增量。
   */
  function updateWindowRuntimeState(id: string, state: WindowRuntimeState) {
    const windowInstance = openWindows.value.find((w) => w.id === id)
    if (!windowInstance) return

    let blockReasonKey = state.blockReasonKey
    if (state.blockReason && !blockReasonKey) {
      blockReasonKey = findI18nKeyByValue(state.blockReason)
    }

    windowInstance.runtimeState = {
      ...windowInstance.runtimeState,
      ...state,
      blockReasonKey: blockReasonKey ?? windowInstance.runtimeState?.blockReasonKey,
    }
  }

  function resolveGlobalOperationBlocker(
    operation: GlobalOperationState,
    options?: { ignoreNodeScope?: boolean },
  ): WindowGuardBlocker | null {
    if (!operation.busy || operation.blocksNodeSwitch === false) return null

    const nodeStore = useNodeStore()
    const shouldBlock =
      options?.ignoreNodeScope ||
      !operation.nodeId ||
      !nodeStore.currentNodeId ||
      operation.nodeId === nodeStore.currentNodeId
    if (!shouldBlock) return null

    return {
      windowId: `global-operation:${operation.operationId}`,
      appId: operation.sourceAppId,
      title: operation.title,
      level: 'busy',
      reason:
        operation.reason ||
        (operation.cancellable ? t('guard.backgroundCancellable') : t('guard.backgroundBusy')),
    }
  }

  /**
   * @description 登记窗口外或可能脱离窗口生命周期的全局运行中操作。
   * @param operation 操作状态。
   */
  function registerGlobalOperation(
    operation: Omit<GlobalOperationState, 'busy'> & { busy?: boolean },
  ) {
    const nextOperation: GlobalOperationState = {
      busy: true,
      blocksNodeSwitch: true,
      ...operation,
    }
    const existingIndex = globalOperations.value.findIndex(
      (item) => item.operationId === nextOperation.operationId,
    )
    if (existingIndex >= 0) {
      globalOperations.value[existingIndex] = {
        ...globalOperations.value[existingIndex],
        ...nextOperation,
      }
    } else {
      globalOperations.value = [...globalOperations.value, nextOperation]
    }
  }

  /**
   * @description 更新已登记的全局运行中操作。
   * @param operationId 操作 ID。
   * @param patch 操作状态增量。
   */
  function updateGlobalOperation(
    operationId: string,
    patch: Partial<Omit<GlobalOperationState, 'operationId'>>,
  ) {
    const operation = globalOperations.value.find((item) => item.operationId === operationId)
    if (!operation) return
    Object.assign(operation, patch)
  }

  /**
   * @description 清理已完成、失败或取消的全局运行中操作。
   * @param operationId 操作 ID。
   */
  function finishGlobalOperation(operationId: string) {
    globalOperations.value = globalOperations.value.filter(
      (item) => item.operationId !== operationId,
    )
  }

  /**
   * @description 检查当前是否允许切换全局节点。
   */
  function checkBeforeNodeSwitch(): WindowGuardResult {
    const windowBlockers = openWindows.value
      .map((windowInstance) => resolveRuntimeBlocker(windowInstance))
      .filter((item): item is WindowGuardBlocker => item !== null)
    const operationBlockers = globalOperations.value
      .map((operation) => resolveGlobalOperationBlocker(operation, { ignoreNodeScope: true }))
      .filter((item): item is WindowGuardBlocker => item !== null)
    const blockers = [...windowBlockers, ...operationBlockers]
    return {
      allowed: blockers.length === 0,
      blockers,
    }
  }

  /**
   * @description 检查窗口是否存在关闭前风险。
   * @param id 窗口实例 ID。
   */
  function checkBeforeCloseWindow(id: string): WindowGuardResult {
    const windowInstance = openWindows.value.find((w) => w.id === id)
    if (!windowInstance) {
      return { allowed: true, blockers: [] }
    }
    const runtimeState = windowInstance.runtimeState
    if (!runtimeState?.dirty && !runtimeState?.busy) {
      return { allowed: true, blockers: [] }
    }
    const blocker = resolveRuntimeBlocker(windowInstance)
    const isCloseBlocker = blocker && (blocker.level === 'dirty' || blocker.level === 'busy')
    return {
      allowed: !isCloseBlocker,
      blockers: isCloseBlocker ? [blocker] : [],
    }
  }

  /**
   * @description 检查全局关闭、退出登录或刷新页面前是否存在风险。
   */
  function checkBeforeCloseAll(): WindowGuardResult {
    const windowBlockers = openWindows.value.flatMap(
      (windowInstance) => checkBeforeCloseWindow(windowInstance.id).blockers,
    )
    const operationBlockers = globalOperations.value
      .map((operation) => resolveGlobalOperationBlocker(operation))
      .filter((item): item is WindowGuardBlocker => item !== null)
    const blockers = [...windowBlockers, ...operationBlockers]
    return {
      allowed: blockers.length === 0,
      blockers,
    }
  }

  async function loadAppCatalog() {
    try {
      const response = await appsApi.fetchApps(nodeStore.currentNodeId || 'local')
      if (response.success && response.data) {
        applyAppCatalog(response.data)
      }
    } catch (error) {
      console.warn('Failed to load app catalog', error)
    }
  }

  function getDesktopGridSize() {
    const desktopHeight = window.innerHeight - HEADER_HEIGHT
    const columns = Math.max(
      1,
      Math.floor((window.innerWidth - DESKTOP_PADDING * 2 - DESKTOP_ICON_WIDTH) / GRID_SIZE_X) + 1,
    )
    const rows = Math.max(
      1,
      Math.floor((desktopHeight - DESKTOP_PADDING * 2 - DESKTOP_ICON_HEIGHT) / GRID_SIZE_Y) + 1,
    )
    return { columns, rows }
  }

  function gridPosition(col: number, row: number) {
    return {
      x: DESKTOP_PADDING + col * GRID_SIZE_X,
      y: DESKTOP_PADDING + row * GRID_SIZE_Y,
    }
  }

  /**
   * @description 按桌面系统习惯查找空位：先从上到下，再从左到右。
   * @param occupiedShortcuts 已占用位置的快捷方式。
   */
  function findNextDesktopShortcutPosition(occupiedShortcuts: DesktopAppShortcut[]) {
    const { columns, rows } = getDesktopGridSize()
    const occupied = new Set(occupiedShortcuts.map((shortcut) => `${shortcut.x}:${shortcut.y}`))
    for (let col = 0; col < columns; col += 1) {
      for (let row = 0; row < rows; row += 1) {
        const position = gridPosition(col, row)
        if (!occupied.has(`${position.x}:${position.y}`)) {
          return position
        }
      }
    }
    const overflowIndex = Math.max(0, occupiedShortcuts.length - columns * rows)
    return gridPosition(columns + Math.floor(overflowIndex / rows), overflowIndex % rows)
  }

  function generateDefaultLayout(ids: AppId[]): DesktopAppShortcut[] {
    const { rows } = getDesktopGridSize()
    return ids.map((appId, index) => {
      const col = Math.floor(index / rows)
      const row = index % rows
      return {
        appId,
        ...gridPosition(col, row),
      }
    })
  }

  async function loadDesktopShortcuts() {
    if (desktopShortcutsLoadPromise) return desktopShortcutsLoadPromise
    desktopShortcutsLoadPromise = loadDesktopShortcutsInner().finally(() => {
      desktopShortcutsLoadPromise = null
    })
    return desktopShortcutsLoadPromise
  }

  async function loadDesktopShortcutsInner() {
    try {
      const currentNodeId = nodeStore.currentNodeId || 'local'
      const response = await desktopApi.fetchDesktopApps(currentNodeId)
      if (response.success && response.data) {
        const filtered = normalizeDesktopAppItems(response.data.apps)
        if (response.data.hasRecord) {
          const visible = filtered.filter(
            (item) => item.visible && !availableAppsState[item.appId]?.hidden,
          )
          // 检查是否有缺失坐标的项，如果有，尝试布局它们
          const shortcuts: DesktopAppShortcut[] = []
          const needsLayout: AppId[] = []

          visible.forEach((item) => {
            if (item.x !== undefined && item.y !== undefined) {
              shortcuts.push({ appId: item.appId, x: item.x, y: item.y })
            } else {
              needsLayout.push(item.appId)
            }
          })

          // 自愈机制：自动检测出当前可用但在数据库记录中完全缺失的新增应用。
          // 套件启用后后端只注册应用入口，桌面坐标由这里复用统一补位算法生成。
          const existingAppIds = new Set(filtered.map((item) => item.appId))
          const visibleCatalogIds = (Object.keys(availableAppsState) as AppId[]).filter(
            (appId) => !availableAppsState[appId].hidden,
          )
          visibleCatalogIds.forEach((appId) => {
            const alreadyOnDesktop = shortcuts.some((shortcut) => shortcut.appId === appId)
            if (!alreadyOnDesktop && !existingAppIds.has(appId)) {
              needsLayout.push(appId)
            }
          })

          if (needsLayout.length > 0) {
            needsLayout.forEach((appId) => {
              const position = findNextDesktopShortcutPosition(shortcuts)
              const shortcut = { appId, ...position }
              shortcuts.push(shortcut)
            })
            await applyDesktopShortcuts(shortcuts)
            await persistDesktopShortcuts()
          } else {
            await applyDesktopShortcuts(shortcuts)
          }
        } else {
          const defaultIds = defaultDesktopApps.filter((appId) => appId in availableAppsState)
          await applyDesktopShortcuts(
            generateDefaultLayout(
              defaultIds.length > 0
                ? defaultIds
                : (Object.keys(availableAppsState) as AppId[]).filter(
                    (id) => !availableAppsState[id].hidden,
                  ),
            ),
          )
          await persistDesktopShortcuts()
        }
      } else if (desktopShortcuts.value.length === 0) {
        const defaultIds = defaultDesktopApps.filter((appId) => appId in availableAppsState)
        await applyDesktopShortcuts(generateDefaultLayout(defaultIds))
        await persistDesktopShortcuts()
      }
    } catch (error) {
      console.warn('Failed to load desktop shortcuts', error)
      if (desktopShortcuts.value.length === 0) {
        const defaultIds = defaultDesktopApps.filter((appId) => appId in availableAppsState)
        await applyDesktopShortcuts(generateDefaultLayout(defaultIds))
      }
    }
  }

  async function applyDesktopShortcuts(shortcuts: DesktopAppShortcut[]) {
    isApplyingDesktopShortcuts.value = true
    desktopShortcuts.value = shortcuts
    await nextTick()
    isApplyingDesktopShortcuts.value = false
  }

  /**
   * @description 刷新应用目录和桌面快捷方式，用于套件启停卸载后同步桌面状态。
   */
  async function refreshDesktopState() {
    await loadAppCatalog()
    await loadDesktopShortcuts()
  }

  async function persistDesktopShortcuts() {
    if (desktopShortcutsPersistPromise) return desktopShortcutsPersistPromise
    desktopShortcutsPersistPromise = persistDesktopShortcutsInner().finally(() => {
      desktopShortcutsPersistPromise = null
    })
    return desktopShortcutsPersistPromise
  }

  async function persistDesktopShortcutsInner() {
    try {
      await desktopApi.saveDesktopApps({
        nodeId: nodeStore.currentNodeId || 'local',
        apps: buildDesktopItemsPayload(),
      })
    } catch (error) {
      console.warn('Failed to save desktop shortcuts', error)
    }
  }

  function beginDesktopReorder() {
    isDesktopDragging.value = true
  }

  function finishDesktopReorder() {
    isDesktopDragging.value = false
    if (hasLoadedDesktopShortcuts.value) {
      void persistDesktopShortcuts()
    }
  }

  /**
   * @description 批量设置桌面快捷方式。
   * @param shortcuts 目标列表。
   */
  function setDesktopShortcuts(shortcuts: DesktopAppShortcut[]) {
    desktopShortcuts.value = shortcuts
  }

  /**
   * @description 添加桌面快捷方式。
   * @param id 目标应用 ID。
   */
  function addDesktopShortcut(id: AppId) {
    if (!desktopShortcuts.value.find((s) => s.appId === id)) {
      desktopShortcuts.value.push({
        appId: id,
        ...findNextDesktopShortcutPosition(desktopShortcuts.value),
      })
    }
  }

  /**
   * @description 删除桌面快捷方式。
   * @param id 要删除的应用 ID。
   */
  function removeDesktopShortcut(id: AppId) {
    desktopShortcuts.value = desktopShortcuts.value.filter((s) => s.appId !== id)
  }

  /**
   * @description 更新桌面快捷方式位置。
   * @param id 应用 ID。
   * @param x X 坐标。
   * @param y Y 坐标。
   */
  function updateDesktopShortcutPosition(id: AppId, x: number, y: number) {
    const shortcut = desktopShortcuts.value.find((s) => s.appId === id)
    if (shortcut) {
      shortcut.x = x
      shortcut.y = y
    }
  }

  /**
   * @description 自动排列桌面快捷方式。
   */
  function autoArrangeDesktopShortcuts() {
    const currentIds = desktopShortcuts.value.map((s) => s.appId)
    if (currentIds.length === 0) return
    void applyDesktopShortcuts(generateDefaultLayout(currentIds)).then(() =>
      persistDesktopShortcuts(),
    )
  }

  // --- Actions ---
  /**
   * @description 切换窗口的最大化状态。
   * @param id 窗口的唯一 ID。
   */
  function toggleMaximize(id: string) {
    const windowInstance = openWindows.value.find((w) => w.id === id)
    if (windowInstance) {
      windowInstance.isMaximized = !windowInstance.isMaximized
    }
  }

  /**
   * @description 打开或聚焦一个应用窗口。
   * @param id 窗口的唯一 ID。
   */
  function openWindowWithPayload(
    id: AppId,
    payload?: Record<string, unknown>,
    options?: {
      instanceId?: string
      title?: string
      icon?: string
      width?: number
      height?: number
      minWidth?: number
      minHeight?: number
      i18nTitleKey?: string
      nodeId?: string
      nodeScope?: AppConfig['nodeScope']
      nodeSwitchPolicy?: AppConfig['nodeSwitchPolicy']
    },
  ) {
    const instanceId = options?.instanceId ?? id
    const windowInstance = openWindows.value.find((w) => w.id === instanceId)
    const appConfig = availableAppsState[id]
    if (!appConfig) {
      console.error(`App config not found for id: ${id}`)
      return
    }
    const title = options?.title ?? resolveAppTitle(appConfig)
    const icon = options?.icon ?? appConfig.icon
    const minWidth = options?.minWidth ?? appConfig.minWidth
    const minHeight = options?.minHeight ?? appConfig.minHeight
    const i18nTitleKey =
      options?.i18nTitleKey ??
      (options?.title || !appConfig.i18nTitleKey ? undefined : appConfig.i18nTitleKey)
    const contextMenuPolicy = appConfig.contextMenuPolicy ?? 'shell'
    const payloadNodeId = typeof payload?.nodeId === 'string' ? payload.nodeId : undefined
    const nodeContext: WindowNodeContext = {
      nodeId: options?.nodeId ?? payloadNodeId,
      nodeScope: options?.nodeScope ?? appConfig.nodeScope,
      nodeSwitchPolicy: options?.nodeSwitchPolicy ?? appConfig.nodeSwitchPolicy,
    }

    if (windowInstance) {
      // 窗口已存在：恢复并聚焦
      if (windowInstance.isMinimized) {
        windowInstance.isMinimized = false
      }
      if (payload) {
        windowInstance.payload = payload
      }
      windowInstance.title = title
      windowInstance.icon = icon
      windowInstance.i18nTitleKey = i18nTitleKey
      windowInstance.minWidth = minWidth
      windowInstance.minHeight = minHeight
      windowInstance.contextMenuPolicy = contextMenuPolicy
      windowInstance.nodeContext = nodeContext
      if (options?.width) {
        windowInstance.width = Math.max(options.width, minWidth)
      } else if (windowInstance.width < minWidth) {
        windowInstance.width = minWidth
      }
      if (options?.height) {
        windowInstance.height = Math.max(options.height, minHeight)
      } else if (windowInstance.height < minHeight) {
        windowInstance.height = minHeight
      }
      focusWindow(instanceId)
    } else {
      // 窗口不存在：创建新窗口
      const newZIndex = maxZIndex.value + 1

      const width = Math.max(options?.width ?? appConfig.defaultWidth, minWidth)
      const height = Math.max(options?.height ?? appConfig.defaultHeight, minHeight)

      const baseX = calculateCenteredX(width)
      const baseY = calculateCenteredY(height)
      const offsetStep = 26
      const offsetIndex = openWindows.value.length % 8
      const offset = offsetIndex * offsetStep
      const desktopAreaHeight = window.innerHeight - HEADER_HEIGHT
      const maxX = Math.max(0, window.innerWidth - width)
      const maxY = Math.max(0, desktopAreaHeight - height)
      const startX = Math.max(0, Math.min(maxX, baseX + offset))
      const startY = Math.max(0, Math.min(maxY, baseY + offset))

      const newWindow: WindowInstance = {
        id: instanceId,
        appId: id,
        component: appConfig.component,
        title,
        i18nTitleKey,
        icon,
        contextMenuPolicy,
        positionX: startX,
        positionY: startY,
        width,
        height,
        minWidth,
        minHeight,
        isMinimized: false,
        isMaximized: false,
        zIndex: newZIndex,
        payload,
        nodeContext,
        runtimeState: getDefaultRuntimeState(nodeContext),
      }
      openWindows.value = [...openWindows.value, newWindow]
    }
  }

  function openWindow(id: AppId) {
    const appConfig = availableAppsState[id]
    if (appConfig?.sourceType === 'suite') {
      if (appConfig.entryType === 'proxied_web' && appConfig.entryTarget) {
        openWindowWithPayload(
          'suite-web-app',
          {
            url: appConfig.entryTarget,
            title: resolveAppTitle(appConfig),
            suiteInstanceId: appConfig.suiteInstanceId,
            nodeId: appConfig.nodeId,
            appEntryId: appConfig.appEntryId,
          },
          {
            instanceId: id,
            title: resolveAppTitle(appConfig),
            icon: appConfig.icon,
            width: appConfig.defaultWidth,
            height: appConfig.defaultHeight,
            minWidth: appConfig.minWidth,
            minHeight: appConfig.minHeight,
            nodeId: appConfig.nodeId,
            nodeScope: appConfig.nodeScope,
            nodeSwitchPolicy: appConfig.nodeSwitchPolicy,
          },
        )
        return
      }
      if (appConfig.entryType === 'compose_detail' && appConfig.entryTarget) {
        openWindowWithPayload('docker-manager', {
          composeProject: appConfig.entryTarget,
          nodeId: appConfig.nodeId || nodeStore.currentNodeId || 'local',
        })
        return
      }
    }
    if (id === 'terminal') {
      const timestamp = Date.now()
      const instanceId = `terminal-${timestamp}`

      // 创建窗口时固定目标节点，窗口标题只表达应用身份。
      const nodeStore = useNodeStore()
      const targetNodeId = nodeStore.currentNodeId || 'local'
      const activeNode = nodeStore.nodes.find((n) => n.id === targetNodeId)
      const nodeName = targetNodeId === 'local' ? 'local' : activeNode?.name || targetNodeId

      openWindowWithPayload(
        id,
        {
          nodeId: targetNodeId,
          nodeName,
        },
        {
          instanceId,
          title: t('app.terminal.appName'),
          i18nTitleKey: 'app.terminal.appName',
        },
      )
    } else {
      const currentNodePayload =
        appConfig?.nodeScope === 'current-node'
          ? { nodeId: nodeStore.currentNodeId || 'local' }
          : undefined
      openWindowWithPayload(id, currentNodePayload)
    }
  }

  /**
   * @description 聚焦指定窗口，将其 Z-index 提到最高。
   * @param id 窗口的唯一 ID。
   */
  function focusWindow(id: string) {
    const windowToFocus = openWindows.value.find((w) => w.id === id)
    if (windowToFocus) {
      const newZIndex = maxZIndex.value + 1
      windowToFocus.zIndex = newZIndex
    }
  }

  /**
   * @description 最小化窗口到 Dock。
   * @param id 窗口的唯一 ID。
   */
  function minimizeWindow(id: string) {
    const windowInstance = openWindows.value.find((w) => w.id === id)
    if (windowInstance) {
      windowInstance.isMinimized = true
    }
  }

  /**
   * @description 关闭窗口。
   * @param id 窗口的唯一 ID。
   */
  function closeWindow(id: string) {
    openWindows.value = openWindows.value.filter((w) => w.id !== id)
  }

  /**
   * @description 更新窗口的位置 (拖动)。
   * @param id 窗口的唯一 ID。
   * @param x 新的 X 坐标。
   * @param y 新的 Y 坐标。
   */
  function updatePosition(id: string, x: number, y: number) {
    const windowInstance = openWindows.value.find((w) => w.id === id)
    if (windowInstance) {
      windowInstance.positionX = x
      windowInstance.positionY = y
    }
  }

  /**
   * @description 更新窗口的大小 (拖动改变尺寸)。
   * @param id 窗口的唯一 ID。
   * @param width 新的宽度。
   * @param height 新的高度。
   */
  function updateSize(id: string, width: number, height: number) {
    const windowInstance = openWindows.value.find((w) => w.id === id)
    if (windowInstance) {
      windowInstance.width = width
      windowInstance.height = height
    }
  }

  function closeWindowsBySuiteInstanceId(suiteInstanceId: string) {
    openWindows.value = openWindows.value.filter((w) => {
      const appConfig = availableAppsState[w.appId]
      if (appConfig?.suiteInstanceId === suiteInstanceId) return false
      if (w.payload?.suiteInstanceId === suiteInstanceId) return false
      if (w.id.includes(suiteInstanceId) || w.appId.includes(suiteInstanceId)) return false
      return true
    })
  }

  function beginWindowOperation(id: string, type: WindowOperationType) {
    activeWindowOperation.value = { id, type }
  }

  function finishWindowOperation(id: string) {
    if (activeWindowOperation.value?.id === id) {
      activeWindowOperation.value = null
    }
  }

  return {
    openWindows,
    globalOperations,
    activeWindowOperation,
    availableApps: availableAppsState,
    maxZIndex,
    desktopShortcuts,
    appCatalogOrder,
    loadAppCatalog,
    refreshDesktopState,
    openWindow,
    openWindowWithPayload,
    closeWindow,
    closeWindowsBySuiteInstanceId,
    minimizeWindow,
    focusWindow,
    updatePosition,
    updateSize,
    beginWindowOperation,
    finishWindowOperation,
    toggleMaximize,
    addDesktopShortcut,
    removeDesktopShortcut,
    updateDesktopShortcutPosition,
    autoArrangeDesktopShortcuts,
    setDesktopShortcuts,
    beginDesktopReorder,
    finishDesktopReorder,
    updateWindowRuntimeState,
    registerGlobalOperation,
    updateGlobalOperation,
    finishGlobalOperation,
    checkBeforeNodeSwitch,
    checkBeforeCloseWindow,
    checkBeforeCloseAll,
    resolveAppTitle,
  }
})
