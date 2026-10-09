/**
 * 界面偏好 / 强调色渐变的纯函数（无 DOM、无副作用，可直接单测）
 *
 *  - parseGradient('a,b')        → { from, to } | null（校验两个十六进制色）
 *  - formatGradient(from, to)    → 'a,b'（落盘用）
 *  - normalizeUiMode(v)          → 'simple' | 'advanced'（未知值回落默认 simple）
 *  - normalizeAppearanceStyle(v) → 'glass' | 'mono'（未知值回落默认 glass）
 *
 * DOM 侧的应用（写 <html> 上的 CSS 变量 / data-style）住在 services/theme.ts。
 */

/** 转小写 "#rrggbb"；非法返回 null（与 services/theme.ts 的 parseAccentHex 同口径：前导 # 可省略） */
export function normalizeHex(v: string): string | null {
  const m = /^#?([0-9a-fA-F]{6})$/.exec(String(v ?? '').trim())
  return m ? `#${m[1].toLowerCase()}` : null
}

/**
 * 解析自定义渐变 "起色,止色"（如 '#7c4dff,#22d3ee'）。
 * 两个色值都必须是合法十六进制色，否则返回 null（空串 / 只有一个色 / 多余分隔也都不合法）。
 */
export function parseGradient(v: string | null | undefined): { from: string; to: string } | null {
  const s = String(v ?? '').trim()
  if (!s) return null
  const parts = s.split(',')
  if (parts.length !== 2) return null
  const from = normalizeHex(parts[0])
  const to = normalizeHex(parts[1])
  if (!from || !to) return null
  return { from, to }
}

/** 组装落盘用的渐变串（两个色值都合法才有值，否则返回空串） */
export function formatGradient(from: string, to: string): string {
  const a = normalizeHex(from)
  const b = normalizeHex(to)
  return a && b ? `${a},${b}` : ''
}

/** 界面模式校验：只认 simple / advanced，其余（含 undefined / 历史脏值）一律回落 simple */
export function normalizeUiMode(v: string | null | undefined): 'simple' | 'advanced' {
  return v === 'advanced' ? 'advanced' : 'simple'
}

/** 外观风格校验：只认 glass / mono，其余一律回落 glass（现状） */
export function normalizeAppearanceStyle(v: string | null | undefined): 'glass' | 'mono' {
  return v === 'mono' ? 'mono' : 'glass'
}
