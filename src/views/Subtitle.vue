<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { NIcon, NProgress, NSelect, NSwitch, useMessage } from 'naive-ui'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import {
  ChatbubblesOutline,
  DocumentOutline,
  DownloadOutline,
  FolderOpenOutline,
  SparklesOutline,
  OptionsOutline,
} from '@vicons/ionicons5'
import ConvertRow from '@/components/ConvertRow.vue'
import SettingsGroup from '@/components/SettingsGroup.vue'
import { useTaskStore } from '@/stores/tasks'
import { useSettingsStore } from '@/stores/settings'
import { whisper, languages, OUTPUT_FORMATS, models, modelProgressText } from '@/services/whisper'
import { asr, CLOUD_PROVIDERS, cloudAllowedLanguages, cloudConfigValid, cloudSupportsTranslation, parseCloudTaskModel, type AsrEngineStatus, type CloudProvider, type LocalAsrEngine } from '@/services/asr'
import * as ipc from '@/services/ipc'
import { localizeBackendError } from '@/services/backendError'
import { basename, formatBytes } from '@/services/utils'
import { dirname, rowProductPath } from '@/services/subtitleProducts'
import type { SubtitleTask, WhisperModelInfo } from '@/types'
const { t: tr } = useI18n()

const store = useTaskStore()
const settingsStore = useSettingsStore()
const message = useMessage()

const videoPath = ref('')
/** 导出语言（多选）：每个选中语言各起一个转写任务 */
const langs = ref<string[]>(['auto'])
/** 「翻译成英文」：开启后每个语言再补一个 translate_to=en 的任务 */
const translateEn = ref(false)
const model = ref('base')
const engine = ref<'whisper' | 'chinese' | 'cloud'>('whisper')
const chineseModel = ref<LocalAsrEngine>('sensevoice')
const cloudProvider = ref<CloudProvider>('groq')
const cloudPreset = computed(() => CLOUD_PROVIDERS[cloudProvider.value])
const cloudBaseUrl = ref(CLOUD_PROVIDERS.groq.baseUrl)
const cloudModel = ref(CLOUD_PROVIDERS.groq.model)
// Session-only: credentials never enter settings, localStorage, or queue records.
const cloudApiKey = ref('')
const cloudAppId = ref('')
const cloudSecretId = ref('')
const cloudSecretKey = ref('')
// A queue retry may represent just the extra translation, not a new batch.
const translationRetry = ref(false)
function clearCloudSecrets() {
  cloudApiKey.value = ''
  cloudAppId.value = ''
  cloudSecretId.value = ''
  cloudSecretKey.value = ''
}
const cloudConfig = computed(() => ({
  provider: cloudProvider.value, base_url: cloudBaseUrl.value.trim(), model: cloudModel.value.trim(),
  api_key: cloudApiKey.value.trim(), app_id: cloudAppId.value.trim(), secret_id: cloudSecretId.value.trim(), secret_key: cloudSecretKey.value.trim(),
}))
const cloudProviderOptions = computed(() => (Object.entries(CLOUD_PROVIDERS) as [CloudProvider, typeof CLOUD_PROVIDERS[CloudProvider]][]).map(([value, preset]) => ({
  label: ['compatible', 'tencent', 'baidu', 'xunfei'].includes(value) ? tr(preset.label) : preset.label, value,
})))
watch(cloudProvider, (provider) => {
  clearCloudSecrets()
  cloudBaseUrl.value = CLOUD_PROVIDERS[provider].baseUrl
  cloudModel.value = CLOUD_PROVIDERS[provider].model
}, { flush: 'sync' })
const supportsTranslation = computed(() => engine.value === 'whisper'
  || (engine.value === 'cloud' && cloudSupportsTranslation(cloudModel.value, cloudProvider.value)))
const engineOptions = computed(() => [
  { label: 'Whisper', value: 'whisper' },
  { label: tr('中文识别'), value: 'chinese' },
  { label: tr('云端'), value: 'cloud' },
])
const chineseModelOptions = [
  { label: 'SenseVoice', value: 'sensevoice' },
  { label: 'Paraformer', value: 'paraformer' },
]
const outputFormat = ref('srt')
const dragging = ref(false)
const starting = ref(false)
const outputDir = ref('')
const choosingDir = ref(false)
const subtitleDir = computed(() => {
  const sourceDir = dirname(videoPath.value)
  return outputDir.value || (/^[A-Za-z]:$/.test(sourceDir) ? `${sourceDir}\\` : sourceDir)
})
let disposed = false
let stopDrag: (() => void) | undefined
let modelLoadVersion = 0
let fileMetaVersion = 0
onUnmounted(() => {
  disposed = true
  modelLoadVersion++
  fileMetaVersion++
  clearCloudSecrets()
  stopDrag?.()
})
/** 本次会话里由前端发起的「翻译成英文」任务（后端 SubtitleTask 不回传 translate_to） */
const translated = ref<Record<string, boolean>>({})

/**
 * 这一行是不是「额外生成的那份英文字幕」。
 * 优先用后端回填的 translate_to（重启后仍在），并保留本会话的临时标记做兜底 ——
 * 两份产物现在文件名不同（`<基名>.<语言>.srt` 与 `<基名>.<语言>-to-en.srt`），
 * 行与行之间可以按名字区分，标记只是让它更一眼可读。
 */
function isTranslated(t: SubtitleTask): boolean {
  return t.translate_to === 'en' || !!translated.value[t.id]
}

/** 模型选项：后端清单优先（内置 + 自定义都在里面），没下载的直接在标签上标出来 */
const modelOptions = computed<{ label: string; value: string }[]>(() => {
  const builtinLabel = new Map(models().map((m) => [m.value as string, m.label as string]))
  const list = modelList.value.length
    ? modelList.value.map((m) => ({
        label: m.downloaded
          ? m.builtin
            ? (builtinLabel.get(m.name) ?? m.name)
            : tr('自定义 · {name}', { name: m.file })
          : `${m.builtin ? (builtinLabel.get(m.name) ?? m.name) : tr('自定义 · {name}', { name: m.file })}${tr('（未下载）')}`,
        value: m.name,
      }))
    : models().map((m) => ({ label: m.label as string, value: m.value as string }))
  // 配置里的模型不在清单里（例如手动拷进去的文件）也要能选中
  const cur = settingsStore.settings.whisper_model
  if (cur && !list.some((o) => o.value === cur)) {
    list.push({ label: tr('自定义 · {name}', { name: cur }), value: cur })
  }
  return list
})
const langOptions = computed(() => {
  const list: { label: string; value: string }[] = languages().map((l) => ({ label: l.label, value: l.value }))
  if (engine.value === 'chinese') {
    if (chineseModel.value === 'paraformer') return list.filter((l) => ['auto', 'zh'].includes(l.value))
    return [...list.filter((l) => ['auto', 'zh', 'en', 'ja', 'ko'].includes(l.value)), { label: tr('粤语'), value: 'yue' }]
  }
  if (engine.value === 'cloud') {
    const supported = cloudAllowedLanguages(cloudProvider.value, cloudModel.value)
    if (supported) return [...list, { label: tr('粤语'), value: 'yue' }].filter((l) => supported.includes(l.value))
  }
  return list
})
const fmtOptions = OUTPUT_FORMATS.map((f) => ({ label: f.toUpperCase(), value: f as string }))

const whisperFound = computed(() => !!store.tools.find((x) => x.name === 'whisper')?.found)
const ffmpegFound = computed(() => !!store.tools.find((x) => x.name === 'ffmpeg')?.found)
const asrStatuses = ref<Partial<Record<LocalAsrEngine, AsrEngineStatus>>>({})
const asrErrors = ref<Partial<Record<LocalAsrEngine, string>>>({})
const asrLoading = ref<Partial<Record<LocalAsrEngine, boolean>>>({})
const asrVersions: Record<LocalAsrEngine, number> = { sensevoice: 0, paraformer: 0 }
const installingAsr = ref<LocalAsrEngine | null>(null)
const selectedAsrStatus = computed(() => asrStatuses.value[chineseModel.value])
const selectedAsrError = computed(() => asrErrors.value[chineseModel.value])
const selectedAsrLoading = computed(() => !!asrLoading.value[chineseModel.value])

async function loadAsrStatus(target = chineseModel.value) {
  if (disposed || !ipc.isTauri()) return
  const version = ++asrVersions[target]
  asrLoading.value[target] = true
  asrErrors.value[target] = ''
  try {
    const status = await asr.status(target)
    if (!disposed && version === asrVersions[target]) asrStatuses.value[target] = status
  } catch (e) {
    if (!disposed && version === asrVersions[target]) asrErrors.value[target] = localizeBackendError(e)
  } finally {
    if (!disposed && version === asrVersions[target]) asrLoading.value[target] = false
  }
}

async function installAsr() {
  if (disposed || installingAsr.value) return
  const target = chineseModel.value
  installingAsr.value = target
  try {
    await asr.install(target)
    if (!disposed) message.success(tr('{name} 安装完成', { name: target === 'sensevoice' ? 'SenseVoice' : 'Paraformer' }))
  } catch (e) {
    if (!disposed) message.error(localizeBackendError(e))
  } finally {
    installingAsr.value = null
    await loadAsrStatus(target)
  }
}

watch([engine, chineseModel], () => {
  if (engine.value === 'chinese') void loadAsrStatus()
})
watch(engine, () => { translateEn.value = false }, { flush: 'sync' })
watch([engine, chineseModel, cloudProvider, cloudModel, langs], () => {
  if (engine.value === 'whisper') return
  const allowed = new Set(langOptions.value.map((l) => l.value))
  const filtered = langs.value.filter((lang) => allowed.has(lang))
  if (filtered.length !== langs.value.length) langs.value = filtered.length ? filtered : [allowed.has('auto') ? 'auto' : (allowed.has('zh') ? 'zh' : [...allowed][0])]
}, { flush: 'sync' })
watch(supportsTranslation, (supported) => { if (!supported) translateEn.value = false }, { flush: 'sync' })
watch([videoPath, engine, cloudProvider, cloudModel, langs, translateEn], () => {
  translationRetry.value = false
}, { flush: 'sync' })

/* ------------------- 模型清单：默认选中已下载的模型 ------------------- */
/**
 * 配置里写的模型可能还没下载（例如默认 base，而机器上只有 tiny）。
 * 这里拉一次后端模型清单：已经下载的模型优先选中，并提示本次会自动用哪一个 ——
 * 与后端 start_subtitle 的自动回落口径一致（BUG-15）。
 */
const modelList = ref<WhisperModelInfo[]>([])
const installedModels = ref<string[]>([])
const modelsLoaded = ref(false)
const modelsLoading = ref(false)
const modelsError = ref('')
const configuredModel = computed(() => settingsStore.settings.whisper_model || '')
const fallbackModel = computed(() =>
  modelsLoaded.value && configuredModel.value && !installedModels.value.includes(configuredModel.value)
    ? installedModels.value[0] || ''
    : '',
)
async function loadModels() {
  if (disposed || !ipc.isTauri()) return
  const version = ++modelLoadVersion
  modelsLoading.value = true
  modelsError.value = ''
  try {
    const rows = await ipc.listWhisperModels()
    if (disposed || version !== modelLoadVersion) return
    modelList.value = rows
    installedModels.value = rows.filter((m) => m.downloaded).map((m) => m.name)
    modelsLoaded.value = true
  } catch (e) {
    if (disposed || version !== modelLoadVersion) return
    modelsLoaded.value = false
    modelsError.value = localizeBackendError(e)
  } finally {
    if (!disposed && version === modelLoadVersion) modelsLoading.value = false
  }
}

/* ------------- 模型就地下载 / 自定义链接导入（原设置页能力，搬到这里） ------------- */
const customSpec = ref('')
/**
 * 初始化回落不改写偏好；初始化后的每次明确选择都正常保存。
 */
let initializingModel = true
/** 'model:<file>' | 'custom' | null —— 同一时刻只跑一个下载 */
const installing = ref<string | null>(null)
const modelProgress = computed(() => store.toolProgress['whisper-model'])
const modelProgressTextValue = computed(() => modelProgressText(modelProgress.value))
const selectedModelInfo = computed(() => modelList.value.find((m) => m.name === model.value) ?? null)
const modelBusy = computed(() => !!selectedModelInfo.value && installing.value === `model:${selectedModelInfo.value.file}`)

/** 下载并启用选中的模型（体积与来源由后端决定；换镜像源/断点续传都会在进度文案里体现） */
async function downloadSelected() {
  const m = selectedModelInfo.value
  if (!m || installing.value) return
  installing.value = `model:${m.file}`
  try {
    const st = await ipc.installWhisperModel(m.name)
    const activate = !disposed && model.value === m.name
    if (activate) await switchModel(m, false)
    else void store.refreshTools()
    if (!disposed) message.success(
      !activate ? tr('{name} 安装完成', { name: m.name }) : st?.origin
        ? tr('{name} 模型已下载并启用（来源：{source}）', { name: m.name, source: st.origin })
        : tr('{name} 模型已下载并启用', { name: m.name }),
    )
  } catch (e: any) {
    message.error(localizeBackendError(e))
  } finally {
    installing.value = null
    await loadModels()
  }
}

/** 切换使用的模型（文件已在磁盘上；换完立刻写回设置 → 下次开窗也是它） */
async function switchModel(m: WhisperModelInfo, notify = true) {
  settingsStore.patch({ whisper_model: m.name } as any)
  model.value = m.name
  await settingsStore.save()
  await loadModels()
  if (notify) message.success(tr('已切换到 {name}', { name: m.builtin ? m.name : m.file }))
  void store.refreshTools()
}

/** 自定义模型：https 直链 / GitHub 直链 / 仓库文件名 / 本地 .bin，装完自动启用 */
async function importCustom() {
  const spec = customSpec.value.trim()
  if (!spec || installing.value) return
  installing.value = 'custom'
  const previousModel = model.value
  try {
    const st = await ipc.installWhisperCustom(spec)
    const file = basename(st.path || '')
    const activate = !disposed && model.value === previousModel && !!file
    if (activate) {
      settingsStore.patch({ whisper_model: file } as any)
      model.value = file
      await settingsStore.save()
    }
    if (!disposed) message.success(activate ? tr('已导入自定义模型并切换为 {name}', { name: file }) : tr('已导入自定义模型'))
    if (customSpec.value.trim() === spec) customSpec.value = ''
    void store.refreshTools()
  } catch (e: any) {
    message.error(localizeBackendError(e))
  } finally {
    installing.value = null
    await loadModels()
  }
}

async function pickModelFile() {
  try {
    const picked = await openDialog({ multiple: false, directory: false })
    if (typeof picked === 'string') customSpec.value = picked
  } catch (e: any) {
    message.error(localizeBackendError(e))
  }
}

/** 下拉里换一个已下载的模型 = 直接启用（没下载的不写设置，等点「下载模型」） */
watch(model, (name) => {
  // 首屏回落（自动改用已下载的模型）不算用户切换，不写回设置
  if (initializingModel) return
  const m = modelList.value.find((x) => x.name === name)
  if (!m?.downloaded || settingsStore.settings.whisper_model === name) return
  settingsStore.patch({ whisper_model: name } as any)
  void settingsStore.save()
  void store.refreshTools()
}, { flush: 'sync' })

/** Starting uses the selected model, so availability must describe that exact selection. */
const ready = computed(() => {
  if (engine.value === 'cloud') return ffmpegFound.value && cloudConfigValid(cloudConfig.value)
  if (engine.value === 'chinese') return ffmpegFound.value && !!selectedAsrStatus.value?.ready
    && !selectedAsrLoading.value && !selectedAsrError.value && installingAsr.value !== chineseModel.value
  return modelsLoaded.value && !modelsLoading.value && !modelsError.value
    && whisperFound.value && !!selectedModelInfo.value?.downloaded && !installing.value
})

/* ------------------------- 多语言 → 多任务 ------------------------- */
/** 本次要起的任务：每个语言一个；开了翻译再给每个语言补一个英文任务 */
const jobs = computed<{ lang: string; translate: boolean }[]>(() => {
  const out: { lang: string; translate: boolean }[] = []
  for (const l of langs.value) {
    if (!translationRetry.value) out.push({ lang: l, translate: false })
    if (supportsTranslation.value && translateEn.value) out.push({ lang: l, translate: true })
  }
  return out
})
const taskCount = computed(() => jobs.value.length)
const startLabel = computed(() => engine.value === 'cloud'
  ? taskCount.value > 1 ? tr('上传并生成字幕（{n} 个任务）', { n: taskCount.value }) : tr('上传并生成字幕')
  : taskCount.value > 1 ? tr('开始转写（{n} 个任务）', { n: taskCount.value }) : tr('生成字幕'))

function langLabel(v: string) {
  if (!v || v === 'auto') return tr('自动检测')
  if (v === 'yue') return tr('粤语')
  return languages().find((l) => l.value === v)?.label ?? v
}
/** 队列行的元信息：语言（+ 英文翻译标记）· 模型 */
function subLabel(t: SubtitleTask) {
  const head = `${langLabel(t.language)}${isTranslated(t) ? ` ${tr('→ 英文')}` : ''}`
  return `${head} · ${t.model}`
}

/**
 * 这一行的产物路径 —— 队列行的显示名 / 大小 / 「打开文件 / 在文件夹中显示」都以它为准。
 *
 * 译文行永远指向自己那份 `<基名>.<源语言标签>-to-en.<扩展名>`：1.8.6 之前的记录把原文文件
 * 记在了译文行上，直接信 `subtitle_path` 就会让译文行的「在文件夹中显示」定位到原文文件 ——
 * 用户看到的正是「（英文）字幕文件不在文件夹里显示」。规则与后端同一套（见 subtitleProducts.ts）。
 */
function productPath(t: SubtitleTask): string {
  return rowProductPath({
    video_path: t.video_path,
    language: t.language,
    model: t.model,
    subtitle_path: t.subtitle_path,
    translated: isTranslated(t),
  })
}

/** 队列行的展示名：有产物就展示**该行自己的产物名**（译文行是 `-to-en` 那份），还没产物（识别中 / 失败）就展示源视频名 */
function rowName(t: SubtitleTask): string {
  if (!t.subtitle_path && t.status !== 'done') return basename(t.video_path) || t.video_path
  return basename(productPath(t)) || basename(t.video_path) || t.video_path
}

/** 队列行的元信息：`语言 · 模型`（+ 英文翻译标记），产物就在磁盘上时再补上真实大小 */
function metaText(t: SubtitleTask): string {
  const size = t.status === 'done' ? sizeText(productPath(t)) : ''
  return size ? `${subLabel(t)} · ${size}` : subLabel(t)
}

async function pick() {
  try {
    const picked = await openDialog({
      multiple: false,
      filters: [
        { name: tr('视频文件'), extensions: ['mp4', 'mkv', 'mov', 'avi', 'webm', 'flv', 'ts', 'm4v', 'mp3', 'm4a', 'wav', 'aac', 'flac'] },
        { name: tr('全部文件'), extensions: ['*'] },
      ],
    })
    if (!disposed && typeof picked === 'string') videoPath.value = picked
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}

async function pickOutputDir() {
  if (choosingDir.value) return
  choosingDir.value = true
  try {
    const picked = await openDialog({ directory: true, multiple: false, defaultPath: subtitleDir.value || settingsStore.settings.download_dir || undefined })
    if (!disposed && typeof picked === 'string') outputDir.value = picked
  } catch (e: any) {
    if (!disposed) message.error(String(e?.message ?? e))
  } finally {
    choosingDir.value = false
  }
}

/** 逐个语言各起一个任务（后端 start_subtitle 只收单语言，这里循环调用） */
async function start() {
  if (disposed || starting.value) return
  if (!videoPath.value.trim()) {
    message.warning(tr('请先选择视频文件'))
    return
  }
  if (!langs.value.length) {
    message.warning(tr('请至少选择一个识别语言'))
    return
  }
  if (!ready.value) return
  const request = {
    videoPath: videoPath.value.trim(),
    model: model.value,
    outputFormat: outputFormat.value,
    outputDir: outputDir.value || null,
  }
  const requestJobs = jobs.value.map((job) => ({ ...job }))
  const requestEngine = engine.value === 'chinese' ? chineseModel.value : engine.value
  const requestCloud = requestEngine === 'cloud' ? { ...cloudConfig.value } : undefined
  translationRetry.value = false
  starting.value = true
  let ok = 0
  const errs: string[] = []
  for (const job of requestJobs) {
    try {
      const opts = {
        ...request,
        language: job.lang,
        translateTo: job.translate ? 'en' : null,
      }
      const t = requestEngine === 'whisper' ? await whisper.start(opts)
        : await asr.start({ ...opts, model: null, engine: requestEngine, cloud: requestCloud })
      store.enqueueSubtitle(t)
      if (job.translate) translated.value = { ...translated.value, [t.id]: true }
      ok++
    } catch (e: any) {
      errs.push(`${langLabel(job.lang)}${job.translate ? ` ${tr('→ 英文')}` : ''}：${String(e?.message ?? e)}`)
    }
  }
  starting.value = false
  if (!disposed && ok) message.success(tr('已开始 {n} 个转写任务', { n: ok }))
  if (!disposed && errs.length) message.error(errs.join(' ｜ '))
}

async function cancel(id: string) {
  await whisper.cancel(id)
}
const resuming = new Set<string>()
async function resume(id: string) {
  if (disposed || starting.value || resuming.has(id)) return
  const t = store.subtitles.find((x) => x.id === id)
  if (!t) return
  if (t.model.startsWith('cloud:')) {
    const restored = parseCloudTaskModel(t.model)
    cloudProvider.value = restored.provider
    clearCloudSecrets()
    cloudModel.value = restored.model
    if (CLOUD_PROVIDERS[restored.provider].editableUrl) cloudBaseUrl.value = ''
    engine.value = 'cloud'
    videoPath.value = t.video_path
    langs.value = [t.language]
    translateEn.value = supportsTranslation.value && isTranslated(t)
    translationRetry.value = translateEn.value
    message.info(tr('请确认云端配置，再点击「上传并生成字幕」重试。'))
    return
  }
  const retryEngine = t.model === 'sensevoice' || t.model === 'paraformer' ? t.model : 'whisper'
  const opts = {
    videoPath: t.video_path,
    language: t.language,
    model: retryEngine === 'whisper' ? t.model : null,
    translateTo: retryEngine === 'whisper' && isTranslated(t) ? 'en' : null,
    outputFormat: outputFormat.value,
  }
  resuming.add(id)
  try {
    if (retryEngine !== 'whisper') {
      await loadAsrStatus(retryEngine)
      if (disposed) return
      if (!ffmpegFound.value || !asrStatuses.value[retryEngine]?.ready || asrErrors.value[retryEngine] || installingAsr.value) {
        chineseModel.value = retryEngine
        engine.value = 'chinese'
        message.warning(tr('请先安装中文识别组件和 FFmpeg，再重试。'))
        return
      }
    }
    const nt = retryEngine === 'whisper' ? await whisper.start(opts) : await asr.start({ ...opts, engine: retryEngine })
    store.enqueueSubtitle(nt)
    if (isTranslated(t)) translated.value = { ...translated.value, [nt.id]: true }
  } catch (e: any) {
    if (!disposed) message.error(String(e?.message ?? e))
  } finally {
    resuming.delete(id)
  }
}
async function remove(id: string) {
  try {
    await whisper.remove(id)
    // 后端已删库：本地立刻摘行（store 是队列的唯一数据出口，不用切页/重启）
    store.dropTask(id)
  } catch (e: any) {
    // 后端失败：一行都不动，把原因摊给用户
    message.error(String(e?.message ?? e))
  }
}

/** 这一行的产物确实不在磁盘上时的提示（不再让 explorer 弹个空窗口假装成功） */
function missingFileMessage(p: string): string {
  return tr('字幕文件不存在（可能尚未生成，或已被移动/删除）：{path}', { path: p })
}

async function openFile(t: SubtitleTask) {
  const p = productPath(t)
  if (!subtitleExists(t)) {
    message.error(missingFileMessage(p))
    return
  }
  try {
    await ipc.openPath(p)
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}
async function reveal(t: SubtitleTask) {
  const p = productPath(t)
  if (!subtitleExists(t)) {
    message.error(missingFileMessage(p))
    return
  }
  try {
    await ipc.revealPath(p)
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}

/* ------------------------- 队列行的真实文件信息 ------------------------- */
/**
 * 队列行要显示产物大小、并诚实判断产物是否还在磁盘上（「文件已丢失」），但 SubtitleTask
 * 只带路径不带大小 —— 走后端 file_sizes 批量取（只读元数据，不启子进程）。
 * 与 Converter.vue 一样直接用 invoke，不在 ipc.ts 里加封装（该文件由并行改动维护）。
 */
const fileMeta = ref<Record<string, { size: number | null; exists: boolean }>>({})

async function refreshFileMeta() {
  const version = ++fileMetaVersion
  if (!ipc.isTauri()) return
  const paths = new Set<string>()
  // 取这一行自己的产物（译文行是 `-to-en` 那份；老记录里记的是原文文件，不能拿它当存在性依据）
  for (const t of store.subtitles) {
    const p = productPath(t)
    if (p) paths.add(p)
  }
  if (!paths.size) {
    fileMeta.value = {}
    return
  }
  try {
    const rows = await invoke<{ path: string; size: number | null; exists: boolean }[]>('file_sizes', {
      paths: [...paths],
    })
    if (disposed || version !== fileMetaVersion) return
    const next: typeof fileMeta.value = {}
    for (const r of rows) next[r.path] = { size: r.size ?? null, exists: !!r.exists }
    fileMeta.value = next
  } catch (e) {
    // 浏览器预览 / 老后端没有该命令：行降级成「只有文件名与语言模型」，不弹错
    console.warn('读取字幕文件信息失败', e)
  }
}

/** 产物大小（只有真实存在才算数） */
function sizeText(p: string): string {
  const m = fileMeta.value[p]
  return m && m.exists && m.size ? formatBytes(m.size) : ''
}

/** 这一行的产物是否真的还在磁盘上（决定「打开 / 显示」按钮与「文件已丢失」提示） */
function subtitleExists(t: SubtitleTask): boolean {
  const p = productPath(t)
  return !!p && !!fileMeta.value[p]?.exists
}

/* 队列里任务一出现 / 状态一变（完成时）就刷新真实大小与存在性；进度跳动不触发（签名里不含 progress） */
watch(
  () => JSON.stringify(store.subtitles.map((t) => [t.id, t.status, productPath(t)])),
  () => void refreshFileMeta(),
  { immediate: true },
)

onMounted(async () => {
  if (!store.ready) await store.bootstrap()
  if (disposed) return
  if (!settingsStore.loaded) await settingsStore.load()
  if (disposed) return
  await loadModels()
  if (disposed) return
  // 默认选中「真正能用」的模型：配置的已下载就用它，否则用已安装的（后端也会这样回落）
  const configured = settingsStore.settings.whisper_model || ''
  model.value = installedModels.value.includes(configured)
    ? configured
    : installedModels.value[0] || configured || 'base'
  // 应用设置中的字幕默认值（默认识别语言是单选，落成初始选中项）
  if (settingsStore.settings.subtitle_language) langs.value = [settingsStore.settings.subtitle_language]
  if (settingsStore.settings.subtitle_format) outputFormat.value = settingsStore.settings.subtitle_format
  initializingModel = false
  try {
    const wv = getCurrentWebview()
    const unlisten = await wv.onDragDropEvent((ev) => {
      if (disposed) return
      const p = ev.payload as any
      if (p?.type === 'over') dragging.value = true
      else if (p?.type === 'drop') {
        dragging.value = false
        const paths: string[] = p.paths || []
        if (paths.length) videoPath.value = paths[0]
      } else if (p?.type === 'leave') dragging.value = false
    })
    if (disposed) unlisten()
    else stopDrag = unlisten
  } catch {
    /* ignore */
  }
})
</script>

<template>
  <div class="umi-page">
    <section class="umi-card p-4 transition-all" :class="dragging ? 'drop-active' : ''">
      <div class="umi-card-title">
        <NIcon :size="14" :component="ChatbubblesOutline" class="text-accent-pink" />{{ $t('视频 / 音频文件') }}
        <span class="umi-spacer" />
      </div>

      <div class="flex gap-2">
        <input v-model="videoPath" class="umi-input flex-1" :placeholder="$t('选择文件或直接拖拽到窗口内…')" readonly data-test="subtitle-path" />
        <button class="umi-btn-primary whitespace-nowrap" data-test="subtitle-pick" @click="pick">
          <span class="flex items-center gap-1.5">
            <NIcon :size="14" :component="DocumentOutline" />{{ $t('选择文件') }}</span>
        </button>
      </div>

      <div class="mt-3">
        <label class="umi-label">{{ $t('输出目录') }}</label>
        <div class="flex gap-2">
          <input :value="subtitleDir" class="umi-input min-w-0 flex-1" readonly :title="subtitleDir" :placeholder="$t('默认与源文件同目录')" data-test="subtitle-output-path" />
          <button class="umi-btn umi-btn-sm whitespace-nowrap" :disabled="choosingDir" data-test="subtitle-output-pick" @click="pickOutputDir">
            <span class="flex items-center gap-1"><NIcon :size="14" :component="FolderOpenOutline" />{{ $t('选择目录') }}</span>
          </button>
        </div>
      </div>

      <div class="mt-3">
        <label class="umi-label">{{ $t('识别引擎') }}</label>
        <NSelect v-model:value="engine" :options="engineOptions" size="small" data-test="subtitle-engine-select" />
      </div>

      <div v-if="engine === 'chinese'" class="mt-3" data-test="subtitle-chinese-config">
        <label class="umi-label">{{ $t('中文模型') }}</label>
        <NSelect v-model:value="chineseModel" :options="chineseModelOptions" size="small" data-test="subtitle-chinese-model" />
        <div class="mt-1.5 flex flex-wrap items-center gap-2">
          <span class="umi-hint min-w-0 flex-1" data-test="subtitle-asr-status">
            {{ selectedAsrLoading ? $t('加载中…') : selectedAsrError || (selectedAsrStatus?.ready ? $t('已就绪 · 本地识别，音频不上传') : selectedAsrStatus?.message || $t('尚未安装中文识别组件')) }}
          </span>
          <button v-if="selectedAsrError" class="umi-btn umi-btn-xs" data-test="subtitle-asr-retry" @click="loadAsrStatus()">{{ $t('刷新') }}</button>
          <button
            v-if="!selectedAsrStatus?.ready || installingAsr === chineseModel"
            class="umi-btn-primary umi-btn-xs shrink-0"
            :disabled="!!installingAsr || selectedAsrLoading"
            data-test="subtitle-asr-install"
            @click="installAsr"
          >{{ installingAsr === chineseModel ? $t('安装中…') : $t('安装引擎与模型') }}</button>
        </div>
        <p class="umi-hint mt-1">{{ $t('仅在点击安装后下载组件；中文模型只生成原文字幕。') }}</p>
      </div>

      <div v-if="engine === 'cloud'" class="mt-3 space-y-2" data-test="subtitle-cloud-config">
        <div>
          <label class="umi-label">{{ $t('云端服务商') }}</label>
          <NSelect v-model:value="cloudProvider" :options="cloudProviderOptions" size="small" data-test="subtitle-cloud-provider" />
        </div>
        <div class="grid gap-2 sm:grid-cols-2">
          <div v-if="cloudPreset.editableUrl">
            <label class="umi-label">{{ cloudProvider === 'azure' ? $t('Azure 资源地址') : $t('接口地址（Base URL）') }}</label>
            <input v-model="cloudBaseUrl" class="umi-input w-full" type="url" spellcheck="false" autocomplete="off" :placeholder="cloudProvider === 'azure' ? 'https://your-resource.cognitiveservices.azure.com' : ''" data-test="subtitle-cloud-base" />
          </div>
          <div v-if="cloudPreset.showModel">
            <label class="umi-label">{{ $t('云端模型') }}</label>
            <input v-model="cloudModel" class="umi-input w-full" spellcheck="false" autocomplete="off" data-test="subtitle-cloud-model" />
          </div>
        </div>
        <div class="grid gap-2 sm:grid-cols-2">
          <div v-if="cloudPreset.credentials.includes('app_id')">
            <label class="umi-label">{{ $t('应用 ID') }}</label>
            <input v-model="cloudAppId" class="umi-input w-full" type="password" autocomplete="off" spellcheck="false" data-test="subtitle-cloud-app-id" />
          </div>
          <div v-if="cloudPreset.credentials.includes('api_key')">
            <label class="umi-label">{{ $t('API 密钥（仅本次会话）') }}</label>
            <input v-model="cloudApiKey" class="umi-input w-full" type="password" autocomplete="off" spellcheck="false" data-test="subtitle-cloud-key" />
          </div>
          <div v-if="cloudPreset.credentials.includes('secret_id')">
            <label class="umi-label">Secret ID</label>
            <input v-model="cloudSecretId" class="umi-input w-full" type="password" autocomplete="off" spellcheck="false" data-test="subtitle-cloud-secret-id" />
          </div>
          <div v-if="cloudPreset.credentials.includes('secret_key')">
            <label class="umi-label">Secret Key</label>
            <input v-model="cloudSecretKey" class="umi-input w-full" type="password" autocomplete="off" spellcheck="false" data-test="subtitle-cloud-secret-key" />
          </div>
        </div>
        <button class="umi-btn umi-btn-sm" :disabled="!cloudApiKey && !cloudAppId && !cloudSecretId && !cloudSecretKey" data-test="subtitle-cloud-clear-key" @click="clearCloudSecrets">{{ $t('清空密钥') }}</button>
        <p class="umi-hint" data-test="subtitle-cloud-notice">{{ $t('点击「上传并生成字幕」会将音频上传至所填接口，可能产生服务费用。密钥不会保存。') }}</p>
        <p v-if="translationRetry" class="umi-hint" data-test="subtitle-translation-retry">{{ $t('请确认云端配置，再点击「上传并生成字幕」重试。') }} {{ $t('→ 英文') }}</p>
        <p v-if="cloudProvider === 'compatible'" class="umi-hint">{{ $t('需支持分段时间戳的 Whisper 兼容接口；远程地址须使用 HTTPS，本机接口可不填密钥。') }}</p>
        <p v-if="cloudProvider === 'baidu'" class="umi-hint">{{ $t('百度极速转写目前需要开通邀测权限。') }}</p>
        <p v-if="cloudProvider === 'xunfei'" class="umi-hint">{{ $t('讯飞默认自动识别中文、英文和方言。') }}</p>
      </div>

      <div v-if="engine !== 'whisper' && !ffmpegFound" class="mt-2.5 rounded-xl border border-amber-500/25 bg-amber-500/10 px-3 py-2 text-[12px] text-amber-200/90">
        {{ $t('请先在设置中安装 FFmpeg，以提取音频。') }}
      </div>

      <div
        v-if="engine === 'whisper' && !ready"
        class="mt-2.5 rounded-xl border border-amber-500/25 bg-amber-500/10 px-3 py-2 text-[12px] text-amber-200/90"
      >
        <div class="flex items-center gap-2">
          <NIcon :size="14" :component="SparklesOutline" />
          <template v-if="modelsError">
            <span class="min-w-0 flex-1">{{ $t('读取模型清单失败') }}：{{ modelsError }}</span>
            <button class="umi-btn umi-btn-xs shrink-0" data-test="subtitle-model-retry" @click="loadModels">{{ $t('刷新') }}</button>
          </template>
          <span v-else-if="!modelsLoaded || modelsLoading">{{ $t('加载中…') }}</span>
          <span v-else-if="!whisperFound">{{ $t('尚未安装 Whisper 引擎，请前往「设置 → 依赖工具」一键安装。') }}</span>
          <span v-else-if="installing">{{ $t('安装中…') }}</span>
          <span v-else>{{ $t('尚未下载语音模型「{model}」—— 点下方「下载模型」直接安装，或用「自定义模型」贴链接导入。', { model }) }}</span>
        </div>
      </div>
      <!-- 配置的模型没下载、但装了别的模型：本次会自动改用已安装的那个（后端同一套回落逻辑） -->
      <div
        v-else-if="engine === 'whisper' && fallbackModel"
        class="mt-2.5 rounded-xl border border-sky-500/25 bg-sky-500/10 px-3 py-2 text-[12px] text-sky-200/90"
        data-test="subtitle-model-fallback"
      >
        <div class="flex items-center gap-2">
          <NIcon :size="14" :component="SparklesOutline" />
          <span>{{ $t('配置的模型「{model}」尚未下载，本次将自动使用已安装的「{fallback}」。', { model: configuredModel, fallback: fallbackModel }) }}</span>
        </div>
      </div>

      <!-- 识别语言（下拉多选，与下载页「字幕语言」同一款控件）：每个选中语言各起一个转写任务 -->
      <div class="mt-3">
        <label class="umi-label">{{ $t('识别语言（可多选）') }}</label>
        <NSelect
          v-model:value="langs"
          multiple
          :options="langOptions"
          size="small"
          :placeholder="$t('选择要识别的语言')"
          data-test="lang-select"
        />
      </div>

      <div v-if="engine === 'whisper'" class="mt-3" data-test="subtitle-params-visible">
        <div>
          <label class="umi-label">{{ $t('Whisper 模型') }}</label>
          <NSelect
            v-model:value="model"
            :options="modelOptions"
            size="small"
            :consistent-menu-width="false"
            data-test="subtitle-model-select"
          />
          <!-- 选中的模型没下载 → 就地下载（进度事件与设置页同源，含换源 / 断点续传提示） -->
          <div
            v-if="selectedModelInfo && !selectedModelInfo.downloaded"
            class="mt-1.5"
            data-test="subtitle-model-download"
          >
            <button
              class="umi-btn-primary umi-btn-xs"
              :disabled="!!installing"
              data-test="subtitle-model-download-btn"
              @click="downloadSelected"
            >
              <span class="flex items-center gap-1">
                <NIcon :size="12" :component="DownloadOutline" />
                {{ modelBusy ? $t('下载中…') : $t('下载模型（约 {size}）', { size: selectedModelInfo.size_hint }) }}
              </span>
            </button>
            <NProgress
              v-if="modelBusy"
              class="mt-1"
              type="line"
              :percentage="Math.round(modelProgress?.percent ?? 0)"
              :height="3"
              :show-indicator="false"
              color="rgb(var(--umi-a500))"
            />
            <div v-if="modelBusy" class="s-text-3 mt-0.5 truncate text-[10px]" :title="modelProgressTextValue">
              {{ modelProgressTextValue || $t('准备中…') }}
            </div>
          </div>
          <div v-else-if="selectedModelInfo" class="s-text-3 mt-1 text-[10px]" data-test="subtitle-model-status">
            {{ $t('已下载 · 占用 {size}', { size: formatBytes(selectedModelInfo.size_bytes) }) }}
          </div>
        </div>
      </div>

      <div class="mt-3">
        <label class="umi-label">{{ $t('导出格式') }}</label>
        <NSelect v-model:value="outputFormat" :options="fmtOptions" size="small" />
      </div>
      <SettingsGroup v-if="supportsTranslation" :title="$t('高级选项')" :icon="OptionsOutline" class="mt-3" always>
        <div>
          <label class="umi-label">{{ $t('翻译成英文') }}</label>
          <div class="flex h-[34px] items-center gap-2">
            <NSwitch v-model:value="translateEn" size="small" data-test="translate-en" />
            <span class="umi-hint">{{ $t('每个语言额外生成一份英文字幕') }}</span>
          </div>
        </div>
      </SettingsGroup>

      <!-- 自定义模型：贴链接 / 本地文件直接安装并启用（收成一行，不占版面）。
           常显（不进折叠区组）：模型安装入口在简单模式下也要一眼可见 -->
      <div v-if="engine === 'whisper'" class="mt-2.5 flex flex-wrap items-center gap-1.5" data-test="subtitle-custom-model">
        <span class="s-text-3 shrink-0 text-[11px]">{{ $t('自定义模型') }}</span>
        <input
          v-model="customSpec"
          class="umi-input min-w-[200px] flex-1 !h-[30px] !text-[11.5px]"
          :placeholder="$t('https://…/ggml-xxx.bin 或 GitHub 直链 / 仓库文件名 / 本地 .bin 路径')"
          data-test="subtitle-custom-input"
        />
        <button class="umi-btn umi-btn-sm whitespace-nowrap" @click="pickModelFile">
          <span class="flex items-center gap-1">
            <NIcon :size="12" :component="FolderOpenOutline" />{{ $t('选择文件') }}</span>
        </button>
        <button
          class="umi-btn-primary umi-btn-sm whitespace-nowrap"
          :disabled="!!installing || !customSpec.trim()"
          data-test="subtitle-custom-install"
          @click="importCustom"
        >
          {{ installing === 'custom' ? $t('安装中…') : $t('安装') }}
        </button>
        <div v-if="installing === 'custom'" class="min-w-full">
          <NProgress
            type="line"
            :percentage="Math.round(modelProgress?.percent ?? 0)"
            :height="3"
            :show-indicator="false"
            color="rgb(var(--umi-a500))"
          />
          <div class="s-text-3 mt-0.5 truncate text-[10px]" :title="modelProgressTextValue">
            {{ modelProgressTextValue || $t('准备中…') }}
          </div>
        </div>
      </div>

      <div class="mt-3 flex flex-wrap items-center justify-between gap-3 border-t s-border-soft pt-3">
        <span class="umi-hint">{{ engine === 'whisper' ? $t('流程：FFmpeg 提取音频 → Whisper 语音识别 → 生成时间轴 → 导出字幕文件') : engine === 'chinese' ? $t('流程：提取音频 → 本地中文识别 → 导出字幕文件') : $t('流程：提取音频 → 上传至所填接口 → 导出字幕文件') }}</span>
        <button
          class="umi-btn-primary"
          :disabled="!videoPath || !langs.length || !ready || starting"
          data-test="subtitle-start"
          @click="start"
        >
          <span class="flex items-center gap-1.5">
            <NIcon :size="14" :component="SparklesOutline" />{{ startLabel }}</span>
        </button>
      </div>
    </section>

    <section>
      <div class="umi-head">
        <div class="s-text-2 flex items-center gap-2 text-[12px] font-medium">{{ $t('字幕队列') }}
          <span class="s-surface-2 s-text-3 rounded-md px-1.5 py-0.5 text-[10px] tabular-nums">{{ store.subtitles.length }}</span>
        </div>
      </div>
      <!-- 与转换页同一款简约行列表：无缩略图 / 无预览入口 / 无卡片；队列自己内滚 -->
      <div
        v-if="store.subtitles.length"
        data-role="subtitle-queue"
        class="s-border-soft s-surface-2 max-h-[42vh] overflow-y-auto rounded-lg border px-1"
      >
        <ConvertRow
          v-for="t in store.subtitles"
          :key="t.id"
          row-role="subtitle-row"
          :name="rowName(t)"
          :status="t.status"
          :progress="t.progress"
          :src-text="metaText(t)"
          :error="t.error"
          :file-path="t.status === 'done' ? productPath(t) : null"
          :file-exists="t.status === 'done' ? subtitleExists(t) : undefined"
          @open="openFile(t)"
          @reveal="reveal(t)"
          @remove="remove(t.id)"
          @cancel="cancel(t.id)"
          @resume="resume(t.id)"
        />
      </div>
      <div v-else class="flex items-center justify-center gap-2 py-6 s-text-3">
        <NIcon :size="26" :component="ChatbubblesOutline" />
        <div class="text-[12px]">{{ $t('还没有字幕任务') }}</div>
      </div>
    </section>
  </div>
</template>
