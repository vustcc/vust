export interface DesktopAppItem {
  appId: string
  sortOrder: number
  visible: boolean
  x?: number
  y?: number
}

export interface DesktopAppsResponse {
  apps: DesktopAppItem[]
  hasRecord: boolean
}

export interface DesktopAppsPayload {
  nodeId?: string
  apps: DesktopAppItem[]
}
