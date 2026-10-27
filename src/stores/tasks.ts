import { defineStore } from 'pinia'
import { computed, onScopeDispose, ref, type Ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { ConvertTask, DownloadTask, SubtitleTask, TestCase, TestReport, ToolProgress, ToolStatus } from '@/types'
import * as ipc from '@/services/ipc'
import { enqueueOnce, dropById, dropWhere, isFinishedDownload } from '@/services/taskList'

const STARTUP_TIMEOUT_MS = 10_000

function startupWait<T>(request: Promise<T>, timeoutMs: number) {
  let timer: ReturnType<typeof setTimeout>
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error('Startup operation timed out')), timeoutMs)
  })
  return Promise.race([request, timeout]).finally(() => clearTimeout(timer))
}

/** 快照读回时重放期间发生的事件/本地操作，避免进度回退或已删任务重新出现。 */
function taskQueue<T>(list: Ref<T[]>, fetch: () => Promise<T[]>) {
  let changes: ((items: T[]) => void)[] | null = null
  let loading: Promise<void> | null = null
  let loaded = false

  function change(apply: (items: T[]) => void) {
    apply(list.value)
    changes?.push(apply)
  }

  function load(timeoutMs = STARTUP_TIMEOUT_MS) {
    if (loading) return loading
    loaded = false
    const pending = changes ?? []
    changes = pending
    loading = (async () => {
      try {
        const snapshot = await startupWait(Promise.resolve().then(fetch), timeoutMs)
        pending.forEach((apply) => apply(snapshot))
        list.value = snapshot
        loaded = true
      } finally {
        changes = null
        loading = null
      }
    })()
    return loading
  }

  return { change, load, capture: () => { changes ??= [] }, get loaded() { return loaded } }
}

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
  const startupFailed = ref(false)
  const downloadQueue = taskQueue(downloads, () => ipc.listDownloads())
  const convertQueue = taskQueue(converts, () => ipc.listConverts())
  const subtitleQueue = taskQueue(subtitles, () => ipc.listSubtitles())
  const queues = [downloadQueue, convertQueue, subtitleQueue]

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
    downloadQueue.change((list) => enqueueOnce(list, t))
  }
  function enqueueConvert(t: ConvertTask) {
    convertQueue.change((list) => enqueueOnce(list, t))
  }
  function enqueueSubtitle(t: SubtitleTask) {
    subtitleQueue.change((list) => enqueueOnce(list, t))
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
    downloadQueue.change((list) => dropById(list, id))
    convertQueue.change((list) => dropById(list, id))
    subtitleQueue.change((list) => dropById(list, id))
  }

  /**
   * 「清理已完成 / 清空队列」的本地摘行，与后端 `clear_downloads(which)` 同口径：
   * `done` = 只摘终态行（done / error / canceled），`all` = 整列清空。
   */
  function dropDownloads(which: 'done' | 'all') {
    downloadQueue.change((list) => dropWhere(list, which === 'all' ? () => true : isFinishedDownload))
  }

  const subscriptions = [
    { event: 'download://update', queue: downloadQueue, listen: () => ipc.onEvent('download://update', (t) => downloadQueue.change((list) => upsert(list, t))) },
    {
      event: 'download://removed',
      queue: downloadQueue,
      listen: () => ipc.onEvent('download://removed', (p) => {
        if (p?.all) dropDownloads('all')
        else if (p?.id) downloadQueue.change((list) => dropById(list, p.id))
      }),
    },
    { event: 'convert://update', queue: convertQueue, listen: () => ipc.onEvent('convert://update', (t) => convertQueue.change((list) => upsert(list, t))) },
    { event: 'subtitle://update', queue: subtitleQueue, listen: () => ipc.onEvent('subtitle://update', (t) => subtitleQueue.change((list) => upsert(list, t))) },
    { event: 'selftest://update', listen: () => ipc.onEvent('selftest://update', (c) => upsert(selftestCases.value, c)) },
    {
      event: 'tool://progress',
      listen: () => ipc.onEvent('tool://progress', (p) => {
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
    },
  ]
  const unlisteners = new Map<string, () => void>()
  let disposed = false
  onScopeDispose(() => {
    disposed = true
    unlisteners.forEach((unlisten) => unlisten())
    unlisteners.clear()
  })

  let booting: Promise<void> | null = null
  let backgroundStarted = false
  async function bootstrap() {
    if (booting) return booting // 多个视图可能同时触发，复用同一次初始化
    if (disposed || (ready.value && queues.every((queue) => queue.loaded) && unlisteners.size === subscriptions.length)) return
    const attempt = (async () => {
      const t0 = performance.now()
      const deadline = Date.now() + STARTUP_TIMEOUT_MS
      const remaining = () => Math.max(0, deadline - Date.now())
      const pendingSubscriptions = subscriptions.filter(({ event }) => !unlisteners.has(event))
      const pendingQueues = new Set(queues.filter((queue) => !queue.loaded))
      // 缺失监听期间可能错过完成/删除事件；重连后同步对应队列，补齐这段空窗。
      if (ready.value) pendingSubscriptions.forEach(({ queue }) => { if (queue) pendingQueues.add(queue) })
      pendingQueues.forEach((queue) => queue.capture())
      // 先订阅再读取快照；失败只影响当前项目，下次 bootstrap 只重试缺失项。
      const subscriptionResults = await Promise.allSettled(
        pendingSubscriptions.map(async ({ event, listen }) => {
          let active = true
          const registration = listen().then((unlisten) => {
            if (!active || disposed) unlisten()
            else unlisteners.set(event, unlisten)
          })
          try {
            await startupWait(registration, remaining())
          } catch (e) {
            // 原生注册不能取消；超时后若迟到，立即撤掉，避免重试留下双份监听。
            active = false
            throw e
          }
        }),
      )
      subscriptionResults.forEach((result) => {
        if (result.status === 'rejected') console.warn('初始化事件订阅失败', result.reason)
      })
      const tSubscriptions = performance.now() - t0
      const dataResults = await Promise.allSettled([...pendingQueues].map((queue) => queue.load(remaining())))
      dataResults.forEach((result) => {
        if (result.status === 'rejected') console.warn('初始化数据失败', result.reason)
      })
      invoke('frontend_log', {
        msg: `bootstrap 明细：事件订阅 ${tSubscriptions.toFixed(0)}ms · 数据 ${(performance.now() - t0 - tSubscriptions).toFixed(0)}ms`,
      }).catch(() => {})
      startupFailed.value = !queues.every((queue) => queue.loaded) || unlisteners.size !== subscriptions.length
      ready.value = true
      if (backgroundStarted || disposed) return
      backgroundStarted = true
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
    booting = attempt
    try {
      await attempt
    } finally {
      booting = null
    }
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
    await Promise.all(queues.map((queue) => queue.load()))
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
    startupFailed,
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
