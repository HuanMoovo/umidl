/**
 * 格式目录可用性过滤（纯逻辑，不依赖真机）
 *
 * 转换页的唯一口径来源就是 availableOnly()：`available === false` 的条目整条消失，
 * total / count 按可用项重算；可用性字段本身仍保留在数据结构里。
 * 另有一条用真机 convert_formats 夹具（%APPDATA%/umi-downloader/convproof/catalog.json）
 * 校准的口径断言：88 条 → 80 条。
 */
import { describe, expect, it } from 'vitest'
import fs from 'node:fs'
import path from 'node:path'
import {
  FALLBACK_CATALOG,
  availableOnly,
  flatten,
  normalizeCatalog,
  type ConvertFormatCatalog,
} from '@/services/formatCatalog'

const FIXTURE = path.join(process.env.APPDATA || '', 'umi-downloader', 'convproof', 'catalog.json')
const live: any = fs.existsSync(FIXTURE) ? JSON.parse(fs.readFileSync(FIXTURE, 'utf8')) : null

/** 手搓小目录：2 可用 + 1 不可用，覆盖 count / total 重算 */
const SAMPLE: ConvertFormatCatalog = {
  total: 3,
  engines: { ffmpeg: true, pandoc: false },
  groups: [
    {
      id: 'video',
      label: '视频',
      count: 2,
      formats: [
        { id: 'mp4', label: 'MP4', available: true, input: true, engines: ['ffmpeg'], note: 'H.264 / AAC' },
        { id: 'ape', label: 'APE', available: false, input: true, engines: ['ffmpeg'], note: null },
      ],
    },
    {
      id: 'document',
      label: '文档',
      count: 1,
      formats: [
        { id: 'pdf', label: 'PDF', available: true, input: false, engines: ['poppler'], note: null },
        { id: 'xls', label: 'XLS', available: false, input: true, engines: ['pandoc'], note: null },
      ],
    },
  ],
}

describe('availableOnly（只保留本机能写出的格式）', () => {
  it('不可用条目整条剔除，count / total 按可用项重算', () => {
    const out = availableOnly(SAMPLE)
    expect(out.groups.map((g) => g.id)).toEqual(['video', 'document'])
    expect(out.groups[0].formats.map((f) => f.id)).toEqual(['mp4'])
    expect(out.groups[1].formats.map((f) => f.id)).toEqual(['pdf'])
    expect(out.groups.map((g) => g.count)).toEqual([1, 1])
    expect(out.total).toBe(2)
  })

  it('不修改原目录（内部分组 / total 保持原样）', () => {
    availableOnly(SAMPLE)
    expect(SAMPLE.total).toBe(3)
    expect(SAMPLE.groups[0].formats.map((f) => f.id)).toEqual(['mp4', 'ape'])
  })

  it('可用性字段本身仍在（只是不再进界面），过滤后全部为 true', () => {
    const out = availableOnly(SAMPLE)
    for (const f of flatten(out.groups)) expect(f.available).toBe(true)
    expect(flatten(out.groups).length).toBe(2)
  })

  it('浏览器预览的兜底目录不携带不可用条目（保持全部展示）', () => {
    const out = availableOnly(FALLBACK_CATALOG)
    expect(out.total).toBe(FALLBACK_CATALOG.total)
    expect(flatten(out.groups).every((f) => f.available)).toBe(true)
    expect(flatten(FALLBACK_CATALOG.groups).some((f) => !f.available)).toBe(false)
  })
})

describe.skipIf(!live)('真机目录口径（convert_formats 夹具）', () => {
  it('88 条 → 80 条可用，且分组计数与后端一致', () => {
    const catalog = normalizeCatalog(live)
    const out = availableOnly(catalog)
    const groups: Record<string, number> = {}
    for (const g of out.groups) groups[g.id] = g.formats.length
    console.log('[真机目录]', catalog.total, '→', out.total, JSON.stringify(groups))

    expect(catalog.total).toBe(88)
    expect(out.total).toBe(80)
    expect(groups).toEqual({ video: 22, audio: 20, image: 20, document: 18 })
    expect(out.groups.reduce((n, g) => n + g.count, 0)).toBe(80)
    expect(flatten(out.groups).every((f) => f.available)).toBe(true)
  })
})
