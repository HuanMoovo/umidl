/**
 * 纯函数工具集（无 Tauri 依赖，可直接单元测试）
 */
import type { VideoFormat, MediaInfo, MediaProbe, DownloadTask } from '@/types'
import { currentLocale, tr } from '@/i18n'

/** 秒 → 00:03:21 */
export function formatDuration(sec?: number | null): string {
  if (sec === undefined || sec === null || !isFinite(sec) || sec < 0) return '--:--'
  const s = Math.floor(sec % 60)
  const m = Math.floor((sec / 60) % 60)
  const h = Math.floor(sec / 3600)
  const p = (n: number) => String(n).padStart(2, '0')
  return h > 0 ? `${h}:${p(m)}:${p(s)}` : `${p(m)}:${p(s)}`
}

/** 字节 → 人类可读 */
export function formatBytes(bytes?: number | null): string {
  if (bytes === undefined || bytes === null || !isFinite(bytes) || bytes <= 0) return '--'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let i = 0
  let v = bytes
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v.toFixed(v >= 100 || i === 0 ? 0 : v >= 10 ? 1 : 2)} ${units[i]}`
}

/** "12.34MiB/s" / "1.2MB/s" → 归一化文本 */
export function normalizeSpeed(raw?: string | null): string {
  if (!raw) return ''
  const m = raw.match(/([\d.]+)\s*([KMG]i?B)\/s/i)
  if (!m) return raw
  const unit = m[2].replace('i', '').toUpperCase()
  return `${parseFloat(m[1]).toFixed(2)} ${unit}/s`
}

/** 从 yt-dlp 的 [download] 进度行解析进度信息 */
export function parseYtdlpProgress(line: string): {
  percent?: number
  downloaded?: number
  total?: number
  speed?: string
  eta?: string
} | null {
  if (!line.includes('[download]')) return null
  const out: any = {}
  const pct = line.match(/\[download\]\s+([\d.]+)%/)
  if (pct) out.percent = parseFloat(pct[1])
  const size = line.match(/of\s+~?\s*([\d.]+)(KiB|MiB|GiB|B)/i)
  if (size) out.total = toBytes(parseFloat(size[1]), size[2])
  const dl = line.match(/\[download\]\s+([\d.]+)(KiB|MiB|GiB|B)\s+of/i)
  if (dl) out.downloaded = toBytes(parseFloat(dl[1]), dl[2])
  const sp = line.match(/at\s+([\d.]+\s*[KMG]i?B\/s)/i)
  if (sp) out.speed = normalizeSpeed(sp[1])
  const eta = line.match(/ETA\s+([\d:]+)/i)
  if (eta) out.eta = eta[1]
  return Object.keys(out).length ? out : null
}

export function toBytes(value: number, unit: string): number {
  const u = unit.toUpperCase().replace('I', '')
  const map: Record<string, number> = { B: 1, KB: 1024, MB: 1024 ** 2, GB: 1024 ** 3, TB: 1024 ** 4 }
  return Math.round(value * (map[u] ?? 1))
}

/** 分辨率字符串排序权重 */
export function resolutionWeight(res?: string | null): number {
  if (!res || res === 'audio only') return 0
  const m = res.match(/(\d+)\s*x\s*(\d+)/)
  if (!m) return 0
  return parseInt(m[1], 10) * parseInt(m[2], 10)
}

/** 过滤出「视频+音频合并」的推荐格式（按分辨率降序） */
export function recommendedFormats(formats: VideoFormat[]): VideoFormat[] {
  return formats
    .filter((f) => f.has_video)
    .slice()
    .sort((a, b) => resolutionWeight(b.resolution) - resolutionWeight(a.resolution))
}

/** 从完整格式列表中挑出展示用的精简列表（同容器+分辨率+轨道组合只留一条） */
export function simplifyFormats(formats: VideoFormat[]): VideoFormat[] {
  const seen = new Set<string>()
  const out: VideoFormat[] = []
  for (const f of formats) {
    const key = `${f.ext}|${f.resolution}|${f.has_video ? 'v' : ''}${f.has_audio ? 'a' : ''}`
    if (seen.has(key)) continue
    seen.add(key)
    out.push(f)
  }
  return out
}

/** 判断输入是否像一条可解析的媒体链接 */
export function looksLikeUrl(input: string): boolean {
  const t = input.trim()
  if (!t || /\s/.test(t)) return false
  try {
    const u = new URL(t)
    return u.protocol === 'http:' || u.protocol === 'https:'
  } catch {
    return false
  }
}

/** 从路径中取文件名（兼容 Windows 反斜杠） */
export function basename(p: string): string {
  if (!p) return ''
  const parts = p.replace(/\\/g, '/').split('/')
  return parts[parts.length - 1] || p
}

/** 去掉扩展名 */
export function stripExt(name: string): string {
  const i = name.lastIndexOf('.')
  return i > 0 ? name.slice(0, i) : name
}

/**
 * 任务卡上的 format_note 是后端在下载时生成的中文快照（如「视频 · 最佳」），
 * 这里按「 · 」分段重译，老记录切语言后也能正确显示；未收录的片段原样返回。
 */
export function translateFormatNote(note?: string | null): string {
  if (!note) return ''
  return note
    .split(' · ')
    .map((seg) => tr(seg.trim()))
    .filter((seg) => seg && seg !== '·')
    .join(' · ')
}

/** 任务是否为活跃状态 */
export function isActive(task: DownloadTask): boolean {
  return ['pending', 'parsing', 'downloading', 'converting', 'extracting', 'transcribing'].includes(task.status)
}

/** 汇总媒体信息里的最佳预览描述 */
export function describeMedia(info: MediaInfo): string {
  const bits: string[] = []
  if (info.extractor) bits.push(info.extractor)
  if (info.duration) bits.push(formatDuration(info.duration))
  const vf = recommendedFormats(info.formats)
  if (vf.length) bits.push(tr('最高 {res}', { res: vf[0].resolution || tr('未知') }))
  if (info.playlist_count && info.playlist_count > 1) bits.push(tr('{n} 个视频', { n: info.playlist_count }))
  return bits.join(' · ')
}

/** ffprobe 结果 → 摘要 */
export function describeProbe(p: MediaProbe): string {
  const bits: string[] = []
  if (p.video_codec) bits.push(tr('视频 {codec}', { codec: p.video_codec }))
  if (p.audio_codec) bits.push(tr('音频 {codec}', { codec: p.audio_codec }))
  if (p.width && p.height) bits.push(`${p.width}x${p.height}`)
  if (p.duration) bits.push(formatDuration(p.duration))
  if (p.size) bits.push(formatBytes(p.size))
  return bits.join(' · ')
}

/** 校验时间轴是否为递增序列（字幕自检用） */
export function isMonotonic(times: number[]): boolean {
  for (let i = 1; i < times.length; i++) {
    if (times[i] < times[i - 1]) return false
  }
  return true
}

/* ==================== 下载增强 ==================== */

/**
 * 观看量 / 点赞量 → 可读计数
 * 中文：12345 → 1.2万 / 2.5亿（中文习惯的万、亿）
 * 其它语言：12345 → 12.3K / 2.5M（拉丁字母的千分阶，避免出现「1.2×10K」这类怪格式）
 */
export function formatCount(n?: number | null): string {
  if (n === undefined || n === null || !isFinite(n) || n < 0) return '--'
  if (n < 10000) return String(Math.round(n))
  if (currentLocale() === 'zh') {
    if (n < 100000000) return tr('{n}万', { n: (n / 10000).toFixed(n < 100000 ? 1 : 0) })
    return tr('{n}亿', { n: (n / 100000000).toFixed(2) })
  }
  if (n < 1000000) return `${(n / 1000).toFixed(n < 100000 ? 1 : 0)}K`
  if (n < 1000000000) return `${(n / 1000000).toFixed(1)}M`
  return `${(n / 1000000000).toFixed(1)}B`
}

/** 字幕轨道 → 展示标签（语言 + 是否自动生成） */
export function subtitleLabel(track: { lang: string; name?: string | null; auto?: boolean }): string {
  const base = track.name?.trim() || track.lang
  return track.auto ? tr('{base}（自动）', { base }) : base
}

/** 常用语言的中文名（用于字幕语言选择） */
const LANG_NAMES: Record<string, string> = {
  zh: tr('中文'),
  'zh-Hans': tr('简体中文'),
  'zh-Hant': tr('繁体中文'),
  'zh-CN': tr('简体中文'),
  'zh-TW': tr('繁体中文'),
  en: tr('英语'),
  ja: tr('日语'),
  ko: tr('韩语'),
  es: tr('西班牙语'),
  fr: tr('法语'),
  de: tr('德语'),
  ru: tr('俄语'),
  pt: tr('葡萄牙语'),
  it: tr('意大利语'),
  ar: tr('阿拉伯语'),
  hi: tr('印地语'),
  th: tr('泰语'),
  vi: tr('越南语'),
}

export function languageName(lang: string): string {
  if (LANG_NAMES[lang]) return LANG_NAMES[lang]
  const short = lang.split('-')[0]
  return LANG_NAMES[short] ? `${LANG_NAMES[short]}（${lang}）` : lang
}

/** 按人类习惯给格式排序：先分辨率，再容器 */
export function sortedVideoFormats(formats: VideoFormat[]): VideoFormat[] {
  return simplifyFormats(formats)
    .filter((f) => f.has_video)
    .sort((a, b) => {
      const w = resolutionWeight(b.resolution) - resolutionWeight(a.resolution)
      if (w !== 0) return w
      return (a.ext || '').localeCompare(b.ext || '')
    })
}

/* ==================== 转换：自动检测 ==================== */

export interface AutoTarget {
  format: string
  video: string
  audio: string
  extractAudio: boolean
  /** 是否可无损直接封装 */
  remux: boolean
  reason: string
}

/**
 * 依据探测结果推荐最佳输出格式（自动检测）：
 * 1) 已是 mp4 友好编码（h264/aac）→ 推荐 mp4 + copy（无损、秒级完成）
 * 2) 已是 webm 友好编码（vp8/vp9/av1 + opus/vorbis）→ 推荐 webm + copy
 * 3) 纯音频：无损源保留 flac，其余推荐 mp3
 * 4) 其它未知编码 → mp4 + libx264/aac 重编码
 */
export function autoDetectTarget(p: MediaProbe | null | undefined): AutoTarget {
  const v = (p?.video_codec || '').toLowerCase()
  const a = (p?.audio_codec || '').toLowerCase()
  const hasVideo = !!v && v !== 'none'
  const hasAudio = !!a && a !== 'none'

  if (!hasVideo) {
    // 纯音频源：无损保留，其它转码为兼容性最好的 mp3
    const lossless = ['flac', 'alac', 'pcm_s16le', 'pcm_s24le', 'wav'].some((c) => a.includes(c))
    if (lossless) {
      return {
        format: 'flac', video: 'none', audio: 'flac', extractAudio: true, remux: true,
        reason: tr('源为无损音频（{codec}），推荐 FLAC 无损导出', { codec: a || tr('未知') }),
      }
    }
    if (a === 'opus') {
      return { format: 'opus', video: 'none', audio: 'libopus', extractAudio: true, remux: true, reason: tr('源为 Opus，直接封装为 .opus 无需重编码') }
    }
    return {
      format: 'mp3', video: 'none', audio: 'libmp3lame', extractAudio: true, remux: false,
      reason: tr('源音频编码 {codec} 兼容性一般，推荐转 MP3（通用性最好）', { codec: a || tr('未知') }),
    }
  }

  // MP4 家族且音频兼容 → 无损封装
  if (['h264', 'avc1', 'hevc', 'h265'].includes(v) && (!hasAudio || ['aac', 'mp3', 'ac3', 'eac3'].includes(a))) {
    return {
      format: 'mp4', video: 'copy', audio: 'copy', extractAudio: false, remux: true,
      reason: tr('源为 {codecs}，可直接无损封装为 MP4（不重编码，速度最快）', { codecs: `${v}${hasAudio ? ` + ${a}` : ''}` }),
    }
  }
  // WebM 家族 → 无损封装
  if (['vp8', 'vp9', 'av1'].includes(v) && (!hasAudio || ['opus', 'vorbis'].includes(a))) {
    return {
      format: 'webm', video: 'copy', audio: 'copy', extractAudio: false, remux: true,
      reason: tr('源为 {codecs}，可直接无损封装为 WebM', { codecs: `${v}${hasAudio ? ` + ${a}` : ''}` }),
    }
  }
  // 其它 → 通用 MP4
  return {
    format: 'mp4', video: 'libx264', audio: 'aac', extractAudio: false, remux: false,
    reason: tr('源编码 {codec} 通用性不足，推荐重编码为 MP4（H.264 + AAC，兼容性最好）', { codec: v }),
  }
}

/** 探测结果 → 源文件类型标签 */
export function probeKind(p: MediaProbe | null | undefined): 'video' | 'audio' | 'unknown' {
  if (!p) return 'unknown'
  if (p.video_codec && p.video_codec !== 'none') return 'video'
  if (p.audio_codec && p.audio_codec !== 'none') return 'audio'
  return 'unknown'
}
