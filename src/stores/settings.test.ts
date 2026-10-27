import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
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

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}

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

afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe('settings request ordering', () => {
  it('settles a stalled initial read and ignores its response after a successful retry', async () => {
    vi.useFakeTimers()
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const pending = deferred<AppSettings>()
    vi.mocked(ipc.getSettings).mockReturnValueOnce(pending.promise).mockResolvedValue({ ...backendSettings, concurrency: 6 })
    const store = useSettingsStore()
    let settled = false
    const loading = store.load().then(() => { settled = true })
    store.settings.theme = 'light'
    await vi.advanceTimersByTimeAsync(10_000)
    expect(settled).toBe(true)
    await loading
    expect(store.loaded).toBe(true)
    expect(store.loadFailed).toBe(true)
    expect(store.settings.theme).toBe('light')

    await store.load()
    expect(store.loadFailed).toBe(false)
    expect(store.settings.concurrency).toBe(6)
    pending.resolve({ ...backendSettings, concurrency: 3 })
    await Promise.resolve()
    expect(store.settings.concurrency).toBe(6)
  })

  it('retains loaded settings if only the default-directory lookup fails', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    vi.mocked(ipc.getSettings).mockResolvedValue({ ...backendSettings, download_dir: '', theme: 'light', concurrency: 6 })
    vi.mocked(ipc.defaultDownloadDir).mockRejectedValue(new Error('directory unavailable'))
    const store = useSettingsStore()
    await store.load()

    expect(store.settings.theme).toBe('light')
    expect(store.settings.concurrency).toBe(6)
    expect(store.loaded).toBe(true)
  })

  it('hydrates persisted fields before saving an edit made during the initial load', async () => {
    const pending = deferred<AppSettings>()
    vi.mocked(ipc.getSettings).mockReturnValue(pending.promise)
    const store = useSettingsStore()
    const loading = store.load()
    store.settings.theme = 'light'
    const saving = store.save()
    await Promise.resolve()
    expect(ipc.saveSettings).not.toHaveBeenCalled()
    pending.resolve({ ...backendSettings, concurrency: 6 })
    await Promise.all([loading, saving])

    const persisted = vi.mocked(ipc.saveSettings).mock.calls[0][0]
    expect(persisted.concurrency).toBe(6)
    expect(persisted.theme).toBe('light')
    expect(store.settings.concurrency).toBe(6)
    expect(store.settings.theme).toBe('light')
  })

  it('reads settings after an already pending save has persisted', async () => {
    const pending = deferred<AppSettings>()
    let persisted = { ...backendSettings }
    vi.mocked(ipc.saveSettings).mockImplementation(async () => {
      persisted = await pending.promise
      return persisted
    })
    vi.mocked(ipc.getSettings).mockImplementation(async () => persisted)
    const store = useSettingsStore()
    store.settings.theme = 'light'
    const saving = store.save()
    const loading = store.load()
    await Promise.resolve()
    expect(ipc.getSettings).not.toHaveBeenCalled()
    pending.resolve({ ...backendSettings, theme: 'light' })
    await Promise.all([saving, loading])

    expect(store.settings.theme).toBe('light')
  })

  it('coalesces concurrent loads and preserves edits made before the response', async () => {
    const pending = deferred<AppSettings>()
    vi.mocked(ipc.getSettings).mockReturnValue(pending.promise)
    const store = useSettingsStore()
    const first = store.load()
    const second = store.load()
    store.settings.theme = 'light'
    store.patch({ download_dir: 'D:/chosen' })

    pending.resolve({ ...backendSettings, concurrency: 6 })
    await Promise.all([first, second])

    expect(store.settings.theme).toBe('light')
    expect(store.settings.download_dir).toBe('D:/chosen')
    expect(store.settings.concurrency).toBe(6)
    expect(ipc.getSettings).toHaveBeenCalledTimes(1)
    expect(store.loaded).toBe(true)
  })

  it('keeps a chosen directory while the default-directory lookup is pending', async () => {
    const pending = deferred<string>()
    vi.mocked(ipc.getSettings).mockResolvedValue({ ...backendSettings, download_dir: '' })
    vi.mocked(ipc.defaultDownloadDir).mockReturnValue(pending.promise)
    const store = useSettingsStore()
    const loading = store.load()
    await vi.waitFor(() => expect(ipc.defaultDownloadDir).toHaveBeenCalledTimes(1))
    store.settings.download_dir = 'D:/chosen'
    pending.resolve('D:/default')
    await loading

    expect(store.settings.download_dir).toBe('D:/chosen')
  })

  it('keeps edits made during a save and applies normalization to untouched fields', async () => {
    const pending = deferred<AppSettings>()
    vi.mocked(ipc.saveSettings).mockReturnValue(pending.promise)
    const store = useSettingsStore()
    store.patch({ concurrency: 99 })
    const saving = store.save()
    store.settings.theme = 'light'
    store.patch({ download_dir: 'D:/chosen' })
    pending.resolve({ ...backendSettings, concurrency: 16 })
    await saving

    expect(store.settings.theme).toBe('light')
    expect(store.settings.download_dir).toBe('D:/chosen')
    expect(store.settings.concurrency).toBe(16)
    expect(store.saving).toBe(false)
  })

  it('preserves a field edited back to its initial value while loading', async () => {
    const pending = deferred<AppSettings>()
    vi.mocked(ipc.getSettings).mockReturnValue(pending.promise)
    const store = useSettingsStore()
    const loading = store.load()
    store.settings.theme = 'light'
    store.settings.theme = 'dark'
    pending.resolve({ ...backendSettings, theme: 'light' })
    await loading

    expect(store.settings.theme).toBe('dark')
  })

  it('writes rapid saves in request order and stays busy until the last write finishes', async () => {
    const firstResponse = deferred<AppSettings>()
    const secondResponse = deferred<AppSettings>()
    let persistedTheme = 'dark'
    vi.mocked(ipc.saveSettings).mockImplementation(async (snapshot) => {
      const response = await (snapshot.theme === 'light' ? firstResponse.promise : secondResponse.promise)
      persistedTheme = response.theme
      return response
    })
    const store = useSettingsStore()
    store.settings.theme = 'light'
    const first = store.save()
    store.settings.theme = 'dark'
    const second = store.save()
    await vi.waitFor(() => expect(ipc.saveSettings).toHaveBeenCalled())

    expect(ipc.saveSettings).toHaveBeenCalledTimes(1)
    firstResponse.resolve({ ...backendSettings, theme: 'light' })
    await first
    expect(store.saving).toBe(true)
    expect(store.settings.theme).toBe('dark')
    secondResponse.resolve({ ...backendSettings, theme: 'dark' })
    await second

    expect(persistedTheme).toBe('dark')
    expect(store.saving).toBe(false)
  })

  it('allows a later save to succeed after an earlier queued write fails', async () => {
    const pending = deferred<AppSettings>()
    vi.mocked(ipc.saveSettings).mockReturnValueOnce(pending.promise).mockResolvedValueOnce({ ...backendSettings, theme: 'light' })
    const store = useSettingsStore()
    const first = store.save().catch((error) => error)
    store.settings.theme = 'light'
    const second = store.save()
    pending.reject(new Error('disk busy'))
    expect(await first).toBeInstanceOf(Error)
    await second

    expect(store.settings.theme).toBe('light')
    expect(store.saving).toBe(false)
  })
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
