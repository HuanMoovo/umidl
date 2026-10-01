/**
 * 转换队列回归（真机数据驱动）
 *
 * 数据来源全部是真机产物（不是手搓 mock）：
 *   - `%APPDATA%/umi-downloader/convproof/psd/task.json`  —— 真机跑完 png→PSD 转换后由后端
 *     `converter::new_convert_task` 生成的 ConvertTask（字段与 start_convert 落库的一致）
 *   - `%APPDATA%/umi-downloader/convproof/catalog.json`   —— 真机 convert_formats 的输出（88 条，
 *     其中 80 条 available=true：psd/dds 由 imagemagick 真写出，ape/heic/svg/doc/xls/ppt/json/xml
 *     本机引擎写不出来）
 *   - 字节数直接从磁盘上的真实文件读
 *
 * IPC 边界（window.__TAURI_INTERNALS__.invoke）用真机数据应答，视图/行组件代码全部是真实代码路径。
 */
import { describe, expect, it, beforeAll, vi } from 'vitest'
import fs from 'node:fs'
import path from 'node:path'
import { createApp, h, nextTick } from 'vue'
import { createPinia } from 'pinia'
import { NMessageProvider } from 'naive-ui'
import Converter from '@/views/Converter.vue'
import { i18n } from '@/i18n'
import { useTaskStore } from '@/stores/tasks'
import { formatBytes } from '@/services/utils'

const PROOF = path.join(process.env.APPDATA || '', 'umi-downloader', 'convproof')
const TASK_JSON = path.join(PROOF, 'psd', 'task.json')
const CATALOG_JSON = path.join(PROOF, 'catalog.json')

const proof: any = fs.existsSync(TASK_JSON) ? JSON.parse(fs.readFileSync(TASK_JSON, 'utf8')) : null
const catalog: any = fs.existsSync(CATALOG_JSON) ? JSON.parse(fs.readFileSync(CATALOG_JSON, 'utf8')) : null

/** 真机目录基准：可用 / 不可用 id 列表 + 各分组可用条数（网格、芯片、统计都必须对齐它） */
const availableIds: string[] = catalog
  ? (catalog.groups as any[]).flatMap((g) => (g.formats as any[]).filter((f) => f.available).map((f) => String(f.id)))
  : []
const unavailableIds: string[] = catalog
  ? (catalog.groups as any[]).flatMap((g) => (g.formats as any[]).filter((f) => !f.available).map((f) => String(f.id)))
  : []
const availableByGroup: Record<string, number> = {}
for (const g of (catalog?.groups ?? []) as any[]) {
  availableByGroup[String(g.id)] = (g.formats as any[]).filter((f) => f.available).length
}

/** 真机已完成任务（status/progress 换成完成态；文件确实在磁盘上） */
const REAL_TASK = proof
  ? { ...proof.task, status: 'done', progress: 100, error: null }
  : { id: 'x', input_file: '', output_file: '', format: '', status: 'done', progress: 100, created_time: 0 }

/** 当前 list_converts 应答（删除用例会改成两条）；非 null 时 remove_convert 直接失败 */
let convertsMock: any[] = [REAL_TASK]
let removeConvertError: string | null = null

/** 第二条转换记录（失败态，产物不在盘上）：删除用例的靶子 */
const T_ERR_CONV: any = {
  ...REAL_TASK,
  id: 'cv-err-probe',
  input_file: 'D:\\Media Files\\Videos\\Umidl\\cv-err-probe.mp4',
  output_file: 'D:\\Media Files\\Videos\\Umidl\\cv-err-probe.mkv',
  format: 'mkv',
  status: 'error',
  progress: 30,
  error: '编码器退出码 1：不支持的参数',
}

function fileSizes(paths: string[]) {
  return paths.map((p) => {
    try {
      const st = fs.statSync(p)
      return { path: p, size: st.size, exists: st.isFile(), modified: Math.round(st.mtimeMs) }
    } catch {
      return { path: p, size: null, exists: false, modified: null }
    }
  })
}

function installInvokeStub() {
  let cbId = 0
  const handlers: Record<string, (args: any) => unknown> = {
    list_converts: () => convertsMock.map((t) => ({ ...t })),
    list_downloads: () => [],
    list_subtitles: () => [],
    detect_tools: () => [],
    list_tools: () => [],
    backfill_covers: () => 0,
    convert_formats: () => catalog ?? { total: 0, groups: [], engines: {} },
    doc_capabilities: () => ({
      pandoc: true,
      poppler: true,
      native: ['docx', 'odt', 'txt', 'md', 'html', 'csv', 'pptx'],
      targets: ['md', 'txt', 'html', 'docx', 'epub', 'rtf', 'csv', 'pptx'],
    }),
    probe_document: () => ({ ok: true, kind: 'image' }),
    probe_media: () => ({ path: REAL_TASK.input_file, size: proof?.input_bytes ?? 0 }),
    get_settings: () => ({ download_dir: 'C:\\Users\\user\\Downloads\\Umidl', default_crf: 23 }),
    default_download_dir: () => 'C:\\Users\\user\\Downloads\\Umidl',
    app_data_dir: () => PROOF,
    file_sizes: (args: any) => fileSizes(args?.paths ?? []),
    open_path: () => null,
    reveal_path: () => null,
    // 真删：remove_convert 成功后 mock 列表也少一条（视图 / store / 后端三方口径一致）
    remove_convert: (args: any) => {
      if (removeConvertError) throw new Error(removeConvertError)
      const i = convertsMock.findIndex((t) => t.id === args?.id)
      if (i >= 0) convertsMock.splice(i, 1)
      return null
    },
    frontend_log: () => null,
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

/** 队列容器（简约行列表） */
function queueBox(): HTMLElement | null {
  return document.querySelector<HTMLElement>('[data-role="convert-queue"]')
}

/** 行文本：队列里包含目标文件名的那个 convert-row */
function queueRow(name: string): HTMLElement | undefined {
  return Array.from(queueBox()?.querySelectorAll<HTMLElement>('[data-role="convert-row"]') ?? []).find((n) =>
    (n.textContent || '').includes(name),
  )
}

const norm = (s: string | undefined) => (s || '').replace(/\s+/g, ' ').trim()

/** 行上的按钮（title 属性） */
function rowButtons(row: HTMLElement | undefined): string[] {
  return Array.from(row?.querySelectorAll('button') ?? []).map(
    (b) => b.getAttribute('title') || norm(b.textContent),
  )
}

/** 格式网格里某个目标格式的可用状态（data-available: '1'；不可用条目已被整条过滤，取到 null） */
function formatAvailability(id: string): string | null {
  const el = document.querySelector(`[data-format="${id}"]`) as HTMLElement | null
  return el?.getAttribute('data-available') ?? null
}

/** 网格里已渲染的目标格式 id */
function renderedFormatIds(): string[] {
  return Array.from(document.querySelectorAll<HTMLElement>('[data-format]')).map(
    (e) => e.getAttribute('data-format') || '',
  )
}

/** 页面里残留的不可用（置灰）条目数 —— 新口径下必须是 0 */
function greyCount(): number {
  return document.querySelectorAll('[data-format][data-available="0"]').length
}

async function mountConverter() {
  const host = document.createElement('div')
  document.body.appendChild(host)
  const pinia = createPinia()
  const app = createApp({
    setup: () => () => h(NMessageProvider, null, { default: () => h(Converter) }),
  })
  app.use(pinia)
  app.use(i18n)
  app.mount(host)
  for (let i = 0; i < 12; i++) await nextTick()
  await new Promise((r) => setTimeout(r, 120))
  for (let i = 0; i < 6; i++) await nextTick()
  return { app, host, pinia }
}

/** 队列里当前渲染的行数 */
function convRows(): number {
  return queueBox()?.querySelectorAll('[data-role="convert-row"]').length ?? 0
}

/** 等一拍：删除是 async（await 后端命令 → 改 store → 重渲染） */
async function flush() {
  for (let i = 0; i < 10; i++) await nextTick()
  await new Promise((r) => setTimeout(r, 30))
  for (let i = 0; i < 6; i++) await nextTick()
}

/** 行内某个 title 的按钮 */
function buttonByTitle(row: HTMLElement | undefined, title: string): HTMLButtonElement | undefined {
  return Array.from(row?.querySelectorAll<HTMLButtonElement>('button') ?? []).find(
    (b) => b.getAttribute('title') === title,
  )
}

describe.skipIf(!proof || !catalog)('转换队列行列表（真机数据）', () => {
  beforeAll(() => {
    stubBrowserApis()
    installInvokeStub()
  })

  it('队列是简约行列表：源→目标与真实字节数齐全，缩略图/预览/卡片包裹全部消失', async () => {
    const { host } = await mountConverter()
    const box = queueBox()
    const row = queueRow(proof.output_name)
    const text = norm(row?.textContent)
    const buttons = rowButtons(row)
    console.log('[行文本]', text)
    console.log('[行按钮]', JSON.stringify(buttons))
    console.log('[格式网格] 条目', renderedFormatIds().length, '置灰', greyCount(), '真机可用', availableIds.length)
    console.log('[头部统计]', norm(document.querySelector('[data-role="format-total"]')?.textContent), norm(document.querySelector('[data-role="format-available"]')?.textContent))

    // 队列容器存在且自己内滚（页面高度不再随记录数增长）
    expect(box, '缺少 [data-role="convert-queue"] 容器').toBeTruthy()
    expect((box?.className || '').includes('overflow-y-auto'), '队列容器应自己内滚').toBe(true)
    expect(box?.querySelectorAll('[data-role="convert-row"]').length, '队列行数应等于记录数').toBe(1)
    // 行本身就是列表项，不再有卡片包裹 / 缩略图块
    expect((row?.className || '').includes('umi-card'), '队列行不该再是卡片').toBe(false)
    expect(row?.querySelector('img'), '队列行不该有缩略图').toBeNull()
    expect(row?.querySelector('div[class*="s-sunken"]'), '队列行不该有缩略图占位块').toBeNull()

    // 行里必须有：目标文件名 + 源文件名 + 目标格式 + 两端真实字节数
    expect(text, '缺少目标文件名').toContain(proof.output_name)
    expect(text, '缺少源文件名').toContain(proof.input_name)
    expect(text, '缺少目标格式').toMatch(/PSD/i)
    const inText = formatBytes(proof.input_bytes)
    const outText = formatBytes(proof.output_bytes)
    expect(text, `缺少源文件大小 ${inText}`).toContain(inText)
    expect(text, `缺少产物大小 ${outText}`).toContain(outText)
    // 产物就在磁盘上，行里绝不能报「文件已丢失」（旧版会误报）
    expect(text, '产物存在却提示文件已丢失').not.toContain('文件已丢失')

    // 产物在磁盘上 → 应有打开 / 在文件夹中显示；「预览」入口已按用户要求从转换模块整体移除
    expect(buttons, '缺少「打开文件」按钮').toContain('打开文件')
    expect(buttons, '缺少「在文件夹中显示」按钮').toContain('在文件夹中显示')
    expect(buttons, '缺少「删除记录」按钮').toContain('删除记录')
    expect(buttons, '转换队列不应该再有预览按钮').not.toContain('预览')
    expect(buttons.length, '已完成任务只留 打开/显示/删除 三个动作').toBe(3)

    host.remove()
  })

  it('格式网格只渲染可用格式：80 条、零置灰、不可用项一条都不出现', async () => {
    const { host } = await mountConverter()
    const rendered = renderedFormatIds()
    console.log('[网格 id]', rendered.length, rendered.slice(0, 8).join(','), '… 不可用项', unavailableIds.join(','))

    // 条目总数 == 后端 available=true 的条数（88 → 80）
    expect(rendered.length, '网格条目数应等于真机可用格式数').toBe(availableIds.length)
    expect(rendered.length, '真机目录里可用格式应为 80 条').toBe(80)
    // 页面里不再存在任何不可用条目
    expect(greyCount(), '页面里不应再出现置灰条目').toBe(0)
    // 不可用的格式一条都不渲染
    for (const id of unavailableIds) {
      expect(rendered, `不可用格式 ${id} 不该出现在网格里`).not.toContain(id)
      expect(formatAvailability(id), `不可用格式 ${id} 不该有 DOM 节点`).toBeNull()
    }
    // 真机可用的格式（含 psd/dds/dts/alac/pnm）全部在网格里且为可选态
    for (const id of ['psd', 'dds', 'dts', 'alac', 'pnm']) {
      expect(formatAvailability(id), `${id} 应可选`).toBe('1')
    }
    for (const id of rendered) expect(formatAvailability(id), `${id} 应是可用态`).toBe('1')
    // 页面文案里不再解释「本机引擎不支持」
    expect(norm(document.querySelector('[data-step="format"]')?.textContent)).not.toContain('本机引擎不支持')

    host.remove()
  })

  it('头部统计与类型芯片计数只算可用项（四类之和 = 80）', async () => {
    const { host } = await mountConverter()
    const totalText = norm(document.querySelector('[data-role="format-total"]')?.textContent)
    const availText = norm(document.querySelector('[data-role="format-available"]')?.textContent)
    const chips = Array.from(document.querySelectorAll<HTMLElement>('[data-role="type-options"] [data-type]')).map(
      (e) => ({
        type: e.getAttribute('data-type') as string,
        count: Number((norm(e.textContent).match(/(\d+)$/) || [])[1]),
      }),
    )
    const groups = chips.filter((c) => c.type !== 'all')
    const sum = groups.reduce((n, c) => n + c.count, 0)
    console.log('[头部统计]', totalText, '|', availText, '[类型芯片]', JSON.stringify(chips), '之和', sum)

    expect(chips.find((c) => c.type === 'all')?.count, '「全部」应等于可用总数').toBe(availableIds.length)
    expect(groups.length, '四类芯片都要在').toBe(4)
    expect(sum, '四类芯片计数之和应等于可用总数').toBe(availableIds.length)
    for (const c of groups) expect(c.count, `${c.type} 芯片计数应只算可用项`).toBe(availableByGroup[c.type])
    expect(totalText).toContain(String(availableIds.length))
    expect(availText).toContain(String(availableIds.length))
    // 五个芯片文本里不应再出现「不可用」字样
    expect(norm(document.querySelector('[data-role="type-options"]')?.textContent)).not.toContain('不可用')

    host.remove()
  })

  it('搜索原来不可用的格式名 → 空网格 + 友好空态（不再有置灰卡）', async () => {
    const { host } = await mountConverter()
    const input = document.querySelector<HTMLInputElement>('[data-role="format-search"]')!
    expect(input, '缺少搜索框').toBeTruthy()
    const type = async (q: string) => {
      input.value = q
      input.dispatchEvent(new Event('input', { bubbles: true }))
      for (let i = 0; i < 10; i++) await nextTick()
    }

    for (const q of ['heic', 'ape']) {
      await type(q)
      const empty = norm(document.querySelector('[data-role="grid-empty"]')?.textContent)
      console.log(`[搜索 ${q}] 网格条目`, renderedFormatIds().length, '置灰', greyCount(), '空态', empty)
      expect(renderedFormatIds(), `搜索 ${q} 不应有结果`).toEqual([])
      expect(greyCount(), `搜索 ${q} 不该留下置灰卡`).toBe(0)
      expect(empty.length, `搜索 ${q} 应有友好空态文案`).toBeGreaterThan(0)
    }

    // 反向：可用格式仍搜得到
    await type('psd')
    expect(renderedFormatIds(), '搜索可用格式应有结果').toContain('psd')

    host.remove()
  })

  /* ---------- 删除记录：后端删掉 → store 与 DOM 必须同步少一行（回归：界面停旧行数） ---------- */

  it('删除记录（后端成功）：store 与 DOM 同步少一行，等于后端剩余条数，无需切页/重启', async () => {
    convertsMock = [REAL_TASK, T_ERR_CONV]
    const { host, pinia } = await mountConverter()
    const store = useTaskStore(pinia)
    const before = { store: store.converts.length, dom: convRows(), backend: convertsMock.length }
    console.log('[删除前] store/dom/backend =', JSON.stringify(before))
    expect(before, '三处口径起手就该一致').toEqual({ store: 2, dom: 2, backend: 2 })

    const row = queueRow(T_ERR_CONV.output_file.split('\\').pop())
    expect(row, '缺少待删除的行').toBeTruthy()
    const del = buttonByTitle(row, '删除记录')
    expect(del, '行内缺少「删除记录」按钮').toBeTruthy()
    del!.click()
    await flush()

    const after = { store: store.converts.length, dom: convRows(), backend: convertsMock.length }
    console.log('[删除后] store/dom/backend =', JSON.stringify(after))
    expect(after.backend, '后端（list_converts）应真的少一条').toBe(1)
    expect(after.store, 'store 必须同步少一条（旧版卡在这里：store 一条不少）').toBe(1)
    expect(after.dom, 'DOM 必须同步少一行').toBe(1)
    expect(after.dom, 'DOM 行数必须等于 store 长度').toBe(after.store)
    expect(convertsMock.map((t) => t.id)).toEqual([REAL_TASK.id])

    convertsMock = [REAL_TASK]
    host.remove()
  })

  it('删除记录（后端报错）：一行都不动，并把失败原因摊给用户', async () => {
    convertsMock = [REAL_TASK, T_ERR_CONV]
    removeConvertError = '删除失败：数据库被占用'
    const { host, pinia } = await mountConverter()
    const store = useTaskStore(pinia)

    const row = queueRow(T_ERR_CONV.output_file.split('\\').pop())
    const del = buttonByTitle(row, '删除记录')
    expect(del, '行内缺少「删除记录」按钮').toBeTruthy()
    del!.click()
    await flush()

    expect(store.converts.length, '后端失败：store 一行都不能少').toBe(2)
    expect(convRows(), '后端失败：DOM 一行都不能少').toBe(2)
    expect(norm(document.body.textContent), '应把后端错误原文摊给用户').toContain('删除失败：数据库被占用')

    removeConvertError = null
    convertsMock = [REAL_TASK]
    host.remove()
  })
})
