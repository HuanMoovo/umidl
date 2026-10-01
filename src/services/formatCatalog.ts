/**
 * 转换格式目录（convert_formats）的薄封装 + 浏览器预览用内置兜底目录。
 *
 * 后端命令返回结构（已固定）：
 *   { total, groups: [{ id, label, count, formats: [{ id, label, available, input, engines: [], note }] }],
 *     engines: { ffmpeg, pandoc, poppler } }
 * 分组 id 固定为 video / audio / image / document，label 分别是 视频/音频/图片/文档。
 *
 * 非 Tauri（浏览器预览）取不到后端能力表时用 FALLBACK_CATALOG 渲染，
 * 保证格式网格在预览环境下依然是完整的（≥60 条，四类各 ≥10）。
 *
 * 界面口径（1.8 起）：转换页只呈现 `available === true` 的格式——
 * 用 availableOnly() 过滤后再渲染计数，置灰条目与「本机引擎不支持」提示全部取消。
 */
import { invoke } from '@tauri-apps/api/core'
import { isTauri } from './ipc'

export interface ConvertFormatItem {
  /** 目标格式标识（小写扩展名），直接作为 start_convert 的 format 入参 */
  id: string
  /** 展示名（通常是大写扩展名，后端给什么显示什么） */
  label: string
  /** 本机引擎是否支持（false = 置灰不可选） */
  available: boolean
  /** 能否作为输入格式 */
  input: boolean
  /** 需要的引擎（ffmpeg / pandoc / poppler …） */
  engines: string[]
  /** 备注（编码 / 用途说明，也可作为不可用时的悬停原因） */
  note?: string | null
}

export interface ConvertFormatGroup {
  id: string
  label: string
  count: number
  formats: ConvertFormatItem[]
}

export interface ConvertFormatCatalog {
  total: number
  groups: ConvertFormatGroup[]
  engines: Record<string, boolean>
}

/** 分组固定顺序（后端 label 不参与排序，渲染时用 i18n 文案） */
export const GROUP_ORDER = ['video', 'audio', 'image', 'document'] as const
export type FormatGroupId = (typeof GROUP_ORDER)[number]

/** 分组 id → i18n key（中文原文即 key，四个语言包都有对应译文） */
export const GROUP_I18N_KEY: Record<string, string> = {
  video: '视频',
  audio: '音频',
  image: '图片',
  document: '文档',
}

/** 引擎展示顺序 */
export const ENGINE_ORDER = ['ffmpeg', 'pandoc', 'poppler'] as const

/* ============================================================
   内置兜底目录：浏览器预览（非 Tauri）下渲染
   共 74 条 —— 视频 20 / 音频 16 / 图片 16 / 文档 22
   预览环境探测不到外部引擎（pandoc / poppler），因此这里不做任何可用性判定：
   全部按「可选」渲染，与真机目录一样不再出现置灰条目。
   ============================================================ */
export const FALLBACK_ENGINES: Record<string, boolean> = { ffmpeg: true, pandoc: false, poppler: false }

const V = (id: string, note: string, input = true): ConvertFormatItem => ({
  id,
  label: id.toUpperCase(),
  available: true,
  input,
  engines: ['ffmpeg'],
  note,
})

const D = (id: string, note: string, engines: string[] = ['pandoc'], input = true): ConvertFormatItem => ({
  id,
  label: id.toUpperCase(),
  available: true, // 预览环境探测不到外部引擎 → 不做可用性判定（全部可选）
  input,
  engines,
  note,
})

export const FALLBACK_CATALOG: ConvertFormatCatalog = {
  total: 74,
  engines: FALLBACK_ENGINES,
  groups: [
    {
      id: 'video',
      label: '视频',
      count: 20,
      formats: [
        V('mp4', 'H.264 / AAC'),
        V('mkv', 'Matroska container'),
        V('mov', 'QuickTime'),
        V('webm', 'VP9 / Opus'),
        V('avi', 'Legacy container'),
        V('flv', 'Flash Video'),
        V('ts', 'MPEG transport stream'),
        V('m4v', 'Apple video'),
        V('mpg', 'MPEG-1 / MPEG-2 PS'),
        V('wmv', 'Windows Media'),
        V('ogv', 'Ogg Theora'),
        V('gif', 'Animated image / no audio'),
        V('m2ts', 'Blu-ray transport stream'),
        V('mxf', 'Broadcast container', false),
        V('3gp', 'Mobile 3GPP'),
        V('vob', 'DVD video'),
        V('asf', 'Advanced Systems Format'),
        V('f4v', 'Flash MP4'),
        V('dv', 'DV stream', false),
        V('y4m', 'Raw YUV4MPEG', false),
      ],
    },
    {
      id: 'audio',
      label: '音频',
      count: 16,
      formats: [
        V('mp3', 'MPEG-1 Layer III'),
        V('m4a', 'AAC in MP4'),
        V('aac', 'Advanced Audio Coding'),
        V('flac', 'Lossless'),
        V('wav', 'PCM'),
        V('opus', 'Opus'),
        V('ogg', 'Vorbis'),
        V('wma', 'Windows Media Audio'),
        V('ac3', 'Dolby Digital'),
        V('aiff', 'Apple PCM'),
        V('alac', 'Apple lossless'),
        V('amr', 'Adaptive multi-rate'),
        V('mka', 'Matroska audio'),
        V('ape', 'Monkey\u2019s Audio'),
        V('dts', 'DTS'),
        V('m4b', 'Audiobook'),
      ],
    },
    {
      id: 'image',
      label: '图片',
      count: 16,
      formats: [
        V('png', 'Lossless / alpha'),
        V('jpg', 'Lossy photo'),
        V('jpeg', 'Lossy photo (alias)'),
        V('webp', 'Small / alpha'),
        V('bmp', 'Bitmap'),
        V('tiff', 'Tagged image'),
        V('ico', 'Icon / favicon'),
        V('avif', 'AV1 image'),
        V('heic', 'HEVC image'),
        V('tga', 'Targa'),
        V('ppm', 'Portable pixmap'),
        V('pcx', 'PCX'),
        V('dds', 'DirectDraw surface', false),
        V('exr', 'OpenEXR', false),
        V('svg', 'Vector', false),
        V('psd', 'Photoshop document', false),
      ],
    },
    {
      id: 'document',
      label: '文档',
      count: 22,
      formats: [
        D('pdf', 'Portable Document Format', ['poppler', 'pandoc']),
        D('docx', 'Office Open XML'),
        D('doc', 'Word 97-2003'),
        D('odt', 'OpenDocument text'),
        D('rtf', 'Rich Text Format'),
        D('txt', 'Plain text'),
        D('md', 'Markdown'),
        D('html', 'HTML'),
        D('epub', 'EPUB e-book'),
        D('mobi', 'Kindle (legacy)', ['pandoc'], false),
        D('azw3', 'Kindle KF8', ['pandoc'], false),
        D('fb2', 'FictionBook', ['pandoc'], false),
        D('xlsx', 'Excel workbook'),
        D('xls', 'Excel 97-2003'),
        D('ods', 'OpenDocument sheet'),
        D('csv', 'Comma separated'),
        D('tsv', 'Tab separated'),
        D('pptx', 'PowerPoint deck'),
        D('ppt', 'PowerPoint 97-2003'),
        D('odp', 'OpenDocument slides'),
        D('tex', 'LaTeX source'),
        D('xml', 'XML'),
      ],
    },
  ],
}

/* ------------------------- 调用与归一化 ------------------------- */

function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) throw new Error(`[browser] 命令 ${cmd} 需要在桌面客户端中运行`)
  return invoke<T>(cmd, args)
}

/** 按固定顺序排序分组、补齐 count/total 与缺失字段（旧后端 / 部分字段也照常渲染） */
export function normalizeCatalog(raw: unknown): ConvertFormatCatalog {
  const r = (raw ?? {}) as Partial<ConvertFormatCatalog>
  const groups: ConvertFormatGroup[] = Array.isArray(r.groups) ? r.groups.map((g) => {
    const formats: ConvertFormatItem[] = Array.isArray(g?.formats)
      ? g.formats
          .filter((f) => f && f.id)
          .map((f) => ({
            id: String(f.id),
            label: String(f.label ?? f.id).toUpperCase(),
            available: f.available !== false,
            input: f.input !== false,
            engines: Array.isArray(f.engines) ? f.engines.map(String) : [],
            note: f.note ?? null,
          }))
      : []
    return {
      id: String(g?.id ?? ''),
      label: String(g?.label ?? g?.id ?? ''),
      count: Number.isFinite(Number(g?.count)) ? Number(g?.count) : formats.length,
      formats,
    }
  }) : []

  const rank = (id: string) => {
    const i = (GROUP_ORDER as readonly string[]).indexOf(id)
    return i < 0 ? GROUP_ORDER.length : i
  }
  groups.sort((a, b) => rank(a.id) - rank(b.id))

  const engines: Record<string, boolean> = {}
  for (const name of ENGINE_ORDER) engines[name] = !!((r.engines ?? {}) as Record<string, boolean>)[name]
  for (const [k, v] of Object.entries(r.engines ?? {})) if (!(k in engines)) engines[k] = !!v

  const sum = groups.reduce((n, g) => n + g.formats.length, 0)
  return {
    total: Number.isFinite(Number(r.total)) && Number(r.total) > 0 ? Number(r.total) : sum,
    groups,
    engines,
  }
}

/**
 * 只保留本机真能写出的格式：`available === false` 的条目整条剔除。
 *
 * 转换页只呈现「能用的格式」——不可用项不渲染、不可选、不参与搜索，
 * 因此 total / count 也一并按可用项重算，保证头部统计与类型芯片口径一致。
 * 可用性字段本身仍保留在数据结构里（内部判定 / 测试断言用），只是不再进入界面。
 */
export function availableOnly(catalog: ConvertFormatCatalog): ConvertFormatCatalog {
  const groups: ConvertFormatGroup[] = catalog.groups.map((g) => {
    const formats = g.formats.filter((f) => f.available)
    return { ...g, formats, count: formats.length }
  })
  return {
    ...catalog,
    total: groups.reduce((n, g) => n + g.formats.length, 0),
    groups,
  }
}

/** 后端格式目录（非 Tauri 会抛错，调用方用 isTauri() 守卫降级到内置目录） */
export const convertFormats = () => call<ConvertFormatCatalog>('convert_formats')

/** 取目录：Tauri 走后端命令，浏览器预览直接用内置表 */
export async function loadFormatCatalog(): Promise<{ catalog: ConvertFormatCatalog; live: boolean }> {
  if (!isTauri()) return { catalog: FALLBACK_CATALOG, live: false }
  try {
    const raw = await convertFormats()
    const catalog = normalizeCatalog(raw)
    if (!catalog.groups.length) return { catalog: FALLBACK_CATALOG, live: false }
    return { catalog, live: true }
  } catch {
    return { catalog: FALLBACK_CATALOG, live: false }
  }
}

/** 扁平化 + 归属分组（用于渲染网格与判定当前选中的目标类型） */
export interface FlatFormat extends ConvertFormatItem {
  group: string
  key: string
}

export function flatten(groups: ConvertFormatGroup[]): FlatFormat[] {
  const out: FlatFormat[] = []
  for (const g of groups) for (const f of g.formats) out.push({ ...f, group: g.id, key: `${g.id}:${f.id}` })
  return out
}

/** 格式 id → 所属分组 id（当前选中的目标是视频/音频/图片/文档） */
export function groupOf(groups: ConvertFormatGroup[], formatId: string): string {
  const id = String(formatId || '').toLowerCase()
  for (const g of groups) if (g.formats.some((f) => f.id.toLowerCase() === id)) return g.id
  return ''
}
