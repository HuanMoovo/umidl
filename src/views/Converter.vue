<script setup lang="ts">
/**
 * 转换页（1.7 起为单一统一流程）
 *
 * 一条流程、四个阶段，只保留一套界面元素：
 *   ① 源文件   —— 拖拽 / 选择文件（音视频 / 图片 / 文档），选中文档走 probe_document 显示页数等
 *   ② 目标格式 —— 类型芯片（全部 / 视频 / 音频 / 图片 / 文档）+ 搜索 + 统一格式网格（唯一一份）
 *   ③ 转换参数 —— 按「选中目标格式所属类型」自动显隐（视频 / 音频 / 图片 / 文档）
 *   ④ 开始转换 + 下方队列
 * 引擎状态（ffmpeg / pandoc / poppler）折叠成一行，未就绪才出现安装按钮。
 * 历史上并列的「音视频参数区 / 文档转换能力区 / 引擎区」全部并入本流程，不再重复占屏。
 */
import { useI18n } from 'vue-i18n'
import { computed, onMounted, ref, watch } from 'vue'
import { NIcon, NProgress, NSelect, NSwitch, NCollapse, NCollapseItem, useMessage } from 'naive-ui'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import {
  DocumentOutline,
  DocumentTextOutline,
  SwapHorizontalOutline,
  FilmOutline,
  SparklesOutline,
  OptionsOutline,
  RefreshOutline,
  CheckmarkCircleOutline,
  AlertCircleOutline,
  GridOutline,
  BuildOutline,
  SearchOutline,
  CloseOutline,
} from '@vicons/ionicons5'
import ConvertRow from '@/components/ConvertRow.vue'
import { useTaskStore } from '@/stores/tasks'
import { useSettingsStore } from '@/stores/settings'
import {
  ffmpeg,
  VIDEO_CODECS,
  AUDIO_CODECS,
  RESOLUTIONS,
  FRAMERATES,
  SAMPLE_RATES,
  SPEEDS,
  AUDIO_BITRATES,
  VIDEO_BITRATES,
  qualityPresets,
  suggestCodecs,
} from '@/services/ffmpeg'
import * as ipc from '@/services/ipc'
import { isTauri } from '@/services/ipc'
import { autoDetectTarget, basename, describeProbe, formatBytes } from '@/services/utils'
import {
  FALLBACK_DOC_EXTS,
  docCapabilities,
  isDocPath,
  normalizeExt,
  probeDocument,
  type DocCapabilities,
  type DocProbe,
} from '@/services/docEngine'
import {
  ENGINE_ORDER,
  FALLBACK_CATALOG,
  availableOnly,
  flatten,
  groupOf,
  loadFormatCatalog,
  type ConvertFormatCatalog,
  type FlatFormat,
} from '@/services/formatCatalog'
import type { ConvertTask, MediaProbe } from '@/types'
const { t: tr } = useI18n()

const store = useTaskStore()
const settingsStore = useSettingsStore()
const message = useMessage()

/* ==================== ① 源文件 ==================== */
const inputFile = ref('')
const probe = ref<MediaProbe | null>(null)
const probing = ref(false)
const dragging = ref(false)
const autoReason = ref('')
const autoMode = ref(false)

/* ==================== ② 目标格式（唯一一份状态） ==================== */
const catalog = ref<ConvertFormatCatalog>(FALLBACK_CATALOG)
const catalogLive = ref(false)
const catalogLoading = ref(false)
const inTauri = isTauri()
const typeFilter = ref<'all' | string>('all')
const search = ref('')
/** 当前选中的目标格式（音视频 / 图片 / 文档共用这一个，直接作为 start_convert 的 format） */
const format = ref<string>('mp4')
const extractAudio = ref(false)
const installingEngine = ref('')

/* ==================== ③ 转换参数 ==================== */
const videoCodec = ref('libx264')
const audioCodec = ref('aac')
const resolution = ref('原分辨率')
const crf = ref<number>(23)
const mute = ref(false)
const videoBitrate = ref('auto')
const audioBitrate = ref('auto')
const sampleRate = ref(0)
const channels = ref(0)
const fps = ref(0)
const speed = ref(1)
const hwaccel = ref(false)
const faststart = ref(true)
const removeMetadata = ref(false)
const outputDir = ref('')

/* ==================== 文档（能力表 + 探测） ==================== */
const docCaps = ref<DocCapabilities | null>(null)
const docProbe = ref<DocProbe | null>(null)
const docProbeBusy = ref(false)

/** 原生输入扩展名（取不到能力表时退回内置兜底列表） */
const docNative = computed(() =>
  docCaps.value?.native?.length ? docCaps.value.native : (FALLBACK_DOC_EXTS as unknown as string[]),
)
const capsLoaded = computed(() => !!docCaps.value)
/** 当前选中的文件是不是文档（决定走文档参数还是音视频 / 图片参数） */
const isDocFile = computed(() => !!inputFile.value && isDocPath(inputFile.value, docCaps.value?.native))

/* ==================== 格式目录（只呈现本机真能写出的格式） ==================== */
/**
 * 可用性过滤的唯一入口：`available === false` 的条目整条不进界面——
 * 渲染 / 搜索 / 头部统计 / 类型芯片计数全部取自这一份，口径不会再分叉。
 * 可用性信息本身仍留在 catalog.value 里（内部判定与测试断言用）。
 */
const usableCatalog = computed<ConvertFormatCatalog>(() => availableOnly(catalog.value))
const allFormatItems = computed<FlatFormat[]>(() => flatten(usableCatalog.value.groups))
const totalCount = computed(() => usableCatalog.value.total || allFormatItems.value.length)
/** 头部「可用 M 种」：只数 available 的条目（网格里已全是可用项，两者数量一致） */
const availableCount = computed(() => allFormatItems.value.filter((f) => f.available).length)

/** 分组名的 i18n 文案（后端 label 只作兜底，避免英文界面露中文） */
const groupI18n = computed<Record<string, string>>(() => ({
  video: tr('视频'),
  audio: tr('音频'),
  image: tr('图片'),
  document: tr('文档'),
}))
const groupLabelById = (id: string) => groupI18n.value[id] ?? id
const groupLabel = (g: { id: string; label: string }) => groupI18n.value[g.id] ?? g.label

/** 类型选项：全部 + 四类，各带数量（数量 = 该类真可用的条数；整类为空则不占位） */
const typeOptions = computed(() => [
  { value: 'all', label: tr('全部'), count: totalCount.value },
  ...usableCatalog.value.groups
    .filter((g) => g.formats.length > 0)
    .map((g) => ({ value: g.id, label: groupLabel(g), count: g.formats.length })),
])

/** 类型 + 搜索（格式名 / 说明）双重过滤后的可见格式 */
const visibleFormats = computed<FlatFormat[]>(() => {
  const type = typeFilter.value
  const q = search.value.trim().toLowerCase()
  return allFormatItems.value.filter((f) => {
    if (type !== 'all' && f.group !== type) return false
    if (!q) return true
    return (
      f.id.toLowerCase().includes(q) ||
      f.label.toLowerCase().includes(q) ||
      String(f.note || '').toLowerCase().includes(q)
    )
  })
})
const filterLabel = computed(() => (typeFilter.value === 'all' ? tr('全部') : groupLabelById(typeFilter.value)))

/** 当前选中的目标格式条目 */
const selectedItem = computed<FlatFormat | null>(
  () => allFormatItems.value.find((f) => f.id.toLowerCase() === String(format.value || '').toLowerCase()) ?? null,
)
/** 目标属于哪一类：video | audio | image | document */
const targetGroup = computed(() => selectedItem.value?.group ?? groupOf(catalog.value.groups, format.value))
const currentTargetLabel = computed(() => selectedItem.value?.label ?? String(format.value || '').toUpperCase())
const currentTargetGroup = computed(() =>
  selectedItem.value ? groupLabelById(selectedItem.value.group) : tr('自定义'),
)
const isSelected = (f: FlatFormat) => f.id.toLowerCase() === String(format.value || '').toLowerCase()

/**
 * 参数区跟着「选中的目标格式所属类型」走：
 * 文档源文件直接按文档处理；视频 / 音频显示编码参数；图片只留输出目录。
 */
const paramKind = computed<'video' | 'audio' | 'image' | 'document'>(() => {
  if (isDocFile.value) return 'document'
  const g = targetGroup.value
  if (g === 'image') return 'image'
  if (g === 'document') return 'document'
  if (g === 'audio' || extractAudio.value) return 'audio'
  return 'video'
})

/** 引擎状态（ffmpeg / pandoc / poppler）：目录表为主，文档能力表能证明就绪时也算就绪 */
function engineOk(name: string) {
  if (name === 'pandoc') return !!(docCaps.value?.pandoc || catalog.value.engines.pandoc)
  if (name === 'poppler') return !!(docCaps.value?.poppler || catalog.value.engines.poppler)
  return catalog.value.engines[name] !== false
}
const engineRows = computed(() => ENGINE_ORDER.map((name) => ({ name, ok: engineOk(name) })))
const enginesReady = computed(() => engineRows.value.every((e) => e.ok))

/** 悬停提示：说明 / 依赖引擎（网格里只会出现本机可用的格式，不再解释「不支持」） */
function formatTip(f: FlatFormat): string {
  return f.note || f.engines.join(' / ')
}

/**
 * 目录刷新后的兜底：当前选中的目标若已被过滤掉（本机写不出的格式），
 * 自动落到同类型的第一个可用格式，参数区不会停在一个不存在的目标上。
 */
function ensureFormatUsable() {
  const items = allFormatItems.value
  if (!items.length) return
  const cur = String(format.value || '').toLowerCase()
  if (items.some((f) => f.id.toLowerCase() === cur)) return
  const gid = groupOf(catalog.value.groups, format.value)
  const next =
    items.find((f) => f.group === gid) ??
    items.find((f) => f.group === (extractAudio.value ? 'audio' : 'video')) ??
    items[0]
  if (next) format.value = next.id
}

/** 类型选项跟随当前目标（用户停留在「全部」时不打扰） */
function followTarget() {
  const g = targetGroup.value
  if (g && typeFilter.value !== 'all' && typeFilter.value !== g) typeFilter.value = g
}

/** 选目标格式（唯一的选中入口；网格里只剩可用格式，无需再判可用性） */
function selectFormat(f: FlatFormat) {
  autoMode.value = false
  format.value = f.id
  if (f.group === 'audio') {
    extractAudio.value = true
  } else if (f.group === 'video' || f.group === 'image') {
    extractAudio.value = false
    if (f.group === 'video') syncCodecs()
  }
  if (typeFilter.value !== 'all') typeFilter.value = f.group
}

/** 「仅提取音频」开关：目标格式与类型选项跟着在音频 / 视频之间切换 */
function onExtractAudio(v: unknown) {
  const on = !!v
  extractAudio.value = on
  const pick = (gid: string, fallback: string) => {
    const g = catalog.value.groups.find((x) => x.id === gid)
    return g?.formats.find((f) => f.available)?.id ?? fallback
  }
  if (on) {
    if (targetGroup.value !== 'audio') format.value = pick('audio', 'mp3')
    if (typeFilter.value !== 'all') typeFilter.value = 'audio'
  } else {
    if (targetGroup.value === 'audio') format.value = pick('video', 'mp4')
    if (typeFilter.value === 'audio') typeFilter.value = 'video'
  }
  syncCodecs()
}

/* 目标格式变化（自动检测 / 手动选择）时，类型选项跟随，保证选中项可见 */
watch(format, followTarget)

const qualityOptions = computed(() =>
  qualityPresets().map((p) => ({ label: tr('{label}（CRF {crf}）', { label: p.label, crf: p.crf }), value: p.crf })),
)
const noZero = (arr: readonly number[], unit: string, zeroLabel: string) =>
  arr.map((v) => ({ label: v === 0 ? zeroLabel : `${v}${unit}`, value: v }))
const speedOptions = computed(() => SPEEDS.map((s) => ({ label: s === 1 ? tr('原速') : `${s}x`, value: s })))

/* ==================== 探测与目录刷新 ==================== */

/** 文档探测：页数 / 工作表 / 幻灯片 / 字符数 */
async function loadDocProbe(path: string) {
  docProbe.value = null
  if (!isTauri()) return // 浏览器预览：只显示文件名，不做探测
  docProbeBusy.value = true
  try {
    docProbe.value = await probeDocument(path)
    if (!docProbe.value.ok && docProbe.value.error) message.warning(docProbe.value.error)
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  } finally {
    docProbeBusy.value = false
  }
}

/** 文档能力表（挂载时静默探测；手动「重新检测」时会把错误提示出来） */
async function refreshDocCaps(silent = false) {
  if (!isTauri()) return
  try {
    const c = await docCapabilities()
    docCaps.value = c
    if (isDocFile.value) await loadDocProbe(inputFile.value)
  } catch (e: any) {
    if (!silent) message.error(String(e?.message ?? e))
  }
}

/** 格式目录（Tauri 走后端 convert_formats，取不到 / 浏览器预览用内置兜底目录） */
async function refreshFormats(silent = true) {
  if (!inTauri) {
    catalog.value = FALLBACK_CATALOG
    catalogLive.value = false
    ensureFormatUsable()
    if (!silent) message.info(tr('浏览器预览：显示内置格式表'))
    return
  }
  catalogLoading.value = true
  try {
    const r = await loadFormatCatalog()
    catalog.value = r.catalog
    catalogLive.value = r.live
    ensureFormatUsable()
    if (!r.live && !silent) message.warning(tr('未取到后端格式目录，已退回内置格式表'))
  } catch (e: any) {
    if (!silent) message.error(String(e?.message ?? e))
  } finally {
    catalogLoading.value = false
  }
}

/** 重新检测：格式目录 + 文档能力表一起刷新 */
async function refreshAll(silent = true) {
  if (!inTauri) {
    message.warning(tr('该功能需要在桌面客户端中运行'))
    return
  }
  await Promise.all([refreshFormats(silent), refreshDocCaps(silent)])
  if (!silent) message.success(tr('已重新检测转换引擎与格式目录'))
}

/** 安装转换引擎（ffmpeg / pandoc / poppler）：沿用 install_tool + tool://progress */
async function installEngine(name: string) {
  if (!inTauri) {
    message.warning(tr('该功能需要在桌面客户端中运行'))
    return
  }
  if (installingEngine.value) return
  installingEngine.value = name
  try {
    await ipc.installTool(name)
    message.success(tr('{name} 安装完成', { name }))
    void store.refreshTools().catch(() => {})
    await refreshFormats(true)
    if (name === 'pandoc' || name === 'poppler') await refreshDocCaps(true)
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  } finally {
    installingEngine.value = ''
  }
}

/* ==================== 源文件载入 ==================== */

async function pick() {
  try {
    const docExts = docNative.value.map(normalizeExt).filter(Boolean)
    const picked = await openDialog({
      multiple: false,
      directory: false,
      filters: [
        {
          name: tr('媒体文件'),
          extensions: [
            'mp4', 'mkv', 'mov', 'avi', 'webm', 'flv', 'ts', 'm4v', 'mpg', 'wmv', 'ogv', 'gif',
            'png', 'jpg', 'jpeg', 'webp', 'bmp', 'tiff', 'ico',
            'mp3', 'm4a', 'aac', 'flac', 'wav', 'opus', 'ogg', 'wma', 'ac3', 'aiff',
          ],
        },
        { name: tr('文档文件'), extensions: docExts.length ? docExts : ['pdf', 'docx'] },
        { name: tr('全部文件'), extensions: ['*'] },
      ],
    })
    if (typeof picked === 'string') await load(picked)
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}

async function load(path: string) {
  inputFile.value = path
  probing.value = true
  probe.value = null
  docProbe.value = null
  autoReason.value = ''
  // 文档文件（PDF / Office / 电子书）：走 probe_document，ffprobe 读不了这些格式
  if (isDocPath(path, docCaps.value?.native)) {
    probing.value = false
    typeFilter.value = 'document'
    // 网格里只有可用格式：不是文档目标就落到文档类第一个可用格式
    const docItems = usableCatalog.value.groups.find((g) => g.id === 'document')?.formats ?? []
    const current = docItems.find((f) => f.id === format.value)
    if (!current && docItems[0]) format.value = docItems[0].id
    await loadDocProbe(path)
    return
  }
  try {
    const p = await ffmpeg.probe(path)
    probe.value = p
    const kind = p.video_codec && p.video_codec !== 'none' ? 'video' : 'audio'
    if (kind === 'audio') {
      extractAudio.value = true
      format.value = 'mp3'
    } else {
      extractAudio.value = false
      format.value = 'mp4'
    }
    syncCodecs()
    // 载入后立刻给出自动检测建议
    applyAuto(false)
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  } finally {
    probing.value = false
  }
}

/** 自动检测：按源文件真实编码推荐目标格式与编码（优先无损封装） */
function applyAuto(notify = true) {
  const t = autoDetectTarget(probe.value)
  autoReason.value = t.reason
  if (!probe.value) return
  autoMode.value = true
  extractAudio.value = t.extractAudio
  format.value = t.format
  if (!t.extractAudio) {
    videoCodec.value = t.video
    resolution.value = '原分辨率'
  }
  audioCodec.value = t.audio
  if (t.remux) crf.value = 18
  if (notify) message.success(t.reason)
}

function syncCodecs() {
  autoMode.value = false
  const s = suggestCodecs(format.value, extractAudio.value)
  if (!extractAudio.value) videoCodec.value = s.video
  audioCodec.value = s.audio
}

/* ==================== 开始转换 ==================== */

/** 图片 / 文档目标：不需要编码参数，直接按扩展名交给后端路由 */
async function startPlain(successKey: string) {
  try {
    const t = await ipc.startConvert({
      input_file: inputFile.value,
      output_dir: outputDir.value || null,
      format: format.value,
      video_codec: null,
      audio_codec: null,
      bitrate: null,
      resolution: null,
      crf: null,
      extract_audio: false,
      mute: false,
      audio_bitrate: null,
      sample_rate: null,
      channels: null,
      fps: null,
      speed: null,
      hwaccel: false,
      faststart: false,
      remove_metadata: false,
      auto_format: null,
    })
    store.enqueueConvert(t)
    message.success(tr(successKey))
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}

async function start() {
  if (!inputFile.value) {
    message.warning(tr('请先选择文件'))
    return
  }
  if (paramKind.value === 'document') {
    await startPlain('已开始转换（文档）')
    return
  }
  if (paramKind.value === 'image') {
    await startPlain('已开始转换（图片）')
    return
  }
  try {
    const t = await ffmpeg.start({
      inputFile: inputFile.value,
      format: format.value,
      outputDir: outputDir.value || null,
      videoCodec: extractAudio.value ? null : videoCodec.value,
      audioCodec: audioCodec.value,
      resolution: extractAudio.value ? null : resolution.value,
      crf: extractAudio.value ? null : crf.value,
      extractAudio: extractAudio.value,
      mute: mute.value,
      bitrate: videoBitrate.value,
      audioBitrate: audioBitrate.value,
      sampleRate: sampleRate.value,
      channels: channels.value,
      fps: fps.value,
      speed: speed.value,
      hwaccel: hwaccel.value,
      faststart: faststart.value,
      removeMetadata: removeMetadata.value,
      autoFormat: autoMode.value,
    })
    store.enqueueConvert(t)
    message.success(autoMode.value ? tr('已开始转换（自动检测模式）') : tr('已开始转换'))
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}

async function cancel(id: string) {
  await ffmpeg.cancel(id)
}
async function resume(id: string) {
  const t = store.converts.find((x) => x.id === id)
  if (!t) return
  try {
    // 沿用当前参数面板：源文件不是同一条时先重新载入
    if (inputFile.value !== t.input_file) await load(t.input_file)
    await start()
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}
async function remove(id: string) {
  try {
    await ffmpeg.remove(id)
    // 后端已删库：本地立刻摘行（store 是队列的唯一数据出口，不用切页/重启）
    store.dropTask(id)
  } catch (e: any) {
    // 后端失败：一行都不动，把原因摊给用户
    message.error(String(e?.message ?? e))
  }
}
async function openFile(p: string) {
  try {
    await ipc.openPath(p)
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}
/* ==================== 转换队列（真实文件信息 · 简约行列表） ==================== */

/**
 * 队列行要显示「源文件 大小 → 目标格式 产物大小」，但 ConvertTask 只带路径不带大小，
 * 且「文件已丢失」判定需要真实的 exists —— 这两件事都走后端 file_sizes 批量拿（只读元数据，不启子进程）。
 *
 * 这里直接用 invoke 而不是往 services/ipc.ts 加封装：ipc.ts 由并行改动维护，避免互相打架。
 */
const fileMeta = ref<Record<string, { size: number | null; exists: boolean; modified: number | null }>>({})

async function refreshFileMeta() {
  if (!inTauri) return
  const paths = new Set<string>()
  for (const t of store.converts) {
    if (t.input_file) paths.add(t.input_file)
    if (t.output_file) paths.add(t.output_file)
  }
  if (!paths.size) {
    fileMeta.value = {}
    return
  }
  try {
    const rows = await invoke<
      { path: string; size: number | null; exists: boolean; modified: number | null }[]
    >('file_sizes', { paths: [...paths] })
    const next: typeof fileMeta.value = {}
    for (const r of rows) next[r.path] = { size: r.size ?? null, exists: !!r.exists, modified: r.modified ?? null }
    fileMeta.value = next
  } catch (e) {
    // 浏览器预览 / 老后端没有该命令：卡片降级成「只有文件名与格式」，不弹错
    console.warn('读取转换文件信息失败', e)
  }
}

/** 目标格式的展示名：优先后端目录里的 label（PSD / ALAC…），取不到就用 id 大写 */
function formatLabel(id: string): string {
  const hit = allFormatItems.value.find((f) => f.id.toLowerCase() === String(id || '').toLowerCase())
  return hit?.label || String(id || '').toUpperCase()
}

function sizeText(p: string): string {
  const m = fileMeta.value[p]
  return m && m.exists && m.size ? formatBytes(m.size) : ''
}

/** 队列行的展示名：产物名 → 源文件名 */
function rowName(t: ConvertTask): string {
  return basename(t.output_file) || basename(t.input_file)
}

/** 队列行左半：`源文件名 源大小`（大小取真实字节数，缺失就只显示名字） */
function srcText(t: ConvertTask): string {
  const srcName = basename(t.input_file) || t.input_file
  const srcSize = sizeText(t.input_file)
  return srcSize ? `${srcName} ${srcSize}` : srcName
}

/** 队列行右半：`目标格式 产物大小`（未完成只有格式名，完成后补上真实产物大小） */
function dstText(t: ConvertTask): string {
  const target = formatLabel(t.format) || basename(t.output_file).split('.').pop()?.toUpperCase() || '?'
  const outSize = t.status === 'done' && t.output_file ? sizeText(t.output_file) : ''
  return outSize ? `${target} ${outSize}` : target
}

/** 产物是否真的还在磁盘上（决定「打开 / 显示」按钮与「文件已丢失」提示） */
function outputExists(t: ConvertTask): boolean {
  if (!t.output_file) return false
  const m = fileMeta.value[t.output_file]
  return !!m?.exists
}

/** 队列行上的动作：只保留打开 / 显示 / 删除 —— 预览功能已按用户要求移除 */
async function reveal(p: string) {
  try {
    await ipc.revealPath(p)
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}

/* 队列里任务一出现 / 状态一变（完成时）就刷新真实大小与存在性；进度跳动不触发（签名里不含 progress） */
watch(
  () => store.converts.map((t) => `${t.id}:${t.status}`).join('|'),
  () => void refreshFileMeta(),
  { immediate: true },
)

onMounted(async () => {
  if (!store.ready) await store.bootstrap()
  if (!settingsStore.loaded) await settingsStore.load()
  crf.value = settingsStore.settings.default_crf || 23
  // 文档能力表 + 目标格式目录：静默探测（浏览器预览下用内置兜底目录渲染）
  void refreshDocCaps(true)
  void refreshFormats(true)
  try {
    const wv = getCurrentWebview()
    await wv.onDragDropEvent((ev) => {
      const p = ev.payload as any
      if (p?.type === 'over') dragging.value = true
      else if (p?.type === 'drop') {
        dragging.value = false
        const paths: string[] = p.paths || []
        if (paths.length) void load(paths[0])
      } else if (p?.type === 'leave') dragging.value = false
    })
  } catch {
    /* 浏览器环境无拖拽事件 */
  }
})
</script>

<template>
  <div class="mx-auto max-w-5xl space-y-5">
    <!-- ==================== 统一转换流程：文件 → 目标格式 → 参数 → 开始 ==================== -->
    <section class="umi-card p-5 transition-all" :class="dragging ? 'drop-active' : ''" data-role="convert-flow">
      <!-- ========== ① 源文件 ========== -->
      <div data-step="file" data-role="file-area">
        <div class="mb-2.5 flex flex-wrap items-center justify-between gap-2">
          <span class="s-text-2 flex items-center gap-2 text-[12px] font-medium">
            <span class="flex h-4 w-4 items-center justify-center rounded-full bg-umi-500/20 text-[10px] text-accent">1</span>
            <NIcon :size="15" :component="FilmOutline" class="text-cyan-600 dark:text-cyan-400" />{{ $t('源文件') }}
          </span>
          <span class="s-text-3 text-[11px]">{{ $t('支持拖拽文件到窗口') }}</span>
        </div>

        <div class="flex gap-2">
          <input v-model="inputFile" class="umi-input flex-1" :placeholder="$t('选择文件或直接拖拽到窗口内…')" readonly data-role="source-input" />
          <button class="umi-btn-primary whitespace-nowrap" @click="pick">
            <span class="flex items-center gap-1.5">
              <NIcon :size="14" :component="DocumentOutline" />{{ $t('选择文件') }}</span>
          </button>
        </div>

        <!-- 音视频探测结果 + 自动检测 -->
        <div v-if="probe" class="s-border-soft s-sunken mt-2.5 rounded-xl border px-3 py-2.5" data-role="media-info">
          <div class="flex flex-wrap items-center gap-2">
            <span class="s-text truncate text-[12px] font-medium">{{ basename(probe.path) }}</span>
            <span class="umi-chip !text-[10px]">{{ describeProbe(probe) }}</span>
            <span v-if="probe.fps" class="umi-chip !text-[10px]">{{ probe.fps.toFixed(2) }} fps</span>
            <span v-if="probe.size" class="umi-chip !text-[10px]">{{ formatBytes(probe.size) }}</span>
            <span class="s-text-3 ml-auto truncate text-[10.5px]">
              {{ autoReason || $t('载入文件后自动检测最佳输出格式') }}</span>
            <button class="umi-btn shrink-0 !py-1 !text-[11px]" @click="applyAuto(true)">
              <span class="flex items-center gap-1">
                <NIcon :size="12" :component="SparklesOutline" />{{ $t('自动检测') }}</span>
            </button>
          </div>
        </div>

        <!-- 文档探测结果：页数 / 工作表 / 幻灯片 / 字符数 -->
        <div v-if="isDocFile" class="s-border-soft s-surface-2 mt-2.5 rounded-xl border px-3 py-2.5" data-role="doc-info">
          <div class="flex flex-wrap items-center gap-2">
            <span class="s-text-2 flex items-center gap-1.5 text-[11.5px] font-medium">
              <NIcon :size="13" :component="DocumentTextOutline" class="text-accent" />{{ $t('文档信息') }}</span>
            <span v-if="docProbeBusy" class="s-text-3 flex items-center gap-1.5 text-[11px]">
              <span class="h-2 w-2 animate-ping rounded-full bg-umi-400" />{{ $t('探测中…') }}</span>
            <template v-else-if="docProbe && docProbe.ok">
              <span class="s-text truncate text-[12px] font-medium">{{ docProbe.title || basename(inputFile) }}</span>
              <span v-if="docProbe.kind" class="umi-chip !text-[10px]">{{ docProbe.kind }}</span>
              <span v-if="docProbe.pages" class="umi-chip !text-[10px]">{{ $t('页数') }} {{ docProbe.pages }}</span>
              <span v-if="docProbe.sheets" class="umi-chip !text-[10px]">{{ $t('工作表') }} {{ docProbe.sheets }}</span>
              <span v-if="docProbe.slides" class="umi-chip !text-[10px]">{{ $t('幻灯片') }} {{ docProbe.slides }}</span>
              <span v-if="docProbe.chars" class="umi-chip !text-[10px]">{{ $t('字符数') }} {{ docProbe.chars }}</span>
            </template>
            <span v-else class="s-text-3 text-[11px] leading-relaxed">
              {{ docProbe?.error || (inTauri ? $t('未取到文档信息') : $t('浏览器预览下不做文档探测')) }}</span>
          </div>
        </div>
      </div>

      <!-- ========== ② 目标格式（唯一的格式选择器） ========== -->
      <div class="s-border-soft mt-4 border-t pt-4" data-step="format" data-role="format-selector">
        <div class="mb-2.5 flex flex-wrap items-center justify-between gap-2">
          <span class="s-text-2 flex flex-wrap items-center gap-2 text-[12px] font-medium">
            <span class="flex h-4 w-4 items-center justify-center rounded-full bg-umi-500/20 text-[10px] text-accent">2</span>
            <NIcon :size="15" :component="GridOutline" class="text-accent" />{{ $t('目标格式') }}
            <span class="umi-chip !text-[10px]" data-role="format-total">{{ $t('共 {n} 种格式', { n: totalCount }) }}</span>
            <span class="umi-chip !text-[10px]" data-role="format-available">{{ $t('可用 {n} 种', { n: availableCount }) }}</span>
            <span class="s-text-3 text-[10.5px]" data-role="filter-summary">
              {{ $t('当前筛选：{name}（{n}）', { name: filterLabel, n: visibleFormats.length }) }}</span>
          </span>
          <div class="flex items-center gap-2">
            <span v-if="!inTauri" class="s-text-3 text-[10.5px]" data-role="fallback-hint">
              {{ $t('浏览器预览：显示内置格式表') }}
            </span>
            <span v-else-if="!catalogLive && !catalogLoading" class="s-text-3 text-[10.5px]" data-role="fallback-hint">
              {{ $t('未取到后端格式目录，已退回内置格式表') }}
            </span>
            <button class="umi-btn !py-1.5 !text-[11px]" :disabled="catalogLoading" @click="refreshAll(false)">
              <span class="flex items-center gap-1">
                <NIcon :size="12" :component="RefreshOutline" />{{ $t('重新检测') }}</span>
            </button>
          </div>
        </div>

        <!-- 引擎状态条（一行：ffmpeg / pandoc / poppler，未就绪才出现安装按钮） -->
        <div class="s-border-soft s-surface-2 flex flex-wrap items-center gap-x-3 gap-y-1.5 rounded-xl border px-3 py-2" data-role="engine-strip">
          <span class="s-text-2 flex items-center gap-1.5 text-[11.5px] font-medium">
            <NIcon :size="13" :component="BuildOutline" class="text-accent" />{{ $t('转换引擎') }}</span>
          <span
            class="umi-chip !text-[10px]"
            :class="enginesReady ? 'text-emerald-600 dark:text-emerald-400' : 'text-amber-600 dark:text-amber-400'"
            data-role="engine-status"
          >{{ enginesReady ? $t('引擎就绪') : $t('引擎未就绪') }}</span>
          <span
            v-for="eng in engineRows"
            :key="`fmt-eng-${eng.name}`"
            class="flex items-center gap-1.5"
            :data-engine="eng.name"
            :data-engine-ok="eng.ok ? '1' : '0'"
          >
            <NIcon
              :size="12"
              class="shrink-0"
              :component="eng.ok ? CheckmarkCircleOutline : AlertCircleOutline"
              :class="eng.ok ? 'text-emerald-600 dark:text-emerald-400' : 'text-amber-600 dark:text-amber-400'"
            />
            <span class="s-text text-[11.5px] font-medium">{{ eng.name }}</span>
            <template v-if="installingEngine === eng.name">
              <NProgress
                class="w-16"
                type="line"
                :percentage="Math.round(store.toolProgress[eng.name]?.percent ?? 0)"
                :height="4"
                :show-indicator="false"
                color="rgb(var(--umi-a500))"
              />
              <span class="s-text-3 max-w-[120px] truncate text-[10px]">
                {{ store.toolProgress[eng.name]?.message || $t('下载中…') }}</span>
            </template>
            <span v-else class="s-text-3 text-[10px]">
              {{ eng.ok ? $t('已就绪') : inTauri ? $t('未安装，依赖它的格式暂不列出') : $t('未检测（浏览器预览）') }}</span>
            <button
              v-if="!eng.ok"
              class="umi-btn-primary shrink-0 !px-2 !py-0.5 !text-[10px]"
              :disabled="!!installingEngine"
              :data-install="eng.name"
              @click="installEngine(eng.name)"
            >
              {{ installingEngine === eng.name ? $t('安装中…') : $t('安装') }}
            </button>
          </span>
        </div>

        <!-- 类型芯片 + 搜索（同一份选择器，不再有第二处类型选择） -->
        <div class="mt-2.5 flex flex-wrap items-center gap-2.5">
          <div class="s-border-soft s-surface-2 flex flex-wrap gap-1 rounded-xl border p-1" data-role="type-options">
            <button
              v-for="o in typeOptions"
              :key="`fmt-type-${o.value}`"
              class="rounded-lg px-3 py-1.5 text-[11.5px] transition-all"
              :data-type="o.value"
              :data-type-active="typeFilter === o.value ? '1' : '0'"
              :class="typeFilter === o.value ? 'bg-umi-500/20 font-medium text-accent' : 's-text-2 hover:bg-umi-500/10'"
              @click="typeFilter = o.value"
            >
              {{ o.label }}<span class="s-text-3 ml-1 text-[10px]">{{ o.count }}</span>
            </button>
          </div>
          <div class="s-border-soft s-surface-2 flex min-w-[190px] flex-1 items-center gap-1.5 rounded-xl border px-2.5 py-1.5">
            <NIcon :size="13" :component="SearchOutline" class="s-text-3 shrink-0" />
            <input
              v-model="search"
              class="s-text w-full bg-transparent text-[11.5px] outline-none"
              :placeholder="$t('搜索格式（名称 / 说明）')"
              data-role="format-search"
            />
            <button
              v-if="search"
              class="s-text-3 shrink-0 transition-colors hover:text-accent"
              :title="$t('清除')"
              data-role="search-clear"
              @click="search = ''"
            >
              <NIcon :size="13" :component="CloseOutline" />
            </button>
          </div>
        </div>

        <!-- 目标格式网格：格式名 + 引擎徽标 + note（唯一个网格；只列本机可用的格式） -->
        <div
          class="mt-2.5 grid max-h-[300px] grid-cols-2 gap-2 overflow-y-auto pr-1 sm:grid-cols-3 lg:grid-cols-4"
          data-role="format-grid"
        >
          <button
            v-for="f in visibleFormats"
            :key="f.key"
            class="rounded-xl border px-2.5 py-1.5 text-left transition-all"
            :data-format="f.id"
            :data-format-group="f.group"
            :data-available="f.available ? '1' : '0'"
            :data-selected="isSelected(f) ? '1' : '0'"
            :title="formatTip(f)"
            :class="
              isSelected(f) ? 'border-umi-400/60 bg-umi-500/15' : 's-border-soft s-surface-2 hover:border-umi-400/30'
            "
            @click="selectFormat(f)"
          >
            <div class="flex items-center justify-between gap-1.5">
              <span class="s-text truncate text-[11.5px] font-medium">{{ f.label }}</span>
              <span v-if="isSelected(f)" class="umi-chip !px-1.5 !text-[9px] text-accent">{{ $t('已选') }}</span>
            </div>
            <div class="mt-0.5 flex flex-wrap items-center gap-1">
              <span v-if="!f.engines.length" class="umi-chip !px-1.5 !text-[9px]">{{ $t('原生') }}</span>
              <span
                v-for="e in f.engines"
                :key="`${f.key}-${e}`"
                class="umi-chip !px-1.5 !text-[9px]"
                :class="engineOk(e) ? '' : 'text-amber-600 dark:text-amber-400'"
              >{{ e }}</span>
            </div>
            <div v-if="f.note" class="s-text-3 mt-0.5 truncate text-[9.5px]" :title="f.note">{{ f.note }}</div>
          </button>
        </div>
        <div v-if="!visibleFormats.length" class="s-text-3 py-6 text-center text-[11.5px]" data-role="grid-empty">
          {{ $t('没有匹配的格式（换个类型或清空搜索）') }}</div>

        <div class="s-text-3 mt-2 text-[10.5px] leading-relaxed">
          {{ $t('选中即作为转换任务的目标格式；视频 / 音频目标才显示下方编码参数。') }}
        </div>
      </div>

      <!-- ========== ③ 转换参数（按目标格式所属类型自动显隐） ========== -->
      <div class="s-border-soft mt-4 border-t pt-4" data-step="params" data-role="param-area" :data-param-kind="paramKind">
        <div class="mb-2.5 flex flex-wrap items-center gap-2">
          <span class="s-text-2 flex items-center gap-2 text-[12px] font-medium">
            <span class="flex h-4 w-4 items-center justify-center rounded-full bg-umi-500/20 text-[10px] text-accent">3</span>
            <NIcon :size="14" :component="OptionsOutline" class="text-accent" />{{ $t('转换参数') }}
          </span>
          <span class="umi-chip !text-[10px]" data-role="current-target">{{ currentTargetLabel }}</span>
          <span class="s-text-3 text-[10px]" data-role="current-target-group">{{ currentTargetGroup }}</span>
        </div>

        <div class="grid grid-cols-2 gap-3 sm:grid-cols-4">
          <div v-if="paramKind === 'video'" data-field="video-codec">
            <label class="umi-label">{{ $t('视频编码') }}</label>
            <NSelect
              v-model:value="videoCodec"
              :options="VIDEO_CODECS.map((c) => ({ label: c === 'copy' ? tr('copy（不重编码 / 最快）') : c, value: c as string }))"
              size="small"
            />
          </div>
          <div v-if="paramKind === 'video'" data-field="resolution">
            <label class="umi-label">{{ $t('分辨率') }}</label>
            <NSelect
              v-model:value="resolution"
              :options="RESOLUTIONS.map((r) => ({ label: r === '原分辨率' ? tr('原分辨率') : r, value: r }))"
              size="small"
            />
          </div>
          <div v-if="paramKind === 'video'" data-field="quality">
            <label class="umi-label">{{ $t('质量预设') }}</label>
            <NSelect v-model:value="crf" :options="qualityOptions" size="small" />
          </div>
          <div v-if="paramKind === 'audio'" data-field="audio-codec">
            <label class="umi-label">{{ $t('音频编码') }}</label>
            <NSelect
              v-model:value="audioCodec"
              :options="AUDIO_CODECS.map((c) => ({ label: c === 'copy' ? tr('copy（不重编码）') : c, value: c as string }))"
              size="small"
            />
          </div>
          <div v-if="paramKind === 'audio'" data-field="audio-bitrate">
            <label class="umi-label">{{ $t('音频码率') }}</label>
            <NSelect
              v-model:value="audioBitrate"
              :options="AUDIO_BITRATES.map((b) => ({ label: b === 'auto' ? $t('自动') : b, value: b as string }))"
              size="small"
            />
          </div>
          <div v-if="paramKind === 'audio'" data-field="sample-rate">
            <label class="umi-label">{{ $t('采样率') }}</label>
            <NSelect v-model:value="sampleRate" :options="noZero(SAMPLE_RATES, ' Hz', $t('保持原样'))" size="small" />
          </div>
          <div v-if="paramKind === 'audio'" data-field="channels">
            <label class="umi-label">{{ $t('声道') }}</label>
            <NSelect
              v-model:value="channels"
              :options="[
                { label: $t('保持原样'), value: 0 },
                { label: $t('单声道'), value: 1 },
                { label: $t('立体声'), value: 2 },
                { label: $t('5.1 环绕'), value: 6 },
              ]"
              size="small"
            />
          </div>
          <div data-field="output-dir">
            <label class="umi-label">{{ $t('输出目录') }}</label>
            <input v-model="outputDir" class="umi-input !py-1.5 text-[12px]" :placeholder="$t('默认与源文件同目录')" />
          </div>
        </div>

        <!-- 图片目标：只留输出目录 + 说明 -->
        <div v-if="paramKind === 'image'" class="s-text-3 mt-2.5 text-[10.5px] leading-relaxed" data-field="image-note">
          {{ $t('图片格式直接输出，无需编码参数。') }}
        </div>

        <!-- 文档目标：文档专属说明 + 后端原生输入扩展名 -->
        <div v-else-if="paramKind === 'document'" class="mt-2.5 space-y-2" data-field="doc-note">
          <div class="s-text-3 text-[10.5px] leading-relaxed">
            {{ $t('文档格式在上方网格选择；转换按扩展名自动路由到文档引擎，不再走 ffmpeg。') }}
          </div>
          <div class="flex flex-wrap items-center gap-1.5">
            <span class="s-text-3 text-[10.5px]">{{ $t('支持格式（输入）') }}</span>
            <span v-for="e in docNative" :key="`doc-ext-${e}`" class="umi-chip !px-1.5 !text-[9.5px]">
              {{ normalizeExt(e).toUpperCase() }}</span>
            <span class="s-text-3 text-[10px]">
              {{ capsLoaded ? $t('（来自后端能力表）') : inTauri ? $t('（后端未提供能力表，使用内置列表）') : $t('（浏览器预览，使用内置列表）') }}</span>
          </div>
        </div>

        <!-- 高级选项（仅视频 / 音频目标） -->
        <div v-if="paramKind === 'video' || paramKind === 'audio'" class="s-border-soft mt-3 rounded-xl border">
          <NCollapse arrow-placement="right">
            <NCollapseItem>
              <template #header>
                <span class="s-text-2 flex items-center gap-2 text-[12px] font-medium">
                  <NIcon :size="13" :component="OptionsOutline" />{{ $t('高级选项（更多选择）') }}</span>
              </template>
              <div class="grid grid-cols-2 gap-3 pb-1 sm:grid-cols-3">
                <div v-if="paramKind === 'video'" data-field="video-bitrate">
                  <label class="umi-label">{{ $t('视频码率') }}</label>
                  <NSelect
                    v-model:value="videoBitrate"
                    :options="VIDEO_BITRATES.map((b) => ({ label: b === 'auto' ? $t('自动（按质量预设）') : b, value: b as string }))"
                    size="small"
                  />
                </div>
                <div v-if="paramKind === 'video'" data-field="fps">
                  <label class="umi-label">{{ $t('帧率') }}</label>
                  <NSelect v-model:value="fps" :options="noZero(FRAMERATES, ' fps', $t('保持原样'))" size="small" />
                </div>
                <div data-field="speed">
                  <label class="umi-label">{{ $t('播放速度') }}</label>
                  <NSelect v-model:value="speed" :options="speedOptions" size="small" />
                </div>
              </div>
              <div class="mt-3 flex flex-wrap items-center gap-5">
                <label class="s-text-2 flex cursor-pointer items-center gap-2 text-[12px]">
                  <NSwitch v-model:value="hwaccel" size="small" />{{ $t('硬件加速解码') }}</label>
                <label class="s-text-2 flex cursor-pointer items-center gap-2 text-[12px]">
                  <NSwitch v-model:value="faststart" size="small" />{{ $t('网络快速起播（faststart）') }}</label>
                <label class="s-text-2 flex cursor-pointer items-center gap-2 text-[12px]">
                  <NSwitch v-model:value="removeMetadata" size="small" />{{ $t('清除元数据') }}</label>
              </div>
            </NCollapseItem>
          </NCollapse>
        </div>

        <div class="mt-3.5 flex items-center justify-between gap-3">
          <div class="flex flex-wrap items-center gap-5">
            <label
              v-if="paramKind === 'video' || paramKind === 'audio'"
              class="s-text-2 flex cursor-pointer items-center gap-2 text-[12px]"
            >
              <NSwitch :value="extractAudio" size="small" @update:value="onExtractAudio" />{{ $t('仅提取音频') }}</label>
            <label
              v-if="paramKind === 'video'"
              class="s-text-2 flex cursor-pointer items-center gap-2 text-[12px]"
            >
              <NSwitch v-model:value="mute" size="small" :disabled="extractAudio" />{{ $t('去除音轨') }}</label>
          </div>
          <button class="umi-btn-primary shrink-0" :disabled="!inputFile || probing" data-role="start-convert" @click="start">
            <span class="flex items-center gap-1.5">
              <NIcon :size="14" :component="SwapHorizontalOutline" />{{ $t('开始转换') }}</span>
          </button>
        </div>
      </div>
    </section>

    <!-- ==================== 转换队列（简约行列表：无缩略图 / 无预览 / 无卡片） ==================== -->
    <section>
      <div class="s-text-2 mb-2 flex items-center gap-2 text-[12px] font-medium">{{ $t('转换队列') }}<span class="s-surface-2 s-text-3 rounded-md px-1.5 py-0.5 text-[10px]">{{ store.converts.length }}</span>
      </div>
      <!-- 队列自己内滚：记录再多也不把页面撑高 -->
      <div
        v-if="store.converts.length"
        data-role="convert-queue"
        class="s-border-soft s-surface-2 max-h-[42vh] overflow-y-auto rounded-lg border px-1"
      >
        <ConvertRow
          v-for="t in store.converts"
          :key="t.id"
          :name="rowName(t)"
          :status="t.status"
          :progress="t.progress"
          :src-text="srcText(t)"
          :dst-text="dstText(t)"
          :error="t.error"
          :file-path="t.status === 'done' && t.output_file ? t.output_file : null"
          :file-exists="t.status === 'done' ? outputExists(t) : undefined"
          @open="openFile(t.output_file)"
          @reveal="reveal(t.output_file)"
          @remove="remove(t.id)"
          @cancel="cancel(t.id)"
          @resume="resume(t.id)"
        />
      </div>
      <div v-else class="umi-card s-text-3 flex flex-col items-center gap-2 py-12">
        <NIcon :size="26" :component="SwapHorizontalOutline" />
        <div class="text-[12px]">{{ $t('还没有转换任务') }}</div>
      </div>
    </section>
  </div>
</template>
