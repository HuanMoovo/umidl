import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { DEFAULT_QUEUE_LAYOUT, FALLBACK_SETTINGS, useSettingsStore } from '@/stores/settings'
import type { AppSettings } from '@/types'
import * as ipc from '@/services/ipc'

// 后端不可用（浏览器 / 单测）时也能跑：只 mock IPC 层
vi.mock('@/services/ipc', () => ({
  getSettings: vi.fn(),
  saveSettings: vi.fn(),
  defaultDownloadDir: vi.fn(),
}))

const LAYOUT_KEY = 'umi.queue_layout'

/** 后端返回的设置（Rust 结构体里没有 queue_layout，模拟真实往返） */
const backendSettings = { ...FALLBACK_SETTINGS, download_dir: 'D:/downloads' } as AppSettings

beforeEach(() => {
  localStorage.clear()
  vi.mocked(ipc.getSettings).mockReset().mockResolvedValue({ ...backendSettings })
  vi.mocked(ipc.defaultDownloadDir).mockReset().mockResolvedValue('D:/default')
  // 模拟 save_settings：Rust 端反序列化时丢弃未声明字段（queue_layout）
  vi.mocked(ipc.saveSettings).mockReset().mockImplementation(async (s: AppSettings) => {
    const copy: Record<string, unknown> = { ...s }
    delete copy.queue_layout
    return copy as unknown as AppSettings
  })
  setActivePinia(createPinia())
})

describe('设置：下载队列布局 queue_layout', () => {
  it('默认值为 table（下载队列只有详细列表一种布局），未持久化过时取默认值', () => {
    expect(FALLBACK_SETTINGS.queue_layout).toBe('table')
    expect(DEFAULT_QUEUE_LAYOUT).toBe('table')
    expect(useSettingsStore().queueLayout).toBe('table')
  })

  it('切换到 table：立即生效、写入设置字段并落盘保存', async () => {
    const store = useSettingsStore()
    await store.setQueueLayout('table')

    expect(store.settings.queue_layout).toBe('table')
    expect(store.queueLayout).toBe('table')
    expect(localStorage.getItem(LAYOUT_KEY)).toBe('table')
    // 保存请求里带上了该字段
    const sent = vi.mocked(ipc.saveSettings).mock.calls[0][0]
    expect(sent.queue_layout).toBe('table')
    // 后端往返丢字段后，内存里仍是用户选的 table（不会被打回 card）
    expect(store.settings.queue_layout).toBe('table')
  })

  it('load() 时后端不回传 queue_layout：队列仍是 table', async () => {
    const store = useSettingsStore()
    await store.setQueueLayout('table')

    setActivePinia(createPinia()) // 模拟重新进入页面 / 重启后重新加载设置
    const fresh = useSettingsStore()
    await fresh.load()

    expect(fresh.queueLayout).toBe('table')
    expect(fresh.settings.queue_layout).toBe('table')
  })

  it('重启后（内存丢失、只有本地镜像）布局仍然是 table', () => {
    localStorage.setItem(LAYOUT_KEY, 'table')
    const store = useSettingsStore()
    expect(store.queueLayout).toBe('table')
  })

  it('历史遗留的 card 偏好被忽略：本地镜像 / 后端字段都按 table 处理', async () => {
    // 旧版本可能把 'card' 留在本地镜像里（卡片模式与切换按钮已从下载页移除）
    localStorage.setItem(LAYOUT_KEY, 'card')
    const store = useSettingsStore()
    expect(store.queueLayout).toBe('table')
    expect(store.settings.queue_layout).toBe('table')

    // 后端若回传 card（理论上不会），load() 之后依然固定 table
    vi.mocked(ipc.getSettings).mockResolvedValue({ ...backendSettings, queue_layout: 'card' } as AppSettings)
    setActivePinia(createPinia())
    const fresh = useSettingsStore()
    await fresh.load()

    expect(fresh.queueLayout).toBe('table')
    expect(fresh.settings.queue_layout).toBe('table')
  })

  it('保存失败（无桌面后端）时切换仍生效且不抛错', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    vi.mocked(ipc.saveSettings).mockRejectedValue(new Error('命令 save_settings 需要在桌面客户端中运行'))
    const store = useSettingsStore()

    await expect(store.setQueueLayout('table')).resolves.toBe('table')
    expect(store.queueLayout).toBe('table')
    expect(localStorage.getItem(LAYOUT_KEY)).toBe('table')

    // 历史值 'card' 仍可写入镜像且不抛错（兼容旧调用），但界面已不使用它
    await expect(store.setQueueLayout('card')).resolves.toBe('card')
    expect(localStorage.getItem(LAYOUT_KEY)).toBe('card')
    warn.mockRestore()
  })
})
