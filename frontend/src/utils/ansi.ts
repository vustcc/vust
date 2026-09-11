/**
 * @file ansi.ts
 * @description 将含有 ANSI Escape Codes 的文本转换为安全的带样式 HTML 字符串。
 *
 * 覆盖范围：
 * - 基础 SGR 属性：粗体、暗淡、斜体、下划线、删除线及其独立重置
 * - 标准 8 色前景/背景（30-37 / 40-47）与亮色变体（90-97 / 100-107）
 * - 256 色扩展（38;5;N / 48;5;N）
 * - 24 位真彩色（38;2;R;G;B / 48;2;R;G;B）
 * - 非 SGR 的 CSI 序列（光标移动、清屏等）自动剥离
 */

/** VDL 品牌调色板：标准 + 亮色前景 */
const FG_MAP: Record<number, string> = {
  30: 'var(--vdl-bg-panel)',
  31: 'var(--vdl-danger)',
  32: 'var(--vdl-success)',
  33: 'var(--vdl-warning)',
  34: 'var(--vdl-primary)',
  35: '#d371e3',
  36: 'var(--vdl-info)',
  37: 'var(--vdl-text-primary)',
  90: 'var(--vdl-text-muted)',
  91: 'var(--vdl-danger)',
  92: 'var(--vdl-success)',
  93: 'var(--vdl-warning)',
  94: 'var(--vdl-primary)',
  95: '#e894f7',
  96: 'var(--vdl-info)',
  97: 'var(--vdl-text-primary)',
}

/** VDL 品牌调色板：标准 + 亮色背景（半透明叠加） */
const BG_MAP: Record<number, string> = {
  40: '#000000',
  41: 'rgba(239,68,68,0.2)',
  42: 'rgba(34,197,94,0.2)',
  43: 'rgba(234,179,8,0.2)',
  44: 'rgba(59,130,246,0.2)',
  45: 'rgba(168,85,247,0.2)',
  46: 'rgba(6,182,212,0.2)',
  47: 'rgba(255,255,255,0.1)',
  100: 'rgba(127,127,127,0.2)',
  101: 'rgba(255,0,0,0.2)',
  102: 'rgba(0,255,0,0.2)',
  103: 'rgba(255,255,0,0.2)',
  104: 'rgba(92,92,255,0.2)',
  105: 'rgba(255,0,255,0.2)',
  106: 'rgba(0,255,255,0.2)',
  107: 'rgba(255,255,255,0.15)',
}

/**
 * 256 色索引转 CSS 颜色值。
 * 0-7 标准色 · 8-15 亮色 · 16-231 6×6×6 色立方 · 232-255 灰阶
 */
const STANDARD_256 = [
  '#000000',
  '#cd0000',
  '#00cd00',
  '#cdcd00',
  '#0000ee',
  '#cd00cd',
  '#00cdcd',
  '#e5e5e5',
  '#7f7f7f',
  '#ff0000',
  '#00ff00',
  '#ffff00',
  '#5c5cff',
  '#ff00ff',
  '#00ffff',
  '#ffffff',
]

function color256(n: number): string | undefined {
  if (n < 0 || n > 255) return undefined
  if (n < 16) return STANDARD_256[n]
  if (n < 232) {
    const idx = n - 16
    const r = Math.floor(idx / 36) * 51
    const g = Math.floor((idx % 36) / 6) * 51
    const b = (idx % 6) * 51
    return `rgb(${r},${g},${b})`
  }
  const gray = (n - 232) * 10 + 8
  return `rgb(${gray},${gray},${gray})`
}

// ---- 样式状态 ----

interface SgrState {
  bold: boolean
  dim: boolean
  italic: boolean
  underline: boolean
  strikethrough: boolean
  color: string | undefined
  bgColor: string | undefined
}

function emptyState(): SgrState {
  return {
    bold: false,
    dim: false,
    italic: false,
    underline: false,
    strikethrough: false,
    color: undefined,
    bgColor: undefined,
  }
}

function hasStyle(s: SgrState): boolean {
  return s.bold || s.dim || s.italic || s.underline || s.strikethrough || !!s.color || !!s.bgColor
}

function styleToAttr(s: SgrState): string {
  const parts: string[] = []
  if (s.bold) parts.push('font-weight:bold')
  if (s.dim) parts.push('opacity:0.65')
  if (s.italic) parts.push('font-style:italic')
  const deco: string[] = []
  if (s.underline) deco.push('underline')
  if (s.strikethrough) deco.push('line-through')
  if (deco.length) parts.push(`text-decoration:${deco.join(' ')}`)
  if (s.color) parts.push(`color:${s.color}`)
  if (s.bgColor) parts.push(`background-color:${s.bgColor}`)
  return parts.join(';')
}

/**
 * 解析 SGR 参数序列，就地更新样式状态。
 * 支持基础属性、标准色、256 色（38;5;N）和真彩色（38;2;R;G;B）。
 */
function applySgr(params: number[], state: SgrState): void {
  let i = 0
  while (i < params.length) {
    const p = params[i]
    switch (p) {
      case 0:
        Object.assign(state, emptyState())
        break
      case 1:
        state.bold = true
        break
      case 2:
        state.dim = true
        break
      case 3:
        state.italic = true
        break
      case 4:
        state.underline = true
        break
      case 9:
        state.strikethrough = true
        break
      case 22:
        state.bold = false
        state.dim = false
        break
      case 23:
        state.italic = false
        break
      case 24:
        state.underline = false
        break
      case 29:
        state.strikethrough = false
        break
      case 39:
        state.color = undefined
        break
      case 49:
        state.bgColor = undefined
        break
      case 38: // 扩展前景色
        if (params[i + 1] === 5 && i + 2 < params.length) {
          state.color = color256(params[i + 2])
          i += 2
        } else if (params[i + 1] === 2 && i + 4 < params.length) {
          state.color = `rgb(${params[i + 2]},${params[i + 3]},${params[i + 4]})`
          i += 4
        }
        break
      case 48: // 扩展背景色
        if (params[i + 1] === 5 && i + 2 < params.length) {
          state.bgColor = color256(params[i + 2])
          i += 2
        } else if (params[i + 1] === 2 && i + 4 < params.length) {
          state.bgColor = `rgba(${params[i + 2]},${params[i + 3]},${params[i + 4]},0.2)`
          i += 4
        }
        break
      default:
        if (FG_MAP[p] !== undefined) state.color = FG_MAP[p]
        else if (BG_MAP[p] !== undefined) state.bgColor = BG_MAP[p]
    }
    i++
  }
}

// ---- 主转换函数 ----

/** CSI 正则：ESC [ (参数) (终止字母)，匹配所有 CSI 序列 */
const ESC = String.fromCharCode(0x1b)
const CSI_RE = new RegExp(`${ESC}\\[([0-9;]*)([A-Za-z])`, 'g')

/**
 * 将含 ANSI 转义码的纯文本转换为安全的 HTML 字符串。
 * 非 SGR 的 CSI 序列（光标移动、清屏等）会被静默剥离。
 */
export function ansiToHtml(text: string): string {
  if (!text) return ''

  // 先转义 HTML 实体，防止 XSS
  const safe = text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')

  const state = emptyState()
  let result = ''
  let last = 0
  let open = 0

  CSI_RE.lastIndex = 0
  let m: RegExpExecArray | null
  while ((m = CSI_RE.exec(safe)) !== null) {
    result += safe.slice(last, m.index)
    last = CSI_RE.lastIndex

    // 仅处理 SGR（终止字母 m），其余 CSI 静默丢弃
    if (m[2] !== 'm') continue

    // 关闭当前 span
    if (open > 0) {
      result += '</span>'
      open--
    }

    const paramStr = m[1]
    if (paramStr === '' || paramStr === '0') {
      Object.assign(state, emptyState())
    } else {
      applySgr(paramStr.split(';').map(Number), state)
    }

    if (hasStyle(state)) {
      result += `<span style="${styleToAttr(state)}">`
      open++
    }
  }

  result += safe.slice(last)
  while (open-- > 0) result += '</span>'

  return result
}
