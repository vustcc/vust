export interface AppItem {
  appId: string
  i18nTitleKey: string
  title?: string
  /** VDL 图标名称，默认与应用 ID 一致。 */
  icon: string
  defaultWidth: number
  defaultHeight: number
  minWidth: number
  minHeight: number
  hidden: boolean
  sortOrder: number
  sourceType: 'builtin' | 'suite'
  suiteInstanceId?: string
  nodeId?: string
  appEntryId?: string
  entryType?: 'proxied_web' | 'compose_detail'
  entryTarget?: string
}
