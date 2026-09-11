import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useToastStore } from '@/stores/toast'

describe('ToastStore', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.useFakeTimers()
  })

  it('只维护内存即时反馈并按持续时间移除', () => {
    const store = useToastStore()
    store.success('Saved', 1000)
    expect(store.activeNotifications).toHaveLength(1)
    expect(store.activeNotifications[0]?.message).toBe('Saved')
    vi.advanceTimersByTime(1000)
    expect(store.activeNotifications).toHaveLength(0)
  })
})
