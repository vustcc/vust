import http from '@/api'
import type { VustNetworkConfig, VustNetworkUpdateResult } from '../interface/vust'

export const vustApi = {
  fetchNetwork: () => {
    return http.get<VustNetworkConfig>('/vust/network')
  },
  updateNetwork: (payload: VustNetworkConfig) => {
    return http.put<VustNetworkUpdateResult>('/vust/network', payload)
  },
  fetchInterfaces: () => {
    return http.get<{ name: string; ip: string; label: string }[]>('/vust/network-interfaces')
  },
}
