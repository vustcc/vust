/**
 * @file task.ts
 * @description 计划任务前端查询参数与复合响应类型。
 */

import type { ScheduledTaskOperation, ScheduledTaskSummary } from '@/api/generated/scheduled-tasks'

export type * from '@/api/generated/scheduled-tasks'

/** 服务端任务列表查询。 */
export interface ScheduledTaskListQuery {
  nodeId?: string
  keyword?: string
  enabled?: boolean
  deploymentStatus?: string
  page?: number
  pageSize?: number
  sortBy?: 'name' | 'nextRunAt' | 'updatedAt'
  sortOrder?: 'asc' | 'desc'
}

/** 创建、编辑和启停共同返回的任务与后台操作。 */
export interface ScheduledTaskMutationResponse {
  task: ScheduledTaskSummary
  operation: ScheduledTaskOperation
}
