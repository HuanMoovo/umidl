import { describe, expect, it } from 'vitest'
import {
  AUDIO_FORMATS,
  VIDEO_FORMATS,
  audioFor,
  buildFfmpegArgs,
  normalizeVideoCodec,
  qualityArgs,
  suggestCodecs,
  type ConvertOptions,
} from '@/services/ffmpeg'

const base = (o: Partial<ConvertOptions> = {}): ConvertOptions => ({
  inputFile: 'C:/in/a.mp4',
  format: 'mp4',
  extractAudio: false,
  ...o,
})

describe('buildFfmpegArgs', () => {
  it('基础参数包含输入输出与进度管道', () => {
    const a = buildFfmpegArgs(base({ outputFile: 'C:/out/a.mp4' }))
    expect(a).toContain('-i')
    expect(a).toContain('-progress')
    expect(a).toContain('pipe:1')
    expect(a[a.length - 1]).toBe('C:/out/a.mp4')
    expect(a).toContain('-y')
  })

  it('提取音频时移除视频流', () => {
    const a = buildFfmpegArgs(base({ extractAudio: true, format: 'mp3', audioCodec: 'libmp3lame' }))
    expect(a).toContain('-vn')
    expect(a).toContain('-c:a')
    expect(a).toContain('libmp3lame')
  })

  it('copy 模式直接复制编码', () => {
    const a = buildFfmpegArgs(base({ videoCodec: 'copy', audioCodec: 'copy' }))
    expect(a.join(' ')).toContain('-c:v copy')
    expect(a.join(' ')).toContain('-c:a copy')
  })

  it('静音时不写音频参数', () => {
    const a = buildFfmpegArgs(base({ mute: true, audioCodec: 'aac' }))
    expect(a).toContain('-an')
    expect(a).not.toContain('aac')
  })

  it('分辨率转换为 scale 滤镜', () => {
    const a = buildFfmpegArgs(base({ resolution: '1280x720', videoCodec: 'libx264' }))
    const idx = a.indexOf('-vf')
    expect(idx).toBeGreaterThan(-1)
    expect(a[idx + 1]).toContain('scale=1280:720')
  })

  it('mp4 输出启用 faststart', () => {
    const a = buildFfmpegArgs(base({ format: 'mp4' }))
    expect(a.join(' ')).toContain('-movflags +faststart')
  })

  it('webm 不写 faststart', () => {
    const a = buildFfmpegArgs(base({ format: 'webm' }))
    expect(a.join(' ')).not.toContain('faststart')
  })

  it('未提供输出路径时不追加多余参数', () => {
    const a = buildFfmpegArgs(base({ format: 'webm' }))
    expect(a[a.length - 1]).toBe('-nostats')
  })
})

describe('suggestCodecs', () => {
  it('按容器给出推荐编码', () => {
    expect(suggestCodecs('webm', false)).toEqual({ video: 'libvpx-vp9', audio: 'libopus' })
    expect(suggestCodecs('mp4', false).video).toBe('libx264')
    expect(suggestCodecs('mp4', true).video).toBe('none')
  })
})

describe('audioFor', () => {
  it('音频容器到编码映射', () => {
    expect(audioFor('mp3')).toBe('libmp3lame')
    expect(audioFor('flac')).toBe('flac')
    expect(audioFor('wav')).toBe('pcm_s16le')
    expect(audioFor('opus')).toBe('libopus')
    expect(audioFor('m4a')).toBe('aac')
  })
})

describe('格式常量', () => {
  it('覆盖技术方案要求的格式', () => {
    for (const f of ['mp4', 'mkv', 'mov', 'avi', 'webm', 'flv']) {
      expect(VIDEO_FORMATS as readonly string[]).toContain(f)
    }
    for (const f of ['mp3', 'aac', 'flac', 'wav', 'opus']) {
      expect(AUDIO_FORMATS as readonly string[]).toContain(f)
    }
  })
})

describe('normalizeVideoCodec', () => {
  it('别名归一化为真实编码器', () => {
    expect(normalizeVideoCodec('hevc')).toBe('libx265')
    expect(normalizeVideoCodec('H.265')).toBe('libx265')
    expect(normalizeVideoCodec('h264')).toBe('libx264')
    expect(normalizeVideoCodec('av1')).toBe('libsvtav1')
    expect(normalizeVideoCodec('vp9')).toBe('libvpx-vp9')
    expect(normalizeVideoCodec('libx264')).toBe('libx264')
  })
})

describe('qualityArgs', () => {
  it('x264 使用 -preset，VP9 使用 -deadline（不能写 -preset）', () => {
    expect(qualityArgs('libx264', 23).join(' ')).toContain('-preset medium')
    const vp9 = qualityArgs('libvpx-vp9', 33).join(' ')
    expect(vp9).toContain('-deadline good')
    expect(vp9).toContain('-b:v 0')
    expect(vp9).not.toContain('-preset')
  })

  it('WebM 转换产出可用的 VP9 参数组合', () => {
    const a = buildFfmpegArgs(base({ format: 'webm', videoCodec: 'vp9', audioCodec: 'libopus', crf: 33 }))
    const j = a.join(' ')
    expect(j).toContain('-c:v libvpx-vp9')
    expect(j).toContain('-c:a libopus')
    expect(j).not.toContain('-preset')
  })

  it('H.265 转换归一化为 libx265', () => {
    const a = buildFfmpegArgs(base({ format: 'mp4', videoCodec: 'hevc', audioCodec: 'aac', crf: 28 }))
    expect(a.join(' ')).toContain('-c:v libx265')
  })
})
