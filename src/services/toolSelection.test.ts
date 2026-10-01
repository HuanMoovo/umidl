/**
 * 「按需勾选下载」纯逻辑单测：勾选集 → 待装列表 / 汇总 / 按钮语义。
 * 这些规则直接决定点「下载所选」会装哪几个工具，必须可测。
 */
import { describe, expect, it } from 'vitest'
import {
  busyNames,
  installActionLabel,
  normalizeSelection,
  pendingInstalls,
  selectableTools,
  selectionFor,
  selectionSummary,
  statusLine,
  summarizeOutcomes,
} from '@/services/toolSelection'
import type { ToolStatus } from '@/types'

function st(name: string, found: boolean, extra: Partial<ToolStatus> = {}): ToolStatus {
  return {
    name,
    found,
    source: found ? 'managed' : 'missing',
    hint: '',
    installable: true,
    ...extra,
  }
}

/** 真实清单（与后端 TOOL_ORDER 一致）：ffprobe / imagemagick 没有自动下载包 */
const STATUSES: ToolStatus[] = [
  st('yt-dlp', true, { version: '2025.09.26', size_hint: '约 17 MB' }),
  st('ffmpeg', true, { version: '8.0.1', size_hint: '约 80 MB' }),
  st('ffprobe', true, { installable: false, size_hint: '随 ffmpeg' }),
  st('whisper', false, { size_hint: '约 22 MB' }),
  st('aria2', false, { size_hint: '约 5 MB' }),
  st('pandoc', false, { size_hint: '约 35 MB' }),
  st('poppler', false, { size_hint: '约 42 MB' }),
  st('emule', false, { size_hint: '约 4 MB' }),
  st('imagemagick', false, { installable: false }),
  st('whisper-model', true, { size_hint: '75 MB ~ 1.6 GB' }),
]

describe('依赖工具：勾选与下载', () => {
  it('只有可自动安装的工具能勾选（ffprobe / imagemagick 不在列）', () => {
    const names = selectableTools(STATUSES).map((t) => t.name)
    expect(names).toEqual(['yt-dlp', 'ffmpeg', 'whisper', 'aria2', 'pandoc', 'poppler', 'emule', 'whisper-model'])
    expect(names).not.toContain('ffprobe')
    expect(names).not.toContain('imagemagick')
  })

  it('勾选集规范化：去空白 / 去重 / 丢弃不可勾选的项', () => {
    expect(normalizeSelection([' ffmpeg ', 'ffmpeg', '', 'ffprobe', 'whisper'], STATUSES)).toEqual([
      'ffmpeg',
      'whisper',
    ])
  })

  it('待装列表：未装的排在前面（先下真正缺的），已装的排在后面（重装语义）', () => {
    expect(pendingInstalls(['yt-dlp', 'whisper', 'aria2'], STATUSES)).toEqual([
      'whisper',
      'aria2',
      'yt-dlp',
    ])
  })

  it('汇总：总数 / 已装 / 待装对得上', () => {
    expect(selectionSummary(['yt-dlp', 'whisper', 'aria2', 'ffmpeg'], STATUSES)).toEqual({
      total: 4,
      installed: 2,
      missing: 2,
    })
    expect(selectionSummary([], STATUSES)).toEqual({ total: 0, installed: 0, missing: 0 })
  })

  it('批量入口：全选 / 只选未装 / 清空', () => {
    expect(selectionFor(STATUSES, 'all')).toHaveLength(8)
    expect(selectionFor(STATUSES, 'missing')).toEqual([
      'whisper',
      'aria2',
      'pandoc',
      'poppler',
      'emule',
    ])
    expect(selectionFor(STATUSES, 'none')).toEqual([])
  })

  it('单个按钮语义：已装 = 重装，未装 = 下载', () => {
    expect(installActionLabel('yt-dlp', STATUSES)).toBe('reinstall')
    expect(installActionLabel('aria2', STATUSES)).toBe('download')
    expect(installActionLabel('不存在的工具', STATUSES)).toBe('download')
  })

  it('状态行：已装给版本号，未装给体积估计', () => {
    expect(statusLine(STATUSES[0])).toEqual({ installed: true, version: '2025.09.26', sizeHint: '约 17 MB' })
    expect(statusLine(STATUSES[3])).toEqual({ installed: false, version: '', sizeHint: '约 22 MB' })
  })

  it('批量结果汇总：成功数与失败名单', () => {
    const r = summarizeOutcomes([
      { name: 'ffmpeg', ok: true },
      { name: 'whisper', ok: false, error: '[tools.install_failed] whisper :: boom' },
      { name: 'aria2', ok: true },
    ])
    expect(r.ok).toBe(2)
    expect(r.failed).toEqual(['whisper'])
    expect(summarizeOutcomes([])).toEqual({ ok: 0, failed: [] })
  })

  it('忙碌判定：批量安装中所有行都禁用，单装只禁自己', () => {
    expect(busyNames(['ffmpeg'], null)).toBe(false)
    expect(busyNames(['ffmpeg'], '__batch__')).toBe(true)
    expect(busyNames(['ffmpeg'], 'ffmpeg')).toBe(true)
    expect(busyNames(['ffmpeg'], 'aria2')).toBe(false)
  })
})
