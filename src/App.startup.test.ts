import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, h, nextTick, type App } from 'vue'
import { createPinia, disposePinia, type Pinia } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import AppView from '@/App.vue'
import { i18n } from '@/i18n'
import { FALLBACK_SETTINGS, useSettingsStore } from '@/stores/settings'
import { useTaskStore } from '@/stores/tasks'
import * as ipc from '@/services/ipc'

vi.mock('@/components/SideBar.vue', () => ({ default: { render: () => null } }))
vi.mock('@/components/TitleBar.vue', () => ({ default: { render: () => null } }))
vi.mock('@/components/ParticleBg.vue', () => ({ default: { render: () => null } }))
vi.mock('@tauri-apps/api/core', async (importOriginal) => ({
  ...await importOriginal<typeof import('@tauri-apps/api/core')>(),
  invoke: vi.fn().mockResolvedValue(undefined),
}))
vi.mock('@/services/ipc', () => ({
  isTauri: vi.fn(),
  getSettings: vi.fn(), defaultDownloadDir: vi.fn(), listDownloads: vi.fn(), listConverts: vi.fn(),
  listSubtitles: vi.fn(), onEvent: vi.fn(), detectTools: vi.fn(), backfillCovers: vi.fn(),
}))

let app: App | undefined
let pinia: Pinia

async function flush() {
  for (let i = 0; i < 45; i++) await nextTick()
}

async function mount() {
  const host = document.createElement('div')
  document.body.append(host)
  pinia = createPinia()
  app = createApp(AppView)
  app.component('RouterView', { render: () => h('div', 'workspace') })
  app.use(pinia).use(i18n)
  app.mount(host)
  await flush()
}

beforeEach(() => {
  vi.mocked(ipc.isTauri).mockReturnValue(true)
  vi.spyOn(console, 'warn').mockImplementation(() => {})
  vi.stubGlobal('matchMedia', () => ({ matches: false, addEventListener() {}, removeEventListener() {} }))
  Object.defineProperty(performance, 'getEntriesByType', { configurable: true, value: () => [] })
  vi.mocked(invoke).mockResolvedValue(undefined)
  vi.mocked(ipc.getSettings).mockReset().mockResolvedValue({ ...FALLBACK_SETTINGS, download_dir: 'C:/Downloads' })
  vi.mocked(ipc.defaultDownloadDir).mockResolvedValue('C:/Downloads')
  vi.mocked(ipc.listDownloads).mockReset().mockResolvedValue([])
  vi.mocked(ipc.listConverts).mockReset().mockResolvedValue([])
  vi.mocked(ipc.listSubtitles).mockReset().mockResolvedValue([])
  vi.mocked(ipc.onEvent).mockReset().mockResolvedValue(() => {})
  vi.mocked(ipc.detectTools).mockResolvedValue([])
  vi.mocked(ipc.backfillCovers).mockResolvedValue(0)
})

afterEach(() => {
  app?.unmount()
  app = undefined
  if (pinia) disposePinia(pinia)
  document.body.innerHTML = ''
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})

describe('startup recovery UI', () => {
  it('identifies browser preview without showing a broken desktop startup banner', async () => {
    vi.mocked(ipc.isTauri).mockReturnValue(false)
    await mount()
    expect(document.querySelector('[data-test="startup-recovery"]')).toBeNull()
    expect(document.querySelector('[data-test="browser-preview"]')).not.toBeNull()
    expect(ipc.getSettings).not.toHaveBeenCalled()
  })
  it('unblocks the workspace after partial startup and retries only the missing queue', async () => {
    vi.mocked(ipc.listConverts).mockRejectedValueOnce(new Error('database unavailable')).mockResolvedValue([])
    await mount()

    expect(document.body.textContent).not.toContain('正在加载 Umidl…')
    expect(document.querySelector('[data-test="startup-recovery"]')).not.toBeNull()
    const button = document.querySelector<HTMLButtonElement>('[data-test="startup-retry"]')!
    button.click()
    button.click()
    await flush()

    expect(useTaskStore(pinia).startupFailed).toBe(false)
    expect(ipc.listDownloads).toHaveBeenCalledTimes(1)
    expect(ipc.listConverts).toHaveBeenCalledTimes(2)
    expect(document.querySelector('[data-test="startup-recovery"]')).toBeNull()
  })

  it('allows a failed settings read to be retried from the same recovery banner', async () => {
    vi.mocked(ipc.getSettings).mockRejectedValueOnce(new Error('settings unavailable'))
    await mount()

    expect(document.body.textContent).not.toContain('正在加载 Umidl…')
    expect(document.querySelector('[data-test="startup-recovery"]')).not.toBeNull()
    document.querySelector<HTMLButtonElement>('[data-test="startup-retry"]')!.click()
    await flush()

    expect(useSettingsStore(pinia).loadFailed).toBe(false)
    expect(ipc.getSettings).toHaveBeenCalledTimes(2)
    expect(ipc.onEvent).toHaveBeenCalledTimes(6)
    expect(document.querySelector('[data-test="startup-recovery"]')).toBeNull()
  })
})
