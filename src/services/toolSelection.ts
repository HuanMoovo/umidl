/**
 * 依赖工具的「按需勾选下载」纯逻辑（不碰 IPC / DOM，便于单测）。
 *
 * 设置 › 引擎 › 依赖工具：
 *   每行一个勾选框 + 状态（已装·版本 / 未装·体积估计）+ 单独的「下载」/「重装」；
 *   顶部「下载所选」把勾选集一次性交给后端 install_tools（逐条返回结果）。
 *
 * 这里只做「勾选集 → 待装列表 / 汇总」的推导，安装本身由后端负责。
 */
import type { ToolInstallOutcome, ToolStatus } from '@/types'

/** 可勾选（= 本平台可自动下载安装）的工具；后端 ToolStatus.installable 为准 */
export function selectableTools(statuses: ToolStatus[]): ToolStatus[] {
  return statuses.filter((t) => t.installable === true)
}

/** 勾选集 → 规范化：去空白、去重、只保留当前可勾选的工具（保序） */
export function normalizeSelection(selected: string[], statuses: ToolStatus[]): string[] {
  const allowed = new Set(selectableTools(statuses).map((t) => t.name))
  const out: string[] = []
  for (const raw of selected) {
    const name = raw.trim()
    if (!name || !allowed.has(name) || out.includes(name)) continue
    out.push(name)
  }
  return out
}

/**
 * 勾选集 → 待装列表：勾了但**还没装**的工具在前（真正要下载的），
 * 已装的在后（属于「重装」语义，后端同样会重新下载覆盖）。
 */
export function pendingInstalls(selected: string[], statuses: ToolStatus[]): string[] {
  const names = normalizeSelection(selected, statuses)
  const installed = new Set(statuses.filter((t) => t.found).map((t) => t.name))
  return [...names.filter((n) => !installed.has(n)), ...names.filter((n) => installed.has(n))]
}

/** 勾选集合计：总数 / 其中已装 / 其中待装 */
export function selectionSummary(
  selected: string[],
  statuses: ToolStatus[],
): { total: number; installed: number; missing: number } {
  const names = normalizeSelection(selected, statuses)
  const installed = new Set(statuses.filter((t) => t.found).map((t) => t.name))
  const have = names.filter((n) => installed.has(n)).length
  return { total: names.length, installed: have, missing: names.length - have }
}

/** 全选 / 全不选 / 只选未装 */
export function selectionFor(
  statuses: ToolStatus[],
  mode: 'all' | 'missing' | 'none',
): string[] {
  const list = selectableTools(statuses)
  if (mode === 'none') return []
  if (mode === 'missing') return list.filter((t) => !t.found).map((t) => t.name)
  return list.map((t) => t.name)
}

/** 单个工具的安装按钮文案：已装 = 重装，未装 = 下载 */
export function installActionLabel(name: string, statuses: ToolStatus[]): 'download' | 'reinstall' {
  const st = statuses.find((t) => t.name === name)
  return st?.found ? 'reinstall' : 'download'
}

/** 状态行文案的数据来源：已装给版本号，未装给体积估计 */
export function statusLine(t: ToolStatus): { installed: boolean; version: string; sizeHint: string } {
  return {
    installed: !!t.found,
    version: (t.version || '').trim(),
    sizeHint: (t.size_hint || '').trim(),
  }
}

/** 批量结果汇总：成功数 / 失败名单 */
export function summarizeOutcomes(outcomes: ToolInstallOutcome[]): {
  ok: number
  failed: string[]
} {
  const failed = outcomes.filter((o) => !o.ok).map((o) => o.name)
  return { ok: outcomes.length - failed.length, failed }
}

/** 是否还有工具在装（界面据此禁用按钮 / 显示进度） */
export function busyNames(names: string[], installing: string | null): boolean {
  if (!installing) return false
  return installing === '__batch__' || names.includes(installing)
}
