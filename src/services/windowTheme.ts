/**
 * 窗口级主题同步：
 * - light/dark → 强制窗口主题（WebView2 的 prefers-color-scheme 也跟着走，避免媒体查询与界面不一致）
 * - system    → 把窗口主题交还系统，系统昼夜切换才会真正传进 WebView
 * 同时监听 Tauri 的窗口主题变化事件（比 matchMedia 更可靠，两者都订阅以防漏报）
 */
import { invoke } from '@tauri-apps/api/core'

export async function syncWindowTheme(mode: string): Promise<void> {
  try {
    await invoke('set_window_theme', { mode: mode || 'system' })
  } catch {
    /* 非 Tauri 环境（浏览器预览）忽略 */
  }
}

/** 订阅窗口主题变化，返回取消订阅函数 */
export async function watchWindowTheme(cb: (dark: boolean) => void): Promise<() => void> {
  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    const un = await getCurrentWindow().onThemeChanged((e) => cb(e.payload === 'dark'))
    return un
  } catch {
    return () => {}
  }
}
