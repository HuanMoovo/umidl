/**
 * 设置页页签表回归测试（布局重构：7 → 5）
 *
 * 守住两件事：
 *  1) 合并后的页签数量与顺序稳定（避免又被拆回 7 个）；
 *  2) 旧深链 ?tab=tools / ?tab=defaults 仍然映射到合并后的页签（零入口丢失）。
 */
import { describe, expect, it } from 'vitest'
import { LEGACY_SETTINGS_TABS, SETTINGS_TABS, resolveSettingsTab } from '@/services/settingsTabs'

describe('设置页页签（合并后）', () => {
  it('合并为 5 个页签，顺序固定', () => {
    expect(SETTINGS_TABS).toEqual(['engine', 'general', 'system', 'appearance', 'plugins'])
  })

  it('新页签名直接落位', () => {
    for (const t of SETTINGS_TABS) expect(resolveSettingsTab(t)).toBe(t)
  })

  it('旧深链不失效：tools → engine、defaults → general', () => {
    expect(LEGACY_SETTINGS_TABS).toEqual({ tools: 'engine', defaults: 'general' })
    expect(resolveSettingsTab('tools')).toBe('engine')
    expect(resolveSettingsTab('defaults')).toBe('general')
  })

  it('未知 / 缺省页签返回 null（保持当前页签，不跳空页）', () => {
    expect(resolveSettingsTab(undefined)).toBeNull()
    expect(resolveSettingsTab(null)).toBeNull()
    expect(resolveSettingsTab('')).toBeNull()
    expect(resolveSettingsTab('nope')).toBeNull()
  })
})
