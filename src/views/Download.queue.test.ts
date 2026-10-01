/**
 * 下载队列 —— 删除记录 / 清理已完成的本地摘行回归
 *
 * 背景（实机复现过的同类 bug）：删除只 await 后端命令，store 不更新，界面停在旧行数
 * （字幕 / 转换页实机复现；下载页当时靠「删完 reload 全量」掩盖，属于同一条纪律）。
 * 这里用 mock 的 IPC 边界驱动真实视图：后端删掉几条，DOM 行数 / store 长度 / 后端剩余
 * 条数必须三者一致，且删除失败时一行都不许动。
 */
import { beforeAll, describe, expect, it, vi } from 'vitest'
import { createApp, h, nextTick } from 'vue'
import { createPinia } from 'pinia'
import { NDialogProvider, NMessageProvider } from 'naive-ui'
import Download from '@/views/Download.vue'
import router from '@/router'
import { i18n } from '@/i18n'
import { useTaskStore } from '@/stores/tasks'
import type { DownloadTask } from '@/types'

const DIR = String.raw`D:\Videos\umi`

const done = (id: string, title: string): DownloadTask => ({
  id,
  title,
  url: `https://example.com/${id}`,
  status: 'done',
  progress: 100,
  file_path: `${DIR}\\${id}.mp4`,
  created_time: 1,
})
const running = (id: string, title: string): DownloadTask => ({
  id,
  title,
  url: `https://example.com/${id}`,
  status: 'downloading',
  progress: 42.5,
  created_time: 2,
})

/** 当前 list_downloads 应答（后端真库的替身） */
let downloadsMock: DownloadTask[] = []
/** 非 null 时 remove_download 直接失败 */
let removeError: string | null = null
/** 非 null 时 clear_downloads 直接失败 */
let clearError: string | null = null

function installInvokeStub() {
  let cbId = 0
  const handlers: Record<string, (args: any) => unknown> = {
    list_downloads: () => downloadsMock.map((t) => ({ ...t })),
    list_converts: () => [],
    list_subtitles: () => [],
    detect_tools: () => [],
    list_tools: () => [],
    backfill_covers: () => 0,
    file_sizes: (args: any) => (args?.paths ?? []).map((p: string) => ({ path: p, size: null, exists: false, modified: null })),
    get_settings: () => ({ download_dir: DIR, default_crf: 23 }),
    default_download_dir: () => DIR,
    app_data_dir: () => 'C:\\Users\\user\\AppData\\Roaming\\umi-downloader',
    convert_formats: () => ({ total: 0, groups: [], engines: {} }),
    doc_capabilities: () => ({ pandoc: false, poppler: false, native: [], targets: [] }),
    whisper_models: () => [],
    // 真删：后端删掉后才返回 ok，而且 mock 列表同步少一条
    remove_download: (args: any) => {
      if (removeError) throw new Error(removeError)
      const i = downloadsMock.findIndex((t) => t.id === args?.id)
      if (i >= 0) downloadsMock.splice(i, 1)
      return null
    },
    clear_downloads: (args: any) => {
      if (clearError) throw new Error(clearError)
      // 与后端 db.rs 同口径：done 只删终态行
      if (args?.which === 'done') downloadsMock = downloadsMock.filter((t) => !['done', 'error', 'canceled', 'handed_off'].includes(t.status))
      else downloadsMock = []
      return null
    },
    open_path: () => null,
    reveal_path: () => null,
    frontend_log: () => null,
    reveal_in_folder: () => null,
    'plugin:event|listen': () => 1,
    'plugin:event|unlisten': () => null,
    'plugin:dialog|open': () => null,
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

/** 队列里的 <tbody> 行（TaskTable = 唯一表格） */
function rows(): HTMLElement[] {
  return Array.from(document.querySelectorAll<HTMLElement>('table tbody tr'))
}

function rowByTitle(name: string): HTMLElement | undefined {
  return rows().find((r) => (r.textContent || '').includes(name))
}

const norm = (s: string | undefined) => (s || '').replace(/\s+/g, ' ').trim()

function buttonByTitle(row: HTMLElement | undefined, title: string): HTMLButtonElement | undefined {
  return Array.from(row?.querySelectorAll<HTMLButtonElement>('button') ?? []).find(
    (b) => b.getAttribute('title') === title,
  )
}

function clearButton(): HTMLButtonElement | undefined {
  return Array.from(document.querySelectorAll<HTMLButtonElement>('button')).find(
    (b) => norm(b.textContent).includes('清理已完成'),
  )
}

async function flush() {
  for (let i = 0; i < 12; i++) await nextTick()
  await new Promise((r) => setTimeout(r, 40))
  for (let i = 0; i < 8; i++) await nextTick()
}

async function mountDownload() {
  const host = document.createElement('div')
  document.body.appendChild(host)
  const pinia = createPinia()
  const app = createApp({
    setup: () => () =>
      h(NDialogProvider, null, {
        default: () => h(NMessageProvider, null, { default: () => h(Download) }),
      }),
  })
  app.use(pinia)
  app.use(router)
  app.use(i18n)
  app.mount(host)
  for (let i = 0; i < 14; i++) await nextTick()
  await new Promise((r) => setTimeout(r, 150))
  for (let i = 0; i < 8; i++) await nextTick()
  return { app, host, pinia }
}

describe('下载队列：删除记录 / 清理已完成', () => {
  beforeAll(() => {
    stubBrowserApis()
    installInvokeStub()
  })

  it('删除记录（后端成功）：store 与 DOM 同步少一行，等于后端剩余条数', async () => {
    downloadsMock = [done('dl-a', '测试任务 A'), done('dl-b', '测试任务 B')]
    removeError = null
    const { host, pinia } = await mountDownload()
    const store = useTaskStore(pinia)
    const before = { store: store.downloads.length, dom: rows().length, backend: downloadsMock.length }
    console.log('[删除前] store/dom/backend =', JSON.stringify(before))
    expect(before, '三处口径起手就该一致').toEqual({ store: 2, dom: 2, backend: 2 })

    const row = rowByTitle('测试任务 A')
    expect(row, '缺少待删除的行').toBeTruthy()
    const del = buttonByTitle(row, '删除记录')
    expect(del, '行内缺少「删除记录」按钮').toBeTruthy()
    del!.click()
    await flush()

    const after = { store: store.downloads.length, dom: rows().length, backend: downloadsMock.length }
    console.log('[删除后] store/dom/backend =', JSON.stringify(after))
    expect(after.backend, '后端应真的少一条').toBe(1)
    expect(after.store, 'store 必须同步少一条').toBe(1)
    expect(after.dom, 'DOM 必须同步少一行').toBe(1)
    expect(after.dom, 'DOM 行数必须等于 store 长度').toBe(after.store)
    expect(rowByTitle('测试任务 A'), '被删的行不该还在').toBeFalsy()
    expect(store.downloads.every((t) => t.id !== 'dl-a'), 'store 里不该还留着被删的 id').toBe(true)

    host.remove()
  })

  it('删除记录（后端报错）：一行都不动，并把失败原因摊给用户', async () => {
    downloadsMock = [done('dl-a', '测试任务 A'), done('dl-b', '测试任务 B')]
    removeError = '删除失败：数据库被占用'
    const { host, pinia } = await mountDownload()
    const store = useTaskStore(pinia)

    const row = rowByTitle('测试任务 A')
    buttonByTitle(row, '删除记录')!.click()
    await flush()

    expect(store.downloads.length, '后端失败：store 一行都不能少').toBe(2)
    expect(rows().length, '后端失败：DOM 一行都不能少').toBe(2)
    expect(norm(document.body.textContent), '应把后端错误原文摊给用户').toContain('删除失败：数据库被占用')

    removeError = null
    host.remove()
  })

  it('清理已完成（后端成功）：终态行全部摘掉，进行中的行留下，三处口径仍一致', async () => {
    downloadsMock = [done('dl-a', '测试任务 A'), running('dl-run', '测试任务 进行中'), done('dl-b', '测试任务 B')]
    clearError = null
    const { host, pinia } = await mountDownload()
    const store = useTaskStore(pinia)
    const before = { store: store.downloads.length, dom: rows().length, backend: downloadsMock.length }
    console.log('[清理前] store/dom/backend =', JSON.stringify(before))
    expect(before).toEqual({ store: 3, dom: 3, backend: 3 })

    const btn = clearButton()
    expect(btn, '缺少「清理已完成」按钮').toBeTruthy()
    btn!.click()
    await flush()

    const after = { store: store.downloads.length, dom: rows().length, backend: downloadsMock.length }
    console.log('[清理后] store/dom/backend =', JSON.stringify(after))
    expect(after.backend, '后端应只剩进行中的那条').toBe(1)
    expect(after.store, 'store 必须同步摘掉终态行').toBe(1)
    expect(after.dom, 'DOM 必须同步少行').toBe(1)
    expect(after.dom, 'DOM 行数必须等于 store 长度').toBe(after.store)
    expect(rows()[0]?.textContent || '', '留下的应该正好是进行中的那条').toContain('测试任务 进行中')

    host.remove()
  })

  it('清理已完成（后端报错）：一行都不动', async () => {
    downloadsMock = [done('dl-a', '测试任务 A'), done('dl-b', '测试任务 B')]
    clearError = '清理失败：数据库被占用'
    const { host, pinia } = await mountDownload()
    const store = useTaskStore(pinia)

    clearButton()!.click()
    await flush()

    expect(store.downloads.length, '后端失败：store 一行都不能少').toBe(2)
    expect(rows().length, '后端失败：DOM 一行都不能少').toBe(2)
    expect(norm(document.body.textContent), '应把后端错误原文摊给用户').toContain('清理失败：数据库被占用')

    clearError = null
    host.remove()
  })
})
