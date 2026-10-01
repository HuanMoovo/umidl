/**
 * 界面多语言（中 / 英 / 日 / 法）
 *
 * 采用「中文原文即 key」：t('保存') —— 缺失翻译时 vue-i18n 自动回退到 zh 目录，
 * 也就是原样显示中文，永远不会出现空白或 key 泄漏。
 */
import { createI18n } from 'vue-i18n'
import zh from './locales/zh'
import en from './locales/en'
import ja from './locales/ja'
import fr from './locales/fr'

export type LocaleName = 'zh' | 'en' | 'ja' | 'fr'

export const LOCALES: { value: LocaleName; label: string; native: string; code: string; short: string }[] = [
  { value: 'zh', label: '简体中文', native: '简体中文', code: 'zh-CN', short: '中' },
  { value: 'en', label: 'English', native: 'English', code: 'en-US', short: 'EN' },
  { value: 'ja', label: '日本語', native: '日本語', code: 'ja-JP', short: '日' },
  { value: 'fr', label: 'Français', native: 'Français', code: 'fr-FR', short: 'FR' },
]

const HTML_LANG: Record<LocaleName, string> = {
  zh: 'zh-CN',
  en: 'en',
  ja: 'ja',
  fr: 'fr',
}

export function normalizeLocale(v?: string | null): LocaleName {
  const s = (v || '').toLowerCase()
  if (s.startsWith('en')) return 'en'
  if (s.startsWith('ja') || s.startsWith('jp')) return 'ja'
  if (s.startsWith('fr')) return 'fr'
  return 'zh'
}

export const i18n = createI18n({
  legacy: false,
  globalInjection: true, // 模板里可直接用 $t('文案')
  locale: 'zh',
  fallbackLocale: 'zh',
  missingWarn: false,
  fallbackWarn: false,
  messages: { zh, en, ja, fr },
})

/** 切换界面语言（同时同步 <html lang>） */
export function setLocale(name: string | undefined) {
  const l = normalizeLocale(name)
  ;(i18n.global.locale as unknown as { value: LocaleName }).value = l
  if (typeof document !== 'undefined') document.documentElement.lang = HTML_LANG[l]
  return l
}

/** 当前界面语言（供数字/计量单位本地化） */
export function currentLocale(): LocaleName {
  return normalizeLocale((i18n.global.locale as unknown as { value: string }).value)
}

/** 非组件模块里使用的翻译函数（组件内请用 useI18n() 取 tr 以获得响应式） */
export function tr(key: string, named?: Record<string, unknown>): string {
  const fn = i18n.global.t as unknown as (k: string, n?: Record<string, unknown>) => string
  return named ? fn(key, named) : fn(key)
}

export const localeMessages = { zh, en, ja, fr }
