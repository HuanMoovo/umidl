import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, h, nextTick, type App, type Component } from 'vue'
import { createPinia, disposePinia, type Pinia } from 'pinia'
import { NDialogProvider, NMessageProvider } from 'naive-ui'
import Download from '@/views/Download.vue'
import Subtitle from '@/views/Subtitle.vue'
import router from '@/router'
import { i18n } from '@/i18n'
import { useTaskStore } from '@/stores/tasks'
import { useSettingsStore } from '@/stores/settings'
import type { AppSettings, MediaInfo, SubtitleTask, WhisperModelInfo } from '@/types'

const native = vi.hoisted(() => ({ onDragDropEvent: vi.fn() }))
vi.mock('@tauri-apps/api/webview', () => ({ getCurrentWebview: () => native }))

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}

const DIR = String.raw`C:\Downloads\Umidl`
const VIDEO = String.raw`C:\Videos\first.mp4`
const OTHER_VIDEO = String.raw`C:\Videos\second.mp4`
const OUTPUT = String.raw`C:\Subtitles`
const URL_A = 'https://example.com/a'
const URL_B = 'https://example.com/b'
const media = (url: string, title: string): MediaInfo => ({
  id: title, title, webpage_url: url, formats: [], subtitles: [], subtitle_tracks: [], is_playlist: false,
})
const model = (name: string, downloaded = true): WhisperModelInfo => ({
  name, file: `ggml-${name}.bin`, builtin: true, downloaded,
  size_bytes: downloaded ? 1000 : 0, size_hint: '75 MB', quality: '★☆☆', current: false,
})
const subtitleTask = (id: string): SubtitleTask => ({
  id, video_path: VIDEO, language: 'zh', model: 'tiny', status: 'pending', progress: 0, created_time: 1,
})

let app: App | undefined
let pinia: Pinia | undefined
let commands: { cmd: string; args: any }[]
let handlers: Record<string, (args: any) => unknown>
let picked: string | null
let unlisten: ReturnType<typeof vi.fn>

async function flush() {
  for (let n = 0; n < 35; n++) await nextTick()
}
const q = <T extends HTMLElement = HTMLElement>(selector: string) => document.querySelector<T>(selector)
const requestCalls = (cmd: string) => commands.filter((entry) => entry.cmd === cmd)
function input(selector: string, value: string) {
  const el = q<HTMLInputElement>(selector)!
  expect(el).toBeTruthy()
  el.value = value
  el.dispatchEvent(new Event('input', { bubbles: true }))
}
function click(selector: string) {
  const el = q(selector)
  expect(el, selector).toBeTruthy()
  el!.dispatchEvent(new MouseEvent('click', { bubbles: true }))
}
// Drive the real NSelect's public update event without depending on its popup internals.
function select(selector: string, value: string | string[]) {
  let vm = (q(selector) as any)?.__vueParentComponent
  while (vm && vm.type?.name !== 'Select') vm = vm.parent
  expect(vm, selector).toBeTruthy()
  vm.emit('update:value', value)
}
async function mount(component: Component, opts: { engine?: boolean; settings?: Partial<AppSettings> } = {}) {
  const host = document.createElement('div')
  document.body.append(host)
  pinia = createPinia()
  const store = useTaskStore(pinia)
  store.ready = true
  store.tools = [
    { name: 'whisper', found: opts.engine !== false, source: 'managed', hint: '' },
    { name: 'whisper-model', found: true, source: 'managed', hint: '' },
  ]
  handlers.detect_tools = () => store.tools.map((tool) => ({ ...tool }))
  const settings = useSettingsStore(pinia)
  settings.loaded = true
  settings.patch({ download_dir: DIR, whisper_model: 'tiny', subtitle_language: 'zh', subtitle_format: 'vtt', default_container: 'mkv', ...opts.settings })
  app = createApp({ setup: () => () => h(NDialogProvider, null, {
    default: () => h(NMessageProvider, null, { default: () => h(component) }),
  }) })
  app.use(pinia).use(router).use(i18n)
  app.mount(host)
  await flush()
  return store
}
async function probe(url = URL_A, title = 'First video') {
  handlers.probe_url = () => media(url, title)
  input('[data-test="link-input"]', url)
  click('[data-test="link-primary"]')
  await flush()
}
async function pickVideo(path = VIDEO) {
  picked = path
  click('[data-test="subtitle-pick"]')
  await flush()
}

beforeEach(() => {
  commands = []
  picked = null
  unlisten = vi.fn()
  native.onDragDropEvent.mockReset().mockResolvedValue(unlisten)
  handlers = {
    list_downloads: () => [], list_converts: () => [], list_subtitles: () => [],
    list_tools: () => [], detect_tools: () => [], backfill_covers: () => 0,
    list_whisper_models: () => [model('tiny'), model('base', false)],
    file_sizes: ({ paths }) => paths.map((path: string) => ({ path, exists: false, size: null })),
    save_settings: ({ settings }) => settings,
    'plugin:dialog|open': () => picked,
    frontend_log: () => null,
  }
  ;(window as any).__TAURI_INTERNALS__ = {
    invoke: (cmd: string, args: any) => {
      commands.push({ cmd, args })
      try {
        if (!handlers[cmd]) throw new Error(`Unhandled command: ${cmd}`)
        return Promise.resolve(handlers[cmd](args))
      } catch (e) { return Promise.reject(e) }
    },
    metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
    transformCallback: () => 1,
  }
  vi.stubGlobal('ResizeObserver', class { observe() {} unobserve() {} disconnect() {} })
  vi.stubGlobal('matchMedia', () => ({ matches: false, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {} }))
})
afterEach(async () => {
  app?.unmount()
  app = undefined
  if (pinia) disposePinia(pinia)
  pinia = undefined
  document.body.innerHTML = ''
  // Let Naive UI's resize-observer animation frame release the removed targets.
  await new Promise((resolve) => setTimeout(resolve, 35))
  vi.unstubAllGlobals()
})

describe('Download request ownership', () => {
  it('keeps the newer URL result when an older probe finishes last', async () => {
    await mount(Download)
    const first = deferred<MediaInfo>()
    const second = deferred<MediaInfo>()
    handlers.probe_url = ({ url }) => url === URL_A ? first.promise : second.promise
    input('[data-test="link-input"]', URL_A)
    click('[data-test="link-primary"]')
    await flush()
    input('[data-test="link-input"]', URL_B)
    await flush()
    expect(q<HTMLButtonElement>('[data-test="link-primary"]')!.disabled).toBe(false)
    click('[data-test="link-primary"]')
    second.resolve(media(URL_B, 'New result'))
    await flush()
    first.resolve(media(URL_A, 'Obsolete result'))
    await flush()
    expect(q('[data-test="probe-result"]')?.textContent).toContain('New result')
    expect(q('[data-test="probe-result"]')?.textContent).not.toContain('Obsolete result')
  })

  it('clears resolved metadata immediately when the URL changes', async () => {
    await mount(Download)
    await probe()
    input('[data-test="link-input"]', URL_B)
    await flush()
    expect(q('[data-test="probe-result"]')).toBeNull()
    expect(q('[data-test="link-start"]')).toBeNull()
  })

  it('does not show failures from a URL that has been replaced', async () => {
    await mount(Download)
    const pending = deferred<MediaInfo>()
    handlers.probe_url = () => pending.promise
    input('[data-test="link-input"]', URL_A)
    click('[data-test="link-primary"]')
    input('[data-test="link-input"]', URL_B)
    await flush()
    pending.reject(new Error('obsolete failure'))
    await flush()
    expect(document.body.textContent).not.toContain('obsolete failure')
  })

  it('accepts only one probe when Enter repeats before the next render', async () => {
    await mount(Download)
    const pending = deferred<MediaInfo>()
    handlers.probe_url = () => pending.promise
    input('[data-test="link-input"]', URL_A)
    for (let n = 0; n < 2; n++) q('[data-test="link-input"]')!.dispatchEvent(new KeyboardEvent('keyup', { key: 'Enter', bubbles: true }))
    expect(requestCalls('probe_url')).toHaveLength(1)
    pending.resolve(media(URL_A, 'One result'))
    await flush()
  })

  it('does not enqueue twice or overlap part downloads while submission is pending', async () => {
    const store = await mount(Download)
    await probe()
    const pending = deferred<any>()
    handlers.start_download = () => pending.promise
    click('[data-test="link-start"]')
    click('[data-test="link-start"]')
    click('[data-test="download-part-audio"]')
    expect(requestCalls('start_download')).toHaveLength(1)
    expect(requestCalls('start_download')[0].args.req.merge_container).toBe('mkv')
    pending.resolve({ id: 'download-1', url: URL_A, title: 'First video', status: 'pending', progress: 0, created_time: 1 })
    await flush()
    expect(store.downloads).toHaveLength(1)
  })

  it('uses a chosen output folder and keeps it after the folder dialog is cancelled', async () => {
    await mount(Download)
    expect(q<HTMLInputElement>('[data-test="download-output-path"]')?.value).toBe(DIR)
    picked = OUTPUT
    click('[data-test="download-output-pick"]')
    await flush()
    picked = null
    click('[data-test="download-output-pick"]')
    await flush()
    await probe()
    handlers.start_download = () => ({ id: 'd', url: URL_A, title: 'First video', status: 'pending', progress: 0, created_time: 1 })
    click('[data-test="link-start"]')
    await flush()
    expect(requestCalls('start_download')[0].args.req.output_dir).toBe(OUTPUT)
    expect(requestCalls('plugin:dialog|open')[0].args.options).toMatchObject({ directory: true, multiple: false, defaultPath: DIR })
  })

  it('applies the configured resolution preference when choosing a parsed video format', async () => {
    await mount(Download, { settings: { default_quality: '720' } })
    handlers.probe_url = () => ({ ...media(URL_A, 'Quality preference'), formats: [
      { format_id: '1080', ext: 'mp4', resolution: '1920x1080', has_video: true, has_audio: true },
      { format_id: '720', ext: 'mp4', resolution: '1280x720', has_video: true, has_audio: true },
      { format_id: '480', ext: 'mp4', resolution: '854x480', has_video: true, has_audio: true },
    ] })
    input('[data-test="link-input"]', URL_A)
    click('[data-test="link-primary"]')
    await flush()
    handlers.start_download = () => ({ id: 'd', url: URL_A, title: 'Quality preference', status: 'pending', progress: 0, created_time: 1 })
    click('[data-test="link-start"]')
    await flush()
    expect(requestCalls('start_download')[0].args.req).toMatchObject({ mode: 'video', format_id: '720', merge_container: 'mkv' })
  })

  it('retains the configured audio format in the visible simple-mode controls', async () => {
    await mount(Download, { settings: { default_quality: 'audio', default_audio_format: 'flac' } })
    await probe()
    expect(q('[data-test="probe-result"]')?.textContent).toContain('FLAC')
    handlers.start_download = () => ({ id: 'd', url: URL_A, title: 'First video', status: 'pending', progress: 0, created_time: 1 })
    click('[data-test="link-start"]')
    await flush()
    expect(requestCalls('start_download')[0].args.req).toMatchObject({ mode: 'audio', audio_format: 'flac' })
  })
})

describe('Subtitle request ownership and availability', () => {
  it('snapshots every language request and rejects duplicate submissions', async () => {
    const store = await mount(Subtitle)
    await pickVideo()
    select('[data-test="lang-select"]', ['zh', 'ja'])
    await flush()
    const first = deferred<SubtitleTask>()
    handlers.start_subtitle = () => requestCalls('start_subtitle').length === 1 ? first.promise : subtitleTask('sub-2')
    click('[data-test="subtitle-start"]')
    click('[data-test="subtitle-start"]')
    await pickVideo(OTHER_VIDEO)
    select('[data-test="lang-select"]', ['en'])
    first.resolve(subtitleTask('sub-1'))
    await flush()
    expect(requestCalls('start_subtitle').map(({ args }) => args.req)).toEqual([
      { video_path: VIDEO, language: 'zh', model: 'tiny', translate_to: null, output_format: 'vtt', output_dir: null },
      { video_path: VIDEO, language: 'ja', model: 'tiny', translate_to: null, output_format: 'vtt', output_dir: null },
    ])
    expect(store.subtitles).toHaveLength(2)
  })

  it('requires the engine and the selected downloaded model before creating a task', async () => {
    await mount(Subtitle, { engine: false })
    await pickVideo()
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(true)
    click('[data-test="subtitle-start"]')
    expect(requestCalls('start_subtitle')).toHaveLength(0)
    useTaskStore(pinia!).tools[0].found = true
    select('[data-test="subtitle-model-select"]', 'base')
    await flush()
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(true)
  })

  it('shows a recoverable model-list error instead of declaring an unknown model ready', async () => {
    handlers.list_whisper_models = () => { throw new Error('model catalog unavailable') }
    await mount(Subtitle)
    await pickVideo()
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(true)
    expect(document.body.textContent).toContain('model catalog unavailable')
    handlers.list_whisper_models = () => [model('tiny')]
    click('[data-test="subtitle-model-retry"]')
    await flush()
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(false)
  })

  it('releases the page drag listener on navigation', async () => {
    await mount(Subtitle)
    expect(native.onDragDropEvent).toHaveBeenCalledOnce()
    app!.unmount()
    app = undefined
    expect(unlisten).toHaveBeenCalledOnce()
  })

  it('releases a drag listener whose registration finishes after navigation', async () => {
    const registration = deferred<() => void>()
    native.onDragDropEvent.mockReturnValue(registration.promise)
    await mount(Subtitle)
    app!.unmount()
    app = undefined
    registration.resolve(unlisten)
    await flush()
    expect(unlisten).toHaveBeenCalledOnce()
  })

  it('sends the chosen subtitle folder while retaining the source-folder default', async () => {
    await mount(Subtitle)
    await pickVideo()
    expect(q<HTMLInputElement>('[data-test="subtitle-output-path"]')?.value).toBe(String.raw`C:\Videos`)
    picked = OUTPUT
    click('[data-test="subtitle-output-pick"]')
    await flush()
    handlers.start_subtitle = () => subtitleTask('s')
    click('[data-test="subtitle-start"]')
    await flush()
    expect(requestCalls('start_subtitle')[0].args.req.output_dir).toBe(OUTPUT)
  })

  it('ignores older file metadata when a task product changes before the previous read finishes', async () => {
    const first = deferred<any[]>()
    const second = deferred<any[]>()
    let reads = 0
    handlers.file_sizes = () => ++reads === 1 ? first.promise : second.promise
    const store = await mount(Subtitle)
    store.subtitles = [{ ...subtitleTask('s'), status: 'done', subtitle_path: String.raw`C:\Videos\first.zh.vtt` }]
    await flush()
    store.subtitles[0].subtitle_path = String.raw`C:\Subtitles\first.zh.vtt`
    await flush()
    second.resolve([{ path: String.raw`C:\Subtitles\first.zh.vtt`, exists: true, size: 2345 }])
    await flush()
    first.resolve([{ path: String.raw`C:\Videos\first.zh.vtt`, exists: false, size: null }])
    await flush()
    expect(q('[data-role="subtitle-row"]')?.textContent).not.toContain('文件已丢失')
    expect(q('[data-role="subtitle-row"]')?.querySelector('[title="打开文件"]')).toBeTruthy()
  })

  it('keeps a later model selection when an earlier model installation finishes', async () => {
    const pending = deferred<any>()
    handlers.install_whisper_model = () => pending.promise
    await mount(Subtitle)
    await pickVideo()
    select('[data-test="subtitle-model-select"]', 'base')
    await flush()
    click('[data-test="subtitle-model-download-btn"]')
    select('[data-test="subtitle-model-select"]', 'tiny')
    await flush()
    handlers.list_whisper_models = () => [model('tiny'), model('base')]
    pending.resolve({ name: 'whisper-model', found: true, source: 'managed', hint: '' })
    await flush()
    handlers.start_subtitle = () => subtitleTask('s')
    click('[data-test="subtitle-start"]')
    await flush()
    expect(requestCalls('start_subtitle')[0].args.req.model).toBe('tiny')
    expect(useSettingsStore(pinia!).settings.whisper_model).toBe('tiny')
  })

  it('preserves a newly typed custom-model source when the previous import finishes', async () => {
    const pending = deferred<any>()
    handlers.install_whisper_custom = () => pending.promise
    await mount(Subtitle)
    input('[data-test="subtitle-custom-input"]', 'https://example.com/old-model.bin')
    click('[data-test="subtitle-custom-install"]')
    input('[data-test="subtitle-custom-input"]', 'https://example.com/new-model.bin')
    pending.resolve({ name: 'whisper-model', found: true, source: 'managed', hint: '', path: String.raw`C:\Models\old-model.bin` })
    await flush()
    expect(q<HTMLInputElement>('[data-test="subtitle-custom-input"]')!.value).toBe('https://example.com/new-model.bin')
  })

  it('saves an explicit selection back to the initial model after another model was chosen', async () => {
    handlers.list_whisper_models = () => [model('tiny'), model('base')]
    await mount(Subtitle)
    select('[data-test="subtitle-model-select"]', 'base')
    await flush()
    expect(useSettingsStore(pinia!).settings.whisper_model).toBe('base')
    select('[data-test="subtitle-model-select"]', 'tiny')
    await flush()
    expect(useSettingsStore(pinia!).settings.whisper_model).toBe('tiny')
  })
})
