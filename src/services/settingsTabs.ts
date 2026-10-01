/**
 * 设置页页签表（布局重构：7 个页签合并为 5 个）
 *
 * 合并方案：
 *   依赖工具 + 下载引擎      → engine（引擎）
 *   通用设置 + 默认参数      → general（通用）
 *   系统与集成               → system（系统）
 *   主题 / 强调色 / 自定义色 → appearance（外观）
 *   插件                     → plugins（插件）
 *
 * 旧深链 ?tab=tools / ?tab=defaults 仍要落位到合并后的页签，
 * 因此把名称表放在这里集中维护，并由单元测试守住。
 */
export type SettingsTab = 'engine' | 'general' | 'system' | 'appearance' | 'plugins'

export const SETTINGS_TABS: readonly SettingsTab[] = ['engine', 'general', 'system', 'appearance', 'plugins'] as const

/** 旧页签名 → 新页签名（1.4–1.7 期间的深链不能失效） */
export const LEGACY_SETTINGS_TABS: Readonly<Record<string, SettingsTab>> = {
  tools: 'engine',
  defaults: 'general',
}

/** 解析 ?tab= 深链：新名直接用，旧名映射，未知/缺省返回 null（保持当前页签） */
export function resolveSettingsTab(name?: string | null): SettingsTab | null {
  if (typeof name !== 'string' || !name) return null
  if ((SETTINGS_TABS as readonly string[]).includes(name)) return name as SettingsTab
  return LEGACY_SETTINGS_TABS[name] ?? null
}
