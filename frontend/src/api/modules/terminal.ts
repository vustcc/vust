/**
 * @file terminal.ts
 * @description 宿主机终端领域 API，仅通过 Master 的单节点语义网关访问。
 */

import http from '@/api'
import type { TerminalAccess } from '@/api/generated'

const terminalPath = (nodeId: string, suffix: string) =>
  `/node/${encodeURIComponent(nodeId || 'local')}/terminal${suffix}`

/** 构造同源 WebSocket 地址。 */
const buildWebSocketUrl = (path: string) => {
  const url = new URL(`/api/v1${path}`, window.location.origin)
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
  return url.toString()
}

export const terminalApi = {
  fetchAccess(nodeId: string) {
    return http.get<TerminalAccess>(terminalPath(nodeId, '/access'))
  },
  getWebSocketUrl(nodeId: string) {
    return buildWebSocketUrl(terminalPath(nodeId, '/ws'))
  },
}
