import { tr } from '@/i18n'
import { normalizeAppearanceStyle, parseGradient } from '@/utils/accent'
/**
 * 主题系统：深色 / 浅色 + 强调色
 * 所有令牌走 CSS 变量，切换时无需重新加载页面
 * （「跟随系统」已移除：历史配置里的 'system' 一律回退深色，不报错。）
 */

export type ThemeMode = 'dark' | 'light'
export type AccentName = 'violet' | 'cyan' | 'pink' | 'emerald' | 'amber' | 'blue'

export function themeModes(): { value: ThemeMode; label: string; hint: string }[] {
  return [
  { value: 'light', label: tr('浅色'), hint: tr('白天模式') },
  { value: 'dark', label: tr('深色'), hint: tr('夜间模式') },
  ]
}

export function accents(): { value: AccentName; label: string; hex: string }[] {
  return [
  { value: 'violet', label: tr('紫罗兰'), hex: '#7c4dff' },
  { value: 'cyan', label: tr('青蓝'), hex: '#22d3ee' },
  { value: 'pink', label: tr('樱花粉'), hex: '#ec4899' },
  { value: 'emerald', label: tr('翡翠绿'), hex: '#10b981' },
  { value: 'amber', label: tr('琥珀金'), hex: '#f59e0b' },
    { value: 'blue', label: tr('天空蓝'), hex: '#3b82f6' },
  ]
}

export function systemPrefersDark(): boolean {
  if (typeof window === 'undefined') return true
  return !!window.matchMedia?.('(prefers-color-scheme: dark)').matches
}

/**
 * 主题值归一：只认 'light'，其余（含历史配置里的 'system' / 未知脏值）一律回退 'dark'。
 * 「跟随系统」已移除 —— 旧配置不需要迁移，读取时静默回退即可。
 */
export function normalizeTheme(mode: string | undefined | null): ThemeMode {
  return mode === 'light' ? 'light' : 'dark'
}

/** 把主题模式解析成实际生效的明暗（跟随系统已移除：'system' 与未知值都回退深色） */
export function resolveTheme(mode: ThemeMode | string | undefined): 'dark' | 'light' {
  return normalizeTheme(mode)
}

export function accentHex(accent: string | undefined): string {
  return accents().find((a) => a.value === accent)?.hex ?? '#7c4dff'
}

/** 页面当前实际生效的明暗：优先读 applyTheme 写下的 <html data-theme>，没有就按系统偏好 */
export function currentResolvedTheme(): 'dark' | 'light' {
  if (typeof document !== 'undefined') {
    const t = document.documentElement.dataset.theme
    if (t === 'dark' || t === 'light') return t
  }
  return systemPrefersDark() ? 'dark' : 'light'
}

/** 应用主题到 <html>，返回实际生效的明暗 */
export function applyTheme(mode: ThemeMode | string | undefined, accent: string | undefined): 'dark' | 'light' {
  const resolved = resolveTheme(mode)
  if (typeof document === 'undefined') return resolved
  const el = document.documentElement
  el.dataset.theme = resolved
  el.dataset.accent = accent || 'violet'
  el.classList.toggle('dark', resolved === 'dark')
  return resolved
}

/* ============================================================
   外观风格（1.10）：glass 玻璃拟态（默认） | mono 黑白简约
   写 <html data-style>，具体覆盖规则在 style.css 的 [data-style='mono'] 作用域里；
   这里只负责把值落到 DOM，纯校验住在 utils/accent.ts（可单测）。
   ============================================================ */
export function applyAppearanceStyle(style: string | null | undefined): 'glass' | 'mono' {
  const resolved = normalizeAppearanceStyle(style)
  if (typeof document !== 'undefined') document.documentElement.dataset.style = resolved
  return resolved
}

/* ============================================================
   自定义渐变（1.10）：'起色,止色' → --accent-grad-a / --accent-grad-b
   非空且合法时注入根元素（侧栏选中项、主按钮、标题渐变共用）；
   空 / 非法时清除变量，样式自动回落到强调色渐变（现状）。
   ============================================================ */
export function applyAccentGradient(gradient: string | null | undefined): boolean {
  const g = parseGradient(gradient)
  if (!g || typeof document === 'undefined') {
    clearAccentGradient()
    return false
  }
  const el = document.documentElement
  el.style.setProperty('--accent-grad-a', g.from)
  el.style.setProperty('--accent-grad-b', g.to)
  el.dataset.grad = '1'
  return true
}

/** 清除自定义渐变变量（回到强调色渐变） */
export function clearAccentGradient(): void {
  if (typeof document === 'undefined') return
  const el = document.documentElement
  el.style.removeProperty('--accent-grad-a')
  el.style.removeProperty('--accent-grad-b')
  delete el.dataset.grad
}

/**
 * 监听系统昼夜变化（跟随系统已移除：保留为纯工具函数 / 单测对象，不再接入界面）。
 * 用法：返回取消订阅函数。
 */
export function onSystemThemeChange(cb: (dark: boolean) => void): () => void {
  if (typeof window === 'undefined' || !window.matchMedia) return () => {}
  const mq = window.matchMedia('(prefers-color-scheme: dark)')
  const handler = (e: MediaQueryListEvent) => cb(e.matches)
  mq.addEventListener?.('change', handler)
  return () => mq.removeEventListener?.('change', handler)
}

/**
 * 系统昼夜监听的强力版：**媒体查询事件 + 定时轮询 + 回到窗口复核**，三条路一起上。
 * （跟随系统已移除：保留为纯工具函数 / 单测对象，不再接入界面。）
 *
 * 为什么不能只订阅媒体查询事件：`prefers-color-scheme` 的变化依赖**系统广播**
 * （Windows 的 WM_SETTINGCHANGE / ImmersiveColorSet）。实测中，第三方自动昼夜
 * 工具、计划任务或某些“夜间自动切换”方案**只改注册表、不广播**，事件永远不会来，
 * 界面就停在旧主题；窗口长时间最小化时事件也可能积压。轮询（默认 4 秒）兜住
 * 「注册表变了但没广播」，focus / visibilitychange 则保证切回窗口时立刻正确。
 */
export function startSystemThemeWatch(cb: (dark: boolean) => void, everyMs = 4000): () => void {
  if (typeof window === 'undefined') return () => {}
  let last = systemPrefersDark()
  const fire = (dark: boolean) => {
    if (dark === last) return
    last = dark
    cb(dark)
  }
  const mq = window.matchMedia?.('(prefers-color-scheme: dark)')
  const onMq = (e: MediaQueryListEvent) => fire(!!e.matches)
  mq?.addEventListener?.('change', onMq)
  const timer = setInterval(() => fire(systemPrefersDark()), everyMs)
  const onWake = () => fire(systemPrefersDark())
  if (typeof document !== 'undefined') document.addEventListener('visibilitychange', onWake)
  window.addEventListener('focus', onWake)
  return () => {
    mq?.removeEventListener?.('change', onMq)
    clearInterval(timer)
    if (typeof document !== 'undefined') document.removeEventListener('visibilitychange', onWake)
    window.removeEventListener('focus', onWake)
  }
}

/* ============================================================
   自定义强调色：hex → <html> 内联 CSS 变量
     --umi-a50 … --umi-a950   色阶（"R G B" 三元组，tailwind 的 rgb(var(--umi-a500)) 直接用）
     --umi-accent / --umi-accent-hex  基础色（hex 字符串）
     --umi-accent-text        强调色文字（"R G B"）：深色主题取亮档 a300，浅色主题取深档 a700
   算法：按 HSL 明度偏移生成色阶，基础色过亮时收敛到 58% 亮度，保证按钮上的白字对比度。
   写的是内联变量（优先级高于 html[data-accent] 预设），清掉即回到预设色。
   ============================================================ */
export interface AccentRgb {
  r: number
  g: number
  b: number
}

/** 色阶骨架：与 style.css 预设色阶相同的明度偏移（相对基础色亮度） */
const SCALE_STEPS: { key: string; dl: number }[] = [
  { key: '50', dl: 32 },
  { key: '100', dl: 29 },
  { key: '200', dl: 24 },
  { key: '300', dl: 15 },
  { key: '400', dl: 7 },
  { key: '500', dl: 0 },
  { key: '600', dl: -7 },
  { key: '700', dl: -15 },
  { key: '800', dl: -24 },
  { key: '900', dl: -32 },
  { key: '950', dl: -43 },
]

/** "#7c4dff" / "7c4dff" → { r, g, b }；不合法返回 null */
export function parseAccentHex(hex: string): AccentRgb | null {
  const m = /^#?([0-9a-fA-F]{6})$/.exec(String(hex ?? '').trim())
  if (!m) return null
  const n = parseInt(m[1], 16)
  return { r: (n >> 16) & 255, g: (n >> 8) & 255, b: n & 255 }
}

/** 统一成小写 "#rrggbb"（前导 # 可省略） */
export function normalizeAccentHex(hex: string): string {
  const t = String(hex ?? '').trim()
  return (t.startsWith('#') ? t : `#${t}`).toLowerCase()
}

function rgbToHsl({ r, g, b }: AccentRgb) {
  const R = r / 255
  const G = g / 255
  const B = b / 255
  const max = Math.max(R, G, B)
  const min = Math.min(R, G, B)
  const l = (max + min) / 2
  const d = max - min
  let h = 0
  let s = 0
  if (d !== 0) {
    s = d / (1 - Math.abs(2 * l - 1))
    if (max === R) h = ((G - B) / d) % 6
    else if (max === G) h = (B - R) / d + 2
    else h = (R - G) / d + 4
    h *= 60
    if (h < 0) h += 360
  }
  return { h, s, l }
}

function hslToRgb(h: number, s: number, l: number): AccentRgb {
  const c = (1 - Math.abs(2 * l - 1)) * s
  const hp = ((((h % 360) + 360) % 360) / 60)
  const x = c * (1 - Math.abs((hp % 2) - 1))
  const seg =
    hp < 1 ? [c, x, 0] : hp < 2 ? [x, c, 0] : hp < 3 ? [0, c, x] : hp < 4 ? [0, x, c] : hp < 5 ? [x, 0, c] : [c, 0, x]
  const m = l - c / 2
  return {
    r: Math.round((seg[0] + m) * 255),
    g: Math.round((seg[1] + m) * 255),
    b: Math.round((seg[2] + m) * 255),
  }
}

/** 基础色亮度收敛区间：过亮压到 58%（白字可读），过暗提到 36% */
function clampBaseLightness(percent: number): number {
  return Math.min(Math.max(percent, 36), 58)
}

function rgbTriple({ r, g, b }: AccentRgb): string {
  return `${r} ${g} ${b}`
}

/** 从基础色推导 50–950 色阶；基础色过亮时收窄亮度，保证按钮白字对比度。非法 hex 返回 null */
export function buildAccentScale(hex: string): Record<string, string> | null {
  const rgb = parseAccentHex(hex)
  if (!rgb) return null
  const { h, s, l } = rgbToHsl(rgb)
  const baseL = clampBaseLightness(l * 100)
  const out: Record<string, string> = {}
  for (const step of SCALE_STEPS) {
    out[step.key] = rgbTriple(hslToRgb(h, s, Math.min(98, Math.max(4, baseL + step.dl)) / 100))
  }
  return out
}

/** 基础色是否被自动加深（亮色会被压暗以保证白字可读），设置页据此给提示 */
export function isAccentAutoDarkened(hex: string): boolean {
  const rgb = parseAccentHex(hex)
  if (!rgb) return false
  return rgbToHsl(rgb).l * 100 > 58
}

/** 自定义色的基准色（hex）：与色阶同一套亮度收敛，用作 naive-ui 组件主色。非法 hex 返回 null */
export function customAccentBaseHex(hex: string): string | null {
  const rgb = parseAccentHex(hex)
  if (!rgb) return null
  const { h, s, l } = rgbToHsl(rgb)
  const base = hslToRgb(h, s, clampBaseLightness(l * 100) / 100)
  return `#${[base.r, base.g, base.b].map((n) => n.toString(16).padStart(2, '0')).join('')}`
}

/** 自定义强调色当前是否已生效（<html> 上存在内联覆盖） */
let customAccentApplied = false
export function isCustomAccentApplied(): boolean {
  return customAccentApplied
}

/**
 * 应用自定义强调色：写 --umi-a50…a950 / --umi-accent / --umi-accent-hex / --umi-accent-text。
 * dark 省略时按当前 <html data-theme> 判断（主题切换后重算 accent-text 用得上）。
 * hex 为空 / 非法 / 无 document 时返回 false —— 调用方应转而 clearCustomAccent()。
 */
export function applyCustomAccent(hex: string | null | undefined, dark?: boolean): boolean {
  const scale = buildAccentScale(String(hex ?? ''))
  if (!scale || typeof document === 'undefined') return false
  const el = document.documentElement
  const value = normalizeAccentHex(String(hex))
  for (const step of SCALE_STEPS) el.style.setProperty(`--umi-a${step.key}`, scale[step.key])
  el.style.setProperty('--umi-accent-hex', value)
  el.style.setProperty('--umi-accent', value)
  const isDark = dark ?? currentResolvedTheme() === 'dark'
  el.style.setProperty('--umi-accent-text', isDark ? scale['300'] : scale['700'])
  customAccentApplied = true
  return true
}

/** 撤掉自定义强调色覆盖（回到 html[data-accent] 预设色） */
export function clearCustomAccent(): void {
  customAccentApplied = false
  if (typeof document === 'undefined') return
  const el = document.documentElement
  for (const step of SCALE_STEPS) el.style.removeProperty(`--umi-a${step.key}`)
  el.style.removeProperty('--umi-accent-hex')
  el.style.removeProperty('--umi-accent')
  el.style.removeProperty('--umi-accent-text')
}

/**
 * 强调色取值：预设名（violet…）查表；自定义 hex（#rrggbb）走同一套亮度收敛后使用，
 * 两者都识别不了时回退默认紫罗兰。naive-ui 主色与 CSS 变量因此保持一致。
 */
export function accentColorHex(accent: string | undefined | null): string {
  const v = String(accent ?? '').trim()
  if (/^#?[0-9a-fA-F]{6}$/.test(v)) return customAccentBaseHex(v) ?? accentHex(undefined)
  return accentHex(v)
}

/** 生成 naive-ui 的主题覆盖（跟随强调色与明暗）；accent 可以是预设名，也可以是自定义 hex。
 *  mono = 黑白简约（1.10）：强调色退化为黑 / 白反色，开关与进度条一起跟着变。 */
export function naiveOverrides(accent: string | undefined, dark: boolean, mono = false) {
  const hex = mono ? (dark ? '#ffffff' : '#101010') : accentColorHex(accent)
  const light = dark ? hex : shadeColor(hex, 0.08)
  return {
    common: {
      primaryColor: hex,
      primaryColorHover: light,
      primaryColorPressed: shadeColor(hex, -0.15),
      primaryColorSuppl: light,
      borderRadius: '12px',
      fontFamily: "'Inter','HarmonyOS Sans SC','Microsoft YaHei',system-ui,sans-serif",
      bodyColor: 'transparent',
      cardColor: dark ? 'rgba(255,255,255,0.045)' : 'rgba(255,255,255,0.75)',
      modalColor: dark ? 'rgba(26,26,44,0.96)' : 'rgba(255,255,255,0.98)',
      popoverColor: dark ? 'rgba(26,26,44,0.96)' : 'rgba(255,255,255,0.98)',
      borderColor: dark ? 'rgba(255,255,255,0.10)' : 'rgba(24,24,70,0.13)',
      textColorBase: dark ? 'rgba(255,255,255,0.92)' : 'rgba(20,20,44,0.94)',
    },
    // 黑白简约：白色轨道上用黑滑块（反之亦然），避免滑点在轨道上「消失」
    Switch: mono
      ? { railColorActive: hex, buttonColor: dark ? '#000000' : '#ffffff' }
      : {},
    Slider: mono ? { fillColor: hex, handleColor: dark ? '#000000' : '#ffffff' } : {},
    Checkbox: mono
      ? { colorChecked: hex, borderChecked: hex, checkMarkColor: dark ? '#000000' : '#ffffff' }
      : {},
    Card: {
      color: dark ? 'rgba(255,255,255,0.04)' : 'rgba(255,255,255,0.72)',
      borderColor: dark ? 'rgba(255,255,255,0.08)' : 'rgba(24,24,70,0.10)',
    },
    Input: { color: dark ? 'rgba(0,0,0,0.25)' : 'rgba(255,255,255,0.85)', colorFocus: dark ? 'rgba(0,0,0,0.35)' : '#fff' },
    Select: { peers: { InternalSelection: { color: dark ? 'rgba(0,0,0,0.25)' : 'rgba(255,255,255,0.85)' } } },
    Progress: {
      railColor: dark ? 'rgba(255,255,255,0.08)' : 'rgba(24,24,70,0.10)',
      fillColor: mono ? hex : `linear-gradient(90deg,${hex},#22d3ee)`,
    },
  }
}

function shadeColor(hex: string, amount: number): string {
  const m = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex)
  if (!m) return hex
  const nums = [1, 2, 3].map((i) => {
    const v = parseInt(m[i], 16)
    const t = amount >= 0 ? 255 : 0
    const p = Math.abs(amount)
    return Math.round(v + (t - v) * p)
  })
  return `#${nums.map((n) => n.toString(16).padStart(2, '0')).join('')}`
}
