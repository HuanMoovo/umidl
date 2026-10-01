/**
 * 批量导入（1.6）：多链接 / txt 文本一次性入队。
 *
 * 为什么单独一个模块：`src/services/ipc.ts` 由另一条线维护（本任务禁改），
 * 这里自带一个等价的 `call()` 封装 `enqueue_links` 与 `read_text_file`。
 *
 * 关键约定：链接是否合法、是否命中过滤规则（扩展名 / 域名黑白名单 / 最小体积）、
 * 是否重复，全部由后端判定；前端只做「识别 / 去重计数」的展示，
 * 绝不自行丢弃任何链接（被拦截的链接也要如实展示后端给出的统计）。
 */
import { invoke } from '@tauri-apps/api/core'
import { isTauri } from './ipc'
import type { DownloadTask } from '@/types'

/** 后端 `enqueue_links` 的返回结构（见 src-tauri/src/lib.rs） */
export interface EnqueueResult {
  /** 成功入队条数 */
  added: number
  /** 命中过滤规则被拦截条数 */
  blocked: number
  /** 重复 / 已在下载中 / 入队失败而跳过条数 */
  skipped: number
  /** 本次新建的任务对象（与下载队列同构） */
  tasks: DownloadTask[]
  /** 被拦截的链接及原因（后端原文） */
  blocked_reasons?: string[]
  /** 入队失败的链接及错误（后端原文） */
  errors?: string[]
}

/** 单次最多处理条数（与后端 enqueue_links 的 200 上限一致） */
export const ENQUEUE_LIMIT = 200

/**
 * 粘贴 / 导入文本的长度上限（字符数，≈ 512 KB）。
 *
 * 为什么必须有：粘贴路径没有长度保护时，100 万行（36.9 MB）会在主线程同步解析，
 * 冻结窗口 10 秒（BUG-07）。上限之外的内容直接丢弃并给用户可见提示，
 * 真正能入队的上限本来也只有 200 条。
 */
export const BATCH_TEXT_MAX_CHARS = 512 * 1024

/** 分块解析：单次步进最多处理的行数（保证单次主线程耗时在毫秒级） */
export const PARSE_CHUNK_LINES = 2000

export interface ParsedLinks {
  /** 非空、非注释的原始条目数（含重复） */
  total: number
  /** 去重后的链接（保持出现顺序） */
  unique: string[]
  /** 超出 ENQUEUE_LIMIT 的条数（不会入队） */
  overflow: number
}

export interface TruncateResult {
  /** 截断后的文本（未超限时原样返回） */
  text: string
  /** 是否发生了截断 */
  truncated: boolean
  /** 被丢弃的字符数 */
  removedChars: number
}

/**
 * 文本长度上限截断（纯函数）：超过 `limit` 时尽量在**行边界**截断，
 * 避免把最后一条链接切成半截；找不到换行就按字符数硬截。
 */
export function truncateBatchText(text: string, limit: number = BATCH_TEXT_MAX_CHARS): TruncateResult {
  const s = String(text ?? '')
  if (s.length <= limit) return { text: s, truncated: false, removedChars: 0 }
  let cut = s.lastIndexOf('\n', limit)
  if (cut <= 0) cut = limit
  return { text: s.slice(0, cut), truncated: true, removedChars: s.length - cut }
}

/** 分块解析游标：可在多次调用之间保留进度（主线程每次只推进一块） */
export interface ParseCursor {
  /** 已扫描到的字符偏移（下一次从这里继续） */
  offset: number
  /** 已识别的条目数（含重复） */
  total: number
  /** 去重后的链接（保持出现顺序） */
  unique: string[]
  /** 已见过的链接（去重用） */
  seen: Set<string>
  /** 是否已扫描完整个文本 */
  done: boolean
}

export function createParseCursor(): ParseCursor {
  return { offset: 0, total: 0, unique: [], seen: new Set<string>(), done: false }
}

/**
 * 分块解析：对文本做**一次**步进（最多 `maxLines` 行），进度写在 `cursor` 里。
 *
 * 这是 BUG-07 的关键：100 万行的输入不会在任何一次调用里被一次性算完，
 * 调用方（组件 / 测试）按块推进即可让主线程保持可响应。
 */
export function parseChunk(
  cursor: ParseCursor,
  text: string,
  maxLines: number = PARSE_CHUNK_LINES,
): ParseCursor {
  const s = String(text ?? '')
  let lines = 0
  while (cursor.offset < s.length && lines < maxLines) {
    let nl = s.indexOf('\n', cursor.offset)
    if (nl < 0) nl = s.length
    let line = s.slice(cursor.offset, nl)
    if (line.endsWith('\r')) line = line.slice(0, -1)
    cursor.offset = nl + 1
    lines += 1
    const trimmed = line.trim()
    if (!trimmed) continue
    if (trimmed.startsWith('#') || trimmed.startsWith('//')) continue
    for (const tok of trimmed.split(/[\t ,，]+/)) {
      const v = tok.trim()
      if (!v) continue
      cursor.total += 1
      if (!cursor.seen.has(v)) {
        cursor.seen.add(v)
        cursor.unique.push(v)
      }
    }
  }
  cursor.done = cursor.offset >= s.length
  return cursor
}

/**
 * 链接识别（仅用于界面计数，真正解析与过滤由后端完成）：
 *  - 空行忽略；
 *  - `#` / `//` 开头的整行按注释忽略（含注释文字，不折算成链接）；
 *  - 其余行按 换行 / 制表符 / 空格 / 半角与全角逗号 切分，允许一行写多条；
 *  - 保留出现顺序去重。
 *
 * 与后端的差异（有意为之）：后端 enqueue_links 先把整段文本按同一批分隔符切词，
 * 再逐词判注释，因此 `# 注释 文字` 这类「注释符 + 空格 + 文字」在后端会多出一个
 * 待处理词（随后被过滤规则拦下或入队失败）。前端计数按「整行注释」处理，
 * 保证“识别到 N 条链接”是用户真正粘进来的链接数；入队结果永远取后端返回值，
 * 所以拦截 / 跳过条数始终以引擎为准。发送给后端的文本原样不动，前端不做任何过滤。
 *
 * 实现上复用 [`parseChunk`]（一次不限块数的步进），保证增量解析与一次性解析
 * 的口径完全一致。
 */
export function parseLinks(text: string): ParsedLinks {
  const cursor = createParseCursor()
  parseChunk(cursor, text, Number.POSITIVE_INFINITY)
  return {
    total: cursor.total,
    unique: cursor.unique,
    overflow: Math.max(0, cursor.unique.length - ENQUEUE_LIMIT),
  }
}

function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) {
    throw new Error(`[browser] 命令 ${cmd} 需要在桌面客户端中运行`)
  }
  return invoke<T>(cmd, args)
}

/**
 * 批量入队（后端逐条过过滤规则 + 自动开始下载）。
 * 失败时把后端错误原文抛给调用方展示，不做任何包装。
 */
export function enqueueLinks(text: string, outputDir?: string | null): Promise<EnqueueResult> {
  return call<EnqueueResult>('enqueue_links', { text, outputDir: outputDir ?? null })
}

/** 读取本地文本文件（txt 批量导入用） */
export function readTextFile(path: string): Promise<string> {
  return call<string>('read_text_file', { path })
}
