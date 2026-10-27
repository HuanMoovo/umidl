import { defineStore } from 'pinia'
import { computed, ref, watch } from 'vue'
import type { AppSettings, QueueLayout } from '@/types'
import * as ipc from '@/services/ipc'
import { normalizeAppearanceStyle, normalizeUiMode } from '@/utils/accent'
import { normalizeTheme } from '@/services/theme'

const STARTUP_TIMEOUT_MS = 10_000

function startupWait<T>(request: Promise<T>, timeoutMs: number) {
  let timer: ReturnType<typeof setTimeout>
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error('Settings load timed out')), timeoutMs)
  })
  return Promise.race([request, timeout]).finally(() => clearTimeout(timer))
}

/* ==================== 队列布局（固定 table） ==================== */
/**
 * 下载队列只有「详细列表」一种布局：卡片模式与切换按钮已从下载页移除。
 * queue_layout 字段保留仅为兼容历史设置 —— Rust 端 AppSettings 结构体暂未声明该字段，
 * save_settings 往返（反序列化 → 序列化）会把它丢掉，所以额外落一份本地镜像；
 * 但读取时不再认历史值，一律回落 DEFAULT_QUEUE_LAYOUT（table）。
 */
const QUEUE_LAYOUT_KEY = 'umi.queue_layout'

export const DEFAULT_QUEUE_LAYOUT: QueueLayout = 'table'

/**
 * 旧版本可能把 'card' 写进本地镜像：界面已没有卡片布局，
 * 这里不再读它，任何历史值都按 table 处理（避免又渲染出卡片流）。
 */
function readQueueLayout(): QueueLayout {
  return DEFAULT_QUEUE_LAYOUT
}

function writeQueueLayout(layout: QueueLayout) {
  try {
    localStorage.setItem(QUEUE_LAYOUT_KEY, layout)
  } catch {
    /* 无 localStorage（隐私模式 / 非浏览器环境）时忽略，内存里仍然生效 */
  }
}

export const FALLBACK_SETTINGS: AppSettings = {
  download_dir: '',
  ytdlp_path: null,
  ffmpeg_path: null,
  whisper_path: null,
  whisper_model: 'base',
  theme: 'dark',
  accent: 'violet',
  concurrency: 3,
  proxy: null,
  cookies_file: null,
  keep_original: true,
  /** 受管工具安装目录（空 = 数据目录下的 bin/）；后端为唯一来源 */
  tool_dir: '',
  default_container: 'mp4',
  default_audio_format: 'mp3',
  default_quality: 'best',
  filename_template: null,
  rate_limit: null,
  auto_start: true,
  open_folder_when_done: false,
  notify_on_finish: true,
  default_resolution: '原分辨率',
  default_crf: 23,
  auto_detect_format: true,
  hwaccel: false,
  subtitle_language: 'auto',
  subtitle_format: 'srt',
  animation: true,
  animation_quality: 'medium',
  /** 背景粒子形状：circle 圆形 | diamond 棱形 | square 正方形（后端字段缺失时用默认值兜底） */
  particle_shape: 'circle',
  compact_cards: false,
  /** 下载队列布局：card | table（默认 card） */
  queue_layout: DEFAULT_QUEUE_LAYOUT,
  /* 多线程与并发（1.6）：与设置页「多线程与并发」卡片一致 */
  aria2_connections: 16,
  aria2_split: 16,
  aria2_min_split_mb: 1,
  ytdlp_concurrency: 1,
  /* 界面模式 / 外观风格（1.10）：默认「简单 + 玻璃拟态」，渐变默认关闭 */
  ui_mode: 'simple',
  appearance_style: 'glass',
  accent_gradient: '',
}

export const useSettingsStore = defineStore('settings', () => {
  // 首屏即固定详细列表（卡片流已移除，不存在“切页闪一下卡片”的问题）
  const settings = ref<AppSettings>({ ...FALLBACK_SETTINGS, queue_layout: readQueueLayout() })
  const loaded = ref(false)
  const loadFailed = ref(false)
  const saving = ref(false)

  // 包括 v-model 的直接赋值：即使先改动再改回原值，旧请求也不能覆盖用户选择。
  type SettingKey = keyof AppSettings
  const revisions = new Map<SettingKey, number>()
  let observed = { ...settings.value }
  watch(settings, (current) => {
    const keys = new Set([...Object.keys(observed), ...Object.keys(current)] as SettingKey[])
    keys.forEach((key) => {
      if (!Object.is(observed[key], current[key])) revisions.set(key, (revisions.get(key) ?? 0) + 1)
    })
    observed = { ...current }
  }, { deep: true, flush: 'sync' })

  function mergeResponse(remote: AppSettings, started: Map<SettingKey, number>) {
    const unchanged = Object.fromEntries(
      (Object.keys(remote) as SettingKey[])
        .filter((key) => (revisions.get(key) ?? 0) === (started.get(key) ?? 0))
        .map((key) => [key, remote[key]]),
    )
    settings.value = { ...settings.value, ...unchanged }
  }

  const isDark = computed(() => settings.value.theme !== 'light')
  /** 当前队列布局：固定 table（保留该计算属性以兼容既有调用方） */
  const queueLayout = computed<QueueLayout>(() => settings.value.queue_layout ?? DEFAULT_QUEUE_LAYOUT)
  /** 界面模式（1.10）：simple 简单（默认） | advanced 高级 —— 折叠区组是否可见的唯一状态源 */
  const uiMode = computed<'simple' | 'advanced'>(() => normalizeUiMode(settings.value.ui_mode))
  /** 简单模式：默认折叠区组整体不渲染 */
  const isSimple = computed(() => uiMode.value === 'simple')
  /** 外观风格（1.10）：glass 玻璃拟态（默认） | mono 黑白简约 */
  const appearanceStyle = computed<'glass' | 'mono'>(() => normalizeAppearanceStyle(settings.value.appearance_style))

  let loading: Promise<AppSettings> | null = null
  let saveQueue: Promise<unknown> = Promise.resolve()
  function load() {
    if (loading) return loading
    const started = new Map(revisions)
    const priorSaves = saveQueue
    const deadline = Date.now() + STARTUP_TIMEOUT_MS
    const remaining = () => Math.max(0, deadline - Date.now())
    loadFailed.value = false
    loading = (async () => {
      try {
        await startupWait(priorSaves, remaining())
        const remote = await startupWait(ipc.getSettings(), remaining())
        const next = {
          ...FALLBACK_SETTINGS,
          ...remote,
          // 历史配置里的 'system' 静默回退 dark；队列布局由本地兼容层补齐。
          theme: normalizeTheme(remote.theme),
          queue_layout: readQueueLayout(),
        }
        if (!next.download_dir) {
          try {
            next.download_dir = await startupWait(ipc.defaultDownloadDir(), remaining())
          } catch (e) {
            loadFailed.value = true
            console.warn('读取默认下载目录失败', e)
          }
        }
        mergeResponse(next, started)
      } catch (e) {
        loadFailed.value = true
        console.warn('读取设置失败', e)
      } finally {
        loaded.value = true
        loading = null
      }
      return settings.value
    })()
    return loading
  }

  let pendingSaves = 0
  function save() {
    let snapshot = { ...settings.value }
    let started = new Map(revisions)
    const priorLoad = loading
    const layout = queueLayout.value
    writeQueueLayout(layout)
    pendingSaves += 1
    saving.value = true
    // 后端写入也需串行：仅忽略旧响应仍会让迟到的旧请求覆盖磁盘上的新设置。
    const request = saveQueue.then(async () => {
      // 首次读取尚未完成时，先补齐持久化字段，避免把首屏兜底值保存到用户配置。
      if (priorLoad) {
        await priorLoad
        snapshot = { ...settings.value }
        started = new Map(revisions)
      }
      const remote = await ipc.saveSettings(snapshot)
      mergeResponse({ ...FALLBACK_SETTINGS, ...remote, queue_layout: snapshot.queue_layout ?? DEFAULT_QUEUE_LAYOUT }, started)
      return settings.value
    }).finally(() => {
      pendingSaves -= 1
      saving.value = pendingSaves > 0
    })
    saveQueue = request.catch(() => {})
    return request
  }

  function patch(part: Partial<AppSettings>) {
    settings.value = { ...settings.value, ...part }
  }

  /**
   * 写入队列布局偏好（历史 API，已无人调用）：界面固定详细列表，
   * 这里只更新设置字段 + 本地镜像，不影响渲染。
   * 保存失败（例如在浏览器里预览、后端暂不支持该字段）也不抛错。
   */
  async function setQueueLayout(layout: QueueLayout) {
    patch({ queue_layout: layout })
    writeQueueLayout(layout)
    try {
      await save()
    } catch (e) {
      console.warn('保存队列布局失败', e)
    }
    return queueLayout.value
  }

  return { settings, loaded, loadFailed, saving, isDark, queueLayout, uiMode, isSimple, appearanceStyle, load, save, patch, setQueueLayout }
})
