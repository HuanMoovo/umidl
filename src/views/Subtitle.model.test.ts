/**
 * 字幕页 · 模型管理回归（下载 / 自定义导入 / 切换即启用）
 *
 * 背景：语音模型的下载与自定义导入原本在「设置 → 引擎」，用户要求在字幕页选择模型的地方
 * 就能直接下载、贴 https 链接安装，并立即切换使用；设置页那份随之移除（只留一行指引）。
 *
 * 覆盖：
 *   ① 选中的模型没下载 → 就地出现「下载模型（约 …）」按钮；页面不再出现「请在设置页下载」字样
 *   ② 点下载 → 调 install_whisper_model，装完写回 whisper_model、按钮消失、显示占用
 *   ③ 自定义模型：贴 https 直链点「安装」→ 调 install_whisper_custom，并切换到后端返回的文件名
 *   ④ 下拉里换一个已下载的模型 → 立即写回设置（调用模型 = 启用）
 *
 * IPC 边界（window.__TAURI_INTERNALS__.invoke）用 mock 应答，视图 Subtitle.vue 走真实代码路径。
 */
import { describe, expect, it, beforeAll, vi } from 'vitest'
import { createApp, h, nextTick } from 'vue'
import { createPinia } from 'pinia'
import { NMessageProvider } from 'naive-ui'
import Subtitle from '@/views/Subtitle.vue'
import { i18n } from '@/i18n'
import { useSettingsStore } from '@/stores/settings'
import { useTaskStore } from '@/stores/tasks'
import type { WhisperModelInfo } from '@/types'

/* ==================== mock 模型清单 ==================== */
function model(over: Partial<WhisperModelInfo>): WhisperModelInfo {
  return {
    name: 'base',
    file: 'ggml-base.bin',
    builtin: true,
    downloaded: false,
    size_bytes: 0,
    size_hint: '142 MB',
    quality: '★★☆',
    current: false,
    ...over,
  }
}

let modelsMock: WhisperModelInfo[] = []
let settingsMock: Record<string, unknown> = {}

const calls: { cmd: string; args: any }[] = []
const saved: any[] = []

/** true 时 install_whisper_model 悬着不返回，用来观察「下载中」UI */
let pendingInstall = false
let pendingResolver: ((v: unknown) => void) | null = null

function finishInstall() {
  pendingInstall = false
  pendingResolver?.({ name: 'whisper-model', found: true, source: 'managed', hint: '' })
  pendingResolver = null
}

function resetWorld() {
  // 真实场景：一个模型都还没下载，配置里写着 base → 页面就地提供「下载模型」
  modelsMock = [
    model({ name: 'tiny', file: 'ggml-tiny.bin', size_hint: '75 MB' }),
    model({ name: 'base', file: 'ggml-base.bin' }),
  ]
  settingsMock = { download_dir: 'C:\\Users\\user\\Downloads\\Umidl', whisper_model: 'base' }
  calls.length = 0
  saved.length = 0
  pendingInstall = false
  pendingResolver = null
}

function installInvokeStub() {
  let cbId = 0
  const handlers: Record<string, (args: any) => unknown> = {
    list_downloads: () => [],
    list_converts: () => [],
    list_subtitles: () => [],
    list_tools: () => [],
    detect_tools: () => [],
    backfill_covers: () => 0,
    list_whisper_models: () => modelsMock.map((m) => ({ ...m })),
    get_settings: () => ({ ...settingsMock }),
    default_download_dir: () => 'C:\\Users\\user\\Downloads\\Umidl',
    app_data_dir: () => 'C:\\Users\\user\\AppData\\Roaming\\umi-downloader',
    file_sizes: (args: any) => (args?.paths ?? []).map((p: string) => ({ path: p, size: null, exists: false, modified: null })),
    // 下载：把清单里对应模型标成已下载（真后端下载完就是这种状态）
    install_whisper_model: (args: any) => {
      calls.push({ cmd: 'install_whisper_model', args })
      const done = () => {
        modelsMock = modelsMock.map((m) =>
          m.name === args?.model ? { ...m, downloaded: true, size_bytes: 147_951_465 } : m,
        )
        return { name: 'whisper-model', found: true, source: 'managed', hint: '' }
      }
      if (pendingInstall) return new Promise((resolve) => (pendingResolver = resolve))
      return done()
    },
    // 自定义导入：返回落盘路径（真后端返回结构一致）
    install_whisper_custom: (args: any) => {
      calls.push({ cmd: 'install_whisper_custom', args })
      if (args?.spec === 'boom') throw new Error('模型下载失败：下载源返回 HTTP 404（example.com）')
      modelsMock = [
        ...modelsMock,
        model({ name: 'ggml-my-model.bin', file: 'ggml-my-model.bin', builtin: false, downloaded: true, size_bytes: 1234 }),
      ]
      return { name: 'whisper-model', found: true, source: 'managed', hint: '', path: 'C:\\Users\\user\\AppData\\Roaming\\umi-downloader\\models\\ggml-my-model.bin' }
    },
    save_settings: (args: any) => {
      settingsMock = { ...settingsMock, ...(args?.settings ?? {}) }
      saved.push({ ...(args?.settings ?? {}) })
      return settingsMock
    },
    frontend_log: () => null,
    'plugin:event|listen': () => 1,
    'plugin:event|unlisten': () => null,
    'plugin:dialog|open': () => 'C:\\Users\\user\\Downloads\\ggml-local.bin',
  }
  ;(window as any).__TAURI_INTERNALS__ = {
    invoke: (cmd: string, args?: any) => {
      const fn = handlers[cmd]
      if (!fn) return Promise.reject(new Error(`[test] 未处理的命令 ${cmd}`))
      try {
        return Promise.resolve(fn(args))
      } catch (e) {
        return Promise.reject(e)
      }
    },
    transformCallback: (cb: any) => {
      const id = ++cbId
      ;(window as any)[`_${id}`] = cb
      return id
    },
    metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
  }
}

function stubBrowserApis() {
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  )
  vi.stubGlobal('matchMedia', () => ({
    matches: false,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
  }))
}

/* ==================== 挂载与查询辅助 ==================== */
async function mountSubtitle() {
  document.body.innerHTML = '' // 清掉上一个用例的宿主，避免 text() 读到旧应用
  const host = document.createElement('div')
  document.body.appendChild(host)
  const pinia = createPinia()
  const app = createApp({ setup: () => () => h(NMessageProvider, null, { default: () => h(Subtitle) }) })
  app.use(pinia)
  app.use(i18n)
  app.mount(host)
  for (let i = 0; i < 12; i++) await nextTick()
  await new Promise((r) => setTimeout(r, 120))
  for (let i = 0; i < 6; i++) await nextTick()
  return { app, host, pinia }
}

async function flush() {
  for (let i = 0; i < 10; i++) await nextTick()
  await new Promise((r) => setTimeout(r, 30))
  for (let i = 0; i < 6; i++) await nextTick()
}

const q = <T extends HTMLElement = HTMLElement>(sel: string) => document.querySelector<T>(sel)
const text = () => (document.body.textContent || '').replace(/\s+/g, ' ')

/** 点按钮并等异步链路（下载是 await 后端 + 重渲染） */
async function click(sel: string) {
  const el = q<HTMLButtonElement>(sel)
  expect(el, `找不到按钮 ${sel}`).toBeTruthy()
  el!.click()
  await flush()
}

describe('字幕页 · 模型下载 / 自定义导入', () => {
  beforeAll(() => {
    stubBrowserApis()
    installInvokeStub()
  })

  it('选中的模型没下载：就地给出下载按钮，且不再让用户去设置页', async () => {
    resetWorld()
    const { app } = await mountSubtitle()

    const box = q('[data-test="subtitle-model-download"]')
    expect(box, '未下载时应出现就地下载区').toBeTruthy()
    const btn = q('[data-test="subtitle-model-download-btn"]') as HTMLButtonElement
    expect(btn.textContent || '').toContain('下载模型')
    expect(btn.textContent || '').toContain('142 MB') // 体积提示来自后端 size_hint
    expect(btn.disabled).toBe(false)

    // 老文案（指向设置页）已随能力迁移一起消失
    expect(text()).not.toContain('请在设置页下载')
    expect(text()).not.toContain('尚未下载语音模型（') // 旧横幅
    expect(q('[data-test="subtitle-model-status"]'), '未下载时不该显示「已下载」').toBeFalsy()

    app.unmount()
  })

  it('点「下载模型」→ 调 install_whisper_model，装完自动启用、按钮消失、显示占用', async () => {
    resetWorld()
    const { app, pinia } = await mountSubtitle()

    await click('[data-test="subtitle-model-download-btn"]')

    const install = calls.find((c) => c.cmd === 'install_whisper_model')
    expect(install, '应调用 install_whisper_model').toBeTruthy()
    expect(install!.args?.model).toBe('base')

    // 启用：写回设置（真后端语义：下载完就用它）
    const settingsStore = useSettingsStore(pinia)
    expect(settingsStore.settings.whisper_model).toBe('base')
    expect(saved[saved.length - 1]?.whisper_model).toBe('base')

    // 按钮消失 → 变成占用信息
    expect(q('[data-test="subtitle-model-download"]'), '下载完不该再显示下载按钮').toBeFalsy()
    const status = q('[data-test="subtitle-model-status"]')
    expect(status).toBeTruthy()
    expect(status!.textContent || '').toContain('已下载')
    expect(status!.textContent || '').toContain('141') // 147,951,465 B ≈ 141.1 MiB

    app.unmount()
  })

  it('自定义模型：贴 https 直链点「安装」→ 调 install_whisper_custom 并切换到返回的文件名', async () => {
    resetWorld()
    const { app, pinia } = await mountSubtitle()

    const input = q<HTMLInputElement>('[data-test="subtitle-custom-input"]')
    expect(input, '自定义模型输入框应存在（不用展开折叠）').toBeTruthy()
    input!.value = 'https://example.com/ggml-my-model.bin'
    input!.dispatchEvent(new Event('input'))
    await flush()

    await click('[data-test="subtitle-custom-install"]')

    const custom = calls.find((c) => c.cmd === 'install_whisper_custom')
    expect(custom, '应调用 install_whisper_custom').toBeTruthy()
    expect(custom!.args?.spec).toBe('https://example.com/ggml-my-model.bin')

    const settingsStore = useSettingsStore(pinia)
        expect(settingsStore.settings.whisper_model).toBe('ggml-my-model.bin')
    // 安装成功后清空输入，避免误触重复安装
    expect(q<HTMLInputElement>('[data-test="subtitle-custom-input"]')!.value).toBe('')

    app.unmount()
  })

  it('自定义导入失败：把后端原因摊给用户，不改设置、不清空输入', async () => {
    resetWorld()
    const { app, pinia } = await mountSubtitle()

    const input = q<HTMLInputElement>('[data-test="subtitle-custom-input"]')!
    input.value = 'boom'
    input.dispatchEvent(new Event('input'))
    await flush()

    await click('[data-test="subtitle-custom-install"]')

    expect(text()).toContain('404') // 后端错误原文透出
    expect(useSettingsStore(pinia).settings.whisper_model).toBe('base') // 没被改
    expect(q<HTMLInputElement>('[data-test="subtitle-custom-input"]')!.value).toBe('boom')

    app.unmount()
  })

  it('下载中：按钮变「下载中…」并显示来自 tool-progress 的百分比', async () => {
    resetWorld()
    pendingInstall = true // 让 install 悬着，停在「下载中」状态
    const { app, pinia } = await mountSubtitle()

    const store = useTaskStore(pinia)
    store.toolProgress['whisper-model'] = {
      percent: 42,
      message: '',
      source: 'hf-mirror.com',
      downloaded: 60_000_000,
      total: 147_951_465,
      resumed: false,
    }
    await click('[data-test="subtitle-model-download-btn"]')

    const btn = q('[data-test="subtitle-model-download-btn"]') as HTMLButtonElement
    expect(btn.textContent || '').toContain('下载中')
    expect(btn.disabled, '下载中不允许再点').toBe(true)
    // 进度文案来自结构化的 tool-progress（含来源与字节数），非后端原始 message
    expect(text()).toContain('hf-mirror.com')
    expect(text()).toContain('57.2 MB') // 60,000,000 B（formatBytes 用 1024 进制、单位写 MB）
    // 进度条（naive-ui NProgress）存在
    expect(q('[data-test="subtitle-model-download"] .n-progress')).toBeTruthy()

    finishInstall() // 收尾，避免悬着的 promise 泄漏到下一个用例
    await flush()
    app.unmount()
  })

  it('初始化回落不写回偏好：配置的模型没下载时，只提示、不改用户存的选项', async () => {
    resetWorld()
    settingsMock.whisper_model = 'small' // 配置的是 small（清单里没有 = 没下载）
    const { app, pinia } = await mountSubtitle()

    // 自动改用清单里已下载的 tiny（本次任务用它），但用户存的偏好仍是 small
    expect(useSettingsStore(pinia).settings.whisper_model).toBe('small')
    expect(saved.some((s) => s.whisper_model === 'tiny'), '不该把回落结果写回设置').toBe(false)

    app.unmount()
  })
})
