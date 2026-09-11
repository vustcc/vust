import http from '@/api'
import * as apps from '../interface/apps'

export const appsApi = {
  fetchApps: (nodeId: string) => {
    return http.get<apps.AppItem[]>('/apps', { nodeId })
  },
}
