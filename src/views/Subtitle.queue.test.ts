/**
 * 字幕队列回归（mock 数据驱动）
 *
 * 用户要求：字幕队列改成与转换页完全一致的纯简约行列表 —— 复用 ConvertRow.vue，无缩略图、无预览入口、
 * 无卡片包裹。行内容 = 状态 + 文件名 + 语言/模型（+ 产物大小）+ 进度 + 操作按钮（打开 / 在文件夹中显示 /
 * 删除记录，进行中加取消、失败行加继续）。
 *
 * 数据：手写 mock 的 SubtitleTask（字段与 Rust 端 serde 一致，snake_case），覆盖四种行形态
 * （完成 · 产物在盘上 / 识别中 / 失败 / 完成但产物已丢）。
 * IPC 边界（window.__TAURI_INTERNALS__.invoke）用 mock 应答，视图 Subtitle.vue 与行组件 ConvertRow.vue
 * 全部走真实代码路径。
 */
import { describe, expect, it, beforeAll, vi } from 'vitest'
import { createApp, h, nextTick } from 'vue'
import { createPinia } from 'pinia'
import { NMessageProvider } from 'naive-ui'
import Subtitle from '@/views/Subtitle.vue'
import { i18n } from '@/i18n'
import { useTaskStore } from '@/stores/tasks'
import { basename, formatBytes } from '@/services/utils'
import type { SubtitleTask } from '@/types'

/* ==================== mock 数据（字段与后端 SubtitleTask 一致） ==================== */
const DIR = String.raw`D:\Videos\umi`

/** 完成：产物就在磁盘上 */
const T_DONE: SubtitleTask = {
  id: 'sub-done',
  video_path: `${DIR}\\示例 视频 A.mp4`,
  language: 'zh',
  model: 'base',
  subtitle_path: `${DIR}\\示例 视频 A.zh.srt`,
  status: 'done',
  progress: 100,
  error: null,
  created_time: 1,
}
/** 识别中（活跃态 → 只该有「取消」） */
const T_RUN: SubtitleTask = {
  id: 'sub-run',
  video_path: `${DIR}\\示例 视频 B.mp4`,
  language: 'en',
  model: 'small',
  subtitle_path: null,
  status: 'transcribing',
  progress: 42.5,
  error: null,
  created_time: 2,
}
/** 失败（→ 该有「继续」，并把错误摊在行里） */
const T_ERR: SubtitleTask = {
  id: 'sub-err',
  video_path: `${DIR}\\示例 视频 C.mp4`,
  language: 'auto',
  model: 'medium',
  subtitle_path: null,
  status: 'error',
  progress: 12.5,
  error: 'Whisper 退出码 1：模型文件缺失',
  created_time: 3,
}
/**
 * 完成但产物已不在磁盘上（→ 「文件已丢失」，只留删除记录）
 */
const T_LOST: SubtitleTask = {
  id: 'sub-lost',
  video_path: `${DIR}\\示例 视频 D.mp4`,
  language: 'ja',
  model: 'tiny',
  subtitle_path: `${DIR}\\示例 视频 D.ja.srt`,
  status: 'done',
  progress: 100,
  error: null,
  created_time: 4,
}

/**
 * BUG-19：开着「额外生成一份英文字幕」时的那一对任务 ——
 * 原文 `<视频基名>.detected.srt`、译文 `<视频基名>.detected-to-en.srt`，名字不冲突；
 * 译文任务带 translate_to='en'（后端从 request 列回填，重启后仍在）。
 */
const T_ORIG: SubtitleTask = {
  id: 'sub-orig',
  video_path: `${DIR}\\示例 视频 E.mp4`,
  language: 'auto',
  model: 'tiny',
  subtitle_path: `${DIR}\\示例 视频 E.detected.srt`,
  translate_to: null,
  status: 'done',
  progress: 100,
  error: null,
  created_time: 5,
}
const T_TR: SubtitleTask = {
  id: 'sub-tr',
  video_path: `${DIR}\\示例 视频 E.mp4`,
  language: 'auto',
  model: 'tiny',
  subtitle_path: `${DIR}\\示例 视频 E.detected-to-en.srt`,
  translate_to: 'en',
  status: 'done',
  progress: 100,
  error: null,
  created_time: 6,
}

/**
 * BUG-20：1.8.6 之前的译文任务记录 —— `subtitle_path` 记的是**原文**那份（英文产物没登记进来）。
 * 用户报的正是这个：译文行点「在文件夹中显示」定位到原文文件，英文产物「不在文件夹里显示」。
 */
const T_STALE_TR: SubtitleTask = {
  id: 'sub-stale-tr',
  video_path: `${DIR}\\示例 视频 F.mp4`,
  language: 'auto',
  model: 'tiny',
  subtitle_path: `${DIR}\\示例 视频 F.detected.srt`,
  translate_to: 'en',
  status: 'done',
  progress: 100,
  error: null,
  created_time: 7,
}

const ALL_TASKS: SubtitleTask[] = [T_LOST, T_ERR, T_RUN, T_DONE]

/** 字幕产物真实大小（mock：只有下面这些产物存在） */
const DONE_SRT_SIZE = 1234
const ORIG_SRT_SIZE = 2774
const TR_SRT_SIZE = 2509
/** 老译文记录里指向的那份原文产物（真实大小取自用户机器上的原文字幕） */
const STALE_ORIG_SIZE = 1020
const EXISTING_PRODUCTS = new Map<string, number>([
  [T_DONE.subtitle_path as string, DONE_SRT_SIZE],
  [T_ORIG.subtitle_path as string, ORIG_SRT_SIZE],
  [T_TR.subtitle_path as string, TR_SRT_SIZE],
  // 老译文记录里的原文文件**确实在盘上** —— 陷阱就在这：记录能定位到，但它不是这一行的产物
  [T_STALE_TR.subtitle_path as string, STALE_ORIG_SIZE],
])

/** 当前 list_subtitles 应答（空态用例会改成 []） */
let subtitlesMock: SubtitleTask[] = [...ALL_TASKS]

/** 非 null 时 remove_subtitle 直接失败（用来验「后端报错 → 一行都不动」） */
let removeSubtitleError: string | null = null

/** 「打开文件」按钮点到的路径（验每一行打开的都是自己那份产物） */
const openedPaths: string[] = []

/** 「在文件夹中显示」按钮点到的路径（验每一行定位的都是自己那份产物） */
const revealedPaths: string[] = []

/** 非 null 时 reveal_path 直接失败（验「定位失败 → 明确提示」，而不是假装成功） */
let revealPathError: string | null = null

/** 行的展示名：产物文件名 → 源视频名（与视图同一规则，用于定位行） */
function rowTitle(t: SubtitleTask): string {
  return basename(t.subtitle_path || '') || basename(t.video_path)
}

/* ==================== IPC / 浏览器 API 打桩 ==================== */
function fileSizes(paths: string[]) {
  return paths.map((p) => {
    const size = EXISTING_PRODUCTS.get(p)
    if (size !== undefined) return { path: p, size, exists: true, modified: 1700000000000 }
    return { path: p, size: null, exists: false, modified: null }
  })
}

function installInvokeStub() {
  let cbId = 0
  const handlers: Record<string, (args: any) => unknown> = {
    list_downloads: () => [],
    list_converts: () => [],
    // 真 IPC 会序列化一份新数组给前端；这里必须返回拷贝，否则 mock 的删除会直接改到 store 里那个数组
    list_subtitles: () => subtitlesMock.map((t) => ({ ...t })),
    detect_tools: () => [],
    list_tools: () => [],
    // 字幕页会拉一次「已下载的模型」来决定默认选哪个（BUG-15 回落提示）
    list_whisper_models: () => [],
    backfill_covers: () => 0,
    file_sizes: (args: any) => fileSizes(args?.paths ?? []),
    get_settings: () => ({ download_dir: 'C:\\Users\\user\\Downloads\\Umidl', whisper_model: 'base' }),
    default_download_dir: () => 'C:\\Users\\user\\Downloads\\Umidl',
    app_data_dir: () => 'C:\\Users\\user\\AppData\\Roaming\\umi-downloader',
    cancel_subtitle: () => null,
    // 真删：remove_subtitle 成功后 mock 列表也少一条（视图 / store / 后端三方口径一致）
    remove_subtitle: (args: any) => {
      if (removeSubtitleError) throw new Error(removeSubtitleError)
      const i = subtitlesMock.findIndex((t) => t.id === args?.id)
      if (i >= 0) subtitlesMock.splice(i, 1)
      return null
    },
    start_subtitle: () => ({ ...T_RUN }),
    open_path: (args: any) => {
      openedPaths.push(args?.path ?? '')
      return null
    },
    reveal_path: (args: any) => {
      if (revealPathError) throw new Error(revealPathError)
      revealedPaths.push(args?.path ?? '')
      return null
    },
    read_text_file: () => '',
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

/* ==================== 查询辅助 ==================== */
/** 队列容器（简约行列表） */
function queueBox(): HTMLElement | null {
  return document.querySelector<HTMLElement>('[data-role="subtitle-queue"]')
}

/** 队列里的所有行 */
function rows(): HTMLElement[] {
  return Array.from(queueBox()?.querySelectorAll<HTMLElement>('[data-role="subtitle-row"]') ?? [])
}

/** 行文本：包含目标文件名的那个 subtitle-row */
function queueRow(name: string): HTMLElement | undefined {
  return rows().find((n) => (n.textContent || '').includes(name))
}

const norm = (s: string | undefined) => (s || '').replace(/\s+/g, ' ').trim()

/** 行上的按钮（title 属性 / 文本） */
function rowButtons(row: HTMLElement | undefined): string[] {
  return Array.from(row?.querySelectorAll('button') ?? []).map((b) => b.getAttribute('title') || norm(b.textContent))
}

async function mountSubtitle() {
  const host = document.createElement('div')
  document.body.appendChild(host)
  const pinia = createPinia()
  const app = createApp({
    setup: () => () => h(NMessageProvider, null, { default: () => h(Subtitle) }),
  })
  app.use(pinia)
  app.use(i18n)
  app.mount(host)
  for (let i = 0; i < 12; i++) await nextTick()
  await new Promise((r) => setTimeout(r, 120))
  for (let i = 0; i < 6; i++) await nextTick()
  return { app, host, pinia }
}

/** 等一拍：删除是 async（await 后端命令 → 改 store → 重渲染） */
async function flush() {
  for (let i = 0; i < 10; i++) await nextTick()
  await new Promise((r) => setTimeout(r, 30))
  for (let i = 0; i < 6; i++) await nextTick()
}

/** 行内某个 title 的按钮（找不到就返回 undefined，让断言报错而不是崩） */
function buttonByTitle(row: HTMLElement | undefined, title: string): HTMLButtonElement | undefined {
  return Array.from(row?.querySelectorAll<HTMLButtonElement>('button') ?? []).find(
    (b) => b.getAttribute('title') === title,
  )
}

describe('字幕队列行列表（mock 数据）', () => {
  beforeAll(() => {
    stubBrowserApis()
    installInvokeStub()
  })

  it('队列是简约行列表：行数=记录数，每行有文件名与操作按钮，无缩略图/无预览/无卡片', async () => {
    subtitlesMock = [...ALL_TASKS]
    const { host } = await mountSubtitle()
    const box = queueBox()
    const list = rows()
    console.log('[队列] 行数', list.length, list.map((r) => norm(r.textContent)).join(' || '))
    for (const r of list) console.log('[行按钮]', norm((r.textContent || '').slice(0, 40)), JSON.stringify(rowButtons(r)))

    // 容器存在、自己内滚（记录再多也不把页面撑高）
    expect(box, '缺少 [data-role="subtitle-queue"] 容器').toBeTruthy()
    expect((box?.className || '').includes('overflow-y-auto'), '队列容器应自己内滚').toBe(true)
    expect(box?.className || '', '容器应与转换页同款外观').toContain('max-h-[42vh]')
    // 行数 = 记录数，且不再是转换页的角色名 / 卡片包裹
    expect(list.length, '队列行数应等于记录数').toBe(ALL_TASKS.length)
    expect(box?.querySelectorAll('[data-role="convert-row"]').length, '字幕队列不该混入 convert-row').toBe(0)
    expect(box?.querySelectorAll('[data-role="subtitle-row"]').length).toBe(ALL_TASKS.length)

    // 整条队列：无缩略图、无预览入口、无卡片、无预览面板
    expect(box?.querySelectorAll('img').length, '队列不该有缩略图').toBe(0)
    expect(box?.querySelectorAll('pre').length, '字幕预览面板应已移除').toBe(0)
    expect(norm(box?.textContent), '队列里不该再出现「预览」字样').not.toContain('预览')
    for (const r of list) {
      expect((r.className || '').includes('umi-card'), '队列行不该再是卡片').toBe(false)
      expect(r.querySelectorAll('img').length, '行内不该有缩略图').toBe(0)
      expect(norm(r.textContent), '行内不该出现「预览」字样').not.toContain('预览')
    }

    // 每行都有文件名 + 至少一个操作按钮
    for (const t of ALL_TASKS) {
      const row = queueRow(rowTitle(t))
      expect(row, `缺少 ${rowTitle(t)} 的行`).toBeTruthy()
      expect(rowButtons(row).length, `${rowTitle(t)} 的行应有操作按钮`).toBeGreaterThan(0)
      expect(row?.getAttribute('data-status')).toBe(t.status)
    }

    host.remove()
  })

  it('完成行（产物在盘上）：语言·模型·产物大小齐全，只留 打开/显示/删除', async () => {
    subtitlesMock = [...ALL_TASKS]
    const { host } = await mountSubtitle()
    const row = queueRow(rowTitle(T_DONE))
    const text = norm(row?.textContent)
    const buttons = rowButtons(row)

    expect(text, '缺少字幕文件名').toContain(rowTitle(T_DONE))
    expect(text, '缺少语言').toContain('中文')
    expect(text, '缺少模型').toContain('base')
    expect(text, '缺少状态').toContain('已完成')
    expect(text, '缺少产物大小').toContain(formatBytes(DONE_SRT_SIZE))
    expect(text, '产物存在却提示文件已丢失').not.toContain('文件已丢失')
    expect(text, '缺少进度百分比').toContain('100.0%')
    expect(buttons, '缺少「打开文件」').toContain('打开文件')
    expect(buttons, '缺少「在文件夹中显示」').toContain('在文件夹中显示')
    expect(buttons, '缺少「删除记录」').toContain('删除记录')
    expect(buttons, '完成行不该有取消 / 继续').toEqual(['打开文件', '在文件夹中显示', '删除记录'])

    host.remove()
  })

  it('识别中 / 失败行：进度与错误就地显示，取消 / 继续按钮齐全且没有打开入口', async () => {
    subtitlesMock = [...ALL_TASKS]
    const { host } = await mountSubtitle()

    const run = queueRow(rowTitle(T_RUN))
    const runText = norm(run?.textContent)
    const runButtons = rowButtons(run)
    expect(runText, '缺少源视频文件名').toContain(rowTitle(T_RUN))
    expect(runText, '缺少语言').toContain('English')
    expect(runText, '缺少模型').toContain('small')
    expect(runText, '缺少状态').toContain('AI 识别中')
    expect(runText, '缺少进度百分比').toContain('42.5%')
    expect(runButtons, '识别中应有「取消」').toContain('取消')
    expect(runButtons, '未完成不该有打开 / 显示').toEqual(['取消', '删除记录'])

    const err = queueRow(rowTitle(T_ERR))
    const errText = norm(err?.textContent)
    const errButtons = rowButtons(err)
    expect(errText, '缺少状态').toContain('失败')
    expect(errText, '错误应就地摊在行里').toContain(T_ERR.error as string)
    expect(errButtons, '失败行应有「继续」').toContain('继续')
    expect(errButtons, '失败行不该有打开 / 显示').toEqual(['继续', '删除记录'])

    host.remove()
  })

  it('完成行（产物已丢失）：标「文件已丢失」只留删除记录', async () => {
    subtitlesMock = [...ALL_TASKS]
    const { host } = await mountSubtitle()
    const row = queueRow(rowTitle(T_LOST))
    const text = norm(row?.textContent)
    console.log('[丢失行]', text, JSON.stringify(rowButtons(row)))

    expect(text, '缺少字幕文件名').toContain(rowTitle(T_LOST))
    expect(text, '缺少语言').toContain('日本語')
    expect(text, '产物已丢应标「文件已丢失」').toContain('文件已丢失')
    expect(text, '产物已丢不该再显示大小').not.toContain(formatBytes(DONE_SRT_SIZE))
    expect(rowButtons(row), '产物已丢只留删除记录').toEqual(['删除记录'])

    host.remove()
  })

  it('没有字幕任务：不渲染队列容器，只有空态', async () => {
    subtitlesMock = []
    const { host } = await mountSubtitle()

    expect(rows().length, '空队列不该有行').toBe(0)
    expect(queueBox(), '空队列不该渲染容器').toBeNull()
    expect(norm(document.body.textContent), '应有空态文案').toContain('还没有字幕任务')

    host.remove()
  })

  /* ---------- BUG-19：原文 + 英文字幕两份产物 ---------- */

  it('同时生成原文与英文字幕：两份产物各占一行、文件名不冲突、各自能打开', async () => {
    subtitlesMock = [T_TR, T_ORIG]
    openedPaths.length = 0
    revealedPaths.length = 0
    const { host } = await mountSubtitle()
    const list = rows()
    console.log('[两份产物]', list.map((r) => norm(r.textContent)).join(' || '))

    expect(list.length, '原文任务与译文任务应各占一行').toBe(2)
    const ro = queueRow(rowTitle(T_ORIG))
    const rt = queueRow(rowTitle(T_TR))
    expect(ro, '缺少原文产物的行').toBeTruthy()
    expect(rt, '缺少英文译文产物的行').toBeTruthy()
    expect(ro, '两行不能是同一行（产物名撞了）').not.toBe(rt)

    // 文件名必须不同（回归：译文此前沿用原文基名 → 两行都指向同一个文件）
    expect(rowTitle(T_ORIG)).not.toBe(rowTitle(T_TR))
    expect(norm(ro?.textContent), '原文行缺少自己的产物名').toContain('示例 视频 E.detected.srt')
    expect(norm(ro?.textContent), '原文行不该出现译文产物名').not.toContain('-to-en')
    expect(norm(rt?.textContent), '译文行缺少自己的产物名').toContain('示例 视频 E.detected-to-en.srt')

    // 译文行标「→ 英文」（来自后端 translate_to，重启后也还在）；原文行不标
    expect(norm(rt?.textContent), '译文行缺少「→ 英文」标记').toContain('→ 英文')
    expect(norm(ro?.textContent), '原文行不该被标成译文').not.toContain('→ 英文')

    // 两份产物都在盘上 → 都显示各自大小、都不是「文件已丢失」、都能打开
    expect(norm(ro?.textContent), '原文行缺少产物大小').toContain(formatBytes(ORIG_SRT_SIZE))
    expect(norm(rt?.textContent), '译文行缺少产物大小').toContain(formatBytes(TR_SRT_SIZE))
    expect(norm(ro?.textContent)).not.toContain('文件已丢失')
    expect(norm(rt?.textContent)).not.toContain('文件已丢失')
    expect(rowButtons(ro)).toEqual(['打开文件', '在文件夹中显示', '删除记录'])
    expect(rowButtons(rt)).toEqual(['打开文件', '在文件夹中显示', '删除记录'])

    // 点「打开文件」：每一行打开的都是自己那份产物（译文行不能打开原文文件）
    buttonByTitle(rt, '打开文件')!.click()
    await flush()
    expect(openedPaths[openedPaths.length - 1], '译文行打开的必须是 -to-en 那一份').toBe(T_TR.subtitle_path)
    buttonByTitle(ro, '打开文件')!.click()
    await flush()
    expect(openedPaths[openedPaths.length - 1], '原文行打开的必须是原文那一份').toBe(T_ORIG.subtitle_path)

    // 点「在文件夹中显示」：每一行定位的也必须是自己的产物（回归：译文行曾定位到原文文件）
    buttonByTitle(rt, '在文件夹中显示')!.click()
    await flush()
    expect(revealedPaths[revealedPaths.length - 1], '译文行定位的必须是 -to-en 那一份').toBe(T_TR.subtitle_path)
    buttonByTitle(ro, '在文件夹中显示')!.click()
    await flush()
    expect(revealedPaths[revealedPaths.length - 1], '原文行定位的必须是原文那一份').toBe(T_ORIG.subtitle_path)

    host.remove()
  })

  /* ---------- BUG-20：老译文记录把原文文件记在自己身上 ---------- */

  it('老记录：译文行记的是原文文件 → 行名纠正成 -to-en，不定位原文、不假装成功', async () => {
    subtitlesMock = [T_STALE_TR]
    openedPaths.length = 0
    revealedPaths.length = 0
    const { host } = await mountSubtitle()
    const row = queueRow('示例 视频 F.detected-to-en.srt')
    const text = norm(row?.textContent)
    console.log('[老译文记录]', text, JSON.stringify(rowButtons(row)))

    // 行名按命名规则纠正成自己那份（老记录的原文文件名不能再挂在译文行上）
    expect(row, '译文行的展示名必须是自己那份 -to-en 产物').toBeTruthy()
    expect(text, '不能把原文那份当成自己的产物').not.toContain('示例 视频 F.detected.srt')
    expect(text, '译文标记还得在').toContain('→ 英文')
    // 记录里的原文文件在盘上，但它不是这一行的产物 → 英文产物不在盘上就如实标「文件已丢失」，
    // 并且不给「打开 / 显示」按钮（旧版的按钮会去打开 / 定位原文文件）
    expect(text, '英文产物不在盘上应标「文件已丢失」').toContain('文件已丢失')
    expect(text, '不该再显示原文那份的大小').not.toContain(formatBytes(STALE_ORIG_SIZE))
    expect(rowButtons(row), '产物不在盘上：只留删除记录').toEqual(['删除记录'])
    expect(openedPaths, '不该去打开原文文件').toEqual([])
    expect(revealedPaths, '不该去定位原文文件').toEqual([])

    host.remove()
  })

  it('产物在点按钮前被移走：后端报错 → 明确提示，不假装成功', async () => {
    subtitlesMock = [T_DONE]
    openedPaths.length = 0
    revealedPaths.length = 0
    revealPathError = String.raw`文件已不存在（可能被移动或删除）：D:\Videos\umi\示例 视频 A.zh.srt`
    const { host } = await mountSubtitle()

    const row = queueRow(rowTitle(T_DONE))
    const revealBtn = buttonByTitle(row, '在文件夹中显示')
    expect(revealBtn, '行内缺少「在文件夹中显示」按钮').toBeTruthy()
    revealBtn!.click()
    await flush()

    expect(revealedPaths, '后端已失败：不该记成一次成功定位').toEqual([])
    expect(norm(document.body.textContent), '应把失败原因（文件已不存在）摊给用户').toContain('文件已不存在')

    revealPathError = null
    host.remove()
  })

  /* ---------- 删除记录：后端删掉 → store 与 DOM 必须同步少一行（回归：界面停旧行数） ---------- */

  it('删除记录（后端成功）：store 与 DOM 同步少一行，等于后端剩余条数，无需切页/重启', async () => {
    subtitlesMock = [...ALL_TASKS]
    const { host, pinia } = await mountSubtitle()
    const store = useTaskStore(pinia)
    const before = { store: store.subtitles.length, dom: rows().length, backend: subtitlesMock.length }
    console.log('[删除前] store/dom/backend =', JSON.stringify(before))
    expect(before, '三处口径起手就该一致').toEqual({ store: 4, dom: 4, backend: 4 })

    const row = queueRow(rowTitle(T_DONE))
    expect(row, '缺少待删除的行').toBeTruthy()
    const del = buttonByTitle(row, '删除记录')
    expect(del, '行内缺少「删除记录」按钮').toBeTruthy()
    del!.click()
    await flush()

    const after = { store: store.subtitles.length, dom: rows().length, backend: subtitlesMock.length }
    console.log('[删除后] store/dom/backend =', JSON.stringify(after))
    expect(after.backend, '后端（list_subtitles）应真的少一条').toBe(3)
    expect(after.store, 'store 必须同步少一条（旧版卡在这里：store 一条不少）').toBe(3)
    expect(after.dom, 'DOM 必须同步少一行').toBe(3)
    expect(after.dom, 'DOM 行数必须等于 store 长度').toBe(after.store)
    expect(queueRow(rowTitle(T_DONE)), '被删掉的行不该还在 DOM 里').toBeUndefined()

    host.remove()
  })

  it('删除记录（后端报错）：一行都不动，并把失败原因摊给用户', async () => {
    subtitlesMock = [...ALL_TASKS]
    removeSubtitleError = '删除失败：数据库被占用'
    const { host, pinia } = await mountSubtitle()
    const store = useTaskStore(pinia)

    const row = queueRow(rowTitle(T_ERR))
    const del = buttonByTitle(row, '删除记录')
    expect(del, '行内缺少「删除记录」按钮').toBeTruthy()
    del!.click()
    await flush()

    expect(store.subtitles.length, '后端失败：store 一行都不能少').toBe(4)
    expect(rows().length, '后端失败：DOM 一行都不能少').toBe(4)
    expect(queueRow(rowTitle(T_ERR)), '失败的那行必须还在').toBeTruthy()
    expect(norm(document.body.textContent), '应把后端错误原文摊给用户').toContain('删除失败：数据库被占用')

    removeSubtitleError = null
    host.remove()
  })
})
