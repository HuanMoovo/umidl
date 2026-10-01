/**
 * 字幕服务：Whisper 封装
 */
import * as ipc from './ipc'
import type { SubtitleRequest, SubtitleTask, ToolProgress } from '@/types'
import { tr } from '@/i18n'
import { formatBytes } from './utils'

export interface SubtitleOptions {
  videoPath: string
  language?: string
  model?: string | null
  translateTo?: string | null
  outputFormat?: string
  outputDir?: string | null
}

export const whisper = {
  start(opts: SubtitleOptions): Promise<SubtitleTask> {
    const req: SubtitleRequest = {
      video_path: opts.videoPath,
      language: opts.language ?? 'auto',
      model: opts.model ?? null,
      translate_to: opts.translateTo ?? null,
      output_format: opts.outputFormat ?? 'srt',
      output_dir: opts.outputDir ?? null,
    }
    return ipc.startSubtitle(req)
  },
  list: () => ipc.listSubtitles(),
  cancel: (id: string) => ipc.cancelSubtitle(id),
  remove: (id: string) => ipc.removeSubtitle(id),
  read: (path: string) => ipc.readSubtitle(path),
}

export function languages() {
  return [
  { value: 'auto', label: tr('自动检测') },
  { value: 'zh', label: tr('中文') },
  { value: 'en', label: 'English' },
  { value: 'ja', label: tr('日本語') },
  { value: 'ko', label: '한국어' },
  { value: 'fr', label: 'Français' },
  { value: 'de', label: 'Deutsch' },
  { value: 'es', label: 'Español' },
    { value: 'ru', label: 'Русский' },
  ] as const
}

export const OUTPUT_FORMATS = ['srt', 'ass', 'vtt', 'txt', 'json'] as const

/**
 * 模型下载进度文案：优先用后端给的结构化字段（可翻译），退回后端 message。
 * 设置页与字幕页共用同一套措辞 —— 同一次下载在哪儿看说法都一致。
 */
export function modelProgressText(p: Partial<ToolProgress> | undefined | null): string {
  if (!p) return ''
  const done = Number(p.downloaded ?? 0)
  const total = Number(p.total ?? 0)
  if (p.source && done > 0 && total > 0) {
    const params = { source: p.source, done: formatBytes(done), total: formatBytes(total) }
    return p.resumed && done < total
      ? tr('继续上次未完成的下载：{done} / {total}（{source}）', params)
      : tr('正在从 {source} 下载 {done} / {total}', params)
  }
  return p.message || ''
}

export function models() {
  return [
  { value: 'tiny', label: tr('Tiny · 75MB · 最快'), size: '75 MB', quality: '★☆☆' },
  { value: 'base', label: tr('Base · 142MB · 均衡'), size: '142 MB', quality: '★★☆' },
  { value: 'small', label: tr('Small · 466MB · 推荐'), size: '466 MB', quality: '★★★' },
  { value: 'medium', label: tr('Medium · 1.5GB · 高精度'), size: '1.5 GB', quality: '★★★★' },
    { value: 'large-v3-turbo', label: tr('Turbo · 1.6GB · 快速高精度'), size: '1.6 GB', quality: '★★★★★' },
    { value: 'large-v3', label: tr('Large v3 · 3.1GB · 最高精度'), size: '3.1 GB', quality: '★★★★★' },
  ] as const
}
