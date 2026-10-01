/**
 * 下载服务：对后端 yt-dlp 命令的语义化封装
 */
import * as ipc from './ipc'
import type { DownloadRequest, DownloadTask, MediaInfo } from '@/types'
import { tr } from '@/i18n'

export interface DownloadOptions {
  url: string
  formatId?: string | null
  /** video 完整视频 / audio 仅音频 / best 最佳 / thumb 仅封面 / subs 仅字幕 */
  mode?: 'video' | 'audio' | 'best' | 'thumb' | 'subs'
  outputDir?: string | null
  audioFormat?: string | null
  mergeContainer?: string | null
  embedThumbnail?: boolean
  embedSubs?: boolean
  playlist?: boolean
  rateLimit?: string | null
  subtitleLangs?: string[]
  saveThumbnail?: boolean
  thumbnailUrl?: string | null
  titleHint?: string | null
}

export const downloader = {
  probe(url: string, playlist = false): Promise<MediaInfo> {
    return ipc.probeUrl(url.trim(), playlist)
  },

  start(opts: DownloadOptions): Promise<DownloadTask> {
    const req: DownloadRequest = {
      url: opts.url.trim(),
      format_id: opts.formatId ?? null,
      mode: opts.mode ?? 'best',
      output_dir: opts.outputDir ?? null,
      audio_format: opts.audioFormat ?? null,
      merge_container: opts.mergeContainer ?? 'mp4',
      embed_thumbnail: opts.embedThumbnail ?? false,
      embed_subs: opts.embedSubs ?? false,
      filename_template: null,
      playlist: opts.playlist ?? false,
      rate_limit: opts.rateLimit ?? null,
      subtitle_langs: opts.subtitleLangs ?? [],
      save_thumbnail: opts.saveThumbnail ?? false,
      thumbnail_url: opts.thumbnailUrl ?? null,
      title_hint: opts.titleHint ?? null,
    }
    return ipc.startDownload(req)
  },

  list: () => ipc.listDownloads(),
  pause: (id: string) => ipc.pauseDownload(id),
  resume: (id: string) => ipc.resumeDownload(id),
  cancel: (id: string) => ipc.cancelDownload(id),
  remove: (id: string, deleteFile = false) => ipc.removeDownload(id, deleteFile),
  clear: (which: 'done' | 'all') => ipc.clearDownloads(which),
}

/** 支持的站点（UI 展示用） */
export function supportedSites() {
  return [
  { name: 'YouTube', icon: '▶' },
  { name: 'Bilibili', icon: '📺' },
  { name: 'Vimeo', icon: '🎬' },
  { name: 'TikTok', icon: '🎵' },
  { name: 'Twitter / X', icon: '𝕏' },
    { name: tr('1000+ 站点'), icon: '＋' },
  ]
}
