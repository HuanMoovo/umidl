<script setup lang="ts">
/**
 * 批量导入面板（1.6 新增，挂在下载页单链接输入区旁边）
 *
 *  - 多行文本框：每行一个链接，实时显示「识别到 N 条链接，去重后 M 条」
 *  - 从 txt 文件导入：文件选择 → read_text_file 读回内容填入文本框
 *  - 全部加入队列：调用后端 enqueue_links（去重 / 自动开始都在后端），
 *    提示文案与统计一律取自后端返回值，前端不自行丢弃任何链接。
 */
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { NIcon, NSpin, useMessage } from 'naive-ui'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import {
  AlertCircleOutline,
  CheckmarkCircleOutline,
  CloudUploadOutline,
  DocumentOutline,
  ListOutline,
  PlayCircleOutline,
  ShieldCheckmarkOutline,
} from '@vicons/ionicons5'
import { useTaskStore } from '@/stores/tasks'
import { useSettingsStore } from '@/stores/settings'
import {
  BATCH_TEXT_MAX_CHARS,
  ENQUEUE_LIMIT,
  createParseCursor,
  enqueueLinks,
  parseChunk,
  truncateBatchText,
  readTextFile,
  type EnqueueResult,
  type ParseCursor,
} from '@/services/batch'
import { basename } from '@/services/utils'
const { t: tr } = useI18n()

/** 紧凑嵌入形态：外层不再包 umi-card（由父组件提供卡片），避免卡片套卡片 */
defineProps<{ bare?: boolean }>()

const store = useTaskStore()
const settingsStore = useSettingsStore()
const message = useMessage()

const text = ref('')
const busy = ref(false)
const importing = ref(false)
/** 后端返回的入队统计（唯一提示来源） */
const result = ref<EnqueueResult | null>(null)
/** 后端错误原文 */
const error = ref('')

/* ============================================================
   超长粘贴保护 + 分块解析（BUG-07）
   之前：textarea 直接 v-model → parseLinks 在主线程一次算完，
   粘 100 万行（36.9 MB）会冻结窗口约 10 秒。现在：
    1. 文本先按 BATCH_TEXT_MAX_CHARS 截断（行边界优先），超出部分丢弃并提示；
    2. 计数改为分块解析：一次只推进 PARSE_CHUNK_LINES 行，
       长输入下先显示「正在解析…」，主线程始终保持可响应。
   ============================================================ */
const TEXT_LIMIT_KB = Math.round(BATCH_TEXT_MAX_CHARS / 1024)
/** 是否发生过截断（显示常驻提示） */
const truncated = ref(false)
/** 计数是否还在分块推进中 */
const parsing = ref(false)
/** 已识别条数（含重复） */
const parsedTotal = ref(0)
/** 去重后的条数 */
const parsedUnique = ref(0)
/** textarea 元素（非受控：DOM 值只由本组件写入，避免 Vue 在重渲染时改写 36.9MB 的内容） */
const taRef = ref<HTMLTextAreaElement | null>(null)

let cursor: ParseCursor | null = null
let parseTimer: ReturnType<typeof setTimeout> | undefined

const overflow = computed(() => Math.max(0, parsedUnique.value - ENQUEUE_LIMIT))
const canEnqueue = computed(() => !busy.value && parsedUnique.value > 0)

function notifyTruncated() {
  message.warning(tr('已截断：单次最多 {kb} KB（超出部分已丢弃，可分批粘贴）', { kb: TEXT_LIMIT_KB }))
}

/** 取消未完成的解析步进 */
function stopParse() {
  if (parseTimer !== undefined) {
    clearTimeout(parseTimer)
    parseTimer = undefined
  }
  cursor = null
}

/** 一步解析（最多 PARSE_CHUNK_LINES 行）→ 刷新计数 → 没算完就排下一块 */
function stepParse() {
  parseTimer = undefined
  const c = cursor
  if (!c) return
  parseChunk(c, text.value)
  parsedTotal.value = c.total
  parsedUnique.value = c.unique.length
  if (c.done) {
    parsing.value = false
    cursor = null
    return
  }
  parseTimer = setTimeout(stepParse, 0)
}

/** 重新计数：首块同步完成（短文本一步到位，不闪「正在解析…」），过长才分块推进 */
function restartParse() {
  stopParse()
  const c = createParseCursor()
  parseChunk(c, text.value)
  parsedTotal.value = c.total
  parsedUnique.value = c.unique.length
  if (c.done) {
    parsing.value = false
    return
  }
  cursor = c
  parsing.value = true
  parseTimer = setTimeout(stepParse, 0)
}

/**
 * 把**可见**文本框里的超长内容换掉（BUG-07 的核心处理）。
 *
 * 真机 WebView2 + CDP 实测：
 *  - 36.9MB / 100 万行的内容留在可见文本框里，光它自己的排版就要 **7.6-11s**（整页冻住）；
 *  - 只要在**同一个任务里**先 `display:none` 把它移出渲染树、改成截断值、再恢复显示，
 *    这次排版就根本不会发生 —— 隐藏状态下改写只要 **~88ms**，之后的帧恢复到 6-11ms。
 *
 * 全部同步完成（不用 rAF / setTimeout）：同一任务里做，浏览器在下一帧才计算样式，
 * 看到的是「已恢复显示 + 小内容」，既不会闪一下，也不会被强制同步排版。
 */
function shrinkOversizedDom(node: HTMLTextAreaElement, value: string, focus: boolean) {
  const prevDisplay = node.style.display
  node.style.display = 'none'
  try {
    node.value = value
  } finally {
    node.style.display = prevDisplay
  }
  if (focus) node.focus()
}

/**
 * 统一入口：截断 → 写入状态 → 重算计数。
 *
 * `truncatedFlag` 用于「调用方已经知道原始输入被截断」的场景：传进来的 `next` 已是截断后的
 * 内容，再截一次自然得到 `truncated=false`，会让常驻提示不显示（真机 CDP 检查发现）。
 * `syncDom` 用于导入等非输入路径（textarea 非受控，DOM 值只由本组件写）。
 */
function setText(next: string, notify = false, syncDom = false, truncatedFlag?: boolean) {
  const r = truncateBatchText(next)
  truncated.value = truncatedFlag ?? r.truncated
  text.value = r.text
  if (syncDom && taRef.value) taRef.value.value = r.text
  result.value = null
  error.value = ''
  restartParse()
  if ((truncatedFlag ?? r.truncated) && notify) notifyTruncated()
}

/**
 * 粘贴拦截（BUG-07 的真正源头）：剪贴板文本超长时直接 preventDefault，
 * 只把**截断后**的内容插进文本框 —— 36.9MB 永远不进入 DOM，
 * 那「文本框自身排版 10 秒」的冻结也就无从发生。
 * 正常长度的粘贴不拦截，完全交给浏览器默认行为。
 */
function onPaste(e: ClipboardEvent) {
  const raw = e.clipboardData ? e.clipboardData.getData('text') : ''
  const r = truncateBatchText(raw)
  if (!r.truncated) return
  e.preventDefault()
  const el = taRef.value
  if (el) {
    const start = el.selectionStart ?? el.value.length
    const end = el.selectionEnd ?? start
    el.setRangeText(r.text, start, end, 'end')
  }
  setText(el ? el.value : r.text, false, false, r.truncated)
  notifyTruncated()
}

/**
 * textarea 输入 / 粘贴。
 *
 * 本回调只做两件便宜事：截断（纯字符串，0ms）+ 分块解析的**首页**；
 * 唯一的 DOM 改写是「先把超长内容摘出渲染树再改小」（见 shrinkOversizedDom）。
 * 保证「1e6 行粘贴 → 下一帧」远低于 300ms（BUG-07：旧实现要 ~10s）。
 */
function onInput(e: Event) {
  const el = e.target as HTMLTextAreaElement
  const r = truncateBatchText(el.value)
  if (r.truncated) {
    shrinkOversizedDom(el, r.text, document.activeElement === el)
    // 截断提示放到本轮事件回调之外：naive-ui 的 toast 挂载会立刻触发一次样式/布局，
    // 留在回调里等于给「粘贴 → 下一帧」白算上百毫秒（实测 ~150ms），而它只是句提示。
    setTimeout(notifyTruncated, 0)
  }
  setText(r.text, false, false, r.truncated)
}

/** 从 txt 文件导入：读到的内容整段填进文本框，再由用户确认入队 */
async function importTxt() {
  if (importing.value) return
  try {
    const picked = await openDialog({
      multiple: false,
      directory: false,
      filters: [
        { name: tr('文本文件'), extensions: ['txt', 'text', 'list', 'csv', 'm3u', 'm3u8'] },
        { name: tr('全部文件'), extensions: ['*'] },
      ],
    })
    if (typeof picked !== 'string') return
    importing.value = true
    const content = await readTextFile(picked)
    setText(content, true, true)
    message.success(tr('已导入 {name}', { name: basename(picked) }))
  } catch (e: any) {
    const msg = String(e?.message ?? e)
    error.value = msg
    message.error(`${tr('读取文件失败')}：${msg}`)
  } finally {
    importing.value = false
  }
}

/** 全部加入队列：把整段文本原样交给后端，统计 / 跳过全部以后端返回为准 */
async function enqueue() {
  if (!parsedUnique.value) {
    message.warning(tr('请粘贴至少一个链接'))
    return
  }
  busy.value = true
  error.value = ''
  result.value = null
  try {
    const res = await enqueueLinks(text.value, settingsStore.settings.download_dir || null)
    result.value = res
    message.success(
      tr('已加入 {added} 条，跳过 {skipped} 条', {
        added: res.added ?? 0,
        skipped: res.skipped ?? 0,
      }),
    )
    await store.reload()
  } catch (e: any) {
    // 后端错误原文照展示，不翻译、不截断
    error.value = String(e?.message ?? e)
    message.error(error.value)
  } finally {
    busy.value = false
  }
}

/** 卸载时停掉未完成的解析步进（避免定时器泄漏） */
onBeforeUnmount(stopParse)

/** 非受控 textarea：重新挂载时把当前文本写回 DOM（切页签回来内容不能丢） */
onMounted(() => {
  if (taRef.value && taRef.value.value !== text.value) taRef.value.value = text.value
})
</script>

<template>
  <section :class="bare ? '' : 'umi-card p-4'" data-test="batch-import">
    <div class="umi-card-title">
      <NIcon :size="14" :component="ListOutline" class="text-accent" />{{ $t('批量导入') }}
      <span class="umi-spacer" />
      <span class="umi-hint">{{ $t('每行一个链接，支持粘贴多行；空行与 # / // 注释行会被忽略。') }}</span>
    </div>

    <textarea
      ref="taRef"
      rows="5"
      spellcheck="false"
      class="umi-input w-full resize-y font-mono !text-[11.5px] leading-relaxed"
      :placeholder="$t('每行一个链接，支持粘贴多行；空行与 # / // 注释行会被忽略。')"
      data-test="batch-text"
      @input="onInput"
      @paste="onPaste"
    />

    <div class="mt-2 flex flex-wrap items-center justify-between gap-2">
      <div class="s-text-3 text-[11px] tabular-nums" data-test="batch-count">
        <span v-if="parsing" data-test="batch-parsing">
          {{ $t('正在解析…') }}<template v-if="parsedTotal"> · {{ $t('已识别 {n} 条', { n: parsedTotal }) }}</template>
        </span>
        <span v-else-if="parsedTotal">{{ $t('识别到 {n} 条链接，去重后 {m} 条', { n: parsedTotal, m: parsedUnique }) }}</span>
        <span v-else>{{ $t('等待粘贴链接') }}</span>
      </div>
      <div class="flex shrink-0 gap-2">
        <button
          class="umi-btn umi-btn-sm whitespace-nowrap"
          :disabled="importing"
          data-test="batch-txt"
          @click="importTxt"
        >
          <span class="flex items-center gap-1">
            <NSpin v-if="importing" :size="12" />
            <NIcon v-else :size="13" :component="DocumentOutline" />{{ $t('从 txt 文件导入') }}
          </span>
        </button>
        <button
          class="umi-btn-primary umi-btn-sm whitespace-nowrap"
          :disabled="!canEnqueue"
          data-test="batch-enqueue"
          @click="enqueue"
        >
          <span class="flex items-center gap-1">
            <NSpin v-if="busy" :size="12" />
            <NIcon v-else :size="13" :component="CloudUploadOutline" />
            {{ busy ? $t('加入中…') : $t('全部加入队列') }}
          </span>
        </button>
      </div>
    </div>

    <!-- 后端行为说明（引擎侧的真实规则，不在前端做任何过滤） -->
    <div class="umi-hint mt-2 flex flex-wrap items-center gap-x-4 gap-y-1">
      <span class="flex items-center gap-1">
        <NIcon :size="12" :component="PlayCircleOutline" />{{ $t('加入队列后自动开始') }}
      </span>
      <span class="flex items-center gap-1">
        <NIcon :size="12" :component="ShieldCheckmarkOutline" />{{ $t('跳过重复链接') }}
      </span>
    </div>

    <div
      v-if="overflow"
      class="mt-2 rounded-lg border border-amber-500/25 bg-amber-500/10 px-2.5 py-1.5 text-[11px] text-amber-700 dark:text-amber-400"
    >
      {{ $t('单次最多 {n} 条，超出的不会入队', { n: ENQUEUE_LIMIT }) }}
    </div>

    <!-- 超长粘贴被截断（BUG-07）：界面常驻可见，不做静默丢弃 -->
    <div
      v-if="truncated"
      class="mt-2 rounded-lg border border-amber-500/25 bg-amber-500/10 px-2.5 py-1.5 text-[11px] text-amber-700 dark:text-amber-400"
      data-test="batch-truncated"
    >
      {{ $t('已截断：单次最多 {kb} KB（超出部分已丢弃，可分批粘贴）', { kb: TEXT_LIMIT_KB }) }}
    </div>

    <!-- 后端错误原文 -->
    <div
      v-if="error"
      class="mt-2.5 break-all rounded-lg border border-rose-500/25 bg-rose-500/10 px-2.5 py-1.5 text-[11px] text-rose-500"
      data-test="batch-error"
    >
      {{ error }}
    </div>

    <!-- 入队结果：全部来自后端返回值 -->
    <div v-if="result" class="mt-2.5 space-y-1.5" data-test="batch-result">
      <div
        class="flex items-center gap-2 rounded-lg border px-2.5 py-1.5 text-[11px]"
        :class="
          result.added > 0
            ? 'border-emerald-500/25 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
            : 'border-amber-500/25 bg-amber-500/10 text-amber-700 dark:text-amber-400'
        "
      >
        <NIcon :size="13" :component="result.added > 0 ? CheckmarkCircleOutline : AlertCircleOutline" />
        <span>{{ $t('已加入 {added} 条，跳过 {skipped} 条', { added: result.added, skipped: result.skipped }) }}</span>
      </div>

      <details v-if="result.errors?.length" class="s-border-soft s-surface-2 rounded-lg border px-2.5 py-1.5">
        <summary class="cursor-pointer text-[11px] text-rose-500">{{ $t('失败') }} · {{ result.errors.length }}</summary>
        <div v-for="(r, i) in result.errors" :key="i" class="mt-1 break-all font-mono text-[10px] text-rose-500">{{ r }}</div>
      </details>
    </div>
  </section>
</template>
