/**
 * @file firewall.ts
 * @description Node-scoped 防火墙规则语义 API。
 */

import http from '@/api'
import type { FirewallRuleDetail, FirewallRuleListPage } from '@/api/generated'

export type FirewallRuleSummary = FirewallRuleListPage['entries'][number]
export type FirewallEngine = FirewallRuleSummary['engine']
export type FirewallFamily = FirewallRuleSummary['family']
export type FirewallAction = FirewallRuleSummary['action']
export type FirewallSortBy = 'ruleOrder' | 'action' | 'protocol' | 'source' | 'destination'
export type FirewallSortOrder = 'asc' | 'desc'

export interface FirewallRuleListParams {
  snapshotId?: string
  query?: string
  engine?: FirewallEngine
  family?: FirewallFamily
  action?: FirewallAction
  page: number
  pageSize: number
  sortBy: FirewallSortBy
  sortOrder: FirewallSortOrder
}

const nodePath = (nodeId: string, suffix: string) =>
  `/node/${encodeURIComponent(nodeId)}/${suffix.replace(/^\/+/, '')}`

/** 为当前 Node 创建防火墙规则 API。 */
const createScopedFirewallApi = (nodeId: string) => ({
  listRules: (params: FirewallRuleListParams, signal?: AbortSignal) =>
    http.get<FirewallRuleListPage>(nodePath(nodeId, 'firewall/rules/list'), params, { signal }),
  fetchRuleDetail: (ruleId: string, snapshotId: string, signal?: AbortSignal) =>
    http.get<FirewallRuleDetail>(
      nodePath(nodeId, `firewall/rule/${encodeURIComponent(ruleId)}/detail`),
      { snapshotId },
      { signal },
    ),
})

export const firewallApi = {
  forNode: createScopedFirewallApi,
}
