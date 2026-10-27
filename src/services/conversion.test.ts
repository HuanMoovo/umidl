import { describe, expect, it } from 'vitest'
import { isDocPath } from './docEngine'
import { classifyInput, commonFormats, preferredTarget } from './conversion'
import { flatten, FALLBACK_CATALOG } from './formatCatalog'

describe('conversion choices', () => {
  it('keeps external document inputs recognized after native capabilities load', () => {
    for (const path of ['paper.PDF', 'book.epub', 'notes.rtf', 'notes.markdown', 'page.xhtml', 'notes.rst']) {
      expect(isDocPath(path, ['docx', 'xlsx', 'txt']), path).toBe(true)
    }
  })

  it('classifies an image separately from ffprobe video streams', () => {
    expect(classifyInput('C:/素材/照片.PNG')).toBe('image')
    expect(classifyInput('C:/素材/recording.M4A')).toBe('audio')
    expect(classifyInput('C:/素材/paper.pdf')).toBe('document')
    expect(classifyInput('C:/素材/movie.mkv')).toBe('video')
  })

  it('selects a suitable available default for the source', () => {
    expect(preferredTarget(['png', 'jpg', 'webp'], 'image', 'photo.jpg')).toBe('png')
    expect(preferredTarget(['txt', 'csv', 'html'], 'document', 'table.xlsx')).toBe('csv')
    expect(preferredTarget(['txt', 'png'], 'document', 'paper.pdf')).toBe('txt')
    expect(preferredTarget(['mkv'], 'video', 'film.mp4')).toBe('mkv')
    expect(preferredTarget([], 'video', 'film.mp4')).toBe('')
  })

  it('keeps a selected uncommon target visible in the common view', () => {
    const formats = flatten(FALLBACK_CATALOG.groups).filter(f => f.group === 'video')
    const ids = commonFormats(formats, 'video', 'mxf').map(f => f.id)
    expect(ids).toContain('mp4')
    expect(ids).toContain('mxf')
    expect(ids).not.toContain('dv')
    expect(ids.length).toBeLessThan(formats.length)
  })
})
