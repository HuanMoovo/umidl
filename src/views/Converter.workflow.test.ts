import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, h, nextTick, type App } from 'vue'
import { createPinia } from 'pinia'
import { NMessageProvider } from 'naive-ui'
import { open } from '@tauri-apps/plugin-dialog'
import Converter from './Converter.vue'
import { i18n } from '@/i18n'
import { useTaskStore } from '@/stores/tasks'
import { useSettingsStore } from '@/stores/settings'
import { FALLBACK_CATALOG, type ConvertFormatCatalog } from '@/services/formatCatalog'
import type { AppSettings, ConvertRequest, ConvertTask, MediaProbe } from '@/types'

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))

/** Catalog transport fixtures retain the backend's four groups and unavailable entries. */
function catalogFor(ids?: readonly string[], engines: Record<string, boolean> = { ffmpeg: true, pandoc: false, poppler: false }): ConvertFormatCatalog {
  const groups = FALLBACK_CATALOG.groups.map((group) => {
    const formats = group.formats.map((format) => ({
      ...format,
      available: !ids || ids.includes(format.id),
      engines: group.id === 'document' && ids?.includes(format.id) ? ['native'] : [...format.engines],
    }))
    return { ...group, formats, count: formats.filter((format) => format.available).length }
  })
  return { groups, total: groups.reduce((total, group) => total + group.count, 0), engines }
}

const imageCatalog = () => catalogFor(['png', 'jpg', 'webp'])
const documentCatalog = () => catalogFor(['csv', 'txt', 'html'], { ffmpeg: false, pandoc: false, poppler: false })
const videoCatalog = () => catalogFor(['mp4', 'mkv', 'mp3', 'flac', 'png', 'jpg'])
const audioCatalog = () => catalogFor(['mp3', 'flac', 'wav'])

function media(path: string, overrides: Partial<MediaProbe> = {}): MediaProbe {
  return { path, format_name: 'mov,mp4,m4a,3gp,3g2,mj2', duration: 60, size: 1_000_000, bit_rate: 128_000,
    video_codec: 'h264', audio_codec: 'aac', width: 1920, height: 1080, fps: 30, ...overrides }
}

function task(request: ConvertRequest): ConvertTask {
  return { id: 'conversion-1', input_file: request.input_file, output_file: `C:\\Output\\result.${request.format}`,
    format: request.format, codec: request.video_codec, status: 'converting', progress: 0, created_time: 1, error: null }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => { resolve = done })
  return { promise, resolve }
}

let app: App | null = null
let handlers: Record<string, (args: any) => unknown>
let commands: { command: string; args: any }[]
const originalScrollTo = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollTo')

async function flush() {
  for (let i = 0; i < 15; i++) await nextTick()
  await new Promise((resolve) => setTimeout(resolve, 0))
  for (let i = 0; i < 5; i++) await nextTick()
}

async function mountConverter(preferences: Partial<AppSettings> = {}) {
  const pinia = createPinia()
  const store = useTaskStore(pinia)
  store.ready = true
  const settings = useSettingsStore(pinia)
  settings.loaded = true
  settings.patch({ default_crf: 23, auto_detect_format: true, hwaccel: false, ...preferences })
  const host = document.createElement('div')
  document.body.append(host)
  app = createApp({ render: () => h(NMessageProvider, null, { default: () => h(Converter) }) })
  app.use(pinia).use(i18n).mount(host)
  await flush()
  return { host, store }
}

async function choose(host: HTMLElement, path: string) {
  vi.mocked(open).mockResolvedValueOnce(path)
  // The second selector also exercises the existing pick button while the stable role is introduced.
  const button = host.querySelector<HTMLButtonElement>('[data-role="choose-source"], [data-role="file-area"] button.umi-btn-primary')
  expect(button, 'source file picker').toBeTruthy()
  button!.click()
  await flush()
}

const startButton = (host: HTMLElement) => host.querySelector<HTMLButtonElement>('[data-role="start-convert"]')!
const renderedTargets = (host: HTMLElement) => [...host.querySelectorAll('[data-format]')].map((format) => format.getAttribute('data-format'))
const submitted = () => commands.filter(({ command }) => command === 'start_convert')

beforeEach(() => {
  commands = []
  handlers = {
    convert_formats: () => catalogFor(),
    convert_targets: ({ path }) => /\.(xlsx|ods)$/i.test(path) ? documentCatalog()
      : /\.(png|jpg|webp)$/i.test(path) ? imageCatalog()
      : /\.(wav|mp3|flac)$/i.test(path) ? audioCatalog() : videoCatalog(),
    doc_capabilities: () => ({ pandoc: false, poppler: false, native: ['docx', 'xlsx', 'pptx', 'odt', 'ods', 'odp'], targets: ['txt', 'md', 'html', 'csv'] }),
    probe_document: ({ path }) => ({ ok: true, kind: 'xlsx', title: path.split(/[\\/]/).pop(), pages: null, sheets: 2, slides: null, chars: 250, error: null }),
    probe_media: ({ path }) => /\.(png|jpg|webp)$/i.test(path)
      ? media(path, { format_name: 'image2', video_codec: 'mjpeg', audio_codec: null, duration: null, width: 800, height: 600, fps: null })
      : /\.(wav|mp3|flac)$/i.test(path) ? media(path, { format_name: 'wav', video_codec: null, audio_codec: 'pcm_s16le', width: null, height: null, fps: null }) : media(path),
    start_convert: ({ req }) => task(req),
    file_sizes: ({ paths }) => paths.map((path: string) => ({ path, size: null, exists: false, modified: null })),
    'plugin:event|listen': () => 1,
    'plugin:event|unlisten': () => null,
    frontend_log: () => null,
  }
  let callbackId = 0
  ;(window as any).__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
    transformCallback: (callback: unknown) => { const id = ++callbackId; (window as any)[`_${id}`] = callback; return id },
    invoke: async (command: string, args: any) => {
      commands.push({ command, args })
      if (!handlers[command]) throw new Error(`Unexpected command: ${command}`)
      return handlers[command](args)
    },
  }
  ;(window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} }
  vi.mocked(open).mockReset()
  vi.stubGlobal('ResizeObserver', class { observe() {} unobserve() {} disconnect() {} })
  vi.stubGlobal('matchMedia', () => ({ matches: false, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {} }))
  Object.defineProperty(HTMLElement.prototype, 'scrollTo', { configurable: true, value() {} })
})

afterEach(() => {
  app?.unmount()
  app = null
  document.body.innerHTML = ''
  delete (window as any).__TAURI_INTERNALS__
  delete (window as any).__TAURI_EVENT_PLUGIN_INTERNALS__
  if (originalScrollTo) Object.defineProperty(HTMLElement.prototype, 'scrollTo', originalScrollTo)
  else delete (HTMLElement.prototype as any).scrollTo
  vi.unstubAllGlobals()
})

describe('Converter source and submission workflow', () => {
  it('keeps format and output controls hidden until a source is chosen', async () => {
    const { host } = await mountConverter()
    expect((host.querySelector('[data-role="format-selector"]') as HTMLElement).style.display).toBe('none')
    expect((host.querySelector('[data-role="param-area"]') as HTMLElement).style.display).toBe('none')
    await choose(host, 'C:\\Input\\photo.jpg')
    expect((host.querySelector('[data-role="format-selector"]') as HTMLElement).style.display).not.toBe('none')
  })

  it('lets a simple-mode user open advanced options in one action', async () => {
    const { host } = await mountConverter({ ui_mode: 'simple' })
    await choose(host, 'C:\\Input\\movie.mp4')
    const toggle = host.querySelector<HTMLButtonElement>('[data-test="group-toggle"]')
    expect(toggle).not.toBeNull()
    toggle!.click()
    await flush()
    expect(toggle!.getAttribute('aria-expanded')).toBe('true')
  })

  it('choosing the audio purpose selects a usable audio target without a second switch', async () => {
    const { host } = await mountConverter()
    await choose(host, 'C:\\Input\\movie.mp4')
    host.querySelector<HTMLButtonElement>('[data-type="audio"]')!.click()
    await flush()
    startButton(host).click()
    await flush()
    expect(submitted()[0]?.args.req).toMatchObject({ format: 'mp3', extract_audio: true, audio_codec: 'libmp3lame' })
  })
  it('uses the image targets and defaults a JPEG source to a plain PNG request', async () => {
    const { host } = await mountConverter()
    await choose(host, 'C:\\Input\\photo.jpg')
    expect(commands.find(({ command }) => command === 'convert_targets')?.args).toEqual({ path: 'C:\\Input\\photo.jpg' })
    expect(host.querySelector('[data-role="param-area"]')?.getAttribute('data-param-kind')).toBe('image')
    expect(host.querySelector('[data-format="png"]')?.getAttribute('data-selected')).toBe('1')
    expect(renderedTargets(host).sort()).toEqual(['jpg', 'png', 'webp'])
    startButton(host).click()
    await flush()
    expect(submitted()[0]?.args.req).toMatchObject({ input_file: 'C:\\Input\\photo.jpg', format: 'png', video_codec: null, audio_codec: null, extract_audio: false, auto_format: null })
  })

  it('offers only spreadsheet outputs returned by the backend and submits CSV without media codecs', async () => {
    const { host } = await mountConverter()
    await choose(host, 'C:\\Input\\sales.xlsx')
    expect(renderedTargets(host).sort()).toEqual(['csv', 'html', 'txt'])
    expect(host.querySelector('[data-role="param-area"]')?.getAttribute('data-param-kind')).toBe('document')
    expect(host.querySelector('[data-format="csv"]')?.getAttribute('data-selected')).toBe('1')
    expect(startButton(host).disabled).toBe(false)
    startButton(host).click()
    await flush()
    expect(submitted()[0]?.args.req).toMatchObject({ input_file: 'C:\\Input\\sales.xlsx', format: 'csv', video_codec: null, audio_codec: null, extract_audio: false, auto_format: null })
  })

  it('keeps conversion unavailable when source-specific target detection fails', async () => {
    handlers.convert_targets = () => { throw new Error('Source target command unavailable') }
    const { host } = await mountConverter()
    await choose(host, 'C:\\Input\\sales.xlsx')
    expect(commands.filter(({ command }) => command === 'convert_targets')).toHaveLength(1)
    expect(renderedTargets(host)).toEqual([])
    expect(startButton(host).disabled).toBe(true)
    startButton(host).click()
    await flush()
    expect(submitted()).toHaveLength(0)
  })

  it('keeps the newer source and its audio state when an older media probe resolves last', async () => {
    const older = deferred<MediaProbe>()
    handlers.probe_media = ({ path }) => path.endsWith('older.mp4') ? older.promise
      : media(path, { format_name: 'wav', video_codec: null, audio_codec: 'pcm_s16le', width: null, height: null, fps: null })
    const { host } = await mountConverter()
    await choose(host, 'C:\\Input\\older.mp4')
    await choose(host, 'C:\\Input\\newer.wav')
    expect(host.querySelector('[data-role="media-info"]')?.textContent).toContain('newer.wav')
    older.resolve(media('C:\\Input\\older.mp4'))
    await flush()
    expect(host.querySelector<HTMLInputElement>('[data-role="source-input"]')?.value).toBe('newer.wav')
    expect(host.querySelector<HTMLInputElement>('[data-role="source-input"]')?.title).toBe('C:\\Input\\newer.wav')
    expect(host.querySelector('[data-role="media-info"]')?.textContent).toContain('newer.wav')
    expect(host.querySelector('[data-role="media-info"]')?.textContent).not.toContain('older.mp4')
    expect(host.querySelector('[data-role="param-area"]')?.getAttribute('data-param-kind')).toBe('audio')
    expect(renderedTargets(host).sort()).toEqual(['flac', 'mp3', 'wav'])
  })

  it('sends one request when the user starts the same pending conversion twice', async () => {
    const pending = deferred<ConvertTask>()
    let request!: ConvertRequest
    handlers.start_convert = ({ req }) => { request = req; return pending.promise }
    const { host, store } = await mountConverter()
    await choose(host, 'C:\\Input\\photo.jpg')
    const button = startButton(host)
    button.click()
    button.click()
    await flush()
    const requestCount = submitted().length
    const disabledWhilePending = button.disabled
    pending.resolve(task(request))
    await flush()
    expect(requestCount).toBe(1)
    expect(disabledWhilePending).toBe(true)
    expect(store.converts).toHaveLength(1)
  })

  it('synchronizes the codec when a video source is manually switched to MP3 output', async () => {
    const { host } = await mountConverter()
    await choose(host, 'C:\\Input\\movie.mp4')
    host.querySelector<HTMLButtonElement>('[data-type="audio"]')!.click()
    await flush()
    const audioTarget = host.querySelector<HTMLButtonElement>('[data-format="mp3"]')
    expect(audioTarget).toBeTruthy()
    audioTarget!.click()
    await flush()
    expect(host.querySelector('[data-role="param-area"]')?.getAttribute('data-param-kind')).toBe('audio')
    startButton(host).click()
    await flush()
    expect(submitted()[0]?.args.req).toMatchObject({ input_file: 'C:\\Input\\movie.mp4', format: 'mp3', audio_codec: 'libmp3lame', extract_audio: true, auto_format: null })
  })

  it('preserves the stored resize preference instead of enabling automatic stream copy', async () => {
    const { host } = await mountConverter({ default_resolution: '1280x720', auto_detect_format: true })
    await choose(host, 'C:\\Input\\movie.mp4')
    startButton(host).click()
    await flush()
    expect(submitted()[0]?.args.req).toMatchObject({
      input_file: 'C:\\Input\\movie.mp4',
      resolution: '1280x720',
      video_codec: 'libx264',
      auto_format: null,
    })
  })

  it('reencodes video using the manually selected quality after automatic stream copy', async () => {
    const { host } = await mountConverter({ ui_mode: 'advanced', default_resolution: '原分辨率', auto_detect_format: true })
    await choose(host, 'C:\\Input\\movie.mp4')
    host.querySelector<HTMLButtonElement>('[data-role="param-area"] [data-test="group-toggle"]')!.click()
    await flush()
    expect(host.querySelector('[data-field="video-codec"]')?.textContent).toContain('copy')

    const quality = host.querySelector<HTMLElement>('[data-field="quality"] .n-base-selection')!
    expect(quality.textContent).toContain('CRF 18')
    quality.click()
    await flush()
    // Use the real select's keyboard path: 18 → 23 → 28, then confirm.
    quality.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }))
    quality.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }))
    quality.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))
    await flush()
    expect(host.querySelector('[data-field="quality"]')?.textContent).toContain('CRF 28')

    startButton(host).click()
    await flush()
    expect(submitted()[0]?.args.req).toMatchObject({
      input_file: 'C:\\Input\\movie.mp4',
      video_codec: 'libx264',
      crf: 28,
      auto_format: null,
    })
  })
})
