/**
 * 界面偏好纯函数的单元测试（1.10）：
 *  - parseGradient / formatGradient：'起色,止色' 的解析与合法性校验
 *  - normalizeUiMode / normalizeAppearanceStyle：简单/高级、玻璃/黑白 的校验与回落
 */
import { describe, expect, it } from 'vitest'
import {
  formatGradient,
  normalizeAppearanceStyle,
  normalizeHex,
  normalizeUiMode,
  parseGradient,
} from '@/utils/accent'

describe('parseGradient：自定义渐变解析', () => {
  it('合法输入：两个十六进制色 → { from, to }（统一小写带 #）', () => {
    expect(parseGradient('#7c4dff,#22d3ee')).toEqual({ from: '#7c4dff', to: '#22d3ee' })
    // 前导 # 可省略；大小写归一
    expect(parseGradient('7C4DFF,22D3EE')).toEqual({ from: '#7c4dff', to: '#22d3ee' })
    // 两侧空白容忍
    expect(parseGradient(' #000000 , #ffffff ')).toEqual({ from: '#000000', to: '#ffffff' })
  })

  it('非法输入一律返回 null（空 / 单色 / 多段 / 非十六进制 / 简写）', () => {
    expect(parseGradient('')).toBeNull()
    expect(parseGradient('   ')).toBeNull()
    expect(parseGradient('#7c4dff')).toBeNull()
    expect(parseGradient('#7c4dff,#22d3ee,#000000')).toBeNull()
    expect(parseGradient('#7c4dff,zzzzzz')).toBeNull()
    expect(parseGradient('red,#22d3ee')).toBeNull()
    expect(parseGradient('#fff,#000')).toBeNull()
    expect(parseGradient(undefined)).toBeNull()
    expect(parseGradient(null)).toBeNull()
  })

  it('formatGradient 与 parseGradient 往返一致；非法色返回空串', () => {
    expect(formatGradient('#7C4DFF', '22d3ee')).toBe('#7c4dff,#22d3ee')
    expect(parseGradient(formatGradient('#7c4dff', '#22d3ee'))).toEqual({ from: '#7c4dff', to: '#22d3ee' })
    expect(formatGradient('nope', '#22d3ee')).toBe('')
    expect(formatGradient('#7c4dff', '')).toBe('')
  })

  it('normalizeHex：补 # 并转小写，非法返回 null', () => {
    expect(normalizeHex('ABCDEF')).toBe('#abcdef')
    expect(normalizeHex('#AbCdEf')).toBe('#abcdef')
    expect(normalizeHex('abc')).toBeNull()
    expect(normalizeHex('')).toBeNull()
  })
})

describe('ui_mode / appearance_style 校验', () => {
  it('ui_mode：只认 simple / advanced，其余回落 simple（默认）', () => {
    expect(normalizeUiMode('simple')).toBe('simple')
    expect(normalizeUiMode('advanced')).toBe('advanced')
    expect(normalizeUiMode(undefined)).toBe('simple')
    expect(normalizeUiMode(null)).toBe('simple')
    expect(normalizeUiMode('')).toBe('simple')
    expect(normalizeUiMode('SIMPLE')).toBe('simple')
    expect(normalizeUiMode('weird')).toBe('simple')
  })

  it('appearance_style：只认 glass / mono，其余回落 glass（现状）', () => {
    expect(normalizeAppearanceStyle('glass')).toBe('glass')
    expect(normalizeAppearanceStyle('mono')).toBe('mono')
    expect(normalizeAppearanceStyle(undefined)).toBe('glass')
    expect(normalizeAppearanceStyle(null)).toBe('glass')
    expect(normalizeAppearanceStyle('MONO')).toBe('glass')
    expect(normalizeAppearanceStyle('neon')).toBe('glass')
  })
})
