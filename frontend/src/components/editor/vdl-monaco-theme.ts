/**
 * @file vdl-monaco-theme.ts
 * @description 将 VUST Design Language (VDL) Token 映射为 Monaco Editor 自定义主题。
 * 支持深色与浅色模式，运行时从 CSS 自定义属性读取色值以保持与 VDL 一致。
 */

import type * as monaco from 'monaco-editor'

/** VDL 主题名称常量 */
export const VDL_THEME_DARK = 'vdl-dark'
export const VDL_THEME_LIGHT = 'vdl-light'

/**
 * 从当前 DOM 读取 VDL CSS 自定义属性值
 * @param prop - CSS 自定义属性名（含 `--` 前缀）
 * @param fallback - 读取失败时的回退值
 */
function resolveToken(prop: string, fallback: string): string {
  if (typeof document === 'undefined') return fallback
  const raw = getComputedStyle(document.documentElement).getPropertyValue(prop).trim()
  return raw || fallback
}

/** 返回需要以数字传给 Monaco 的公共滚动条尺寸。 */
export function resolveVdlScrollbarSize(): number {
  const size = Number.parseFloat(resolveToken('--vdl-scrollbar-size', '10px'))
  return Number.isFinite(size) ? size : 10
}

/** 将 rgba/rgb 格式色值转换为 Monaco 需要的 #RRGGBB 或 #RRGGBBAA 格式 */
function toHex(color: string): string {
  if (color.startsWith('#')) return color

  const rgbaMatch =
    color.match(/rgba?\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)(?:\s*,\s*([\d.]+%?))?\s*\)/) ??
    color.match(/rgba?\(\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)(?:\s*\/\s*([\d.]+%?))?\s*\)/)
  if (!rgbaMatch) return color

  const r = Math.round(Number(rgbaMatch[1]))
  const g = Math.round(Number(rgbaMatch[2]))
  const b = Math.round(Number(rgbaMatch[3]))
  const alpha = rgbaMatch[4]
  const a =
    alpha === undefined ? 1 : alpha.endsWith('%') ? Number(alpha.slice(0, -1)) / 100 : Number(alpha)

  const hex = `#${r.toString(16).padStart(2, '0')}${g.toString(16).padStart(2, '0')}${b.toString(16).padStart(2, '0')}`
  if (a < 1) {
    return `${hex}${Math.round(a * 255)
      .toString(16)
      .padStart(2, '0')}`
  }
  return hex
}

/**
 * 生成 VDL 深色模式 Monaco 主题定义
 */
export function createSdlDarkTheme(): monaco.editor.IStandaloneThemeData {
  const bg = resolveToken('--vdl-bg-panel', '#162033')
  const bgCard = resolveToken('--vdl-bg-card', '#1a2438')
  const bgMuted = resolveToken('--vdl-bg-muted', '#202b40')
  const bgActive = resolveToken('--vdl-bg-active', 'rgba(0, 200, 255, 0.16)')
  const textPrimary = resolveToken('--vdl-text-primary', '#e6edf7')
  const textSecondary = resolveToken('--vdl-text-secondary', '#c1cfde')
  const textMuted = resolveToken('--vdl-text-muted', '#91a2b8')
  const primary = resolveToken('--vdl-primary', '#00c8ff')
  const accent = resolveToken('--vdl-accent', '#7c6cff')
  const success = resolveToken('--vdl-success', '#00d27a')
  const warning = resolveToken('--vdl-warning', '#ffb547')
  const danger = resolveToken('--vdl-danger', '#ff5e7a')
  const borderSubtle = resolveToken('--vdl-border-subtle', 'rgba(148, 163, 184, 0.12)')
  const scrollbarThumb = resolveToken('--vdl-scrollbar-thumb', 'rgba(148, 163, 184, 0.32)')
  const scrollbarThumbHover = resolveToken(
    '--vdl-scrollbar-thumb-hover',
    'rgba(174, 190, 208, 0.46)',
  )
  const scrollbarThumbActive = resolveToken(
    '--vdl-scrollbar-thumb-active',
    'rgba(200, 216, 232, 0.58)',
  )

  return {
    base: 'vs-dark',
    inherit: true,
    rules: [
      // 基础文本
      { token: '', foreground: toHex(textPrimary) },
      { token: 'comment', foreground: toHex(textMuted), fontStyle: 'italic' },
      { token: 'comment.doc', foreground: toHex(textMuted), fontStyle: 'italic' },

      // 关键字 & 控制流
      { token: 'keyword', foreground: toHex(accent) },
      { token: 'keyword.control', foreground: toHex(accent) },
      { token: 'keyword.operator', foreground: toHex(textSecondary) },

      // 字符串 & 正则
      { token: 'string', foreground: toHex(success) },
      { token: 'string.escape', foreground: toHex(warning) },
      { token: 'regexp', foreground: toHex(danger) },

      // 数字 & 常量
      { token: 'number', foreground: toHex(warning) },
      { token: 'constant', foreground: toHex(warning) },

      // 类型 & 标识符
      { token: 'type', foreground: toHex(primary) },
      { token: 'type.identifier', foreground: toHex(primary) },
      { token: 'identifier', foreground: toHex(textPrimary) },
      { token: 'variable', foreground: toHex(textPrimary) },

      // 函数
      { token: 'function', foreground: '#82AAFF' },
      { token: 'function.declaration', foreground: '#82AAFF' },

      // 标签 (HTML/XML)
      { token: 'tag', foreground: toHex(danger) },
      { token: 'attribute.name', foreground: toHex(warning) },
      { token: 'attribute.value', foreground: toHex(success) },

      // YAML / JSON key
      { token: 'key', foreground: toHex(primary) },

      // 分隔符 & 括号
      { token: 'delimiter', foreground: toHex(textSecondary) },
      { token: 'delimiter.bracket', foreground: toHex(textSecondary) },

      // 注解
      { token: 'annotation', foreground: toHex(warning) },
      { token: 'metatag', foreground: toHex(warning) },
    ],
    colors: {
      // 编辑器基础
      'editor.background': toHex(bg),
      'editor.foreground': toHex(textPrimary),
      'editor.lineHighlightBackground': toHex(bgCard),
      'editor.selectionBackground': toHex(bgActive),
      'editor.inactiveSelectionBackground': toHex(borderSubtle),

      // 行号
      'editorLineNumber.foreground': toHex(textMuted),
      'editorLineNumber.activeForeground': toHex(primary),

      // 光标
      'editorCursor.foreground': toHex(primary),

      // 缩进辅助线
      'editorIndentGuide.background': toHex(borderSubtle),
      'editorIndentGuide.activeBackground': toHex(bgActive),

      // 搜索高亮
      'editor.findMatchBackground': '#FFB54740',
      'editor.findMatchHighlightBackground': '#FFB54720',

      // 滚动条
      'scrollbarSlider.background': toHex(scrollbarThumb),
      'scrollbarSlider.hoverBackground': toHex(scrollbarThumbHover),
      'scrollbarSlider.activeBackground': toHex(scrollbarThumbActive),

      // Minimap
      'minimap.background': toHex(bgMuted),

      // Widget（搜索框等弹出框）
      'editorWidget.background': toHex(bgCard),
      'editorWidget.border': toHex(borderSubtle),

      // 编辑器面包屑
      'editorGutter.background': toHex(bg),

      // 括号匹配
      'editorBracketMatch.background': toHex(bgActive),
      'editorBracketMatch.border': toHex(primary),
    },
  }
}

/**
 * 生成 VDL 浅色模式 Monaco 主题定义
 */
export function createSdlLightTheme(): monaco.editor.IStandaloneThemeData {
  const bg = resolveToken('--vdl-bg-panel', '#f8fafc')
  const bgCard = resolveToken('--vdl-bg-card', '#ffffff')
  const bgMuted = resolveToken('--vdl-bg-muted', '#f1f5f9')
  const bgActive = resolveToken('--vdl-bg-active', 'rgba(29, 99, 237, 0.08)')
  const textPrimary = resolveToken('--vdl-text-primary', '#1f2a44')
  const textSecondary = resolveToken('--vdl-text-secondary', '#4b5d7f')
  const textMuted = resolveToken('--vdl-text-muted', '#6c7a95')
  const primary = resolveToken('--vdl-primary', '#1d63ed')
  const borderSubtle = resolveToken('--vdl-border-subtle', 'rgba(13, 30, 55, 0.08)')
  const scrollbarThumb = resolveToken('--vdl-scrollbar-thumb', 'rgba(108, 122, 149, 0.3)')
  const scrollbarThumbHover = resolveToken(
    '--vdl-scrollbar-thumb-hover',
    'rgba(86, 105, 139, 0.46)',
  )
  const scrollbarThumbActive = resolveToken(
    '--vdl-scrollbar-thumb-active',
    'rgba(65, 87, 126, 0.58)',
  )

  return {
    base: 'vs',
    inherit: true,
    rules: [
      { token: '', foreground: toHex(textPrimary) },
      { token: 'comment', foreground: toHex(textMuted), fontStyle: 'italic' },
      { token: 'keyword', foreground: '#7C3AED' },
      { token: 'string', foreground: '#059669' },
      { token: 'number', foreground: '#D97706' },
      { token: 'type', foreground: toHex(primary) },
      { token: 'function', foreground: '#2563EB' },
      { token: 'tag', foreground: '#DC2626' },
      { token: 'attribute.name', foreground: '#D97706' },
      { token: 'attribute.value', foreground: '#059669' },
      { token: 'key', foreground: toHex(primary) },
      { token: 'delimiter', foreground: toHex(textSecondary) },
    ],
    colors: {
      'editor.background': toHex(bg),
      'editor.foreground': toHex(textPrimary),
      'editor.lineHighlightBackground': toHex(bgCard),
      'editor.selectionBackground': toHex(bgActive),
      'editorLineNumber.foreground': toHex(textMuted),
      'editorLineNumber.activeForeground': toHex(primary),
      'editorCursor.foreground': toHex(primary),
      'editorIndentGuide.background': toHex(borderSubtle),
      'scrollbarSlider.background': toHex(scrollbarThumb),
      'scrollbarSlider.hoverBackground': toHex(scrollbarThumbHover),
      'scrollbarSlider.activeBackground': toHex(scrollbarThumbActive),
      'minimap.background': toHex(bgMuted),
      'editorWidget.background': toHex(bgCard),
      'editorWidget.border': toHex(borderSubtle),
      'editorGutter.background': toHex(bg),
      'editorBracketMatch.background': toHex(bgActive),
      'editorBracketMatch.border': toHex(primary),
    },
  }
}
