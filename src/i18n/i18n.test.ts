import { describe, expect, it } from 'vitest'
import { LOCALES, localeMessages, normalizeLocale } from '@/i18n'

const LANGS = ['zh', 'en', 'ja', 'fr'] as const

describe('i18n 语言包', () => {
  it('四种语言都有语言包', () => {
    for (const l of LANGS) {
      expect(localeMessages[l], `${l} 缺少语言包`).toBeTruthy()
    }
  })

  it('每种语言的条目数与 key 完全一致（避免漏译）', () => {
    const base = Object.keys(localeMessages.zh)
    expect(base.length).toBeGreaterThan(200)
    for (const l of LANGS) {
      const keys = Object.keys(localeMessages[l])
      const missing = base.filter((k) => !(k in localeMessages[l]))
      const extra = keys.filter((k) => !(k in localeMessages.zh))
      expect(missing, `${l} 缺少 ${missing.length} 条：${missing.slice(0, 5).join(' / ')}`).toEqual([])
      expect(extra, `${l} 多出 ${extra.length} 条：${extra.slice(0, 5).join(' / ')}`).toEqual([])
    }
  })

  it('译文非空、且占位符与中文源一致', () => {
    const ph = (s: string) => (s.match(/\{[a-z]+\}/gi) || []).sort().join(',')
    for (const l of LANGS) {
      for (const [key, val] of Object.entries(localeMessages[l])) {
        expect(val, `${l}: ${key} 译文为空`).toBeTruthy()
        expect(String(val).trim(), `${l}: ${key} 译文只有空白`).not.toBe('')
        if (l !== 'zh') {
          expect(ph(String(val)), `${l}: ${key} 占位符不匹配（源 ${ph(key)} / 译 ${ph(String(val))}）`).toBe(ph(key))
        }
      }
    }
  })

  it('normalizeLocale 容错（含系统语言变体）', () => {
    expect(normalizeLocale('zh-CN')).toBe('zh')
    expect(normalizeLocale('en-US')).toBe('en')
    expect(normalizeLocale('ja-JP')).toBe('ja')
    expect(normalizeLocale('fr-FR')).toBe('fr')
    expect(normalizeLocale('fr')).toBe('fr')
    expect(normalizeLocale('de-DE')).toBe('zh')
    expect(normalizeLocale(undefined)).toBe('zh')
    expect(normalizeLocale('')).toBe('zh')
  })

  it('语言列表包含中文/英语/日语/法语四个选项', () => {
    expect(LOCALES.map((l) => l.value)).toEqual(['zh', 'en', 'ja', 'fr'])
    for (const l of LOCALES) {
      expect(l.native.length).toBeGreaterThan(0)
      expect(l.short.length).toBeGreaterThan(0)
    }
  })
})
