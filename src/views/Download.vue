<script setup lang="ts">
/**
 * 下载页（布局重构）
 *
 * 结构：一张「链接输入」卡（含解析结果与全部下载参数）+ 队列。
 *  - 批量导入 / ED2K 引擎两个次级入口默认折叠成卡内一行按钮（原来各占一整张卡，
 *    把队列挤到首屏之外）；点开后在同一位置展开，控件与调用一字未改。
 *  - 链接被判为 ed2k 时自动展开 ED2K 面板（原来链接提示与面板是分开的两块）。
 */
import { useI18n } from 'vue-i18n'
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import { NIcon, NSelect, NSwitch, NSpin, NModal, useMessage, useDialog } from 'naive-ui'
import { convertFileSrc } from '@tauri-apps/api/core'
import { openUrl } from '@tauri-apps/plugin-opener'
import {
  CloudDownloadOutline,
  SearchOutline,
  FolderOpenOutline,
  TrashOutline,
  RefreshOutline,
  LinkOutline,
  ImageOutline,
  MusicalNotesOutline,
  TextOutline,
  FilmOutline,
  EyeOutline,
  PlayOutline,
  CalendarOutline,
  GitNetworkOutline,
  ListCircleOutline,
} from '@vicons/ionicons5'
import TaskTable from '@/components/TaskTable.vue'
import BatchImportPanel from '@/components/BatchImportPanel.vue'
import Ed2kPanel from '@/components/Ed2kPanel.vue'
import { useTaskStore } from '@/stores/tasks'
import { useSettingsStore } from '@/stores/settings'
import { downloader } from '@/services/downloader'
import * as ipc from '@/services/ipc'
import { explainRoute, looksLikeEd2k } from '@/services/ed2k'
import {
  describeMedia,
  formatBytes,
  formatCount,
  formatDuration,
  languageName,
  looksLikeUrl,
  sortedVideoFormats,
  subtitleLabel,
  translateFormatNote,
} from '@/services/utils'
import type { DownloadTask, MediaInfo } from '@/types'
const { t: tr } = useI18n()

const route = useRoute()
const store = useTaskStore()
const settingsStore = useSettingsStore()
const message = useMessage()
const dialog = useDialog()

const url = ref('')
const probing = ref(false)
const info = ref<MediaInfo | null>(null)
const error = ref('')
const mode = ref<'video' | 'audio' | 'best'>('best')
const selectedFormat = ref<string | null>(null)
const audioFormat = ref('mp3')
const container = ref('mp4')
const embedThumb = ref(false)
const embedSubs = ref(false)
const outputDir = ref('')
const submitting = ref(false)
const busy = ref('')
const previewOpen = ref(false)
const subLangs = ref<string[]>([])

/** 次级入口折叠状态：批量导入 / ED2K 引擎（默认收起，保持首屏只有输入 + 队列） */
const showBatch = ref(false)
const showEd2k = ref(false)

/* ------------------------- 链接类型提示（ed2k → eMule 引擎接管）------------------------- */
/**
 * 粘入链接时问一次 explain_route：只有 engine === 'ed2k' 才提示「将由 eMule 引擎接管」，
 * 普通 http(s) / 磁力链接保持安静。输入防抖 400ms，且只在真的像链接时才打后端。
 */
const ed2kRouted = ref(false)
let routeTimer: ReturnType<typeof setTimeout> | undefined

/** 值得问后端的链接（http(s) / ftp / 磁力 / ed2k） */
function looksRoutable(u: string): boolean {
  return /^(https?:\/\/|ftp:\/\/|magnet:|ed2k:)\S+$/i.test(u)
}

async function detectRoute(target: string) {
  if (!ipc.isTauri()) return
  try {
    const v = await explainRoute(target)
    // 结果回来时输入框可能已经变了：过期结果直接丢弃
    if (url.value.trim() !== target) return
    ed2kRouted.value = v?.engine === 'ed2k'
  } catch {
    ed2kRouted.value = false
  }
}

watch(url, (v) => {
  ed2kRouted.value = false
  if (routeTimer) clearTimeout(routeTimer)
  const s = String(v || '').trim()
  if (!looksRoutable(s)) return
  routeTimer = setTimeout(() => void detectRoute(s), 400)
})

/** 判定归 eMule 引擎的链接：自动展开 ED2K 面板（否则保持界面安静） */
watch(ed2kRouted, (v) => {
  if (v) showEd2k.value = true
})

/** 主按钮文案：ed2k 直接交引擎，其余仍是解析 */
const primaryBusy = computed(() => (ed2kRouted.value ? submitting.value : probing.value))
const primaryLabel = computed(() => {
  if (ed2kRouted.value) return submitting.value ? tr('提交中…') : tr('交给引擎下载')
  return probing.value ? tr('解析中…') : tr('解析')
})

/** 主按钮：ed2k 走引擎接管（后端 start_download 会路由到 eMule），其余走解析 */
function primaryAction() {
  if (looksLikeEd2k(url.value)) void start()
  else void probe()
}

/* ------------------------- 下载队列：固定「详细列表」一种布局 ------------------------- */
/** 队列首次加载中（store 还没就绪）：显示加载态（加载 / 空态由 TaskTable 内部渲染） */
const queueLoading = computed(() => !store.ready)

const downloadDir = computed(() => outputDir.value || settingsStore.settings.download_dir)
const probeMs = computed(() => info.value?.probe_ms ?? null)

/** 封面：优先用后端缓存到本地的文件（离线可显示），否则回退远程地址 */
const cover = computed(() => {
  const i = info.value
  if (!i) return ''
  if (i.thumbnail_local) {
    try {
      return convertFileSrc(i.thumbnail_local)
    } catch {
      return i.thumbnail || ''
    }
  }
  return i.thumbnail || ''
})

/* ------------------------- 队列任务预览 ------------------------- */
const taskPreview = ref<DownloadTask | null>(null)
const taskPreviewOpen = ref(false)
const taskCoverFailed = ref(false)

/** 任务封面：本地文件走 asset 协议，远程地址直接用 */
const taskCover = computed(() => {
  const p = taskPreview.value?.thumbnail
  if (!p) return ''
  if (/^[a-zA-Z]:[\\/]/.test(p) || p.startsWith('file:')) {
    try {
      return convertFileSrc(p)
    } catch {
      return ''
    }
  }
  return p
})

const STATUS_LABEL: Record<string, string> = {
  pending: tr('等待中'),
  parsing: tr('解析中'),
  downloading: tr('下载中'),
  paused: tr('已暂停'),
  extracting: tr('提取音频'),
  transcribing: tr('AI 识别中'),
  converting: tr('转换中'),
  done: tr('已完成'),
  error: tr('失败'),
  canceled: tr('已取消'),
  canceled_partial: tr('部分完成'),
  handed_off: tr('已交给引擎'),
}

function statusLabel(s: string) {
  return STATUS_LABEL[s] ?? s
}

function openTaskPreview(t: DownloadTask) {
  taskPreview.value = t
  taskCoverFailed.value = false
  taskPreviewOpen.value = true
}

const formatOptions = computed(() => {
  if (!info.value) return []
  return sortedVideoFormats(info.value.formats).map((f) => ({
    label: `${f.resolution}  ·  ${f.ext.toUpperCase()}  ·  ${(f.vcodec || '').split('.')[0] || '—'}${f.fps ? ` · ${Math.round(f.fps)}fps` : ''}${f.filesize_text ? ` · ${f.filesize_text}` : ''}`,
    value: f.format_id,
  }))
})

const audioOptions = computed(() => [
  { label: tr('MP3 · 兼容性最好'), value: 'mp3' },
  { label: tr('AAC / M4A · 苹果生态'), value: 'm4a' },
  { label: tr('FLAC · 无损音质'), value: 'flac' },
  { label: tr('WAV · 未压缩'), value: 'wav' },
  { label: tr('OPUS · 体积最小'), value: 'opus' },
])

const subtitleOptions = computed(() => {
  const tracks = info.value?.subtitle_tracks || []
  return tracks.map((t) => ({
    label: `${languageName(t.lang)} · ${t.ext.toUpperCase()}${t.auto ? tr(' · 自动') : ''}`,
    value: t.lang,
  }))
})

const bestFormatBytes = computed(() => {
  const list = info.value ? sortedVideoFormats(info.value.formats) : []
  if (list[0]?.filesize_text) return list[0].filesize_text
  return info.value?.filesize_approx ? formatBytes(info.value.filesize_approx) : ''
})

async function probe() {
  // ed2k 链接不走 yt-dlp 解析：直接交给引擎接管（后端 start_download 会路由到 eMule）
  if (looksLikeEd2k(url.value)) {
    await start()
    return
  }
  if (!looksLikeUrl(url.value)) {
    message.warning(tr('请输入完整的视频链接'))
    return
  }
  probing.value = true
  error.value = ''
  info.value = null
  try {
    const res = await downloader.probe(url.value)
    info.value = res
    selectedFormat.value = formatOptions.value[0]?.value ?? null
    // 默认勾选手动字幕，没有则退而选第一条
    const manual = (res.subtitle_tracks || []).filter((t) => !t.auto).map((t) => t.lang)
    subLangs.value = manual.length ? manual : (res.subtitle_tracks || []).slice(0, 1).map((t) => t.lang)
    message.success(tr('解析成功：{title}', { title: res.title }))
  } catch (e: any) {
    error.value = String(e?.message ?? e)
    message.error(error.value)
  } finally {
    probing.value = false
  }
}

async function start() {
  if (!url.value.trim()) return
  const ed2k = looksLikeEd2k(url.value)
  submitting.value = true
  try {
    const t = await downloader.start({
      url: url.value,
      mode: mode.value,
      formatId: mode.value === 'video' ? selectedFormat.value : null,
      outputDir: outputDir.value || null,
      audioFormat: audioFormat.value,
      mergeContainer: container.value,
      embedThumbnail: embedThumb.value,
      embedSubs: embedSubs.value,
      titleHint: info.value?.title ?? null,
      thumbnailUrl: info.value?.thumbnail ?? null,
    })
    // ed2k：任务由后端交给 eMule 引擎接管，进度在引擎窗口看
    message.success(ed2k ? tr('已交给引擎接管，进度在引擎窗口查看') : tr('已加入下载队列'))
    store.enqueueDownload(t)
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  } finally {
    submitting.value = false
  }
}

/** 单独下载：封面 / 字幕 / 音频 / 视频 */
async function downloadPart(kind: 'thumb' | 'subs' | 'audio' | 'video') {
  if (!info.value) {
    message.warning(tr('请先解析链接'))
    return
  }
  if (kind === 'subs' && !subLangs.value.length) {
    message.warning(tr('请先选择要下载的字幕语言'))
    return
  }
  busy.value = kind
  try {
    const common = {
      url: url.value,
      outputDir: outputDir.value || null,
      titleHint: info.value.title,
      thumbnailUrl: info.value.thumbnail ?? null,
    }
    let t
    if (kind === 'thumb') {
      t = await downloader.start({ ...common, mode: 'thumb' })
    } else if (kind === 'subs') {
      t = await downloader.start({ ...common, mode: 'subs', subtitleLangs: [...subLangs.value] })
    } else if (kind === 'audio') {
      t = await downloader.start({ ...common, mode: 'audio', audioFormat: audioFormat.value })
    } else {
      t = await downloader.start({
        ...common,
        mode: mode.value === 'video' ? 'video' : 'best',
        formatId: mode.value === 'video' ? selectedFormat.value : null,
        mergeContainer: container.value,
        embedThumbnail: embedThumb.value,
        embedSubs: embedSubs.value,
      })
    }
    store.enqueueDownload(t)
    const label = { thumb: tr('封面'), subs: tr('字幕'), audio: tr('音频'), video: tr('视频') }[kind]
    message.success(tr('已加入{label}下载任务', { label }))
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  } finally {
    busy.value = ''
  }
}

function openInBrowser() {
  const u = info.value?.webpage_url || url.value
  if (u) void openUrl(u).catch((e: unknown) => message.error(String((e as any)?.message ?? e)))
}

async function pause(id: string) {
  await downloader.pause(id)
}
async function resume(id: string) {
  await downloader.resume(id)
  message.info(tr('已继续下载'))
}
async function cancel(id: string) {
  await downloader.cancel(id)
}
function remove(id: string, hasFile: boolean) {
  /**
   * 后端删除成功后才动本地列表：只摘这一行（不再 reload 全量）。
   * 失败时一行都不动并把错误摊出来 —— 界面上的行数必须和后端库里一致。
   */
  const after = async (delFile: boolean) => {
    try {
      await downloader.remove(id, delFile)
      store.dropTask(id)
    } catch (e: any) {
      message.error(String(e?.message ?? e))
    }
  }
  if (!hasFile) {
    void after(false)
    return
  }
  dialog.warning({
    title: tr('删除任务'),
    content: tr('是否同时删除已下载的文件？'),
    positiveText: tr('同时删除文件'),
    negativeText: tr('仅删除记录'),
    onPositiveClick: () => after(true),
    onNegativeClick: () => after(false),
  })
}

async function refreshList() {
  await store.reload()
  message.success(tr('列表已刷新'))
}

async function openFile(p: string) {
  try {
    await ipc.openPath(p)
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}
async function reveal(p: string) {
  try {
    await ipc.revealPath(p)
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}
async function openDir() {
  try {
    await ipc.openPath(downloadDir.value)
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}

async function clearDone() {
  try {
    await downloader.clear('done')
    // 后端已按同一口径删完终态记录：本地同步摘掉这些行（不重拉全量）
    store.dropDownloads('done')
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}

watch(mode, (m) => {
  if (m === 'best') container.value = 'mp4'
})

onMounted(async () => {
  if (!store.ready) await store.bootstrap()
  else await store.reload() // 每次进入页面重新拉一次，刷新「文件是否还在」等状态
  if (!settingsStore.loaded) await settingsStore.load()
  const s = settingsStore.settings
  container.value = s.default_container || 'mp4'
  audioFormat.value = s.default_audio_format || 'mp3'
  const q = route.query.url
  if (typeof q === 'string' && q) {
    url.value = q
    void probe()
  }
})

onBeforeUnmount(() => {
  if (routeTimer) clearTimeout(routeTimer)
})
</script>

<template>
  <div class="umi-page">
    <!-- 输入区：单链接主入口 + 解析结果 + 下载参数 + 次级入口 -->
    <section class="umi-card p-4">
      <div class="umi-card-title">
        <NIcon :size="14" :component="LinkOutline" class="text-accent" />{{ $t('视频链接') }}
      </div>
      <div class="flex gap-2">
        <input
          v-model="url"
          class="umi-input flex-1"
          placeholder="https://www.youtube.com/watch?v=... / https://www.bilibili.com/video/BV... / ed2k://|file|..."
          spellcheck="false"
          data-test="link-input"
          @keyup.enter="primaryAction"
        />
        <button class="umi-btn-primary whitespace-nowrap" :disabled="primaryBusy" data-test="link-primary" @click="primaryAction">
          <span class="flex items-center gap-1.5">
            <NSpin v-if="primaryBusy" :size="12" />
            <NIcon v-else :size="14" :component="ed2kRouted ? GitNetworkOutline : SearchOutline" />
            {{ primaryLabel }}
          </span>
        </button>
      </div>

      <!-- 链接类型提示：只有后端判定「归 eMule 引擎」的链接才提示，普通链接不打扰 -->
      <div
        v-if="ed2kRouted"
        class="mt-2 flex items-center gap-2 rounded-xl border border-umi-500/25 bg-umi-500/10 px-3 py-1.5 text-[11.5px]"
        data-test="ed2k-hint"
      >
        <NIcon :size="14" :component="GitNetworkOutline" class="text-accent shrink-0" />
        <span class="s-text">{{ $t('检测到 ED2K 链接 · 将由 eMule 引擎接管') }}</span>
      </div>

      <!-- 解析结果 -->
      <div v-if="info" class="umi-inner mt-3" data-test="probe-result">
        <div class="flex gap-3.5">
          <!-- 封面（点击预览） -->
          <div
            class="s-sunken s-border-soft group relative h-[80px] w-[142px] shrink-0 cursor-pointer overflow-hidden rounded-lg border"
            @click="previewOpen = true"
          >
            <img
              v-if="cover"
              :src="cover"
              class="h-full w-full object-cover transition-transform duration-300 group-hover:scale-105"
              :alt="$t('封面')"
            />
            <div v-else class="s-text-3 flex h-full w-full items-center justify-center">
              <NIcon :size="22" :component="ImageOutline" />
            </div>
            <div
              class="absolute inset-0 flex items-center justify-center bg-black/45 opacity-0 transition-opacity group-hover:opacity-100"
            >
              <NIcon :size="22" :component="EyeOutline" color="#fff" />
            </div>
            <span
              v-if="info.duration"
              class="absolute bottom-1 right-1 rounded bg-black/70 px-1.5 py-0.5 text-[10px] text-white"
            >
              {{ formatDuration(info.duration) }}
            </span>
          </div>

          <div class="min-w-0 flex-1">
            <div class="s-text line-clamp-2 text-[13px] font-medium">{{ info.title }}</div>
            <div class="s-text-3 mt-1 text-[11px]">
              {{ describeMedia(info) }}<span v-if="bestFormatBytes">{{ $t(' · 最高画质 {size}', { size: bestFormatBytes }) }}</span>
              <span v-if="probeMs">{{ $t(' · 解析 {ms}ms', { ms: probeMs }) }}</span>
            </div>
            <div class="mt-1.5 flex flex-wrap gap-1.5">
              <span v-if="info.uploader" class="umi-chip">{{ info.uploader }}</span>
              <span v-if="info.view_count" class="umi-chip">{{ $t('播放 {count}', { count: formatCount(info.view_count) }) }}</span>
              <span v-if="info.like_count" class="umi-chip">{{ $t('点赞 {count}', { count: formatCount(info.like_count) }) }}</span>
              <span v-if="info.subtitle_tracks.length" class="umi-chip">{{ $t('字幕 {count} 种', { count: info.subtitle_tracks.length }) }}</span>
              <span v-if="info.filesize_approx" class="umi-chip">≈ {{ formatBytes(info.filesize_approx) }}</span>
            </div>
            <div class="mt-2 flex gap-2">
              <button class="umi-btn umi-btn-sm" @click="previewOpen = true">
                <span class="flex items-center gap-1"><NIcon :size="12" :component="EyeOutline" />{{ $t('预览') }}</span>
              </button>
              <button class="umi-btn umi-btn-sm" @click="openInBrowser">
                <span class="flex items-center gap-1"><NIcon :size="12" :component="LinkOutline" />{{ $t('在浏览器打开') }}</span>
              </button>
            </div>
          </div>
        </div>

        <!-- 主下载参数 -->
        <div class="mt-3 grid grid-cols-2 gap-3">
          <div>
            <label class="umi-label">{{ $t('下载模式') }}</label>
            <div class="flex gap-1.5">
              <button
                v-for="m in [
                  { v: 'video', t: $t('指定清晰度') },
                  { v: 'best', t: $t('最佳画质') },
                  { v: 'audio', t: $t('仅音频') },
                ]"
                :key="m.v"
                class="umi-btn umi-btn-sm flex-1"
                :class="mode === m.v ? 'chip-active' : ''"
                @click="mode = m.v as any"
              >
                {{ m.t }}
              </button>
            </div>
          </div>
          <div v-if="mode === 'video'">
            <label class="umi-label">{{ $t('清晰度 / 格式') }}</label>
            <NSelect v-model:value="selectedFormat" :options="formatOptions" size="small" :consistent-menu-width="false" />
          </div>
          <div v-else-if="mode === 'audio'">
            <label class="umi-label">{{ $t('音频格式') }}</label>
            <NSelect v-model:value="audioFormat" :options="audioOptions" size="small" />
          </div>
          <div v-else>
            <label class="umi-label">{{ $t('封装容器') }}</label>
            <NSelect
              v-model:value="container"
              :options="['mp4', 'mkv', 'webm'].map((v) => ({ label: v.toUpperCase(), value: v }))"
              size="small"
            />
          </div>
        </div>

        <!-- 单独下载 -->
        <div class="mt-3">
          <div class="s-text-2 mb-1.5 text-[12px] font-medium">{{ $t('单独下载') }}</div>
          <div class="grid grid-cols-4 gap-2">
            <button class="umi-btn flex flex-col items-center gap-0.5 !py-2" :disabled="busy === 'video'" @click="downloadPart('video')">
              <NIcon :size="16" :component="FilmOutline" class="text-accent" />
              <span class="text-[11px]">{{ $t('仅视频') }}</span>
              <span class="s-text-3 text-[10px]">{{ mode === 'best' ? $t('最佳画质') : $t('所选清晰度') }}</span>
            </button>
            <button class="umi-btn flex flex-col items-center gap-0.5 !py-2" :disabled="busy === 'audio'" @click="downloadPart('audio')">
              <NIcon :size="16" :component="MusicalNotesOutline" class="text-cyan-600 dark:text-cyan-400" />
              <span class="text-[11px]">{{ $t('仅音频') }}</span>
              <span class="s-text-3 text-[10px]">{{ audioFormat.toUpperCase() }}</span>
            </button>
            <button class="umi-btn flex flex-col items-center gap-0.5 !py-2" :disabled="busy === 'thumb'" @click="downloadPart('thumb')">
              <NIcon :size="16" :component="ImageOutline" class="text-pink-600 dark:text-pink-400" />
              <span class="text-[11px]">{{ $t('仅封面') }}</span>
              <span class="s-text-3 text-[10px]">
                {{ info.thumbnail_width ? info.thumbnail_width + $t('px 原图') : $t('原图') }}
              </span>
            </button>
            <button
              class="umi-btn flex flex-col items-center gap-0.5 !py-2"
              :disabled="busy === 'subs' || !info.subtitle_tracks.length"
              @click="downloadPart('subs')"
            >
              <NIcon :size="16" :component="TextOutline" class="text-emerald-600 dark:text-emerald-400" />
              <span class="text-[11px]">{{ $t('仅字幕') }}</span>
              <span class="s-text-3 text-[10px]">
                {{ info.subtitle_tracks.length ? $t('{n}/{total} 种', { n: subLangs.length, total: info.subtitle_tracks.length }) : $t('无字幕') }}
              </span>
            </button>
          </div>

          <!-- 字幕语言多选 -->
          <div v-if="info.subtitle_tracks.length" class="mt-2.5">
            <label class="umi-label">{{ $t('字幕语言（可多选）') }}</label>
            <NSelect
              v-model:value="subLangs"
              multiple
              :options="subtitleOptions"
              size="small"
              :placeholder="$t('选择要下载的字幕语言')"
            />
          </div>
        </div>

        <div class="mt-3 flex flex-wrap items-center justify-between gap-3">
          <div class="flex items-center gap-4">
            <label class="s-text-2 flex cursor-pointer items-center gap-2 text-[12px]">
              <NSwitch v-model:value="embedThumb" size="small" />{{ $t('嵌入封面') }}</label>
            <label class="s-text-2 flex cursor-pointer items-center gap-2 text-[12px]">
              <NSwitch v-model:value="embedSubs" size="small" />{{ $t('嵌入字幕') }}</label>
          </div>
          <button class="umi-btn-primary" :disabled="submitting" data-test="link-start" @click="start">
            <span class="flex items-center gap-1.5">
              <NIcon :size="14" :component="CloudDownloadOutline" />{{ $t('开始下载') }}</span>
          </button>
        </div>
      </div>

      <div v-else-if="error" class="mt-2.5 rounded-xl border border-rose-500/25 bg-rose-500/10 px-3 py-2 text-[12px] text-rose-500">
        {{ error }}
      </div>

      <!-- 次级入口：默认收成一行，按需展开 -->
      <div class="mt-3 flex flex-wrap items-center gap-2 border-t s-border-soft pt-3">
        <button
          class="umi-btn umi-btn-sm"
          :class="showBatch ? 'chip-active' : ''"
          :aria-expanded="showBatch"
          data-test="toggle-batch"
          @click="showBatch = !showBatch"
        >
          <span class="flex items-center gap-1"><NIcon :size="12" :component="ListCircleOutline" />{{ $t('批量导入') }}</span>
        </button>
        <button
          class="umi-btn umi-btn-sm"
          :class="showEd2k ? 'chip-active' : ''"
          :aria-expanded="showEd2k"
          data-test="toggle-ed2k"
          @click="showEd2k = !showEd2k"
        >
          <span class="flex items-center gap-1"><NIcon :size="12" :component="GitNetworkOutline" />{{ $t('ED2K 电驴引擎') }}</span>
        </button>
        <span class="umi-hint">{{ $t('多链接批量入队 / eMule 引擎接管') }}</span>
      </div>
    </section>

    <!-- 批量导入（1.6）：多链接 / txt 一次性入队（默认折叠） -->
    <section v-if="showBatch" class="umi-card p-4" data-test="batch-slot">
      <BatchImportPanel bare />
    </section>

    <!-- ED2K 电驴引擎：链接由上方主输入框传入，面板不放第二个输入框 -->
    <section v-if="showEd2k" class="umi-card p-4" data-test="ed2k-slot">
      <Ed2kPanel bare compact :link="url" />
    </section>

    <!-- 任务队列 -->
    <section>
      <div class="umi-head">
        <div class="s-text-2 flex items-center gap-2 text-[12px] font-medium">{{ $t('下载队列') }}
          <span class="s-surface-2 s-text-3 rounded-md px-1.5 py-0.5 text-[10px] tabular-nums">{{ store.downloads.length }}</span>
        </div>
        <span class="umi-spacer" />
        <div class="flex flex-wrap items-center gap-2">
          <button class="umi-btn umi-btn-sm" :title="$t('刷新列表')" @click="refreshList">
            <span class="flex items-center gap-1"><NIcon :size="12" :component="RefreshOutline" />{{ $t('刷新') }}</span>
          </button>
          <button class="umi-btn umi-btn-sm" @click="openDir">
            <span class="flex items-center gap-1"><NIcon :size="12" :component="FolderOpenOutline" />{{ $t('打开下载目录') }}</span>
          </button>
          <button class="umi-btn umi-btn-sm" @click="clearDone">
            <span class="flex items-center gap-1"><NIcon :size="12" :component="TrashOutline" />{{ $t('清理已完成') }}</span>
          </button>
        </div>
      </div>

      <!-- 队列列表：固定详细表格（唯一布局；加载态 / 空态由 TaskTable 内部渲染） -->
      <TaskTable
        :tasks="store.downloads"
        :loading="queueLoading"
        :can-pause="true"
        :can-cancel="true"
        @pause="pause"
        @resume="resume"
        @cancel="cancel"
        @remove="(t) => remove(t.id, !!t.file_exists)"
        @open="(t) => openFile(t.file_path!)"
        @reveal="(t) => reveal(t.file_path!)"
        @preview="openTaskPreview"
      />
    </section>

    <!-- 预览弹窗 -->
    <NModal v-model:show="previewOpen" preset="card" class="!w-[760px] !max-w-[92vw]" :bordered="false" :title="$t('视频预览')">
      <div v-if="info" class="space-y-4">
        <div class="s-sunken s-border-soft overflow-hidden rounded-xl border">
          <img v-if="cover" :src="cover" class="max-h-[420px] w-full object-contain" :alt="$t('封面')" />
          <div v-else class="s-text-3 flex h-56 items-center justify-center text-[12px]">{{ $t('该视频没有封面图') }}</div>
        </div>
        <div class="s-text text-[15px] font-semibold">{{ info.title }}</div>
        <div class="flex flex-wrap gap-1.5">
          <span v-if="info.extractor" class="umi-chip">{{ info.extractor }}</span>
          <span v-if="info.uploader" class="umi-chip">{{ info.uploader }}</span>
          <span v-if="info.duration" class="umi-chip">{{ formatDuration(info.duration) }}</span>
          <span v-if="info.upload_date" class="umi-chip">
            <NIcon :size="11" :component="CalendarOutline" class="mr-1" />{{ info.upload_date }}
          </span>
          <span v-if="info.view_count" class="umi-chip">{{ $t('播放 {count}', { count: formatCount(info.view_count) }) }}</span>
          <span v-if="info.like_count" class="umi-chip">{{ $t('点赞 {count}', { count: formatCount(info.like_count) }) }}</span>
          <span v-if="info.thumbnail_width" class="umi-chip">{{ $t('封面 {w}×{h}', { w: info.thumbnail_width, h: info.thumbnail_height }) }}</span>
        </div>

        <div>
          <div class="s-text-2 mb-1.5 text-[12px] font-medium">{{ $t('可选清晰度（{count}）', { count: formatOptions.length }) }}</div>
          <div class="s-sunken s-border-soft max-h-40 overflow-y-auto rounded-lg border p-2">
            <div v-for="o in formatOptions" :key="o.value" class="s-text-2 py-0.5 text-[11.5px]">{{ o.label }}</div>
          </div>
        </div>

        <div v-if="info.subtitle_tracks.length">
          <div class="s-text-2 mb-1.5 text-[12px] font-medium">{{ $t('字幕轨道（{count}）', { count: info.subtitle_tracks.length }) }}</div>
          <div class="flex flex-wrap gap-1.5">
            <span v-for="t in info.subtitle_tracks" :key="t.lang + String(t.auto)" class="umi-chip">
              {{ subtitleLabel(t) }} · {{ t.ext.toUpperCase() }}
            </span>
          </div>
        </div>

        <div v-if="info.description">
          <div class="s-text-2 mb-1.5 text-[12px] font-medium">{{ $t('简介') }}</div>
          <div class="s-text-3 s-sunken s-border-soft max-h-40 overflow-y-auto whitespace-pre-wrap rounded-lg border p-2.5 text-[11.5px] leading-relaxed">
            {{ info.description }}
          </div>
        </div>

        <div class="flex justify-end gap-2 pt-1">
          <button class="umi-btn" @click="openInBrowser">
            <span class="flex items-center gap-1"><NIcon :size="13" :component="LinkOutline" />{{ $t('在浏览器打开') }}</span>
          </button>
          <button class="umi-btn-primary" @click="previewOpen = false">
            <span class="flex items-center gap-1"><NIcon :size="13" :component="PlayOutline" />{{ $t('知道了') }}</span>
          </button>
        </div>
      </div>
    </NModal>

    <!-- 队列任务预览（点卡片封面或"预览"按钮） -->
    <NModal
      v-model:show="taskPreviewOpen"
      preset="card"
      class="!w-[680px] !max-w-[92vw]"
      :bordered="false"
      :title="$t('任务预览')"
    >
      <div v-if="taskPreview" class="space-y-3">
        <div class="s-sunken s-border-soft overflow-hidden rounded-xl border">
          <img
            v-if="taskCover && !taskCoverFailed"
            :src="taskCover"
            class="max-h-[380px] w-full object-contain"
            :alt="$t('封面')"
            @error="taskCoverFailed = true"
          />
          <div v-else class="s-text-3 flex h-48 flex-col items-center justify-center gap-2 text-[12px]">
            <NIcon :size="24" :component="ImageOutline" />
            <span>{{ $t('暂无封面') }}</span>
            <span class="text-[11px] opacity-80">{{ $t('视频下载完成后会自动从画面生成封面') }}</span>
          </div>
        </div>

        <div class="s-text text-[15px] font-semibold">{{ taskPreview.title || $t('未命名任务') }}</div>

        <div class="flex flex-wrap gap-1.5">
          <span class="umi-chip">{{ statusLabel(taskPreview.status) }}</span>
          <span v-if="taskPreview.format_note" class="umi-chip">{{ translateFormatNote(taskPreview.format_note) }}</span>
          <span v-if="taskPreview.duration" class="umi-chip">{{ formatDuration(taskPreview.duration) }}</span>
          <span v-if="taskPreview.total" class="umi-chip">{{ formatBytes(taskPreview.total) }}</span>
          <span v-if="taskPreview.uploader" class="umi-chip">{{ taskPreview.uploader }}</span>
        </div>

        <div v-if="taskPreview.error" class="rounded-lg border border-rose-500/25 bg-rose-500/10 px-2.5 py-1.5 text-[11px] text-rose-500">
          {{ taskPreview.error }}
        </div>

        <div v-if="taskPreview.file_path" class="s-text-3 break-all text-[11px]">
          {{ taskPreview.file_path }}
        </div>

        <div class="flex flex-wrap gap-2 pt-1">
          <button
            v-if="taskPreview.file_path"
            class="umi-btn-primary !text-[12px]"
            @click="openFile(taskPreview.file_path!)"
          >{{ $t('打开文件') }}</button>
          <button
            v-if="taskPreview.file_path"
            class="umi-btn !text-[12px]"
            @click="reveal(taskPreview.file_path!)"
          >{{ $t('在文件夹中显示') }}</button>
          <button class="umi-btn !text-[12px]" @click="openUrl(taskPreview.url)">
            <span class="flex items-center gap-1"><NIcon :size="13" :component="LinkOutline" />{{ $t('打开原网页') }}</span>
          </button>
        </div>
      </div>
    </NModal>
  </div>
</template>
