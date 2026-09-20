import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import type { ITerminalOptions } from '@xterm/xterm'

export type ThemeMode = 'light' | 'dark'

const THEME_STORAGE_KEY = 'vust_theme'
const GLASS_STORAGE_KEY = 'vust_glass_enabled'
const DEFAULT_THEME: ThemeMode = 'light'

/**
 * @description 读取本地持久化的主题配置，默认返回浅色主题。
 */
export function getStoredTheme(): ThemeMode {
  if (typeof window === 'undefined') {
    return DEFAULT_THEME
  }

  const storedTheme = window.localStorage.getItem(THEME_STORAGE_KEY)
  return storedTheme === 'light' || storedTheme === 'dark'
    ? (storedTheme as ThemeMode)
    : DEFAULT_THEME
}

/**
 * @description 将主题值同步到根节点和浏览器配色提示。
 * @param theme 当前应用主题。
 */
export function applyTheme(theme: ThemeMode = getStoredTheme()): ThemeMode {
  if (typeof document !== 'undefined') {
    document.documentElement.dataset.theme = theme
    document.documentElement.style.colorScheme = theme
  }

  if (typeof window !== 'undefined') {
    window.localStorage.setItem(THEME_STORAGE_KEY, theme)
  }

  return theme
}

/**
 * @description 读取本地 Liquid Glass 偏好；缺失或无效值默认开启。
 */
export function getStoredGlassEnabled(): boolean {
  if (typeof window === 'undefined') return true

  const stored = window.localStorage.getItem(GLASS_STORAGE_KEY)
  if (stored === 'false') return false
  return true
}

/**
 * @description 将 Liquid Glass 偏好同步到文档根节点并持久化。
 * @param enabled 是否启用 Liquid Glass。
 */
export function applyGlassPreference(enabled = getStoredGlassEnabled()): boolean {
  if (typeof document !== 'undefined') {
    document.documentElement.dataset.glass = enabled ? 'enabled' : 'disabled'
  }

  if (typeof window !== 'undefined') {
    window.localStorage.setItem(GLASS_STORAGE_KEY, String(enabled))
  }

  return enabled
}

/**
 * @description 生成 ECharts 主题所需的颜色集合。
 * @param mode 当前主题模式。
 */
export function buildChartTheme(mode: ThemeMode) {
  const isDark = mode === 'dark'
  return {
    titleColor: isDark ? '#d9e1ef' : '#4b5d7f',
    textColor: isDark ? '#a9b8cf' : '#6c7a95',
    gridLineColor: isDark ? 'rgba(148, 163, 184, 0.16)' : '#e4eaf3',
    primary: '#3b82f6',
    primarySoft: isDark ? 'rgba(59, 130, 246, 0.22)' : 'rgba(59, 130, 246, 0.14)',
    success: '#22c55e',
    successSoft: isDark ? 'rgba(34, 197, 94, 0.22)' : 'rgba(34, 197, 94, 0.14)',
    warning: '#f59e0b',
    warningSoft: isDark ? 'rgba(245, 158, 11, 0.22)' : 'rgba(245, 158, 11, 0.14)',
    pieTrack: isDark ? 'rgba(148, 163, 184, 0.14)' : '#e8eef9',
    tooltipBackground: isDark ? 'rgba(12, 18, 28, 0.94)' : 'rgba(255, 255, 255, 0.96)',
    tooltipBorder: isDark ? 'rgba(148, 163, 184, 0.28)' : 'rgba(13, 30, 55, 0.12)',
  }
}

/**
 * @description 生成 xterm 终端主题。
 * @param mode 当前主题模式。
 */
export function buildTerminalTheme(mode: ThemeMode): ITerminalOptions['theme'] {
  if (mode === 'dark') {
    return {
      background: '#0f172a',
      foreground: '#e2e8f0',
      cursor: '#f8fafc',
      cursorAccent: '#0f172a',
      selectionBackground: '#334155',
      black: '#0f172a',
      red: '#ef4444',
      green: '#22c55e',
      yellow: '#f59e0b',
      blue: '#3b82f6',
      magenta: '#a855f7',
      cyan: '#06b6d4',
      white: '#e2e8f0',
      brightBlack: '#475569',
      brightRed: '#f87171',
      brightGreen: '#4ade80',
      brightYellow: '#fbbf24',
      brightBlue: '#60a5fa',
      brightMagenta: '#c084fc',
      brightCyan: '#22d3ee',
      brightWhite: '#f8fafc',
    }
  }

  return {
    background: '#f5f8fd',
    foreground: '#132033',
    cursor: '#1d4ed8',
    cursorAccent: '#f5f8fd',
    selectionBackground: 'rgba(59, 130, 246, 0.2)',
    black: '#1f2937',
    red: '#dc2626',
    green: '#15803d',
    yellow: '#b45309',
    blue: '#1d4ed8',
    magenta: '#9333ea',
    cyan: '#0f766e',
    white: '#e5e7eb',
    brightBlack: '#4b5563',
    brightRed: '#ef4444',
    brightGreen: '#22c55e',
    brightYellow: '#f59e0b',
    brightBlue: '#3b82f6',
    brightMagenta: '#a855f7',
    brightCyan: '#14b8a6',
    brightWhite: '#ffffff',
  }
}

export const useThemeStore = defineStore('theme', () => {
  const currentTheme = ref<ThemeMode>(getStoredTheme())
  const glassEnabled = ref(getStoredGlassEnabled())

  /**
   * @description 初始化主题状态，并立即应用到根节点。
   */
  function initializeTheme() {
    currentTheme.value = applyTheme(getStoredTheme())
    glassEnabled.value = applyGlassPreference(getStoredGlassEnabled())
  }

  /**
   * @description 设置指定主题并持久化。
   * @param theme 目标主题。
   */
  function setTheme(theme: ThemeMode) {
    currentTheme.value = applyTheme(theme)
  }

  /**
   * @description 在浅色与深色主题之间切换。
   */
  function toggleTheme() {
    setTheme(currentTheme.value === 'light' ? 'dark' : 'light')
  }

  /**
   * @description 设置 Liquid Glass 偏好并持久化。
   * @param enabled 是否启用 Liquid Glass。
   */
  function setGlassEnabled(enabled: boolean) {
    glassEnabled.value = applyGlassPreference(enabled)
  }

  const isDark = computed(() => currentTheme.value === 'dark')

  return {
    currentTheme,
    glassEnabled,
    isDark,
    initializeTheme,
    setTheme,
    toggleTheme,
    setGlassEnabled,
    applyTheme: setTheme,
  }
})
