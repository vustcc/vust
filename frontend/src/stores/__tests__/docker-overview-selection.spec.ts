import { describe, expect, it } from 'vitest'
import type { TrendContainerItem } from '@/api/interface/docker'
import { MAX_OVERVIEW_CONTAINERS, selectOverviewContainerIds } from '@/stores/docker'

const container = (id: string, state: string): TrendContainerItem => ({
  id,
  name: id,
  createdAt: 1,
  state,
})

describe('Docker Store 概览容器选择', () => {
  it('默认只选择前七个运行中的容器', () => {
    const items = [
      container('exited', 'exited'),
      container('running-1', 'running'),
      container('paused', 'paused'),
      container('running-2', 'RUNNING'),
      container('running-3', 'running'),
      container('running-4', 'running'),
      container('running-5', 'running'),
      container('running-6', 'running'),
      container('running-7', 'running'),
      container('running-8', 'running'),
    ]

    expect(selectOverviewContainerIds(items, [])).toEqual([
      'running-1',
      'running-2',
      'running-3',
      'running-4',
      'running-5',
      'running-6',
      'running-7',
    ])
    expect(MAX_OVERVIEW_CONTAINERS).toBe(7)
  })

  it('没有运行中的容器时不默认选择容器', () => {
    const items = [container('exited', 'exited'), container('paused', 'paused')]

    expect(selectOverviewContainerIds(items, [])).toEqual([])
  })

  it('保留仍然有效的用户选择', () => {
    const items = [container('running', 'running'), container('exited', 'exited')]

    expect(selectOverviewContainerIds(items, ['exited'])).toEqual(['exited'])
  })
})
