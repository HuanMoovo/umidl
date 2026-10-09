/**
 * UI 布局重构回归测试（静态断言，守住「一套写法」）
 *
 * 重构前的问题：同一个东西有多种写法 ——
 *   `.umi-btn !py-1.5 !text-[11px]` / `.umi-btn !px-2 !py-1 !text-[11px]` / 手写 `s-border-soft
 *   s-surface-2 rounded-xl border` / `s-text-3 text-[10.5px] leading-relaxed` 散落各处。
 * 重构后统一成设计令牌：.umi-card-title / .umi-btn-sm / .umi-btn-xs / .umi-inner / .umi-hint /
 *   .umi-page / .umi-seg / .umi-entry / .umi-row。
 *
 * 这里用文件扫描把这些约定固化，避免后续又各自发挥。
 */
import { readFileSync, readdirSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const ROOT = resolve(__dirname, '..')
const VIEWS = join(ROOT, 'views')
const COMPONENTS = join(ROOT, 'components')

function read(p: string): string {
  return readFileSync(p, 'utf-8')
}

/** 本次重构覆盖的文件（不含并行代理负责的 Converter.vue 与未纳入范围的 TaskTable.vue） */
const REFACTORED = [
  'views/Home.vue',
  'views/Download.vue',
  'views/Subtitle.vue',
  'views/Settings.vue',
  'components/SettingsV14.vue',
  'components/SettingsPlugins.vue',
  'components/BatchImportPanel.vue',
  'components/TaskCard.vue',
  'components/SideBar.vue',
  'components/TitleBar.vue',
]

/** 已被令牌取代的旧写法（出现即视为回退） */
const LEGACY_PATTERNS: { pattern: RegExp; hint: string }[] = [
  { pattern: /!py-1\.5\s+!text-\[11px\]/, hint: '改用 umi-btn umi-btn-sm' },
  { pattern: /!px-2(\.5)?\s+!py-1\s+!text-\[11px\]/, hint: '改用 umi-btn umi-btn-xs' },
  { pattern: /!py-1\s+!text-\[11px\]/, hint: '改用 umi-btn-sm' },
  { pattern: /s-border-soft\s+s-surface-2\s+rounded-xl\s+border/, hint: '改用 umi-inner' },
  { pattern: /s-text-3\s+text-\[10\.5px\]\s+leading-relaxed/, hint: '改用 umi-hint' },
  { pattern: /mx-auto max-w-5xl space-y-5/, hint: '改用 umi-page' },
]

describe('UI 设计令牌', () => {
  it('style.css 定义了统一令牌', () => {
    const css = read(join(ROOT, 'style.css'))
    for (const tok of ['.umi-page', '.umi-card-title', '.umi-inner', '.umi-btn-sm', '.umi-btn-xs', '.umi-hint', '.umi-seg', '.umi-entry', '.umi-row']) {
      expect(css, `style.css 缺少 ${tok}`).toContain(tok)
    }
  })

  it('重构覆盖的文件里不再出现旧写法', () => {
    const hits: string[] = []
    for (const rel of REFACTORED) {
      const src = read(join(ROOT, rel))
      for (const { pattern, hint } of LEGACY_PATTERNS) {
        const m = src.match(pattern)
        if (m) hits.push(`${rel}: ${m[0]} → ${hint}`)
      }
    }
    expect(hits).toEqual([])
  })

  it('带标题的页面都走 umi-card-title（字号/间距只有一份）', () => {
    // Home 是 Hero 页，卡片不带小节标题，不在此列
    const missing: string[] = []
    for (const rel of ['views/Download.vue', 'views/Subtitle.vue', 'views/Settings.vue']) {
      const src = read(join(ROOT, rel))
      if (!src.includes('umi-card-title') && !src.includes('umi-head')) missing.push(rel)
    }
    expect(missing).toEqual([])
  })

  it('卡片内边距统一（umi-card 只配 p-4 / p-5，不再出现 p-3.5 等杂值）', () => {
    const bad: string[] = []
    for (const rel of REFACTORED) {
      const src = read(join(ROOT, rel))
      for (const m of src.matchAll(/umi-card[^"]*\bp-3\.5\b/g)) bad.push(`${rel}: ${m[0]}`)
    }
    expect(bad).toEqual([])
  })

  it('卡片预算是收敛的（合并后每文件卡片数不反弹）', () => {
    const budget: Record<string, number> = {
      'views/Home.vue': 2,
      'views/Settings.vue': 8,
      'components/SettingsV14.vue': 6,
      'components/SettingsPlugins.vue': 4,
    }
    for (const [rel, max] of Object.entries(budget)) {
      const n = (read(join(ROOT, rel)).match(/class="umi-card[ "]/g) || []).length
      expect(n, `${rel} 卡片数 ${n} 超出预算 ${max}`).toBeLessThanOrEqual(max)
    }
  })

  it('空态不再嵌一张卡片（原插件页 py-10 空卡已内联）', () => {
    const src = read(join(ROOT, 'components/SettingsPlugins.vue'))
    expect(src).not.toMatch(/umi-card s-text-3 flex flex-col items-center/)
  })
})
