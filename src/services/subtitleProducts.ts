/**
 * 字幕产物路径推导 —— 与 Rust 端 `subtitle.rs` 同一套命名规则
 *
 * 后端的产物命名（`subtitle_product_tag` / `subtitle_out_base` / `product_path`）：
 *   原文：`<视频基名>.<源语言标签>.<格式>`                —— `auto` / 空值 → `detected`
 *   译文：`<视频基名>.<源语言标签>-to-<目标语言>.<格式>`  —— 目前只有 `en`
 * 例：`song.detected.srt`、`song.detected-to-en.srt`、`song.zh.srt`、`song.zh-to-en.srt`
 *
 * 前端拿它回答「这一行的产物到底该在哪」：队列行的展示名、产物大小 / 存在性判定、
 * 「打开文件 / 在文件夹中显示」的目标，全部以它为准。
 *
 * 为什么必须推导而不是直接信记录里的 `subtitle_path`：1.8.6 之前的译文任务把**原文那份**
 * 记在了自己身上（译文行 `subtitle_path` = `song.detected.srt`），于是译文行「在文件夹中显示」
 * 定位到的是原文文件 —— 用户看到的就是「（英文）字幕文件不在文件夹里显示」。
 * 译文行的产物按规则永远是 `<基名>.<源语言标签>-to-en.<格式>`，认它才认得到自己那一份。
 *
 * 纯函数、无 IO：命名规则由单测（`subtitleProducts.test.ts`）锁死。
 */

/** 文件名安全标签：只留 ASCII 字母 / 数字 / `-` / `_` / `.`，其余换 `_`，去掉首尾分隔符（对齐 Rust `sanitize_tag`） */
export function sanitizeTag(raw: string): string {
  const mapped = String(raw ?? '')
    .split('')
    .map((c) => (/[A-Za-z0-9._-]/.test(c) ? c : '_'))
    .join('')
  const s = mapped.replace(/^[._]+/, '').replace(/[._]+$/, '')
  return s || 'detected'
}

/** 源语言标签：`auto` / 空值 → `detected`（对齐 Rust `language_tag`） */
export function languageTag(language: string): string {
  const raw = String(language ?? '').trim()
  const v = raw === '' ? 'auto' : raw
  return v.toLowerCase() === 'auto' ? 'detected' : sanitizeTag(v)
}

/** 译文目标语言标签：`en` / `english` → `en`；没开译文 → null（对齐 Rust `translate_tag`） */
export function translateTag(translateTo?: string | null): string | null {
  const raw = String(translateTo ?? '').trim()
  if (!raw) return null
  const low = raw.toLowerCase()
  return low === 'en' || low === 'english' ? 'en' : sanitizeTag(low)
}

/** 产物标签：原文 `<源语言>`、译文 `<源语言>-to-<目标语言>`（对齐 Rust `subtitle_product_tag`） */
export function productTag(language: string, translateTo?: string | null): string {
  const lang = languageTag(language)
  const to = translateTag(translateTo)
  return to ? `${lang}-to-${to}` : lang
}

/** 目录部分：`D:\a\b.srt` → `D:\a`；`b.srt` → ''（与 Node path.dirname 同口径，两种分隔符都认） */
export function dirname(p: string): string {
  const s = String(p ?? '')
  const i = Math.max(s.lastIndexOf('\\'), s.lastIndexOf('/'))
  if (i < 0) return ''
  // 根目录（`D:\` / `/`）保留分隔符本身
  if (i === 0) return s.slice(0, 1)
  return s.slice(0, i)
}

/** 基名（最后一个分隔符之后的部分；`basename` 的同口径简化版） */
export function baseName(p: string): string {
  const s = String(p ?? '')
  const i = Math.max(s.lastIndexOf('\\'), s.lastIndexOf('/'))
  return i < 0 ? s : s.slice(i + 1)
}

/** 视频基名：去掉最后一个扩展名（对齐 Rust `file_stem()`：`clip.mp4` → `clip`，`a.b.mp4` → `a.b`） */
export function videoStem(videoPath: string): string {
  const base = baseName(videoPath)
  const i = base.lastIndexOf('.')
  return i > 0 ? base.slice(0, i) : base
}

/** 产物扩展名：优先沿用记录里那份的扩展名（vtt / ass…），没有就用默认 `srt` */
export function productExt(subtitlePath?: string | null, fallback = 'srt'): string {
  const base = baseName(String(subtitlePath ?? ''))
  const i = base.lastIndexOf('.')
  const ext = i > 0 ? base.slice(i + 1).toLowerCase() : ''
  return /^[a-z0-9]+$/.test(ext) ? ext : fallback
}

/**
 * 拼接路径：目录为空时只回文件名（相对路径），目录已带分隔符时不重复加。
 * 放在这里而不是 utils，是为了让「字幕产物路径」的全部规则集中在一个可单测的模块里。
 */
export function joinPath(dir: string, name: string): string {
  const d = String(dir ?? '')
  if (!d) return name
  return /[\\/]$/.test(d) ? `${d}${name}` : `${d}\\${name}`
}

/**
 * 这一行的产物**应该**在哪（绝对路径）：
 *   `<目录>/<视频基名>.<产物标签>.<扩展名>`
 * 目录默认取视频所在目录（后端 `subtitle_out_base` 在没给 output_dir 时也是这么落盘的）。
 */
export function expectedProductPath(
  videoPath: string,
  language: string,
  translateTo?: string | null,
  ext = 'srt',
  dir?: string,
): string {
  const stem = videoStem(videoPath) || 'subtitle'
  const folder = dir && dir.trim() ? dir : dirname(videoPath)
  return joinPath(folder, `${stem}.${productTag(language, translateTo)}.${ext || 'srt'}`)
}

/** 队列行里与产物推导有关的字段（`SubtitleTask` 的子集） */
export interface SubtitleProductRow {
  video_path: string
  language: string
  subtitle_path?: string | null
  /** 这一行是不是「额外生成的那份英文字幕」（译文的产物名一定带 `-to-en`） */
  translated?: boolean
}

/**
 * 这一行的产物路径 —— 队列行显示名 / 大小 / 「打开文件 / 在文件夹中显示」都用它。
 *
 *   * 译文行：以推导出的 `<基名>.<源语言>-to-en.<扩展名>` 为准（旧记录把原文那份记在译文行上，
 *     直接信记录就会定位到原文文件 —— 用户看到的「文件不在文件夹里显示」）。
 *   * 其它行：优先用记录里那份（保留历史目录 / output_dir 的差异），没有记录时才推导。
 */
export function rowProductPath(row: SubtitleProductRow): string {
  const rec = String(row.subtitle_path ?? '').trim()
  if (row.translated) {
    // 目录沿用记录里那份（可能是历史 output_dir），没记录就用视频所在目录
    const ext = productExt(rec)
    return expectedProductPath(row.video_path, row.language, 'en', ext, dirname(rec) || dirname(row.video_path))
  }
  return rec || expectedProductPath(row.video_path, row.language, null, productExt(rec), dirname(row.video_path))
}
