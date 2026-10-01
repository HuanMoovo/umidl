/**
 * 转换服务：FFmpeg 封装
 */
import * as ipc from './ipc'
import type { ConvertRequest, ConvertTask, MediaProbe } from '@/types'
import { tr } from '@/i18n'

export interface ConvertOptions {
  inputFile: string
  format: string
  outputDir?: string | null
  /** 输出文件完整路径（构建命令时必须提供，否则不会写进参数） */
  outputFile?: string | null
  videoCodec?: string | null
  audioCodec?: string | null
  bitrate?: string | null
  resolution?: string | null
  crf?: number | null
  extractAudio?: boolean
  mute?: boolean
  /** ===== 高级选项（更多选择） ===== */
  audioBitrate?: string | null
  sampleRate?: number | null
  channels?: number | null
  fps?: number | null
  speed?: number | null
  hwaccel?: boolean
  faststart?: boolean
  removeMetadata?: boolean
  /** 自动检测：由后端解析源信息后自动选择输出参数 */
  autoFormat?: boolean
}

export const ffmpeg = {
  probe: (path: string): Promise<MediaProbe> => ipc.probeMedia(path),

  start(opts: ConvertOptions): Promise<ConvertTask> {
    const req: ConvertRequest = {
      input_file: opts.inputFile,
      output_dir: opts.outputDir ?? null,
      format: opts.format,
      video_codec: opts.videoCodec ?? null,
      audio_codec: opts.audioCodec ?? null,
      bitrate: opts.bitrate ?? null,
      resolution: opts.resolution ?? null,
      crf: opts.crf ?? null,
      extract_audio: opts.extractAudio ?? false,
      mute: opts.mute ?? false,
      audio_bitrate: opts.audioBitrate ?? null,
      sample_rate: opts.sampleRate ?? null,
      channels: opts.channels ?? null,
      fps: opts.fps ?? null,
      speed: opts.speed ?? null,
      hwaccel: opts.hwaccel ?? false,
      faststart: opts.faststart ?? true,
      remove_metadata: opts.removeMetadata ?? false,
      auto_format: opts.autoFormat ? 'auto' : null,
    }
    return ipc.startConvert(req)
  },

  list: () => ipc.listConverts(),
  cancel: (id: string) => ipc.cancelConvert(id),
  remove: (id: string) => ipc.removeConvert(id),
}

/** 目标格式能力表（视频容器） */
export const VIDEO_FORMATS = [
  'mp4',
  'mkv',
  'mov',
  'webm',
  'avi',
  'flv',
  'ts',
  'm4v',
  'mpg',
  'wmv',
  'ogv',
  'gif',
] as const

/** 目标格式能力表（音频容器） */
export const AUDIO_FORMATS = ['mp3', 'm4a', 'aac', 'flac', 'wav', 'opus', 'ogg', 'wma', 'ac3', 'aiff'] as const

/** 视频编码器：软编 + 硬件加速编码 */
export const VIDEO_CODECS = [
  'copy',
  'libx264',
  'libx265',
  'libsvtav1',
  'libvpx-vp9',
  'libvpx',
  'mpeg4',
  'libxvid',
  'prores_ks',
  'h264_nvenc',
  'hevc_nvenc',
  'av1_nvenc',
  'h264_qsv',
  'hevc_qsv',
  'h264_amf',
  'hevc_amf',
] as const

/** 音频编码器 */
export const AUDIO_CODECS = [
  'copy',
  'aac',
  'libmp3lame',
  'libopus',
  'libvorbis',
  'flac',
  'alac',
  'ac3',
  'pcm_s16le',
  'pcm_s24le',
] as const

export const RESOLUTIONS = [
  '原分辨率',
  '7680x4320',
  '3840x2160',
  '2560x1440',
  '1920x1080',
  '1600x900',
  '1280x720',
  '960x540',
  '854x480',
  '640x360',
] as const

export const FRAMERATES = [0, 15, 24, 25, 30, 50, 60, 120] as const
export const SAMPLE_RATES = [0, 8000, 16000, 22050, 32000, 44100, 48000, 96000] as const
export const CHANNELS = [0, 1, 2, 6] as const
export const SPEEDS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 4] as const
export const AUDIO_BITRATES = ['auto', '96k', '128k', '192k', '256k', '320k'] as const
export const VIDEO_BITRATES = ['auto', '1M', '2M', '4M', '8M', '16M', '32M'] as const

/** 常见编码质量预设 */
export function qualityPresets() {
  return [
  { label: tr('接近原始画质'), crf: 14 },
  { label: tr('极速（体积大）'), crf: 18 },
  { label: tr('标准（推荐）'), crf: 23 },
  { label: tr('高压缩（体积小）'), crf: 28 },
    { label: tr('极限压缩'), crf: 32 },
  ] as const
}

/** 硬件编码器集合 */
export const HW_ENCODERS = [
  'h264_nvenc',
  'hevc_nvenc',
  'av1_nvenc',
  'h264_qsv',
  'hevc_qsv',
  'h264_amf',
  'hevc_amf',
]

export function isHardwareEncoder(codec: string): boolean {
  return HW_ENCODERS.includes(codec.trim().toLowerCase())
}

/** 依据输出格式推断默认音视频编码 */
export function suggestCodecs(format: string, extractAudio: boolean): { video: string; audio: string } {
  if (extractAudio) return { video: 'none', audio: audioFor(format) }
  switch (format.toLowerCase()) {
    case 'webm':
      return { video: 'libvpx-vp9', audio: 'libopus' }
    case 'ogv':
      return { video: 'libvpx', audio: 'libvorbis' }
    case 'avi':
      return { video: 'libx264', audio: 'libmp3lame' }
    case 'flv':
      return { video: 'libx264', audio: 'aac' }
    case 'wmv':
      return { video: 'mpeg4', audio: 'wma' }
    case 'gif':
      return { video: 'gif', audio: 'none' }
    case 'ts':
    case 'mpg':
      return { video: 'libx264', audio: 'aac' }
    case 'mkv':
    case 'mov':
    case 'm4v':
      return { video: 'libx264', audio: 'aac' }
    default:
      return { video: 'libx264', audio: 'aac' }
  }
}

export function audioFor(format: string): string {
  switch (format.toLowerCase()) {
    case 'mp3':
      return 'libmp3lame'
    case 'opus':
      return 'libopus'
    case 'ogg':
      return 'libvorbis'
    case 'flac':
      return 'flac'
    case 'wav':
      return 'pcm_s16le'
    case 'aiff':
      return 'pcm_s16le'
    case 'wma':
      return 'wmav2'
    case 'ac3':
      return 'ac3'
    case 'alac':
      return 'alac'
    case 'm4a':
      return 'aac'
    default:
      return 'aac'
  }
}

/** 变速滤镜链：atempo 单次范围 0.5~2.0，超出需串联 */
export function atempoChain(speed: number): string {
  const s = Math.max(0.1, Math.min(100, speed))
  const parts: string[] = []
  let rest = s
  while (rest > 2.0) {
    parts.push('atempo=2.0')
    rest /= 2.0
  }
  while (rest < 0.5) {
    parts.push('atempo=0.5')
    rest *= 2.0
  }
  parts.push(`atempo=${rest.toFixed(4)}`)
  return parts.join(',')
}

/** 构建 ffmpeg 参数（纯函数，可单测；与 Rust 端 build_ffmpeg_args 保持一致） */
export function buildFfmpegArgs(o: ConvertOptions): string[] {
  const args = ['-hide_banner', '-nostdin', '-y']
  if (o.hwaccel) args.push('-hwaccel', 'auto')
  args.push('-i', o.inputFile, '-progress', 'pipe:1', '-nostats')

  const vf: string[] = []
  const af: string[] = []
  const speed = o.speed ?? 1

  if (o.extractAudio) {
    args.push('-vn')
  } else {
    if (o.videoCodec && o.videoCodec.toLowerCase() !== 'copy' && o.videoCodec.toLowerCase() !== 'none') {
      const vc = normalizeVideoCodec(o.videoCodec)
      args.push('-c:v', vc)
      if (o.crf !== undefined && o.crf !== null) args.push(...qualityArgs(vc, o.crf))
      if (o.bitrate && o.bitrate.toLowerCase() !== 'auto') args.push('-b:v', o.bitrate)
    } else if (o.videoCodec && o.videoCodec.toLowerCase() === 'copy') {
      args.push('-c:v', 'copy')
    }
    if (o.resolution && o.resolution !== '原分辨率') {
      vf.push(`scale=${o.resolution.replace('x', ':')}:force_original_aspect_ratio=decrease`)
    }
    if (o.fps && o.fps > 0) vf.push(`fps=${o.fps}`)
    if (speed !== 1) vf.push(`setpts=${(1 / speed).toFixed(6)}*PTS`)
  }

  if (o.mute || o.format.toLowerCase() === 'gif') {
    args.push('-an')
  } else {
    const ac = o.extractAudio ? audioFor(o.format) : o.audioCodec
    if (ac === 'copy' && !o.extractAudio) args.push('-c:a', 'copy')
    else if (ac && ac !== 'copy' && ac !== 'none') args.push('-c:a', normalizeAudioCodec(ac))
    if (o.audioBitrate && o.audioBitrate.toLowerCase() !== 'auto' && ac !== 'copy' && !(ac || '').startsWith('pcm')) {
      args.push('-b:a', o.audioBitrate)
    }
    if (o.sampleRate && o.sampleRate > 0) args.push('-ar', String(o.sampleRate))
    if (o.channels && o.channels > 0) args.push('-ac', String(o.channels))
    if (speed !== 1) af.push(atempoChain(speed))
  }

  if (vf.length) args.push('-vf', vf.join(','))
  if (af.length) args.push('-af', af.join(','))

  if (o.removeMetadata) args.push('-map_metadata', '-1')

  const fmt = o.format.toLowerCase()
  if (o.faststart !== false && ['mp4', 'mov', 'm4a', 'm4v'].includes(fmt)) {
    args.push('-movflags', '+faststart')
  }

  if (o.outputFile) args.push(o.outputFile)
  return args
}

/** 编解码器别名归一化（与 Rust 端 normalize_video_codec 保持一致） */
export function normalizeVideoCodec(codec: string): string {
  switch (codec.trim().toLowerCase()) {
    case 'hevc':
    case 'h265':
    case 'h.265':
    case 'x265':
      return 'libx265'
    case 'h264':
    case 'h.264':
    case 'avc':
    case 'x264':
      return 'libx264'
    case 'av1':
    case 'svtav1':
    case 'svt-av1':
      return 'libsvtav1'
    case 'vp9':
      return 'libvpx-vp9'
    case 'vp8':
      return 'libvpx'
    case 'gif':
      return 'gif'
    default:
      return codec.trim()
  }
}

/** 音频编码器别名归一化 */
export function normalizeAudioCodec(codec: string): string {
  switch (codec.trim().toLowerCase()) {
    case 'mp3':
      return 'libmp3lame'
    case 'opus':
      return 'libopus'
    case 'ogg':
    case 'vorbis':
      return 'libvorbis'
    case 'wma':
      return 'wmav2'
    case 'm4a':
      return 'aac'
    default:
      return codec.trim()
  }
}

/** 依编码器选择质量参数（与 Rust 端 quality_args 保持一致；硬件编码不支持 -preset medium） */
export function qualityArgs(codec: string, crf: number): string[] {
  switch (codec) {
    case 'libx264':
    case 'libx265':
      return ['-crf', String(crf), '-preset', 'medium']
    case 'libvpx-vp9':
    case 'libvpx':
      return ['-crf', String(crf), '-b:v', '0', '-row-mt', '1', '-deadline', 'good', '-cpu-used', '2']
    case 'libsvtav1':
      return ['-crf', String(crf), '-preset', '8']
    case 'h264_nvenc':
    case 'hevc_nvenc':
    case 'av1_nvenc':
      return ['-rc', 'vbr', '-cq', String(crf), '-preset', 'p5', '-b:v', '0']
    case 'h264_qsv':
    case 'hevc_qsv':
      return ['-global_quality', String(crf), '-look_ahead', '1']
    case 'h264_amf':
    case 'hevc_amf':
      return ['-rc', 'cqp', '-qp_i', String(crf), '-qp_p', String(crf)]
    case 'gif':
      return []
    default:
      return ['-crf', String(crf)]
  }
}
