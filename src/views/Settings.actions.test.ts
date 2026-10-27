import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, h, nextTick, type App } from 'vue'
import { createPinia } from 'pinia'
import { createMemoryHistory, createRouter } from 'vue-router'
import { NMessageProvider } from 'naive-ui'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import Settings from './Settings.vue'
import { i18n } from '@/i18n'
import { useTaskStore } from '@/stores/tasks'
import { useSettingsStore } from '@/stores/settings'
import type { ToolStatus } from '@/types'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), convertFileSrc: (path: string) => path }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))

const tools: ToolStatus[] = [
  { name: 'yt-dlp', found: false, source: 'missing', hint: '', installable: true },
  { name: 'ffmpeg', found: false, source: 'missing', hint: '', installable: true },
  { name: 'whisper', found: false, source: 'missing', hint: '', installable: true },
]
let app: App | null = null
let handlers: Record<string, (args?: any) => unknown>
let commands: { command: string; args: any }[]

async function flush() {
  for (let i = 0; i < 12; i++) await nextTick()
  await new Promise((resolve) => setTimeout(resolve, 0))
}

async function mountSettings() {
  const pinia = createPinia()
  useTaskStore(pinia).ready = true
  const settingsStore = useSettingsStore(pinia)
  settingsStore.loaded = true
  settingsStore.patch({ ui_mode: 'advanced', ytdlp_path: '', ffmpeg_path: '', whisper_path: '' })
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/settings', component: Settings }] })
  await router.push('/settings')
  const host = document.createElement('div')
  document.body.append(host)
  app = createApp({ render: () => h(NMessageProvider, null, { default: () => h(Settings) }) })
  app.use(pinia).use(i18n).use(router).mount(host)
  await flush()
  return { host, settingsStore }
}

beforeEach(() => {
  ;(window as any).__TAURI_INTERNALS__ = {}
  vi.stubGlobal('ResizeObserver', class { observe() {} unobserve() {} disconnect() {} })
  vi.stubGlobal('matchMedia', () => ({ matches: false, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {} }))
  commands = []
  handlers = {
    list_tools: () => tools,
    detect_tools: () => tools,
    save_settings: (args) => args.settings,
    tool_dir_info: () => ({ dir: 'C:\\Tools', default_dir: 'C:\\Tools', configured: '', custom: false, valid: true }),
    engine_info: () => ({ engine: 'auto', aria2_ready: false, split: 16, speed_limit_kb: 0, playlist_mode: 'single', proxy_mode: 'system' }),
    install_tool: () => tools[0],
  }
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    commands.push({ command, args })
    if (!handlers[command]) throw new Error(`Unexpected command: ${command}`)
    return handlers[command](args) as any
  })
  vi.mocked(open).mockResolvedValue('C:\\Tools\\chosen.exe')
})

afterEach(() => {
  app?.unmount()
  app = null
  document.body.innerHTML = ''
  delete (window as any).__TAURI_INTERNALS__
  vi.unstubAllGlobals()
})

describe('Settings actions', () => {
  it.each([
    ['yt-dlp', 'ytdlp_path'],
    ['ffmpeg', 'ffmpeg_path'],
    ['whisper', 'whisper_path'],
  ] as const)('saves a selected executable in the matching %s setting', async (name, key) => {
    const { host, settingsStore } = await mountSettings()
    host.querySelector<HTMLButtonElement>(`[data-test="tool-path-${name}"]`)!.click()
    await flush()
    expect(settingsStore.settings[key]).toBe('C:\\Tools\\chosen.exe')
    for (const other of ['ytdlp_path', 'ffmpeg_path', 'whisper_path'] as const) {
      if (other !== key) expect(settingsStore.settings[other]).toBe('')
    }
    expect(commands.find(({ command }) => command === 'save_settings')?.args.settings[key]).toBe('C:\\Tools\\chosen.exe')
  })

  it('does not start the same installation twice before the pending UI renders', async () => {
    let finish!: (value: ToolStatus) => void
    handlers.install_tool = () => new Promise((resolve) => { finish = resolve })
    const { host } = await mountSettings()
    const button = host.querySelector<HTMLButtonElement>('[data-test="tool-install-yt-dlp"]')!
    button.click()
    button.click()
    await flush()
    expect(commands.filter(({ command }) => command === 'install_tool')).toHaveLength(1)
    finish(tools[0])
    await flush()
  })
})
