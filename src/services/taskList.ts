/**
 * 队列写入纪律：同一条任务在列表里只允许存在一份。
 *
 * 背景（实机复现过的 bug）：点一次「开始下载」/「开始转换」后，后端在命令返回前
 * 就已经 emit 过 `download://update`（事件里带着更新后的状态），前端拿到事件把任务
 * upsert 进了列表；紧接着命令返回值又 `unshift` 了同一 id 的「入队瞬间快照」，
 * 于是队列里出现两条一模一样的卡片，其中一条永远停在旧状态（例如「解析中 0%」）。
 *
 * 规则：缺行才插入；已经有同 id 的行就原样保留事件带来的（更新）那份。
 */
export function enqueueOnce<T extends { id: string }>(list: T[], item: T | null | undefined): T[] {
  if (!item || !item.id) return list
  if (list.some((x) => x.id === item.id)) return list
  list.unshift(item)
  return list
}

/* ==================== 删除纪律 ==================== */

/**
 * 后端确认删除成功后，把这条任务从列表里摘掉。
 *
 * 背景（实机复现过的 bug）：字幕页 / 转换页的行内「删除记录」只 `await` 了后端命令，
 * 之后既不刷 store 也不监听事件 —— 库里 9 → 8 条（`list_subtitles` 真的少了），
 * `store.subtitles` 还是 9 条、DOM 还是 9 行，切页回来依旧，只有重启客户端才消失。
 *
 * 规则：**后端返回 ok 之后**才调这里摘行；后端报错就一行都不要动（用户得看见失败）。
 * 相同 id 在别的队列里不存在时天然空转，所以同一条任务在多张表里调用是安全的。
 */
export function dropById<T extends { id: string }>(list: T[], id: string | null | undefined): T[] {
  if (!id) return list
  const i = list.findIndex((x) => x.id === id)
  if (i >= 0) list.splice(i, 1)
  return list
}

/**
 * 按条件批量摘行（「清理已完成」/「清空队列」用）。
 * 谓词在调用侧定义，这里只负责原地删除，便于单测。
 */
export function dropWhere<T>(list: T[], shouldDrop: (t: T) => boolean): T[] {
  for (let i = list.length - 1; i >= 0; i--) if (shouldDrop(list[i])) list.splice(i, 1)
  return list
}

/**
 * 下游下载入口的「终态」口径，与后端 `clear_downloads(which == "done")` 的
 * `status IN ('done','error','canceled')` 严格一致：
 * 清理已完成时前端摘掉的行，必须正好是后端删掉的行。
 */
export const FINISHED_DOWNLOAD_STATUSES = ['done', 'error', 'canceled'] as const

export function isFinishedDownload(t: { status: string } | null | undefined): boolean {
  if (!t) return false
  return (FINISHED_DOWNLOAD_STATUSES as readonly string[]).includes(t.status)
}
