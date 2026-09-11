import { defineComponent, h } from 'vue'
import { createI18n } from 'vue-i18n'
import { createPinia } from 'pinia'
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { TrendContainerItem } from '@/api/interface/docker'
import en from '@/locales/en'
import zh from '@/locales/zh'
import {
  DOCKER_OVERVIEW_PREFERENCES_STORAGE_KEY,
  MAX_OVERVIEW_CONTAINERS,
  readDockerOverviewSelection,
  resolveDockerOverviewContainerIds,
  writeDockerOverviewSelection,
} from '@/stores/docker-overview-preferences'
import { useDockerStore } from '@/stores/docker'

const dockerMocks = vi.hoisted(() => {
  const state: { trendContainers: TrendContainerItem[] } = { trendContainers: [] }
  return {
    state,
    fetchStatus: vi.fn(async () => ({
      success: true,
      data: { dockerAvailable: true, dockerStatus: 'running' },
    })),
    fetchOverviewRealtime: vi.fn(async () => ({
      success: true,
      data: {
        collectedAt: 1,
        containerStates: {
          total: state.trendContainers.length,
          running: state.trendContainers.filter((item) => item.state === 'running').length,
          paused: 0,
          restarting: 0,
          exited: 0,
          other: 0,
        },
        projectStates: { total: 0, healthy: 0, partial: 0, stopped: 0, unknown: 0 },
        images: { total: 0, dangling: 0 },
        resourceUsage: {
          status: 'fresh',
          collectedAt: 1,
          runningContainerCount: state.trendContainers.length,
          sampledContainerCount: state.trendContainers.length,
          cpuHostPercent: 0,
          cpuCorePercent: 0,
          memoryWorkingSetBytes: 0,
          memoryLimitBytes: 0,
          memoryPercent: 0,
        },
        trendContainers: state.trendContainers,
      },
    })),
    fetchContainerResourceUsageHistoryAll: vi.fn(
      async ({ ids }: { ids: string[]; hours?: number }) => ({
        success: true,
        data: { containers: ids.map((id) => ({ id, name: id, points: [] })) },
      }),
    ),
  }
})

vi.mock('@/api/modules/docker', () => ({
  dockerApi: {
    forNode: () => ({
      fetchStatus: dockerMocks.fetchStatus,
      fetchOverviewRealtime: dockerMocks.fetchOverviewRealtime,
      fetchContainerResourceUsageHistoryAll: dockerMocks.fetchContainerResourceUsageHistoryAll,
    }),
  },
}))

const container = (id: string, name = id, state = 'running'): TrendContainerItem => ({
  id,
  name,
  state,
  createdAt: 1,
})

describe('Docker 资源趋势选择偏好', () => {
  const wrappers: VueWrapper[] = []

  const mountStore = () => {
    let store!: ReturnType<typeof useDockerStore>
    const wrapper = mount(
      defineComponent({
        setup() {
          store = useDockerStore()
          return () => h('div')
        },
      }),
      {
        global: {
          plugins: [
            createPinia(),
            createI18n({ legacy: false, locale: 'zh', messages: { zh, en } }),
          ],
        },
      },
    )
    wrappers.push(wrapper)
    return store
  }

  beforeEach(() => {
    localStorage.clear()
    vi.clearAllMocks()
    dockerMocks.state.trendContainers = Array.from({ length: 8 }, (_, index) =>
      container(`container-${index + 1}`),
    )
  })

  afterEach(() => {
    wrappers.splice(0).forEach((wrapper) => wrapper.unmount())
  })

  it('保存七个容器后重建 Store 可恢复，并继续限制第八个', async () => {
    const selectedIds = dockerMocks.state.trendContainers.slice(1, 8).map((item) => item.id)
    const first = mountStore()
    await first.initialLoad()
    first.updateOverviewSelectedContainers(selectedIds)
    await flushPromises()

    expect(first.overviewSelectedContainerIds).toEqual(selectedIds)
    expect(readDockerOverviewSelection('local')).toHaveLength(MAX_OVERVIEW_CONTAINERS)

    const restored = mountStore()
    await restored.initialLoad()
    await flushPromises()
    expect(restored.overviewSelectedContainerIds).toEqual(selectedIds)

    restored.updateOverviewSelectedContainers([
      ...selectedIds,
      dockerMocks.state.trendContainers[0]!.id,
    ])
    expect(restored.overviewSelectedContainerIds).toEqual(selectedIds)
  })

  it('按节点隔离选择并保留显式空数组', () => {
    expect(writeDockerOverviewSelection('node-a', [{ id: 'a', name: 'alpha' }])).toBe(true)
    expect(writeDockerOverviewSelection('node-b', [])).toBe(true)

    expect(readDockerOverviewSelection('node-a')).toEqual([{ id: 'a', name: 'alpha' }])
    expect(readDockerOverviewSelection('node-b')).toEqual([])
    expect(readDockerOverviewSelection('node-c')).toBeNull()
  })

  it('显式取消全部选择后重建 Store 仍保持为空', async () => {
    const first = mountStore()
    await first.initialLoad()
    first.updateOverviewSelectedContainers([])
    await flushPromises()

    const restored = mountStore()
    await restored.initialLoad()
    await flushPromises()
    expect(restored.overviewSelectedContainerIds).toEqual([])
    expect(readDockerOverviewSelection('local')).toEqual([])
  })

  it('优先按 ID 恢复，并在容器重建后按完整名称恢复', () => {
    const items = [container('same-id', 'renamed'), container('new-id', 'stable-name')]
    expect(
      resolveDockerOverviewContainerIds(items, [
        { id: 'same-id', name: 'old-name' },
        { id: 'old-id', name: 'stable-name' },
      ]),
    ).toEqual(['same-id', 'new-id'])
  })

  it('部分保存项失效时只保留有效项，全部失效时恢复默认七项', async () => {
    writeDockerOverviewSelection('local', [
      { id: 'container-3', name: 'container-3' },
      { id: 'missing', name: 'missing' },
    ])
    const partial = mountStore()
    await partial.initialLoad()
    await flushPromises()
    expect(partial.overviewSelectedContainerIds).toEqual(['container-3'])

    localStorage.setItem(
      DOCKER_OVERVIEW_PREFERENCES_STORAGE_KEY,
      JSON.stringify({
        version: 1,
        selectionsByNode: { local: [{ id: 'missing', name: 'missing' }] },
      }),
    )
    const fallback = mountStore()
    await fallback.initialLoad()
    await flushPromises()
    expect(fallback.overviewSelectedContainerIds).toEqual(
      dockerMocks.state.trendContainers.slice(0, MAX_OVERVIEW_CONTAINERS).map((item) => item.id),
    )
  })

  it('容器列表暂时为空时不擦除保存项，并在列表恢复后继续使用', async () => {
    writeDockerOverviewSelection('local', [{ id: 'container-4', name: 'container-4' }])
    dockerMocks.state.trendContainers = []
    const store = mountStore()
    await store.initialLoad()
    await flushPromises()

    expect(store.overviewSelectedContainerIds).toEqual([])
    expect(readDockerOverviewSelection('local')).toEqual([
      { id: 'container-4', name: 'container-4' },
    ])

    dockerMocks.state.trendContainers = [container('container-4')]
    await store.fetchOverviewData()
    await flushPromises()
    expect(store.overviewSelectedContainerIds).toEqual(['container-4'])
  })

  it('损坏存储会安全降级并写入默认选择', async () => {
    localStorage.setItem(DOCKER_OVERVIEW_PREFERENCES_STORAGE_KEY, '{broken')
    const store = mountStore()
    await store.initialLoad()
    await flushPromises()

    expect(store.overviewSelectedContainerIds).toHaveLength(MAX_OVERVIEW_CONTAINERS)
    expect(readDockerOverviewSelection('local')).toHaveLength(MAX_OVERVIEW_CONTAINERS)

    localStorage.setItem(
      DOCKER_OVERVIEW_PREFERENCES_STORAGE_KEY,
      JSON.stringify({ version: 1, selectionsByNode: { local: [{ id: 1, name: null }] } }),
    )
    const malformed = mountStore()
    await malformed.initialLoad()
    await flushPromises()
    expect(malformed.overviewSelectedContainerIds).toHaveLength(MAX_OVERVIEW_CONTAINERS)
  })
})
