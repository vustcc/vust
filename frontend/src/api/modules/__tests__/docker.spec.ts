import { beforeEach, describe, expect, it, vi } from 'vitest'

const http = vi.hoisted(() => ({
  post: vi.fn(),
}))

vi.mock('@/api', () => ({ default: http }))

import { dockerApi } from '../docker'

describe('Docker 批量容器操作请求', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('本地节点和外部节点均使用长请求超时', async () => {
    const payload = { ids: ['container-1'], action: 'stop' as const }

    await dockerApi.batchContainerAction(payload)
    await dockerApi.forNode('node-1').batchContainerAction(payload)

    expect(http.post).toHaveBeenNthCalledWith(1, '/agent/docker/containers/actions', payload, {
      timeout: 610_000,
    })
    expect(http.post).toHaveBeenNthCalledWith(
      2,
      '/node/node-1/agent/docker/containers/actions',
      payload,
      { timeout: 610_000 },
    )
  })
})
