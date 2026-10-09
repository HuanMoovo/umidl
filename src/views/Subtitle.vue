<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import { computed, onMounted, ref, watch } from 'vue'
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
import ModeSwitch from '@/components/ModeSwitch.vue'
import { useTaskStore } from '@/stores/tasks'
import { useSettingsStore } from '@/stores/settings'
import { whisper, languages, OUTPUT_FORMATS, models, modelProgressText } from '@/services/whisper'
import * as ipc from '@/services/ipc'
import { localizeBackendError } from '@/services/backendError'
import { basename, formatBytes } from '@/services/utils'
import { rowProductPath } from '@/services/subtitleProducts'
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
const outputFormat = ref('srt')
const dragging = ref(false)
const starting = ref(false)
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
const langOptions = computed(() => languages().map((l) => ({ label: l.label, value: l.value as string })))
const fmtOptions = OUTPUT_FORMATS.map((f) => ({ label: f.toUpperCase(), value: f as string }))

const modelReady = computed(() => {
  const t = store.tools.find((x) => x.name === 'whisper-model')
  return !!t?.found
})
const whisperFound = computed(() => !!store.tools.find((x) => x.name === 'whisper')?.found)

/* ------------------- 模型清单：默认选中已下载的模型 ------------------- */
/**
 * 配置里写的模型可能还没下载（例如默认 base，而机器上只有 tiny）。
 * 这里拉一次后端模型清单：已经下载的模型优先选中，并提示本次会自动用哪一个 ——
 * 与后端 start_subtitle 的自动回落口径一致（BUG-15）。
 */
const modelList = ref<WhisperModelInfo[]>([])
const installedModels = ref<string[]>([])
const modelsLoaded = ref(false)
const configuredModel = computed(() => settingsStore.settings.whisper_model || '')
const fallbackModel = computed(() =>
  modelsLoaded.value && configuredModel.value && !installedModels.value.includes(configuredModel.value)
    ? installedModels.value[0] || ''
    : '',
)
async function loadModels() {
  if (!ipc.isTauri()) return
  try {
    const rows = await ipc.listWhisperModels()
    modelList.value = rows
    installedModels.value = rows.filter((m) => m.downloaded).map((m) => m.name)
  } catch (e) {
    console.warn('读取模型清单失败', e)
  } finally {
    modelsLoaded.value = true
  }
}

/* ------------- 模型就地下载 / 自定义链接导入（原设置页能力，搬到这里） ------------- */
const customSpec = ref('')
/**
 * 首屏自动选中的模型（配置的已下载就用它，否则回落）。
 * 它不算「用户换过」—— 回落不该改写用户存的偏好，所以 watcher 里要放它一马。
 */
let initialModel = ''
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
    await switchModel(m, false)
    message.success(
      st?.origin
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
  model.value = m.name
  settingsStore.patch({ whisper_model: m.name } as any)
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
  try {
    const st = await ipc.installWhisperCustom(spec)
    const file = basename(st.path || '')
    if (file) {
      model.value = file
      settingsStore.patch({ whisper_model: file } as any)
      await settingsStore.save()
    }
    message.success(file ? tr('已导入自定义模型并切换为 {name}', { name: file }) : tr('已导入自定义模型'))
    customSpec.value = ''
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
  if (!initialModel || name === initialModel) return
  const m = modelList.value.find((x) => x.name === name)
  if (!m?.downloaded || settingsStore.settings.whisper_model === name) return
  settingsStore.patch({ whisper_model: name } as any)
  void settingsStore.save()
  void store.refreshTools()
})

/** 一个可用模型都没有（配置的没下载、也没装别的）时才拦人 */
const noModelAtAll = computed(() => modelsLoaded.value && !modelReady.value && !installedModels.value.length)
const ready = computed(() => !noModelAtAll.value && whisperFound.value)

/* ------------------------- 多语言 → 多任务 ------------------------- */
/** 本次要起的任务：每个语言一个；开了翻译再给每个语言补一个英文任务 */
const jobs = computed<{ lang: string; translate: boolean }[]>(() => {
  const out: { lang: string; translate: boolean }[] = []
  for (const l of langs.value) {
    out.push({ lang: l, translate: false })
    if (translateEn.value) out.push({ lang: l, translate: true })
  }
  return out
})
const taskCount = computed(() => jobs.value.length)
const startLabel = computed(() =>
  taskCount.value > 1 ? tr('开始转写（{n} 个任务）', { n: taskCount.value }) : tr('生成字幕'),
)

function langLabel(v: string) {
  if (!v || v === 'auto') return tr('自动检测')
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
    if (typeof picked === 'string') videoPath.value = picked
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}

/** 逐个语言各起一个任务（后端 start_subtitle 只收单语言，这里循环调用） */
async function start() {
  if (!videoPath.value) {
    message.warning(tr('请先选择视频文件'))
    return
  }
  if (!langs.value.length) {
    message.warning(tr('请至少选择一个识别语言'))
    return
  }
  starting.value = true
  let ok = 0
  const errs: string[] = []
  for (const job of jobs.value) {
    try {
      const t = await whisper.start({
        videoPath: videoPath.value,
        language: job.lang,
        model: model.value,
        translateTo: job.translate ? 'en' : null,
        outputFormat: outputFormat.value,
      })
      store.enqueueSubtitle(t)
      if (job.translate) translated.value = { ...translated.value, [t.id]: true }
      ok++
    } catch (e: any) {
      errs.push(`${langLabel(job.lang)}${job.translate ? ` ${tr('→ 英文')}` : ''}：${String(e?.message ?? e)}`)
    }
  }
  starting.value = false
  if (ok) message.success(tr('已开始 {n} 个转写任务', { n: ok }))
  if (errs.length) message.error(errs.join(' ｜ '))
}

async function cancel(id: string) {
  await whisper.cancel(id)
}
async function resume(id: string) {
  const t = store.subtitles.find((x) => x.id === id)
  if (!t) return
  try {
    const nt = await whisper.start({
      videoPath: t.video_path,
      language: t.language,
      model: t.model,
      translateTo: isTranslated(t) ? 'en' : null,
      outputFormat: outputFormat.value,
    })
    store.enqueueSubtitle(nt)
    if (isTranslated(t)) translated.value = { ...translated.value, [nt.id]: true }
  } catch (e: any) {
    message.error(String(e?.message ?? e))
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
  () => store.subtitles.map((t) => `${t.id}:${t.status}`).join('|'),
  () => void refreshFileMeta(),
  { immediate: true },
)

onMounted(async () => {
  if (!store.ready) await store.bootstrap()
  if (!settingsStore.loaded) await settingsStore.load()
  await loadModels()
  // 默认选中「真正能用」的模型：配置的已下载就用它，否则用已安装的（后端也会这样回落）
  const configured = settingsStore.settings.whisper_model || ''
  model.value = installedModels.value.includes(configured)
    ? configured
    : installedModels.value[0] || configured || 'base'
  // 应用设置中的字幕默认值（默认识别语言是单选，落成初始选中项）
  if (settingsStore.settings.subtitle_language) langs.value = [settingsStore.settings.subtitle_language]
  if (settingsStore.settings.subtitle_format) outputFormat.value = settingsStore.settings.subtitle_format
  initialModel = model.value
  try {
    const wv = getCurrentWebview()
    await wv.onDragDropEvent((ev) => {
      const p = ev.payload as any
      if (p?.type === 'over') dragging.value = true
      else if (p?.type === 'drop') {
        dragging.value = false
        const paths: string[] = p.paths || []
        if (paths.length) videoPath.value = paths[0]
      } else if (p?.type === 'leave') dragging.value = false
    })
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
        <!-- 简单 / 高级模式（与设置页同一状态源）：简单 = 隐藏「高级选项」 -->
        <ModeSwitch />
      </div>

      <div class="flex gap-2">
        <input v-model="videoPath" class="umi-input flex-1" :placeholder="$t('选择文件或直接拖拽到窗口内…')" readonly data-test="subtitle-path" />
        <button class="umi-btn-primary whitespace-nowrap" data-test="subtitle-pick" @click="pick">
          <span class="flex items-center gap-1.5">
            <NIcon :size="14" :component="DocumentOutline" />{{ $t('选择文件') }}</span>
        </button>
      </div>

      <div
        v-if="!ready"
        class="mt-2.5 rounded-xl border border-amber-500/25 bg-amber-500/10 px-3 py-2 text-[12px] text-amber-200/90"
      >
        <div class="flex items-center gap-2">
          <NIcon :size="14" :component="SparklesOutline" />
          <span v-if="!whisperFound">{{ $t('尚未安装 Whisper 引擎，请前往「设置 → 依赖工具」一键安装。') }}</span>
          <span v-else>{{ $t('尚未下载语音模型「{model}」—— 点下方「下载模型」直接安装，或用「自定义模型」贴链接导入。', { model }) }}</span>
        </div>
      </div>
      <!-- 配置的模型没下载、但装了别的模型：本次会自动改用已安装的那个（后端同一套回落逻辑） -->
      <div
        v-else-if="fallbackModel"
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

      <div class="mt-3" data-test="subtitle-params-visible">
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

      <!-- 高级选项（1.10）：导出格式 / 翻译成英文收进默认折叠区组；简单模式整体不渲染 -->
      <SettingsGroup :title="$t('高级选项')" :icon="OptionsOutline" class="mt-3">
        <div class="grid gap-3 sm:grid-cols-2">
          <div>
            <label class="umi-label">{{ $t('导出格式') }}</label>
            <NSelect v-model:value="outputFormat" :options="fmtOptions" size="small" />
          </div>
          <div>
            <label class="umi-label">{{ $t('翻译成英文') }}</label>
            <div class="flex h-[34px] items-center gap-2">
              <NSwitch v-model:value="translateEn" size="small" data-test="translate-en" />
              <span class="umi-hint">{{ $t('每个语言额外生成一份英文字幕') }}</span>
            </div>
          </div>
        </div>
      </SettingsGroup>

      <!-- 自定义模型：贴链接 / 本地文件直接安装并启用（收成一行，不占版面）。
           常显（不进折叠区组）：模型安装入口在简单模式下也要一眼可见 -->
      <div class="mt-2.5 flex flex-wrap items-center gap-1.5" data-test="subtitle-custom-model">
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
        <span class="umi-hint">{{ $t('流程：FFmpeg 提取音频 → Whisper 语音识别 → 生成时间轴 → 导出字幕文件') }}</span>
        <button
          class="umi-btn-primary"
          :disabled="!videoPath || starting"
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
      <div v-else class="umi-card flex flex-col items-center gap-2 py-12 s-text-3">
        <NIcon :size="26" :component="ChatbubblesOutline" />
        <div class="text-[12px]">{{ $t('还没有字幕任务') }}</div>
      </div>
    </section>
  </div>
</template>
