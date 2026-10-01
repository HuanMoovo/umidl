<script setup lang="ts">
/**
 * ED2K 电驴引擎面板（1.7：从「设置 → 系统与集成」搬进下载模块，抽成可复用紧凑卡片）
 *
 *  - ed2k_engine_status() 引擎状态（未安装 → install_tool("emule")；已安装 → 路径 / 运行状态 / Web 端口）
 *  - ed2k_parse(link)      粘贴链接即时解析预览（hash / 名称 / 大小 / 源 / AICH，折叠展示不占地方）
 *  - ed2k_submit(link)     「交给引擎」，成功后刷新下载队列，失败展示后端错误原文
 *
 * 两种形态：
 *  · compact（下载页）：链接由父组件主输入框通过 link 传入，面板不再放第二个输入框；
 *    非 ed2k 链接保持安静，不弹「这不是 ed2k 链接」的噪音提示。
 *  · 默认（独立使用）：面板自带输入框，行为与 1.4 设置卡片一致。
 *
 * 真实行为：链接交给 eMule 引擎接管，下载进度在引擎自己的窗口里看，前端不做进度条。
 * 浏览器预览（非 Tauri）下所有调用都会失败，这里用 inTauri 守卫优雅降级。
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { NIcon, NProgress, useMessage } from 'naive-ui'
import {
  AlertCircleOutline,
  CheckmarkCircleOutline,
  GitNetworkOutline,
  LinkOutline,
  RefreshOutline,
  SendOutline,
} from '@vicons/ionicons5'
import { useTaskStore } from '@/stores/tasks'
import { formatBytes } from '@/services/utils'
import {
  ed2kEngineStatus,
  ed2kParse,
  ed2kSubmit,
  installEd2kEngine,
  looksLikeEd2k,
  type Ed2kEngineStatus,
  type Ed2kParsed,
  type Ed2kSubmitResult,
} from '@/services/ed2k'

const props = withDefaults(
  defineProps<{
    /** 紧凑形态：链接来自父组件（下载页主输入框），不再自带输入框 */
    compact?: boolean
    /** 嵌入形态：外层不再包 umi-card（由父组件提供卡片），避免卡片套卡片 */
    bare?: boolean
    /** 外部链接（compact 形态下由父组件传入；空串表示暂无链接） */
    link?: string
  }>(),
  { compact: false, bare: false, link: '' },
)

const { t: tr } = useI18n()
const message = useMessage()
const store = useTaskStore()

/** 运行在 Tauri 容器内？（浏览器预览时 IPC 一律不可用） */
const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

/* ------------------------- 引擎状态 ------------------------- */
const status = ref<Ed2kEngineStatus | null>(null)
const statusErr = ref('')
const statusBusy = ref(false)
const installing = ref(false)

const engineName = computed(() => {
  const e = status.value?.engine
  if (e === 'mlnet') return 'mlnet'
  return 'eMule'
})

const statusText = computed(() => {
  if (!inTauri) return tr('该功能需要在桌面客户端中运行')
  const s = status.value
  if (!s) return statusErr.value ? `${tr('未检测到 eMule 引擎')}（${statusErr.value}）` : tr('正在检测引擎…')
  if (!s.installed) return tr('未检测到 eMule 引擎')
  if (!s.running) return tr('引擎已安装，但当前没有运行')
  return s.ready ? tr('引擎已就绪') : tr('引擎已安装并运行')
})

const statusTone = computed(() => {
  const s = status.value
  if (!inTauri || !s) return 'warn'
  if (!s.installed) return 'warn'
  return s.ready || s.running ? 'ok' : 'warn'
})

/** 引擎行：装了但没有 status 时（后端命令缺失）也保持可安装 */
const showInstall = computed(() => !status.value?.installed)

/** 已就绪时可交链接（未装引擎就把安装按钮摆在那里，不给死按钮） */
const canSubmit = computed(() => status.value?.installed === true)

const subtitle = computed(() => {
  const s = status.value
  if (!s) return tr('安装 eMule 引擎后，ed2k 链接可交由引擎接管下载')
  const bits: string[] = []
  if (s.path) bits.push(s.path)
  if (s.running) bits.push(tr('运行中'))
  else if (s.installed) bits.push(tr('未运行'))
  if (s.web_port) bits.push(`Web ${s.web_port}`)
  if (s.note) bits.push(s.note)
  return bits.join(' · ') || tr('安装 eMule 引擎后，ed2k 链接可交由引擎接管下载')
})

async function refreshStatus(silent = false) {
  if (!inTauri) {
    if (!silent) message.warning(tr('该功能需要在桌面客户端中运行'))
    return
  }
  statusBusy.value = true
  try {
    status.value = await ed2kEngineStatus()
    statusErr.value = ''
  } catch (e: any) {
    statusErr.value = String(e?.message ?? e)
    if (!silent) message.error(statusErr.value)
  } finally {
    statusBusy.value = false
  }
}

async function installEngine() {
  if (!inTauri) {
    message.warning(tr('该功能需要在桌面客户端中运行'))
    return
  }
  if (installing.value) return
  installing.value = true
  try {
    await installEd2kEngine()
    message.success(tr('eMule 引擎安装完成'))
    void store.refreshTools().catch(() => {})
    await refreshStatus(true)
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  } finally {
    installing.value = false
  }
}

/* ------------------------- 链接：外部传入或自带输入框 ------------------------- */
const draft = ref('')
/** 生效链接：父组件传入优先，其次面板自带输入框 */
const activeLink = computed(() => (props.link || '').trim() || draft.value.trim())
const isEd2k = computed(() => looksLikeEd2k(activeLink.value))

/* ------------------------- 粘贴链接即时解析 ------------------------- */
const parsed = ref<Ed2kParsed | null>(null)
const parseError = ref('')
const parsing = ref(false)
const submitting = ref(false)
const submitError = ref('')
const submitResult = ref<Ed2kSubmitResult | null>(null)
let timer: ReturnType<typeof setTimeout> | undefined

async function doParse(url: string) {
  if (!inTauri) {
    parseError.value = tr('该功能需要在桌面客户端中运行')
    return
  }
  parsing.value = true
  try {
    const r = await ed2kParse(url)
    // 结果回来时链接可能已被改掉：过期结果直接丢弃（避免解析结果错位）
    if (activeLink.value !== url) return
    parsed.value = r
    parseError.value = ''
  } catch (e: any) {
    parsed.value = null
    parseError.value = String(e?.message ?? e)
  } finally {
    parsing.value = false
  }
}

watch(activeLink, (v) => {
  parsed.value = null
  parseError.value = ''
  submitResult.value = null
  submitError.value = ''
  if (timer) clearTimeout(timer)
  const url = String(v || '').trim()
  if (!url) return
  if (!looksLikeEd2k(url)) {
    // 紧凑形态里链接来自下载页主输入框，普通链接本来就不归这里管：保持安静
    if (!props.compact) parseError.value = tr('这不是一条 ed2k 链接（应以 ed2k:// 开头）')
    return
  }
  timer = setTimeout(() => void doParse(url), 400)
})

/** 交给引擎：成功后刷新下载队列，失败展示后端错误原文（不做自动重试） */
async function submit() {
  const url = activeLink.value
  if (!looksLikeEd2k(url)) {
    message.warning(tr('这不是一条 ed2k 链接（应以 ed2k:// 开头）'))
    return
  }
  if (!inTauri) {
    message.warning(tr('该功能需要在桌面客户端中运行'))
    return
  }
  submitting.value = true
  submitError.value = ''
  try {
    const r = await ed2kSubmit(url)
    submitResult.value = r
    message.success(tr('已交给引擎接管，进度在引擎窗口查看'))
    await refreshStatus(true)
    await store.reload()
  } catch (e: any) {
    submitResult.value = null
    submitError.value = String(e?.message ?? e)
    message.error(submitError.value)
  } finally {
    submitting.value = false
  }
}

onMounted(() => {
  void refreshStatus(true)
})

onBeforeUnmount(() => {
  if (timer) clearTimeout(timer)
})
</script>

<template>
  <section :class="bare ? '' : 'umi-card p-4'" data-test="ed2k-panel">
    <div class="umi-card-title">
      <NIcon :size="14" :component="GitNetworkOutline" class="text-accent" />{{ $t('ED2K 电驴引擎') }}
      <span class="umi-chip !text-[10px]" data-test="ed2k-engine-name">{{ engineName }}</span>
      <span class="umi-spacer" />
      <button class="umi-btn umi-btn-sm" :disabled="statusBusy" data-test="ed2k-refresh" @click="refreshStatus(false)">
        <span class="flex items-center gap-1">
          <NIcon :size="12" :component="RefreshOutline" />{{ $t('重新检测') }}</span>
      </button>
    </div>

    <!-- 引擎状态 -->
    <div class="umi-inner flex items-start gap-2.5">
      <NIcon
        :size="16"
        class="mt-[1px] shrink-0"
        :component="statusTone === 'ok' ? CheckmarkCircleOutline : AlertCircleOutline"
        :class="statusTone === 'ok' ? 'text-emerald-600 dark:text-emerald-400' : 'text-amber-600 dark:text-amber-400'"
      />
      <div class="min-w-0 flex-1">
        <div class="s-text text-[12.5px] font-medium" data-test="ed2k-status">{{ statusText }}</div>
        <div class="s-text-3 mt-0.5 break-all text-[10.5px]" data-test="ed2k-status-sub">{{ subtitle }}</div>
        <div v-if="installing" class="mt-1.5 max-w-[260px]" data-test="ed2k-progress">
          <NProgress
            type="line"
            :percentage="Math.round(store.toolProgress['emule']?.percent ?? 0)"
            :height="4"
            :show-indicator="false"
            color="rgb(var(--umi-a500))"
          />
          <div class="s-text-3 mt-0.5 truncate text-[10px]">
            {{ store.toolProgress['emule']?.message || $t('下载中…') }}
          </div>
        </div>
      </div>
      <button
        v-if="showInstall"
        class="umi-btn-primary umi-btn-sm shrink-0"
        :disabled="installing"
        data-test="ed2k-install"
        @click="installEngine"
      >
        {{ installing ? $t('安装中…') : $t('安装 eMule 引擎') }}
      </button>
    </div>

    <!-- 独立形态：自带输入框（compact 时链接来自下载页主输入框） -->
    <div v-if="!compact" class="mt-4">
      <label class="umi-label">{{ $t('粘贴 ed2k 链接（即时解析）') }}</label>
      <input
        v-model="draft"
        class="umi-input font-mono !text-[11.5px]"
        placeholder="ed2k://|file|example.iso|734003200|XXXXXXXX...|/"
        spellcheck="false"
        data-test="ed2k-input"
      />
    </div>

    <!-- 交给引擎 + 解析预览 -->
    <div class="mt-2.5 flex flex-wrap items-center gap-2">
      <button
        v-if="canSubmit"
        class="umi-btn-primary umi-btn-sm"
        :disabled="submitting || !isEd2k"
        data-test="ed2k-submit"
        @click="submit"
      >
        <span class="flex items-center gap-1">
          <NIcon :size="12" :component="SendOutline" />{{ submitting ? $t('提交中…') : $t('交给引擎下载') }}</span>
      </button>
      <span class="umi-hint">
        <NIcon :size="11" :component="LinkOutline" class="mr-1 inline-block align-[-1px]" />
        {{ $t('链接会交给 eMule 引擎接管，进度在引擎窗口查看') }}
      </span>
    </div>

    <!-- 解析中 -->
    <div v-if="parsing" class="s-text-3 mt-2 flex items-center gap-2 text-[11.5px]" data-test="ed2k-parsing">
      <span class="h-2 w-2 animate-ping rounded-full bg-umi-400" />{{ $t('解析中…') }}
    </div>

    <!-- 解析结果：折叠起来，避免占地方 -->
    <details
      v-else-if="parsed"
      class="umi-inner mt-2"
      open
      data-test="ed2k-preview"
    >
      <summary class="s-text-2 cursor-pointer text-[11.5px]">{{ $t('ED2K 解析预览') }}</summary>
      <div class="mt-2">
        <div class="s-text truncate text-[12.5px] font-medium">{{ parsed.name || $t('未知文件名') }}</div>
        <div class="mt-1.5 flex flex-wrap gap-1.5">
          <span class="umi-chip">{{ $t('大小') }} {{ formatBytes(parsed.size) }}</span>
          <span class="umi-chip">{{ $t('来源数') }} {{ parsed.sources }}</span>
          <span v-if="parsed.aich" class="umi-chip">AICH</span>
        </div>
        <div class="s-text-3 mt-1.5 break-all font-mono text-[10.5px]">{{ parsed.hash }}</div>
      </div>
    </details>

    <!-- 非法链接：后端错误原文 -->
    <div
      v-else-if="parseError"
      class="mt-2 break-all rounded-lg border border-rose-500/25 bg-rose-500/10 px-2.5 py-1.5 text-[11px] text-rose-500"
      data-test="ed2k-parse-error"
    >
      {{ parseError }}
    </div>

    <div v-else-if="!compact" class="umi-hint mt-2" data-test="ed2k-idle">
      {{ $t('输入链接后自动预览：文件哈希 / 名称 / 大小 / 源数量（无需开始下载）。') }}
    </div>

    <div
      v-if="submitError"
      class="mt-2 rounded-lg border border-rose-500/25 bg-rose-500/10 px-2.5 py-2 text-[11px] text-rose-500"
      data-test="ed2k-submit-error"
    >
      <div class="mb-0.5 font-medium">{{ $t('交给引擎失败：') }}</div>
      <div class="break-all font-mono text-[10.5px]">{{ submitError }}</div>
    </div>

    <div
      v-else-if="submitResult"
      class="umi-inner mt-2 text-[11px]"
      data-test="ed2k-submit-result"
    >
      <div class="s-text flex flex-wrap items-center gap-x-3 gap-y-1">
        <span>{{ $t('引擎') }} {{ submitResult.engine }}</span>
        <span class="s-text-3">·</span>
        <span>{{ submitResult.started ? $t('已启动') : $t('已接管') }}</span>
        <span v-if="submitResult.web_port" class="s-text-3">· Web {{ submitResult.web_port }}</span>
      </div>
      <div class="s-text-3 mt-1">{{ $t('链接会交给 eMule 引擎接管，进度在引擎窗口查看') }}</div>
    </div>

    <div v-if="!inTauri" class="umi-hint mt-2.5" data-test="ed2k-browser-note">
      {{ $t('当前是浏览器预览：引擎状态与解析结果都不可用，请在桌面客户端中打开本页。') }}
    </div>
  </section>
</template>
