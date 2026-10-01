import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { resolveTheme, startSystemThemeWatch, systemPrefersDark } from './theme'

/**
 * 可编程的 matchMedia 桩：能改 matches，也能模拟「只改状态、不触发事件」
 * （那种情况下系统没广播，只有轮询能发现）。
 */
function stubMatchMedia(initialDark: boolean) {
  type Listener = (e: { matches: boolean }) => void
  const listeners = new Set<Listener>()
  const state = { matches: initialDark }
  const mql = {
    get matches() {
      return state.matches
    },
    media: '(prefers-color-scheme: dark)',
    onchange: null,
    addEventListener: (_: string, fn: Listener) => void listeners.add(fn),
    removeEventListener: (_: string, fn: Listener) => void listeners.delete(fn),
    addListener: (fn: Listener) => void listeners.add(fn),
    removeListener: (fn: Listener) => void listeners.delete(fn),
    dispatchEvent: () => true,
  }
  ;(window as unknown as { matchMedia: unknown }).matchMedia = () => mql
  return {
    /** emit=false 表示静默变化（模拟没广播：注册表改了但没人通知） */
    set(dark: boolean, emit = true) {
      state.matches = dark
      if (emit) listeners.forEach((fn) => fn({ matches: dark }))
    },
  }
}

describe('主题：跟随系统的三条路', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  it('系统广播了变化 → 媒体查询事件立刻回调', () => {
    const mq = stubMatchMedia(false)
    const cb = vi.fn()
    const stop = startSystemThemeWatch(cb)
    mq.set(true)
    expect(cb).toHaveBeenCalledTimes(1)
    expect(cb).toHaveBeenCalledWith(true)
    stop()
  })

  it('没广播（只改了注册表）→ 轮询兜住', () => {
    const mq = stubMatchMedia(false)
    const cb = vi.fn()
    const stop = startSystemThemeWatch(cb, 1000)
    mq.set(true, false) // 静默变化
    expect(cb).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1100)
    expect(cb).toHaveBeenCalledWith(true)
    stop()
  })

  it('窗口重新获得焦点 → 立刻复核，不等下一个轮询周期', () => {
    const mq = stubMatchMedia(false)
    const cb = vi.fn()
    const stop = startSystemThemeWatch(cb, 60_000)
    mq.set(true, false)
    window.dispatchEvent(new Event('focus'))
    expect(cb).toHaveBeenCalledWith(true)
    stop()
  })

  it('取消失去订阅后不再回调', () => {
    const mq = stubMatchMedia(false)
    const cb = vi.fn()
    const stop = startSystemThemeWatch(cb, 500)
    stop()
    mq.set(true)
    vi.advanceTimersByTime(3000)
    window.dispatchEvent(new Event('focus'))
    expect(cb).not.toHaveBeenCalled()
  })

  it('值没变就不重复回调（轮询不会刷回调）', () => {
    const mq = stubMatchMedia(true)
    const cb = vi.fn()
    const stop = startSystemThemeWatch(cb, 500)
    mq.set(true) // 同值事件
    vi.advanceTimersByTime(2000)
    expect(cb).not.toHaveBeenCalled()
    stop()
  })

  it('resolveTheme：system 走系统偏好，显式值原样返回', () => {
    const mq = stubMatchMedia(true)
    expect(systemPrefersDark()).toBe(true)
    expect(resolveTheme('system')).toBe('dark')
    mq.set(false)
    expect(resolveTheme('system')).toBe('light')
    expect(resolveTheme('light')).toBe('light')
    expect(resolveTheme('dark')).toBe('dark')
    expect(resolveTheme(undefined)).toBe('light')
  })
})
