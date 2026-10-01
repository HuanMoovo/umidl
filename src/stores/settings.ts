import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import type { AppSettings, QueueLayout } from '@/types'
import * as ipc from '@/services/ipc'
import { tr } from '@/i18n'

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
  theme: 'system',
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
}

export const useSettingsStore = defineStore('settings', () => {
  // 首屏即固定详细列表（卡片流已移除，不存在“切页闪一下卡片”的问题）
  const settings = ref<AppSettings>({ ...FALLBACK_SETTINGS, queue_layout: readQueueLayout() })
  const loaded = ref(false)
  const saving = ref(false)

  const isDark = computed(() => settings.value.theme !== 'light')
  /** 当前队列布局：固定 table（保留该计算属性以兼容既有调用方） */
  const queueLayout = computed<QueueLayout>(() => settings.value.queue_layout ?? DEFAULT_QUEUE_LAYOUT)

  async function load() {
    try {
      const remote = await ipc.getSettings()
      // 后端不会回传 queue_layout，用本地镜像补齐（取完设置仍是上次选的布局）
      settings.value = { ...FALLBACK_SETTINGS, ...remote, queue_layout: readQueueLayout() }
      if (!settings.value.download_dir) {
        settings.value.download_dir = await ipc.defaultDownloadDir()
      }
    } catch (e) {
      console.warn('读取设置失败', e)
    }
    loaded.value = true
    return settings.value
  }

  async function save() {
    saving.value = true
    try {
      const layout = queueLayout.value // 保存往返后后端会把该字段丢掉，这里自己补回来
      writeQueueLayout(layout)
      settings.value = { ...FALLBACK_SETTINGS, ...(await ipc.saveSettings(settings.value)), queue_layout: layout }
    } finally {
      saving.value = false
    }
    return settings.value
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

  return { settings, loaded, saving, isDark, queueLayout, load, save, patch, setQueueLayout }
})
