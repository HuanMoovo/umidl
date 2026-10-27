import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, h, nextTick, type App } from 'vue'
import { createPinia, disposePinia, type Pinia } from 'pinia'
import { NMessageProvider } from 'naive-ui'
import Subtitle from './Subtitle.vue'
import { i18n } from '@/i18n'
import { useTaskStore } from '@/stores/tasks'
import { useSettingsStore } from '@/stores/settings'
import type { SubtitleTask } from '@/types'

vi.mock('@tauri-apps/api/webview', () => ({ getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} }) }))

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((yes) => { resolve = yes })
  return { promise, resolve }
}
const VIDEO = String.raw`C:\Videos\one.mp4`
const OUTPUT = String.raw`C:\Subtitles`
const task = (id: string, model = 'sensevoice'): SubtitleTask => ({ id, model, video_path: VIDEO, language: 'zh', status: 'pending', progress: 0, created_time: 1 })
const status = (engine: string, ready = true) => ({ engine, runtime_ready: ready, model_ready: ready, ready, message: ready ? 'ready' : 'not installed', model_dir: '' })
let app: App | undefined
let pinia: Pinia | undefined
let commands: { cmd: string; args: any }[]
let handlers: Record<string, (args: any) => unknown>
let picked: string | null
const q = <T extends HTMLElement = HTMLElement>(selector: string) => document.querySelector<T>(selector)
const requests = (cmd: string) => commands.filter((entry) => entry.cmd === cmd)
async function flush() { for (let n = 0; n < 35; n++) await nextTick() }
function click(selector: string) {
  const el = q(selector)
  expect(el, selector).toBeTruthy()
  el!.dispatchEvent(new MouseEvent('click', { bubbles: true }))
}
function selectVm(selector: string) {
  let vm = (q(selector) as any)?.__vueParentComponent
  while (vm && vm.type?.name !== 'Select') vm = vm.parent
  expect(vm, selector).toBeTruthy()
  return vm
}
function select(selector: string, value: string | string[]) { selectVm(selector).emit('update:value', value) }
function input(selector: string, value: string) {
  const el = q<HTMLInputElement>(selector)
  expect(el, selector).toBeTruthy()
  el!.value = value
  el!.dispatchEvent(new Event('input', { bubbles: true }))
}
async function mount(opts: { ffmpeg?: boolean } = {}) {
  const host = document.createElement('div')
  document.body.append(host)
  pinia = createPinia()
  const store = useTaskStore(pinia)
  store.ready = true
  store.tools = [
    { name: 'whisper', found: true, source: 'managed', hint: '' },
    { name: 'ffmpeg', found: opts.ffmpeg !== false, source: 'managed', hint: '' },
  ]
  const settings = useSettingsStore(pinia)
  settings.loaded = true
  settings.patch({ whisper_model: 'tiny', subtitle_language: 'zh', subtitle_format: 'vtt' })
  app = createApp({ setup: () => () => h(NMessageProvider, null, { default: () => h(Subtitle) }) })
  app.use(pinia).use(i18n).mount(host)
  await flush()
  picked = VIDEO
  click('[data-test="subtitle-pick"]')
  await flush()
  return { store, settings }
}
async function engine(value: 'whisper' | 'chinese' | 'cloud') {
  select('[data-test="subtitle-engine-select"]', value)
  await flush()
}
async function provider(value: string) {
  select('[data-test="subtitle-cloud-provider"]', value)
  await flush()
}
beforeEach(() => {
  commands = []
  picked = null
  handlers = {
    list_whisper_models: () => [{ name: 'tiny', file: 'ggml-tiny.bin', builtin: true, downloaded: true, size_bytes: 1000, size_hint: '75 MB', quality: '★☆☆', current: true }],
    detect_tools: () => [], file_sizes: ({ paths }) => paths.map((path: string) => ({ path, exists: false, size: null })),
    save_settings: ({ settings }) => settings,
    asr_engine_status: ({ engine }) => status(engine),
    install_asr_engine: () => null,
    start_asr: ({ req }) => task(`asr-${requests('start_asr').length}`, req.engine === 'cloud' ? `cloud:${req.cloud.model}` : req.engine),
    start_subtitle: () => task('whisper-1', 'tiny'),
    'plugin:dialog|open': () => picked,
  }
  ;(window as any).__TAURI_INTERNALS__ = {
    invoke: (cmd: string, args: any) => {
      commands.push({ cmd, args })
      try {
        if (!handlers[cmd]) throw new Error(`Unhandled command: ${cmd}`)
        return Promise.resolve(handlers[cmd](args))
      } catch (e) { return Promise.reject(e) }
    },
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
  await new Promise((resolve) => setTimeout(resolve, 35))
  vi.unstubAllGlobals()
})

describe('Subtitle engine ownership', () => {
  it('defaults to Groq and clears all credentials when switching providers', async () => {
    await mount()
    await engine('cloud')
    expect(selectVm('[data-test="subtitle-cloud-provider"]').props.value).toBe('groq')
    expect(q<HTMLInputElement>('[data-test="subtitle-cloud-model"]')!.value).toBe('whisper-large-v3-turbo')
    input('[data-test="subtitle-cloud-key"]', 'groq-session-key')
    select('[data-test="subtitle-cloud-provider"]', 'tencent')
    await flush()
    expect(q('[data-test="subtitle-cloud-key"]')).toBeNull()
    input('[data-test="subtitle-cloud-app-id"]', '1001')
    input('[data-test="subtitle-cloud-secret-id"]', 'id-session')
    input('[data-test="subtitle-cloud-secret-key"]', 'secret-session')
    await flush()
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(false)
    select('[data-test="subtitle-cloud-provider"]', 'groq')
    await flush()
    expect(q<HTMLInputElement>('[data-test="subtitle-cloud-key"]')!.value).toBe('')
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(true)
    expect(document.body.textContent).not.toContain('groq-session-key')
  })

  it('shows only relevant credentials and explains Baidu product access', async () => {
    await mount()
    await engine('cloud')
    expect(selectVm('[data-test="subtitle-cloud-provider"]').props.options).toHaveLength(10)
    await provider('baidu')
    expect(q('[data-test="subtitle-cloud-app-id"]')).toBeNull()
    expect(q('[data-test="subtitle-cloud-secret-id"]')).toBeNull()
    expect(q('[data-test="subtitle-cloud-key"]')).toBeTruthy()
    expect(q('[data-test="subtitle-cloud-secret-key"]')).toBeTruthy()
    expect(q('[data-test="subtitle-cloud-config"]')!.textContent).toContain('邀测')
    expect(selectVm('[data-test="lang-select"]').props.options.map((o: any) => o.value)).toEqual(['zh'])
    input('[data-test="subtitle-cloud-key"]', 'baidu-key')
    await flush()
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(true)
    input('[data-test="subtitle-cloud-secret-key"]', 'baidu-secret')
    await flush()
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(false)
    click('[data-test="subtitle-start"]')
    await flush()
    expect(requests('start_asr')[0].args.req.cloud).toMatchObject({ provider: 'baidu', api_key: 'baidu-key', secret_key: 'baidu-secret', model: '80006' })
  })

  it('removes Cantonese when switching from a local model to Fish', async () => {
    await mount()
    await engine('chinese')
    select('[data-test="lang-select"]', ['yue'])
    await engine('cloud')
    await provider('fish')
    expect(selectVm('[data-test="lang-select"]').props.options.map((o: any) => o.value)).not.toContain('yue')
    expect(selectVm('[data-test="lang-select"]').props.value).not.toContain('yue')
  })

  it('requires Azure resource URL and clears credentials when preparing a new-format cloud retry', async () => {
    const { store } = await mount()
    await engine('cloud')
    input('[data-test="subtitle-cloud-key"]', 'previous-secret')
    await provider('azure')
    expect(q<HTMLInputElement>('[data-test="subtitle-cloud-key"]')!.value).toBe('')
    input('[data-test="subtitle-cloud-key"]', 'azure-secret')
    await flush()
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(true)
    input('[data-test="subtitle-cloud-base"]', 'https://example.cognitiveservices.azure.com')
    await flush()
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(false)
    store.subtitles = [{ ...task('failed-fish', 'cloud:fish:transcribe-1-pro'), status: 'error', error: 'cloud failure' }]
    await flush()
    click('[data-role="subtitle-row"] [title="继续"]')
    await flush()
    expect(selectVm('[data-test="subtitle-cloud-provider"]').props.value).toBe('fish')
    expect(q<HTMLInputElement>('[data-test="subtitle-cloud-model"]')!.value).toBe('transcribe-1-pro')
    expect(q<HTMLInputElement>('[data-test="subtitle-cloud-key"]')!.value).toBe('')
    expect(requests('start_asr')).toHaveLength(0)
  })
  it('keeps default Whisper requests unchanged and does not query or install new engines on mount', async () => {
    const { store } = await mount()
    expect(requests('asr_engine_status')).toHaveLength(0)
    expect(requests('install_asr_engine')).toHaveLength(0)
    expect(q('[data-test="subtitle-cloud-config"]')).toBeNull()
    click('[data-test="subtitle-start"]')
    await flush()
    expect(requests('start_subtitle')[0].args.req).toEqual({ video_path: VIDEO, language: 'zh', model: 'tiny', translate_to: null, output_format: 'vtt', output_dir: null })
    expect(requests('start_asr')).toHaveLength(0)
    expect(store.subtitles).toHaveLength(1)
  })

  it('requires explicit Chinese installation and preserves a newer model selection when installation finishes', async () => {
    await mount()
    const pending = deferred<void>()
    handlers.asr_engine_status = ({ engine }) => status(engine, engine === 'paraformer')
    handlers.install_asr_engine = () => pending.promise
    await engine('chinese')
    expect(requests('install_asr_engine')).toHaveLength(0)
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(true)
    click('[data-test="subtitle-asr-install"]')
    click('[data-test="subtitle-asr-install"]')
    expect(requests('install_asr_engine').map(({ args }) => args)).toEqual([{ engine: 'sensevoice' }])
    select('[data-test="subtitle-chinese-model"]', 'paraformer')
    await flush()
    pending.resolve()
    await flush()
    click('[data-test="subtitle-start"]')
    await flush()
    expect(requests('start_asr')[0].args.req.engine).toBe('paraformer')
    expect(requests('save_settings')).toHaveLength(0)
  })

  it('ignores stale model status and offers only languages supported by the selected Chinese model', async () => {
    await mount()
    const old = deferred<ReturnType<typeof status>>()
    handlers.asr_engine_status = ({ engine }) => engine === 'sensevoice' ? old.promise : status(engine, false)
    await engine('chinese')
    select('[data-test="lang-select"]', ['fr', 'ja'])
    select('[data-test="subtitle-chinese-model"]', 'paraformer')
    await flush()
    old.resolve(status('sensevoice'))
    await flush()
    expect(selectVm('[data-test="lang-select"]').props.options.map((o: any) => o.value)).toEqual(['auto', 'zh'])
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(true)
  })

  it('suppresses English translation for local Chinese and shares the normal output and queue contract', async () => {
    const { store } = await mount()
    click('[data-test="group-toggle"]')
    click('[data-test="translate-en"]')
    await engine('chinese')
    expect(q('[data-test="translate-en"]')).toBeNull()
    picked = OUTPUT
    click('[data-test="subtitle-output-pick"]')
    await flush()
    click('[data-test="subtitle-start"]')
    await flush()
    expect(requests('start_asr').map(({ args }) => args.req)).toEqual([{ engine: 'sensevoice', subtitle: { video_path: VIDEO, language: 'zh', model: null, translate_to: null, output_format: 'vtt', output_dir: OUTPUT } }])
    expect(store.subtitles).toHaveLength(1)
    expect(requests('start_subtitle')).toHaveLength(0)
  })

  it('requires FFmpeg for Chinese and cloud without changing default Whisper readiness', async () => {
    await mount({ ffmpeg: false })
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(false)
    await engine('chinese')
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(true)
    await engine('cloud')
    input('[data-test="subtitle-cloud-key"]', 'a-secret')
    await flush()
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(true)
  })

  it('makes cloud upload explicit and keeps password credentials out of settings and browser storage', async () => {
    const { settings } = await mount()
    const storage = vi.spyOn(Storage.prototype, 'setItem')
    await engine('cloud')
    await provider('compatible')
    expect(q<HTMLInputElement>('[data-test="subtitle-cloud-base"]')!.value).toBe('https://api.openai.com/v1')
    expect(q<HTMLInputElement>('[data-test="subtitle-cloud-model"]')!.value).toBe('whisper-1')
    expect(q<HTMLInputElement>('[data-test="subtitle-cloud-key"]')!.type).toBe('password')
    expect(q('[data-test="subtitle-cloud-notice"]')!.textContent).toContain('上传')
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(true)
    input('[data-test="subtitle-cloud-key"]', 'session-secret')
    await flush()
    expect(q('[data-test="subtitle-start"]')!.textContent).toContain('上传并生成字幕')
    click('[data-test="subtitle-start"]')
    await flush()
    expect(requests('start_asr')[0].args.req.cloud).toEqual({ provider: 'compatible', base_url: 'https://api.openai.com/v1', model: 'whisper-1', api_key: 'session-secret', app_id: '', secret_id: '', secret_key: '' })
    expect(requests('save_settings')).toHaveLength(0)
    expect(JSON.stringify(settings.settings)).not.toContain('session-secret')
    expect(storage).not.toHaveBeenCalled()
    storage.mockRestore()
    click('[data-test="subtitle-cloud-clear-key"]')
    await flush()
    expect(q<HTMLInputElement>('[data-test="subtitle-cloud-key"]')!.value).toBe('')
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(true)
  })

  it('snapshots cloud settings and secrets for the entire batch and blocks duplicate starts', async () => {
    const { store } = await mount()
    await engine('cloud')
    await provider('compatible')
    input('[data-test="subtitle-cloud-key"]', 'first-secret')
    select('[data-test="lang-select"]', ['zh', 'ja'])
    await flush()
    const first = deferred<SubtitleTask>()
    handlers.start_asr = () => requests('start_asr').length === 1 ? first.promise : task('cloud-2', 'cloud:whisper-1')
    click('[data-test="subtitle-start"]')
    click('[data-test="subtitle-start"]')
    input('[data-test="subtitle-cloud-key"]', 'new-secret')
    input('[data-test="subtitle-cloud-base"]', 'https://new.example/v1')
    await provider('groq')
    await engine('chinese')
    first.resolve(task('cloud-1', 'cloud:whisper-1'))
    await flush()
    expect(requests('start_asr').map(({ args }) => [args.req.engine, args.req.subtitle.language, args.req.cloud])).toEqual([
      ['cloud', 'zh', { provider: 'compatible', base_url: 'https://api.openai.com/v1', model: 'whisper-1', api_key: 'first-secret', app_id: '', secret_id: '', secret_key: '' }],
      ['cloud', 'ja', { provider: 'compatible', base_url: 'https://api.openai.com/v1', model: 'whisper-1', api_key: 'first-secret', app_id: '', secret_id: '', secret_key: '' }],
    ])
    expect(store.subtitles).toHaveLength(2)
  })

  it('allows an empty key only for loopback and hides translation for other cloud models', async () => {
    await mount()
    await engine('cloud')
    await provider('compatible')
    input('[data-test="subtitle-cloud-base"]', 'http://localhost:8080/v1')
    await flush()
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(false)
    input('[data-test="subtitle-cloud-base"]', 'http://example.com/v1')
    await flush()
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(true)
    input('[data-test="subtitle-cloud-model"]', 'other-model')
    await flush()
    expect(q('[data-test="translate-en"]')).toBeNull()
  })

  it('starts cloud translation disabled even when Whisper translation was enabled', async () => {
    await mount()
    click('[data-test="group-toggle"]')
    click('[data-test="translate-en"]')
    await engine('cloud')
    input('[data-test="subtitle-cloud-key"]', 'new-cloud-key')
    await flush()
    click('[data-test="subtitle-start"]')
    await flush()
    expect(requests('start_asr').map(({ args }) => args.req.subtitle.translate_to)).toEqual([null])
  })

  it('redacts API keys from provider errors shown to the user', async () => {
    await mount()
    await engine('cloud')
    input('[data-test="subtitle-cloud-key"]', 'do-not-display-me')
    handlers.start_asr = () => { throw new Error('provider rejected do-not-display-me') }
    await flush()
    click('[data-test="subtitle-start"]')
    await flush()
    expect(document.body.textContent).toContain('provider rejected [REDACTED]')
    expect(document.body.textContent).not.toContain('do-not-display-me')
  })

  it('retries a Chinese queue task with its original engine without routing it to Whisper', async () => {
    const { store } = await mount()
    store.subtitles = [{ ...task('failed-local', 'paraformer'), status: 'error', error: 'local failure' }]
    await flush()
    click('[data-role="subtitle-row"] [title="继续"]')
    click('[data-role="subtitle-row"] [title="继续"]')
    await flush()
    expect(requests('start_asr').map(({ args }) => args.req)).toEqual([{ engine: 'paraformer', subtitle: { video_path: VIDEO, language: 'zh', model: null, translate_to: null, output_format: 'vtt', output_dir: null } }])
    expect(requests('start_subtitle')).toHaveLength(0)
    expect(store.subtitles).toHaveLength(2)
  })

  it('prepares cloud retries for an explicit upload action instead of silently uploading from the queue', async () => {
    const { store } = await mount()
    store.subtitles = [{ ...task('failed-cloud', 'cloud:whisper-1'), status: 'error', error: 'cloud failure' }]
    await flush()
    click('[data-role="subtitle-row"] [title="继续"]')
    await flush()
    expect(requests('start_asr')).toHaveLength(0)
    expect(requests('start_subtitle')).toHaveLength(0)
    expect(q('[data-test="subtitle-cloud-config"]')).toBeTruthy()
    expect(q('[data-test="subtitle-start"]')!.textContent).toContain('上传并生成字幕')
  })

  it('retries only the failed cloud translation', async () => {
    const { store } = await mount()
    store.subtitles = [{ ...task('failed-translation', 'cloud:whisper-1'), translate_to: 'en', status: 'error', error: 'cloud failure' }]
    await flush()
    click('[data-role="subtitle-row"] [title="继续"]')
    await flush()
    input('[data-test="subtitle-cloud-base"]', 'https://retry.example/v1')
    input('[data-test="subtitle-cloud-key"]', 'retry-secret')
    await flush()
    click('[data-test="subtitle-start"]')
    await flush()
    expect(requests('start_asr').map(({ args }) => args.req.subtitle.translate_to)).toEqual(['en'])
  })

  it.each(['compatible', 'azure'] as const)('requires the destination again when retrying %s', async (vendor) => {
    const { store } = await mount()
    await engine('cloud')
    await provider(vendor)
    input('[data-test="subtitle-cloud-base"]', 'https://unrelated.example/v1')
    store.subtitles = [{ ...task('failed', vendor === 'azure' ? 'cloud:azure:fast-transcription' : 'cloud:whisper-1'), status: 'error' }]
    await flush()
    click('[data-role="subtitle-row"] [title="继续"]')
    await flush()
    expect(q<HTMLInputElement>('[data-test="subtitle-cloud-base"]')!.value).toBe('')
    input('[data-test="subtitle-cloud-key"]', 'retry-secret')
    await flush()
    expect(q<HTMLButtonElement>('[data-test="subtitle-start"]')!.disabled).toBe(true)
  })

  it('clears the compatible endpoint after the provider watcher runs on retry', async () => {
    const { store } = await mount()
    store.subtitles = [{ ...task('failed', 'cloud:whisper-1'), status: 'error' }]
    await flush()
    click('[data-role="subtitle-row"] [title="继续"]')
    await flush()
    expect(q<HTMLInputElement>('[data-test="subtitle-cloud-base"]')!.value).toBe('')
    expect(q<HTMLInputElement>('[data-test="subtitle-cloud-model"]')!.value).toBe('whisper-1')
  })

  it.each(['file', 'provider', 'language', 'translation'] as const)('clears translation-only retry intent when changing %s', async (changed) => {
    const { store } = await mount()
    store.subtitles = [{ ...task('failed', 'cloud:whisper-1'), translate_to: 'en', status: 'error' }]
    await flush()
    click('[data-role="subtitle-row"] [title="继续"]')
    await flush()
    if (changed === 'file') {
      picked = String.raw`C:\Videos\other.mp4`
      click('[data-test="subtitle-pick"]')
    } else if (changed === 'provider') {
      await provider('openai')
    } else if (changed === 'language') {
      select('[data-test="lang-select"]', ['ja'])
    } else {
      click('[data-test="translate-en"]')
      await flush()
      click('[data-test="translate-en"]')
    }
    await flush()
    if (changed !== 'provider') input('[data-test="subtitle-cloud-base"]', 'https://retry.example/v1')
    input('[data-test="subtitle-cloud-key"]', 'retry-secret')
    await flush()
    click('[data-test="subtitle-start"]')
    await flush()
    expect(requests('start_asr').map(({ args }) => args.req.subtitle.translate_to)).toEqual([null, 'en'])
  })
})
