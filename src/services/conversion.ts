import type { MediaProbe } from '@/types'
import type { FlatFormat } from './formatCatalog'
import { isDocPath, normalizeExt } from './docEngine'

export type InputKind = 'video' | 'audio' | 'image' | 'document'

const IMAGE_EXTS = new Set('png jpg jpeg webp bmp tif tiff ico avif heic heif gif psd tga pnm ppm pgm pbm pam pcx exr xbm xwd dds svg'.split(' '))
const AUDIO_EXTS = new Set('mp3 m4a aac flac wav opus ogg wma ac3 aiff aif alac amr mka ape dts m4b mp2 au caf tta'.split(' '))
const COMMON: Record<InputKind, string[]> = {
  video: ['mp4', 'mkv', 'mov', 'webm', 'gif'],
  audio: ['mp3', 'm4a', 'flac', 'wav', 'opus'],
  image: ['png', 'jpg', 'webp', 'avif', 'tiff', 'ico'],
  document: ['txt', 'md', 'html', 'csv', 'docx', 'epub', 'png'],
}

export function classifyInput(path: string, probe?: MediaProbe | null): InputKind {
  if (isDocPath(path)) return 'document'
  const ext = normalizeExt(path.split(/[\\/]/).pop()?.split('.').pop() ?? '')
  if (IMAGE_EXTS.has(ext)) return 'image'
  if (AUDIO_EXTS.has(ext) || (probe && !probe.video_codec && !!probe.audio_codec)) return 'audio'
  return 'video'
}

export function preferredTarget(ids: string[], kind: InputKind, path: string): string {
  const ext = path.split('.').pop()?.toLowerCase()
  const priorities = kind === 'document'
    ? ['xlsx', 'ods', 'csv', 'tsv'].includes(ext ?? '') ? ['csv', 'txt', 'html'] : ['txt', 'md', 'html']
    : COMMON[kind]
  return priorities.find(id => ids.includes(id)) ?? ids[0] ?? ''
}

export function commonFormats(items: FlatFormat[], kind: InputKind, selected = ''): FlatFormat[] {
  const ids = new Set(COMMON[kind])
  if (kind === 'video') for (const id of ['mp3', 'm4a', 'png', 'jpg']) ids.add(id)
  const choices = items.filter(item => ids.has(item.id) || item.id === selected)
  return choices.length ? choices : items.slice(0, 6)
}
