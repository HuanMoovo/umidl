import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, h, nextTick, type App } from 'vue'
import { createPinia } from 'pinia'
import { NMessageProvider } from 'naive-ui'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import SettingsPlugins from './SettingsPlugins.vue'
import { i18n } from '@/i18n'
import type { PluginInfo } from '@/services/plugins'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

const plugins: PluginInfo[] = [
  { id: 'alpha', name: 'Alpha Video', version: '1.0', description: 'Video resolver', enabled: true, sha256: 'aaa', source: 'GitHub', installed_at: '' },
  { id: 'beta', name: 'Beta Audio', version: '2.0', description: 'Music helper', enabled: false, sha256: 'bbb', source: 'Local', installed_at: '' },
]
let app: App | null = null
let handlers: Record<string, (args?: any) => unknown>
let commands: { command: string; args: any }[]

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => { resolve = done })
  return { promise, resolve }
}

async function flush() {
  for (let i = 0; i < 12; i++) await nextTick()
  await new Promise((resolve) => setTimeout(resolve, 0))
}

async function mountPlugins() {
  const host = document.createElement('div')
  document.body.append(host)
  app = createApp({ render: () => h(NMessageProvider, null, { default: () => h(SettingsPlugins) }) })
  app.use(createPinia()).use(i18n).mount(host)
  await flush()
  return host
}

function row(host: HTMLElement, name: string) {
  return [...host.querySelectorAll<HTMLElement>('[data-test="plugin-installed"] .umi-inner')]
    .find((element) => element.textContent?.includes(name))!
}

function button(host: HTMLElement, label: string) {
  return [...host.querySelectorAll<HTMLButtonElement>('button')].find((element) => element.textContent?.trim() === label)!
}

beforeEach(() => {
  ;(window as any).__TAURI_INTERNALS__ = {}
  commands = []
  handlers = {
    plugin_sandbox_info: () => ({ available: true, engine: 'QuickJS', version: '1', memory_limit_mb: 32, script_timeout_ms: 1000 }),
    plugin_list: () => plugins.map((plugin) => ({ ...plugin })),
    plugin_test: () => ({ ok: true, logs: [] }),
    plugin_uninstall: () => null,
    plugin_set_enabled: () => null,
    plugin_install_from_url: () => plugins[0],
    plugin_run_resolvers: () => 'resolved',
  }
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    commands.push({ command, args })
    if (!handlers[command]) throw new Error(`Unexpected command: ${command}`)
    return handlers[command](args) as any
  })
  vi.mocked(listen).mockReset().mockResolvedValue(() => {})
})

afterEach(() => {
  app?.unmount()
  app = null
  document.body.innerHTML = ''
  delete (window as any).__TAURI_INTERNALS__
})

describe('Plugin manager', () => {
  it('keeps the installed list usable when the sandbox probe fails', async () => {
    handlers.plugin_sandbox_info = () => { throw new Error('Sandbox unavailable') }
    const host = await mountPlugins()
    expect(row(host, 'Alpha Video')).toBeTruthy()
    expect(host.textContent).toContain('Sandbox unavailable')
  })

  it('ignores Enter while an installation is already pending', async () => {
    const pending = deferred<PluginInfo>()
    handlers.plugin_install_from_url = () => pending.promise
    const host = await mountPlugins()
    const input = host.querySelector<HTMLInputElement>('[data-test="plugin-install-url"] input')!
    input.value = 'https://github.com/example/plugin'
    input.dispatchEvent(new Event('input'))
    await nextTick()
    input.dispatchEvent(new KeyboardEvent('keyup', { key: 'Enter' }))
    input.dispatchEvent(new KeyboardEvent('keyup', { key: 'Enter' }))
    await flush()
    expect(commands.filter(({ command }) => command === 'plugin_install_from_url')).toHaveLength(1)
    pending.resolve(plugins[0])
    await flush()
  })

  it('locks all actions on a plugin while allowing another plugin to finish independently', async () => {
    const first = deferred<{ ok: boolean; logs: string[] }>()
    const second = deferred<{ ok: boolean; logs: string[] }>()
    handlers.plugin_test = (args) => args.id === 'alpha' ? first.promise : second.promise
    const host = await mountPlugins()
    button(row(host, 'Alpha Video'), '测试').click()
    await flush()
    expect(button(row(host, 'Alpha Video'), '卸载').disabled).toBe(true)
    row(host, 'Alpha Video').querySelector<HTMLElement>('[role="switch"]')!.click()
    await flush()
    expect(commands.filter(({ command }) => command === 'plugin_set_enabled')).toHaveLength(0)
    button(row(host, 'Beta Audio'), '测试').click()
    await flush()
    second.resolve({ ok: true, logs: [] })
    await flush()
    expect(button(row(host, 'Alpha Video'), '卸载').disabled).toBe(true)
    expect(button(row(host, 'Beta Audio'), '卸载').disabled).toBe(false)
    first.resolve({ ok: true, logs: [] })
    await flush()
    expect(button(row(host, 'Alpha Video'), '卸载').disabled).toBe(false)
  })

  it('releases a listener that finishes subscribing after leaving the page', async () => {
    const pending = deferred<() => void>()
    const off = vi.fn()
    vi.mocked(listen).mockImplementation((event) => event === 'plugin://event' ? pending.promise : Promise.resolve(() => {}))
    await mountPlugins()
    app!.unmount()
    app = null
    pending.resolve(off)
    await flush()
    expect(off).toHaveBeenCalledOnce()
  })

  it('combines name/description/source search with enabled-state filtering and can clear filters', async () => {
    const host = await mountPlugins()
    const input = host.querySelector<HTMLInputElement>('[data-test="plugin-search"]')
    expect(input).toBeTruthy()
    input!.value = '  MUSIC  '
    input!.dispatchEvent(new Event('input'))
    await nextTick()
    expect(row(host, 'Alpha Video')).toBeUndefined()
    expect(row(host, 'Beta Audio')).toBeTruthy()
    const filter = host.querySelector<HTMLSelectElement>('[data-test="plugin-status-filter"]')!
    filter.value = 'enabled'
    filter.dispatchEvent(new Event('change'))
    await nextTick()
    expect(row(host, 'Beta Audio')).toBeUndefined()
    host.querySelector<HTMLButtonElement>('[data-test="plugin-clear-filters"]')!.click()
    await nextTick()
    expect(row(host, 'Alpha Video')).toBeTruthy()
    expect(row(host, 'Beta Audio')).toBeTruthy()
    input!.value = 'github'
    input!.dispatchEvent(new Event('input'))
    await nextTick()
    expect(row(host, 'Alpha Video')).toBeTruthy()
    expect(row(host, 'Beta Audio')).toBeUndefined()
  })

  it('ignores Enter while a resolver run is pending', async () => {
    const pending = deferred<string>()
    handlers.plugin_run_resolvers = () => pending.promise
    const host = await mountPlugins()
    const input = host.querySelector<HTMLInputElement>('[data-test="plugin-resolver"] input')!
    input.value = 'https://example.com/video'
    input.dispatchEvent(new Event('input'))
    await nextTick()
    input.dispatchEvent(new KeyboardEvent('keyup', { key: 'Enter' }))
    input.dispatchEvent(new KeyboardEvent('keyup', { key: 'Enter' }))
    await flush()
    expect(commands.filter(({ command }) => command === 'plugin_run_resolvers')).toHaveLength(1)
    pending.resolve('resolved')
    await flush()
  })

  it('keeps the newer installed list when an older refresh arrives late', async () => {
    const older = deferred<PluginInfo[]>()
    const newer = deferred<PluginInfo[]>()
    let request = 0
    handlers.plugin_list = () => ++request === 1 ? older.promise : newer.promise
    const host = await mountPlugins()
    const onInstalled = vi.mocked(listen).mock.calls.find(([event]) => event === 'plugin://installed')![1]
    onInstalled({ event: 'plugin://installed', id: 1, payload: null })
    newer.resolve([{ ...plugins[1], name: 'New Audio' }])
    await flush()
    expect(row(host, 'New Audio')).toBeTruthy()
    older.resolve(plugins)
    await flush()
    expect(row(host, 'New Audio')).toBeTruthy()
    expect(row(host, 'Alpha Video')).toBeUndefined()
  })

  it('removes a successfully uninstalled plugin even if reloading the list fails', async () => {
    const host = await mountPlugins()
    handlers.plugin_list = () => { throw new Error('List unavailable') }
    button(row(host, 'Alpha Video'), '卸载').click()
    await flush()
    expect(row(host, 'Alpha Video')).toBeUndefined()
    expect(row(host, 'Beta Audio')).toBeTruthy()
    expect(host.textContent).toContain('List unavailable')
  })

  it('shows a successful enabled-state change even if reloading the list fails', async () => {
    const host = await mountPlugins()
    handlers.plugin_list = () => { throw new Error('List unavailable') }
    row(host, 'Beta Audio').querySelector<HTMLElement>('[role="switch"]')!.click()
    await flush()
    expect(row(host, 'Beta Audio').querySelector('[role="switch"]')!.getAttribute('aria-checked')).toBe('true')
    expect(host.textContent).toContain('List unavailable')
  })

  it('shows the successfully installed plugin even if reloading the list fails', async () => {
    const host = await mountPlugins()
    handlers.plugin_list = () => { throw new Error('List unavailable') }
    handlers.plugin_install_from_url = () => ({ ...plugins[0], id: 'gamma', name: 'Gamma Resolver' })
    const input = host.querySelector<HTMLInputElement>('[data-test="plugin-install-url"] input')!
    input.value = 'https://github.com/example/gamma'
    input.dispatchEvent(new Event('input'))
    await nextTick()
    input.dispatchEvent(new KeyboardEvent('keyup', { key: 'Enter' }))
    await flush()
    expect(row(host, 'Gamma Resolver')).toBeTruthy()
    expect(host.textContent).toContain('List unavailable')
  })
})
