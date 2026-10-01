import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { ConvertTask, DownloadTask, SubtitleTask, TestCase, TestReport, ToolProgress, ToolStatus } from '@/types'
import * as ipc from '@/services/ipc'
import { enqueueOnce, dropById, dropWhere, isFinishedDownload } from '@/services/taskList'

export const useTaskStore = defineStore('tasks', () => {
  /** 工具刷新的前端兜底超时（后端已有探活超时 + 总预算，这里保证界面一定解锁） */
  const TOOLS_REFRESH_TIMEOUT_MS = 20_000
  const downloads = ref<DownloadTask[]>([])
  const converts = ref<ConvertTask[]>([])
  const subtitles = ref<SubtitleTask[]>([])
  const tools = ref<ToolStatus[]>([])
  const selftest = ref<TestReport | null>(null)
  const selftestCases = ref<TestCase[]>([])
  const testCatalog = ref<{ id: string; group: string; name: string; heavy: boolean }[]>([])
  const toolProgress = ref<Record<string, ToolProgress>>({})
  const ready = ref(false)

  /* ---------- 派生 ---------- */
  const activeDownloads = computed(() =>
    downloads.value.filter((t) => ['pending', 'parsing', 'downloading'].includes(t.status)),
  )
  const activeConverts = computed(() => converts.value.filter((t) => t.status === 'converting'))
  const activeSubtitles = computed(() =>
    subtitles.value.filter((t) => ['extracting', 'transcribing'].includes(t.status)),
  )
  const activeCount = computed(
    () => activeDownloads.value.length + activeConverts.value.length + activeSubtitles.value.length,
  )
  const toolsReady = computed(() => {
    const need = ['yt-dlp', 'ffmpeg']
    return need.every((n) => tools.value.find((t) => t.name === n)?.found)
  })
  const whisperReady = computed(() => {
    const w = tools.value.find((t) => t.name === 'whisper')?.found
    const m = tools.value.find((t) => t.name === 'whisper-model')?.found
    return !!w && !!m
  })

  /* ---------- 操作 ---------- */
  function upsert<T extends { id: string }>(list: T[], item: T) {
    const i = list.findIndex((x) => x.id === item.id)
    if (i >= 0) list[i] = item
    else list.unshift(item)
  }

  /**
   * 入队后立刻上屏（视图调用命令返回值时用这里，别直接 `unshift`）。
   *
   * 后端在命令返回前就 emit 过 `download://update`，事件已经 upsert 了同一条任务；
   * 视图再 unshift 一次命令返回值，队列里就会出现两条一样的卡片（其中一条停在
   * 旧状态）。enqueueOnce 只在缺行时补一条。
   */
  function enqueueDownload(t: DownloadTask) {
    enqueueOnce(downloads.value, t)
  }
  function enqueueConvert(t: ConvertTask) {
    enqueueOnce(converts.value, t)
  }
  function enqueueSubtitle(t: SubtitleTask) {
    enqueueOnce(subtitles.value, t)
  }

  /**
   * 删除记录的统一出口：后端 `remove_download` / `remove_convert` / `remove_subtitle`
   * 返回 ok 之后，视图调这里把任务从三条队列里摘掉。
   *
   * 视图**不要**在删除后 `reload()` 重拉全量（那是把三条队列都重新读一遍的粗暴做法），
   * 也不许只等后端事件 —— 转换 / 字幕后端根本没有 `*://removed` 事件，只等事件就会
   * 「库里删了、界面还在」（实机复现过的 bug）。后端报错时不要调用本函数。
   */
  function dropTask(id: string | null | undefined) {
    dropById(downloads.value, id)
    dropById(converts.value, id)
    dropById(subtitles.value, id)
  }

  /**
   * 「清理已完成 / 清空队列」的本地摘行，与后端 `clear_downloads(which)` 同口径：
   * `done` = 只摘终态行（done / error / canceled / handed_off），`all` = 整列清空。
   */
  function dropDownloads(which: 'done' | 'all') {
    dropWhere(downloads.value, which === 'all' ? () => true : isFinishedDownload)
  }

  let booting: Promise<void> | null = null
  async function bootstrap() {
    if (ready.value) return
    if (booting) return booting // 多个视图可能同时触发，复用同一次初始化
    booting = (async () => {
      const t0 = performance.now()
      try {
        const [d, c, s] = await Promise.all([
          ipc.listDownloads(),
          ipc.listConverts(),
          ipc.listSubtitles(),
        ])
        downloads.value = d
        converts.value = c
        subtitles.value = s
      } catch (e) {
        console.warn('初始化数据失败', e)
      }
      const tData = performance.now() - t0
      // 事件订阅（并行注册，减少启动等待）
      await Promise.all([
        ipc.onEvent('download://update', (t) => upsert(downloads.value, t)),
        // 删除/清理后把卡片从列表移除，否则界面看起来"删了没反应"
        // （视图侧的删除也走同一个出口 store.dropTask / dropDownloads）
        ipc.onEvent('download://removed', (p) => {
          if (p?.all) {
            dropDownloads('all')
          } else if (p?.id) {
            dropById(downloads.value, p.id)
          }
        }),
        ipc.onEvent('convert://update', (t) => upsert(converts.value, t)),
        ipc.onEvent('subtitle://update', (t) => upsert(subtitles.value, t)),
        ipc.onEvent('selftest://update', (c) => upsert(selftestCases.value, c)),
        ipc.onEvent('tool://progress', (p) => {
          toolProgress.value = {
            ...toolProgress.value,
            [p.name]: {
              percent: p.percent,
              message: p.message,
              source: p.source,
              downloaded: p.downloaded,
              total: p.total,
              resumed: p.resumed,
            },
          }
        }),
      ])
      invoke('frontend_log', {
        msg: `bootstrap 明细：数据 ${tData.toFixed(0)}ms · 事件订阅 ${(performance.now() - t0 - tData).toFixed(0)}ms`,
      }).catch(() => {})
      ready.value = true
      // 工具检测首次要启动子进程探测版本（约 4s），放后台完成，不阻塞首屏
      void ipc
        .detectTools()
        .then((t) => {
          tools.value = t
          invoke('frontend_log', { msg: `工具检测回来（+${performance.now().toFixed(0)}ms）` }).catch(() => {})
        })
        .catch(() => {})
      // 历史任务补封面（从本地视频抽帧）：完成后通过 download://update 事件自动刷新卡片
      void ipc
        .backfillCovers()
        .then((n) => {
          if (n > 0) {
            invoke('frontend_log', { msg: `已为 ${n} 条历史任务补齐封面` }).catch(() => {})
          }
        })
        .catch(() => {})
    })()
    return booting
  }

  async function refreshTools() {
    // 前端兜底超时：后端探活已有超时 + 总预算，这里再压一道 ——
    // 万一某次调用久不返回，界面也不能永久停在「处理中…」（按钮禁用、只能 kill 程序）
    let timer: ReturnType<typeof setTimeout> | null = null
    const timeout = new Promise<ToolStatus[]>((resolve) => {
      timer = setTimeout(() => {
        invoke('frontend_log', {
          msg: `工具刷新超过 ${TOOLS_REFRESH_TIMEOUT_MS / 1000}s 未返回，先按上次结果解锁界面`,
        }).catch(() => {})
        resolve(tools.value)
      }, TOOLS_REFRESH_TIMEOUT_MS)
    })
    const done = ipc
      .detectTools()
      .then((t) => {
        tools.value = t
        return tools.value
      })
      .catch((e) => {
        invoke('frontend_log', { msg: `工具刷新失败：${String(e)}` }).catch(() => {})
        return tools.value
      })
    try {
      return await Promise.race([done, timeout])
    } finally {
      if (timer) clearTimeout(timer)
    }
  }

  async function reload() {
    downloads.value = await ipc.listDownloads()
    converts.value = await ipc.listConverts()
    subtitles.value = await ipc.listSubtitles()
  }

  async function loadTestCatalog() {
    if (testCatalog.value.length) return testCatalog.value
    const raw = await ipc.listTestCases()
    testCatalog.value = (raw as any[]).map((r) =>
      Array.isArray(r)
        ? { id: r[0], group: r[1], name: r[2], heavy: r[3] }
        : (r as any),
    )
    return testCatalog.value
  }

  async function runSelftest(deep: boolean) {
    selftestCases.value = []
    selftest.value = await ipc.runSelftest(deep)
    return selftest.value
  }

  return {
    downloads,
    converts,
    subtitles,
    tools,
    selftest,
    selftestCases,
    testCatalog,
    toolProgress,
    ready,
    activeDownloads,
    activeConverts,
    activeSubtitles,
    activeCount,
    toolsReady,
    whisperReady,
    enqueueDownload,
    enqueueConvert,
    enqueueSubtitle,
    dropTask,
    dropDownloads,
    bootstrap,
    refreshTools,
    reload,
    loadTestCatalog,
    runSelftest,
  }
})
