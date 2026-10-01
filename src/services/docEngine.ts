/**
 * 文档转换（1.4 新增）：probe_document / doc_capabilities 的薄封装。
 *
 * 转换本身仍走现有 start_convert（后端按扩展名自动路由到文档引擎，不再走 ffmpeg），
 * 这里只负责「能力表 + 文档探测」两个只读命令。
 * 命令名由后端固定；非 Tauri 环境（浏览器预览）一律抛出可读错误，由调用方用 isTauri() 守卫降级。
 */
import { invoke } from '@tauri-apps/api/core'
import { isTauri } from './ipc'

export interface DocProbe {
  ok: boolean
  kind: string
  title: string
  pages?: number | null
  sheets?: number | null
  slides?: number | null
  chars?: number | null
  error?: string | null
}

export interface DocCapabilities {
  pandoc: boolean
  pandoc_path?: string | null
  poppler: boolean
  poppler_path?: string | null
  /** 后端原生可读的输入扩展名（不带点） */
  native: string[]
  /** 文档引擎可产出的目标格式 */
  targets: string[]
}

/** 后端能力表取不到时（浏览器预览 / 旧后端）的兜底扩展名，仅用于识别「这是个文档」 */
export const FALLBACK_DOC_EXTS = [
  'pdf', 'doc', 'docx', 'docm', 'odt', 'rtf', 'txt', 'md', 'html', 'htm', 'epub',
  'xls', 'xlsx', 'xlsm', 'ods', 'csv', 'tsv',
  'ppt', 'pptx', 'pps', 'ppsx', 'odp',
] as const

/** 后端能力表取不到时的兜底目标格式 */
export const FALLBACK_DOC_TARGETS = ['pdf', 'docx', 'md', 'html', 'txt', 'epub', 'odt', 'rtf'] as const

function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) throw new Error(`[browser] 命令 ${cmd} 需要在桌面客户端中运行`)
  return invoke<T>(cmd, args)
}

/** 探测文档：页数 / 工作表 / 幻灯片 / 字符数（供选择文件后预览） */
export const probeDocument = (path: string) => call<DocProbe>('probe_document', { path })

/** 文档能力表：外部引擎（pandoc / poppler）状态 + 原生输入扩展名 + 可产出目标格式 */
export const docCapabilities = () => call<DocCapabilities>('doc_capabilities')

/** 安装外部引擎（"pandoc" / "poppler"），进度沿用 tool://progress 事件 */
export const installDocEngine = (name: string) => call<unknown>('install_tool', { name })

/** 去掉前导点并小写 */
export function normalizeExt(ext: string): string {
  return String(ext || '').replace(/^\./, '').toLowerCase()
}

/** 扩展名是否属于「文档」（用后端能力表；取不到时退回兜底列表） */
export function isDocExt(ext: string, native?: string[] | null): boolean {
  const e = normalizeExt(ext)
  if (!e) return false
  const list = native && native.length ? native : FALLBACK_DOC_EXTS
  return list.map(normalizeExt).includes(e)
}

/** 路径是否为文档文件 */
export function isDocPath(path: string, native?: string[] | null): boolean {
  const base = String(path || '').split(/[\\/]/).pop() || ''
  const i = base.lastIndexOf('.')
  if (i < 0) return false
  return isDocExt(base.slice(i + 1), native)
}
