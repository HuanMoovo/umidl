/**
 * 插件系统（1.4 新增）：沙箱信息 / 已装插件 / 内容寻址市场 / 从 URL 安装 / 事件日志 的薄封装。
 *
 * 命令名由后端固定；plugin://event 与 plugin://installed 采用宽松 payload 解析。
 * 非 Tauri 环境（浏览器预览）一律抛出可读错误，由页面用 isTauri() 守卫降级。
 */
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { isTauri } from './ipc'

export interface PluginSandboxInfo {
  available: boolean
  engine: string
  version: string
  memory_limit_mb: number
  script_timeout_ms: number
}

export interface PluginInfo {
  id: string
  name: string
  version: string
  description: string
  enabled: boolean
  sha256: string
  source: string
  installed_at: string
}

export interface PluginMarketEntry {
  id: string
  name: string
  version: string
  description: string
  sha256: string
  size: number
}

export interface PluginTestResult {
  ok: boolean
  logs: string[]
  result?: unknown
  error?: string | null
}

/** plugin://event 的宽松 payload：字段可能缺省 */
export interface PluginEventPayload {
  id?: string
  event: string
  level?: string
  message: string
}

function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) throw new Error(`[browser] 命令 ${cmd} 需要在桌面客户端中运行`)
  return invoke<T>(cmd, args)
}

export const pluginSandboxInfo = () => call<PluginSandboxInfo>('plugin_sandbox_info')
export const pluginList = () => call<PluginInfo[]>('plugin_list')
/**
 * 市场索引：后端返回 { version, generated_at, description, plugins: [...] }（对象），
 * 这里统一成数组给页面用 —— 曾经因为按数组解析，市场在界面上永远是空的（BUG）。
 * 兼容直接返回数组的老形状。
 */
export interface PluginMarketIndex {
  version?: number
  generated_at?: string
  description?: string
  plugins?: PluginMarketEntry[]
}

export async function pluginMarketList(): Promise<PluginMarketEntry[]> {
  const res = await call<PluginMarketEntry[] | PluginMarketIndex>('plugin_market_list')
  if (Array.isArray(res)) return res
  return Array.isArray(res?.plugins) ? res.plugins : []
}
export const pluginInstall = (id: string) => call<unknown>('plugin_install', { id })
/**
 * 从 GitHub 仓库 / https 直链安装插件（后端负责：仅 https、≤ 5 MB、超时 60 s、
 * 只写入现有插件目录、下载内容不执行；错误信息为可读中文，直接展示即可）。
 */
export const pluginInstallFromUrl = (url: string) => call<PluginInfo>('plugin_install_from_url', { url })
export const pluginUninstall = (id: string) => call<unknown>('plugin_uninstall', { id })
export const pluginSetEnabled = (id: string, enabled: boolean) =>
  call<unknown>('plugin_set_enabled', { id, enabled })
export const pluginTest = (id: string) => call<PluginTestResult>('plugin_test', { id })
export const pluginRunResolvers = (url: string) => call<unknown>('plugin_run_resolvers', { url })

/**
 * 订阅 `plugin://event`（日志）与 `plugin://installed`（列表变化）。
 * 浏览器预览下返回空订阅，调用方无需特判。
 */
export function onPluginEvent(handler: (p: PluginEventPayload) => void): Promise<UnlistenFn> {
  if (!isTauri()) return Promise.resolve(() => {})
  return listen<PluginEventPayload>('plugin://event', (e) => {
    const raw = (e.payload ?? {}) as Partial<PluginEventPayload>
    handler({
      id: typeof raw.id === 'string' ? raw.id : undefined,
      event: String(raw.event ?? ''),
      level: typeof raw.level === 'string' ? raw.level : undefined,
      message: String(raw.message ?? ''),
    })
  })
}

export function onPluginInstalled(handler: (payload: unknown) => void): Promise<UnlistenFn> {
  if (!isTauri()) return Promise.resolve(() => {})
  return listen<unknown>('plugin://installed', (e) => handler(e.payload))
}

/** 校验两个 sha256 是否一致（列表/市场摘要比对，忽略大小写与空白） */
export function sameSha(a?: string | null, b?: string | null): boolean {
  const norm = (s: string) => String(s || '').trim().toLowerCase()
  const x = norm(a ?? '')
  const y = norm(b ?? '')
  return !!x && x === y
}

/** 摘要太长时截断显示 */
export function shortSha(sha?: string | null, head = 10, tail = 6): string {
  const s = String(sha || '').trim()
  if (s.length <= head + tail + 1) return s
  return `${s.slice(0, head)}…${s.slice(-tail)}`
}
