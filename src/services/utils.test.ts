import { describe, expect, it } from 'vitest'
import {
  autoDetectTarget,
  basename,
  describeMedia,
  formatBytes,
  formatCount,
  translateFormatNote,
  formatDuration,
  isMonotonic,
  languageName,
  looksLikeUrl,
  normalizeSpeed,
  parseYtdlpProgress,
  probeKind,
  recommendedFormats,
  resolutionWeight,
  simplifyFormats,
  sortedVideoFormats,
  stripExt,
  subtitleLabel,
  toBytes,
} from '@/services/utils'
import type { MediaInfo, MediaProbe, VideoFormat } from '@/types'
import { setLocale } from '@/i18n'

const fmt = (o: Partial<VideoFormat>): VideoFormat => ({
  format_id: 'x',
  ext: 'mp4',
  resolution: '1280x720',
  has_video: true,
  has_audio: true,
  ...o,
})

describe('formatDuration', () => {
  it('格式化秒为 mm:ss / hh:mm:ss', () => {
    expect(formatDuration(0)).toBe('00:00')
    expect(formatDuration(65)).toBe('01:05')
    expect(formatDuration(3725)).toBe('1:02:05')
  })
  it('非法值返回占位符', () => {
    expect(formatDuration(null)).toBe('--:--')
    expect(formatDuration(-3)).toBe('--:--')
    expect(formatDuration(Number.NaN)).toBe('--:--')
  })
})

describe('formatBytes', () => {
  it('按单位换算', () => {
    expect(formatBytes(0)).toBe('--')
    expect(formatBytes(512)).toBe('512 B')
    expect(formatBytes(1024)).toBe('1.00 KB')
    expect(formatBytes(1024 * 1024 * 3.5)).toBe('3.50 MB')
  })
})

describe('toBytes / normalizeSpeed', () => {
  it('单位换算正确', () => {
    expect(toBytes(1, 'KiB')).toBe(1024)
    expect(toBytes(1.5, 'MiB')).toBe(1572864)
  })
  it('速度文本归一化', () => {
    expect(normalizeSpeed('3.21MiB/s')).toBe('3.21 MB/s')
    expect(normalizeSpeed('')).toBe('')
  })
})

describe('parseYtdlpProgress', () => {
  it('解析百分比 / 大小 / 速度 / 剩余时间', () => {
    const r = parseYtdlpProgress('[download]  42.3% of  165.53MiB at  3.21MiB/s ETA 00:52')
    expect(r?.percent).toBeCloseTo(42.3)
    expect(r?.total).toBe(Math.round(165.53 * 1024 * 1024))
    expect(r?.speed).toBe('3.21 MB/s')
    expect(r?.eta).toBe('00:52')
  })
  it('非下载行返回 null', () => {
    expect(parseYtdlpProgress('[info] Downloading 1 format')).toBeNull()
  })
})

describe('resolutionWeight / simplifyFormats', () => {
  it('按像素面积排序权重', () => {
    expect(resolutionWeight('1920x1080')).toBeGreaterThan(resolutionWeight('1280x720'))
    expect(resolutionWeight('audio only')).toBe(0)
    expect(resolutionWeight(undefined)).toBe(0)
  })
  it('去重：同扩展名+分辨率+轨道的格式只保留首个', () => {
    const out = simplifyFormats([
      fmt({ format_id: 'a', resolution: '1920x1080', ext: 'mp4' }),
      fmt({ format_id: 'a2', resolution: '1920x1080', ext: 'mp4' }),
      fmt({ format_id: 'b', resolution: '1920x1080', ext: 'webm' }),
      fmt({ format_id: 'c', resolution: '640x360', ext: 'mp4' }),
    ])
    expect(out.map((f) => f.format_id)).toEqual(['a', 'b', 'c'])
  })
  it('视频轨/音频轨组合不同则不去重', () => {
    const out = simplifyFormats([
      fmt({ format_id: 'v', resolution: '1920x1080', has_audio: false }),
      fmt({ format_id: 'va', resolution: '1920x1080', has_audio: true }),
    ])
    expect(out.length).toBe(2)
  })
  it('推荐格式按分辨率降序且仅含视频', () => {
    const list = recommendedFormats([
      fmt({ format_id: 'low', resolution: '640x360' }),
      fmt({ format_id: 'hi', resolution: '1920x1080' }),
      fmt({ format_id: 'audio', resolution: 'audio only', has_video: false }),
    ])
    expect(list.map((f) => f.format_id)).toEqual(['hi', 'low'])
  })
})

describe('looksLikeUrl', () => {
  it('识别合法链接', () => {
    expect(looksLikeUrl('https://www.bilibili.com/video/BV1xx')).toBe(true)
    expect(looksLikeUrl('http://127.0.0.1:8000/a.mp4')).toBe(true)
  })
  it('拒绝非法输入', () => {
    expect(looksLikeUrl('not a url')).toBe(false)
    expect(looksLikeUrl('ftp://x/y')).toBe(false)
    expect(looksLikeUrl('')).toBe(false)
  })
})

describe('path helpers', () => {
  it('basename 兼容反斜杠', () => {
    expect(basename('C:\\Users\\a\\b.mp4')).toBe('b.mp4')
    expect(basename('/tmp/x/y.mkv')).toBe('y.mkv')
  })
  it('stripExt 去扩展名', () => {
    expect(stripExt('a.b.mp4')).toBe('a.b')
    expect(stripExt('noext')).toBe('noext')
  })
})

describe('isMonotonic', () => {
  it('检测时间轴递增', () => {
    expect(isMonotonic([0, 1, 2, 3])).toBe(true)
    expect(isMonotonic([0, 3, 2])).toBe(false)
    expect(isMonotonic([])).toBe(true)
  })
})

describe('describeMedia', () => {
  it('汇总媒体描述', () => {
    const info: MediaInfo = {
      id: '1',
      title: 't',
      webpage_url: 'u',
      extractor: 'Bilibili',
      duration: 125,
      formats: [fmt({ resolution: '1920x1080' })],
      subtitles: [],
      subtitle_tracks: [],
      is_playlist: false,
    }
    const d = describeMedia(info)
    expect(d).toContain('Bilibili')
    expect(d).toContain('02:05')
    expect(d).toContain('1920x1080')
  })
})

describe('下载增强纯函数', () => {
  it('formatCount 中文可读计数', () => {
    expect(formatCount(999)).toBe('999')
    expect(formatCount(12345)).toBe('1.2万')
    expect(formatCount(1234567)).toBe('123万')
    expect(formatCount(250000000)).toBe('2.50亿')
    expect(formatCount(null)).toBe('--')
  })

  it('formatCount 跟随界面语言（非中文用 K/M，不出现「×10K」这类怪格式）', () => {
    const n = 12345
    setLocale('zh')
    expect(formatCount(n)).toBe('1.2万')
    setLocale('en')
    expect(formatCount(n)).toBe('12.3K')
    expect(formatCount(1234567)).toBe('1.2M')
    expect(formatCount(250000000)).toBe('250.0M')
    setLocale('ja')
    expect(formatCount(n)).toBe('12.3K')
    setLocale('fr')
    expect(formatCount(32000)).toBe('32.0K')
    setLocale('zh') // 还原，避免影响其它用例
  })

  it('translateFormatNote 重译后端中文快照（老记录切语言也正常）', () => {
    setLocale('zh')
    expect(translateFormatNote('视频 · 最佳')).toBe('视频 · 最佳')
    setLocale('en')
    expect(translateFormatNote('视频 · 最佳')).toBe('Video · Best')
    expect(translateFormatNote('音频 · mp3')).toBe('Audio · mp3')
    expect(translateFormatNote('封面图片')).toBe('Cover image')
    expect(translateFormatNote('字幕 · 全部语言')).toBe('Subtitles · All languages')
    expect(translateFormatNote('')).toBe('')
    expect(translateFormatNote(null)).toBe('')
    setLocale('zh')
  })

  it('formatBytes 人类可读', () => {
    expect(formatBytes(1024)).toBe('1.00 KB')
    expect(formatBytes(1024 * 1024 * 5)).toBe('5.00 MB')
  })

  it('languageName 语言名', () => {
    expect(languageName('zh')).toBe('中文')
    expect(languageName('en')).toBe('英语')
    expect(languageName('xx-YY')).toBe('xx-YY')
  })

  it('subtitleLabel 自动字幕标记', () => {
    expect(subtitleLabel({ lang: 'en', name: 'English', auto: false })).toBe('English')
    expect(subtitleLabel({ lang: 'zh', name: null, auto: true })).toBe('zh（自动）')
  })

  it('sortedVideoFormats 按分辨率降序', () => {
    const list = sortedVideoFormats([
      fmt({ format_id: 'a', resolution: '640x360', ext: 'mp4' }),
      fmt({ format_id: 'b', resolution: '1920x1080', ext: 'mp4' }),
      fmt({ format_id: 'c', resolution: '1280x720', ext: 'webm' }),
    ])
    expect(list.map((f) => f.resolution)).toEqual(['1920x1080', '1280x720', '640x360'])
  })
})

describe('autoDetectTarget 自动检测', () => {
  const probe = (v?: string, a?: string): MediaProbe => ({ path: 'x', video_codec: v ?? null, audio_codec: a ?? null })

  it('h264 + aac → mp4 无损封装', () => {
    const t = autoDetectTarget(probe('h264', 'aac'))
    expect(t.format).toBe('mp4')
    expect(t.video).toBe('copy')
    expect(t.audio).toBe('copy')
    expect(t.remux).toBe(true)
  })

  it('vp9 + opus → webm 无损封装', () => {
    const t = autoDetectTarget(probe('vp9', 'opus'))
    expect(t.format).toBe('webm')
    expect(t.video).toBe('copy')
  })

  it('纯无损音频 → flac 提取', () => {
    const t = autoDetectTarget(probe(undefined, 'flac'))
    expect(t.format).toBe('flac')
    expect(t.extractAudio).toBe(true)
  })

  it('未知视频编码 → 重编码 mp4', () => {
    const t = autoDetectTarget(probe('mpeg2video', 'pcm_s16le'))
    expect(t.format).toBe('mp4')
    expect(t.video).toBe('libx264')
    expect(t.audio).toBe('aac')
    expect(t.remux).toBe(false)
  })

  it('probeKind 判定源类型', () => {
    expect(probeKind(probe('h264', 'aac'))).toBe('video')
    expect(probeKind(probe(undefined, 'mp3'))).toBe('audio')
    expect(probeKind(null)).toBe('unknown')
  })
})
