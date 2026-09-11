import { defineAsyncComponent, markRaw } from 'vue'

/**
 * @description 应用目录/组件解析器，集中维护 app_id 与组件的映射关系。
 */
export interface AppConfig {
  component: ReturnType<typeof defineAsyncComponent>
  title: string
  i18nTitleKey: string
  /** VDL 图标名称，默认与应用 ID 一致。 */
  icon: string
  /** 应用与节点上下文的关系。 */
  nodeScope: 'global' | 'current-node' | 'explicit-node'
  /** 节点切换时的默认守卫策略。 */
  nodeSwitchPolicy: 'allow' | 'block-while-open' | 'block-when-active' | 'block-when-dirty'
  /** 应用窗口内容区右键策略。 */
  contextMenuPolicy?: 'shell' | 'app' | 'native'
  defaultWidth: number
  defaultHeight: number
  minWidth: number
  minHeight: number
  hidden?: boolean
  sourceType?: 'builtin' | 'suite'
  suiteInstanceId?: string
  nodeId?: string
  appEntryId?: string
  entryType?: 'proxied_web' | 'compose_detail'
  entryTarget?: string
}

// 使用异步组件并标记为非响应式，避免被响应式系统深度代理。
// 内置应用的标题、图标、窗口尺寸、可见性和排序均以数据库 apps 表为唯一来源，
// registry 只注册组件和前端行为。禁止在这里重复填写图标路径等展示元数据；
// 这些字段使用空值或 0 占位，并由运行时 loadAppCatalog() 使用后端应用目录覆写。
export const appRegistry = {
  'docker-manager': {
    component: markRaw(
      defineAsyncComponent(() => import('@/apps/views/docker/DockerManagerView.vue')),
    ),
    title: '',
    i18nTitleKey: '',
    icon: '',
    minWidth: 0,
    minHeight: 0,
    defaultWidth: 0,
    defaultHeight: 0,
    nodeScope: 'current-node',
    nodeSwitchPolicy: 'block-while-open',
    contextMenuPolicy: 'app',
  },
  'operation-log': {
    component: markRaw(defineAsyncComponent(() => import('@/apps/views/OperationLogView.vue'))),
    title: '',
    i18nTitleKey: '',
    icon: '',
    minWidth: 0,
    minHeight: 0,
    defaultWidth: 0,
    defaultHeight: 0,
    nodeScope: 'global',
    nodeSwitchPolicy: 'allow',
  },
  'runtime-log': {
    component: markRaw(defineAsyncComponent(() => import('@/apps/views/RuntimeLogView.vue'))),
    title: '',
    i18nTitleKey: '',
    icon: '',
    minWidth: 0,
    minHeight: 0,
    defaultWidth: 0,
    defaultHeight: 0,
    nodeScope: 'explicit-node',
    nodeSwitchPolicy: 'allow',
  },
  'notification-center': {
    component: markRaw(
      defineAsyncComponent(() => import('@/apps/views/NotificationCenterView.vue')),
    ),
    title: '',
    i18nTitleKey: '',
    icon: '',
    minWidth: 0,
    minHeight: 0,
    defaultWidth: 0,
    defaultHeight: 0,
    nodeScope: 'global',
    nodeSwitchPolicy: 'allow',
  },
  'file-manager': {
    component: markRaw(defineAsyncComponent(() => import('@/apps/views/FileManagerView.vue'))),
    title: '',
    i18nTitleKey: '',
    icon: '',
    minWidth: 0,
    minHeight: 0,
    defaultWidth: 0,
    defaultHeight: 0,
    nodeScope: 'current-node',
    nodeSwitchPolicy: 'block-while-open',
    contextMenuPolicy: 'native',
  },
  'file-editor': {
    component: markRaw(defineAsyncComponent(() => import('@/apps/views/FileEditorView.vue'))),
    title: '',
    i18nTitleKey: '',
    icon: '',
    minWidth: 0,
    minHeight: 0,
    defaultWidth: 0,
    defaultHeight: 0,
    nodeScope: 'current-node',
    nodeSwitchPolicy: 'block-while-open',
    contextMenuPolicy: 'native',
  },
  settings: {
    component: markRaw(defineAsyncComponent(() => import('@/apps/views/SettingsView.vue'))),
    title: '',
    i18nTitleKey: '',
    icon: '',
    minWidth: 0,
    minHeight: 0,
    defaultWidth: 0,
    defaultHeight: 0,
    nodeScope: 'current-node',
    nodeSwitchPolicy: 'block-while-open',
  },
  'node-manager': {
    component: markRaw(defineAsyncComponent(() => import('@/apps/views/NodeManagerView.vue'))),
    title: '',
    i18nTitleKey: '',
    icon: '',
    minWidth: 0,
    minHeight: 0,
    defaultWidth: 0,
    defaultHeight: 0,
    nodeScope: 'global',
    nodeSwitchPolicy: 'allow',
  },
  'system-monitor': {
    component: markRaw(
      defineAsyncComponent(() => import('@/apps/views/system-monitor/SystemMonitorView.vue')),
    ),
    title: '',
    i18nTitleKey: '',
    icon: '',
    minWidth: 0,
    minHeight: 0,
    defaultWidth: 0,
    defaultHeight: 0,
    nodeScope: 'current-node',
    nodeSwitchPolicy: 'block-while-open',
  },
  'firewall-manager': {
    component: markRaw(defineAsyncComponent(() => import('@/apps/views/FirewallManagerView.vue'))),
    title: '',
    i18nTitleKey: '',
    icon: '',
    minWidth: 0,
    minHeight: 0,
    defaultWidth: 0,
    defaultHeight: 0,
    nodeScope: 'current-node',
    nodeSwitchPolicy: 'block-while-open',
  },
  'process-manager': {
    component: markRaw(defineAsyncComponent(() => import('@/apps/views/ProcessManagerView.vue'))),
    title: '',
    i18nTitleKey: '',
    icon: '',
    minWidth: 0,
    minHeight: 0,
    defaultWidth: 0,
    defaultHeight: 0,
    nodeScope: 'current-node',
    nodeSwitchPolicy: 'block-while-open',
  },
  'disk-manager': {
    component: markRaw(defineAsyncComponent(() => import('@/apps/views/DiskManagerView.vue'))),
    title: '',
    i18nTitleKey: '',
    icon: '',
    minWidth: 0,
    minHeight: 0,
    defaultWidth: 0,
    defaultHeight: 0,
    nodeScope: 'current-node',
    nodeSwitchPolicy: 'block-while-open',
  },
  terminal: {
    component: markRaw(defineAsyncComponent(() => import('@/apps/views/TerminalView.vue'))),
    title: '',
    i18nTitleKey: '',
    icon: '',
    minWidth: 0,
    minHeight: 0,
    defaultWidth: 0,
    defaultHeight: 0,
    nodeScope: 'current-node',
    nodeSwitchPolicy: 'block-while-open',
  },
  'task-scheduler': {
    component: markRaw(defineAsyncComponent(() => import('@/apps/views/TaskSchedulerView.vue'))),
    title: '',
    i18nTitleKey: '',
    icon: '',
    minWidth: 0,
    minHeight: 0,
    defaultWidth: 0,
    defaultHeight: 0,
    nodeScope: 'global',
    nodeSwitchPolicy: 'allow',
  },
  'script-manager': {
    component: markRaw(defineAsyncComponent(() => import('@/apps/views/ScriptManagerView.vue'))),
    title: '',
    i18nTitleKey: 'app.scriptManager.appName',
    icon: '',
    minWidth: 0,
    minHeight: 0,
    defaultWidth: 0,
    defaultHeight: 0,
    nodeScope: 'explicit-node',
    nodeSwitchPolicy: 'allow',
  },
  'suite-web-app': {
    component: markRaw(defineAsyncComponent(() => import('@/apps/views/SuiteWebAppView.vue'))),
    title: '',
    i18nTitleKey: 'app.suiteWebApp.appName',
    icon: 'package',
    minWidth: 0,
    minHeight: 0,
    defaultWidth: 0,
    defaultHeight: 0,
    nodeScope: 'global',
    nodeSwitchPolicy: 'allow',
    contextMenuPolicy: 'native',
    hidden: true,
  },
  'suite-center': {
    component: markRaw(defineAsyncComponent(() => import('@/apps/views/SuiteCenterView.vue'))),
    title: '',
    i18nTitleKey: 'app.suiteCenter.appName',
    icon: '',
    minWidth: 0,
    minHeight: 0,
    defaultWidth: 0,
    defaultHeight: 0,
    nodeScope: 'global',
    nodeSwitchPolicy: 'allow',
    contextMenuPolicy: 'app',
  },
} satisfies Record<string, AppConfig>

export type AppId = string

// 对外导出一份可响应式使用的目录副本。
export const availableApps: Record<AppId, AppConfig> = { ...appRegistry }
