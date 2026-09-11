/**
 * @file useEChartsInstance.ts
 * @description 通用 ECharts 实例生命周期管理 composable。
 *
 * 封装图表的初始化、自适应 resize、主题跟随、销毁等生命周期逻辑，
 * 消除各业务视图中大量重复的 init / dispose / ResizeObserver 样板代码。
 */

import { onMounted, onUnmounted, watch, type Ref, type ShallowRef, shallowRef } from 'vue'
import { init, type ECharts } from 'echarts/core'
import { useThemeStore, buildChartTheme } from '@/stores/theme'

export interface ChartBinding {
  /** ECharts 实例引用 */
  instance: ShallowRef<ECharts | null>
  /** 手动触发 resize */
  resize: () => void
  /** 手动设置 option */
  setOption: (option: Record<string, unknown>) => void
}

/**
 * 创建一个 ECharts 实例管理器。
 *
 * @param containerRef 图表容器 DOM 元素引用。
 * @param containerParentRef 可选的外层容器，用于 ResizeObserver 监听窗口缩放。
 * @returns 图表操作接口。
 */
export function useEChartsInstance(
  containerRef: Ref<HTMLElement | null>,
  containerParentRef?: Ref<HTMLElement | null>,
): ChartBinding {
  const instance = shallowRef<ECharts | null>(null)
  let resizeRaf: number | null = null
  let resizeObserver: ResizeObserver | null = null

  /** 防抖 resize，合并到下一帧执行。 */
  const queueResize = () => {
    if (resizeRaf !== null) {
      cancelAnimationFrame(resizeRaf)
    }
    resizeRaf = requestAnimationFrame(() => {
      instance.value?.resize()
      resizeRaf = null
    })
  }

  /** 手动设置图表配置。 */
  const setOption = (option: Record<string, unknown>) => {
    instance.value?.setOption(option)
  }

  onMounted(() => {
    if (containerRef.value) {
      instance.value = init(containerRef.value)
    }

    // 监听 window resize
    window.addEventListener('resize', queueResize)

    // 监听容器父元素 resize（应对窗口最大化/侧栏折叠等场景）
    const observeTarget = containerParentRef?.value ?? containerRef.value
    if (observeTarget) {
      resizeObserver = new ResizeObserver(queueResize)
      resizeObserver.observe(observeTarget)
    }
  })

  onUnmounted(() => {
    window.removeEventListener('resize', queueResize)
    if (resizeObserver) {
      resizeObserver.disconnect()
      resizeObserver = null
    }
    if (resizeRaf !== null) {
      cancelAnimationFrame(resizeRaf)
      resizeRaf = null
    }
    instance.value?.dispose()
    instance.value = null
  })

  return { instance, resize: queueResize, setOption }
}

/**
 * 批量管理多个 ECharts 实例：统一 resize、主题跟随、销毁。
 *
 * @param bindings 通过 `useEChartsInstance` 创建的图表绑定列表。
 */
export function useChartThemeSync(bindings: ChartBinding[], reapply: () => void) {
  const themeStore = useThemeStore()

  // 主题变化时重新应用图表配置
  watch(
    () => themeStore.currentTheme,
    () => reapply(),
  )
}

/**
 * 获取当前主题对应的图表颜色配置。
 */
export function useChartTheme() {
  const themeStore = useThemeStore()
  return () => buildChartTheme(themeStore.currentTheme)
}
