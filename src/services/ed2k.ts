/**
 * ED2K（电驴）引擎（1.4 新增）：ed2k_engine_status / ed2k_parse / ed2k_submit 的薄封装。
 *
 * 真实行为：链接交给 eMule 引擎接管，下载进度在引擎自己的窗口里看（前端不做进度条）。
 * 非 Tauri 环境（浏览器预览）一律抛出可读错误，由页面用 isTauri() 守卫降级。
 */
import { invoke } from '@tauri-apps/api/core'
import { isTauri } from './ipc'

export interface Ed2kEngineStatus {
  engine: 'emule' | 'mlnet' | null
  path?: string | null
  installed: boolean
  running: boolean
  ready: boolean
  web_port?: number | null
  note: string
}

export interface Ed2kParsed {
  hash: string
  name: string
  size: number
  sources: number
  aich?: string | null
}

export interface Ed2kSubmitResult {
  engine: string
  started: boolean
  handoff: boolean
  web_port?: number | null
}

/** explain_route 的判定结果（下载页用来识别「这条链接归 eMule 引擎」） */
export interface RouteVerdict {
  engine: string
  engine_label: string
  blocked: boolean
  reason?: string | null
  /** 后端显式给出的 ed2k 判定（engine === 'ed2k' 时的补充标志） */
  ed2k?: boolean
}

function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) throw new Error(`[browser] 命令 ${cmd} 需要在桌面客户端中运行`)
  return invoke<T>(cmd, args)
}

/** 引擎状态：装了没有 / 在不在跑 / Web 端口 / 一句话说明 */
export const ed2kEngineStatus = () => call<Ed2kEngineStatus>('ed2k_engine_status')

/** 粘贴时即时解析：hash / 名称 / 大小 / 源数（+ AICH） */
export const ed2kParse = (link: string) => call<Ed2kParsed>('ed2k_parse', { link })

/** 把链接交给引擎（失败会抛错，错误原文由页面展示） */
export const ed2kSubmit = (link: string) => call<Ed2kSubmitResult>('ed2k_submit', { link })

/** 安装引擎（emule），进度沿用 tool://progress 事件 */
export const installEd2kEngine = () => call<unknown>('install_tool', { name: 'emule' })

/** 后端路由判定：这条链接会走哪个引擎（下载页只关心 engine === 'ed2k'） */
export const explainRoute = (url: string) => call<RouteVerdict>('explain_route', { url })

/** 是否是 ed2k 链接（避免每个字符都打后端） */
export function looksLikeEd2k(s: string): boolean {
  return /^ed2k:\/\/\S+/i.test(String(s || '').trim())
}
