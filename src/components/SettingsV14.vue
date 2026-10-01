<script setup lang="ts">
/**
 * 1.4 新增设置卡片（布局重构版：页签合并后按 section 挂载）
 *   section="engine" → 下载引擎 / 链接路由与过滤 / 带宽与并发
 *   section="system" → 浏览器捕获 / 系统与电源 / 更新与诊断
 *   section="accent" → 自定义强调色（无外层卡片，嵌在「外观 → 主题与强调色」里）
 * 按 section 挂载，只有当前页签才会触发 IPC 调用。
 *
 * 重构要点（零功能丢失）：
 *  - 「依赖工具」页签的工具列表已含 aria2 / emule 行（状态 + 一键安装 + 指定路径），
 *    这里不再重复一整块 aria2 状态卡，只留一行状态说明。
 *  - 「全局限速」与「多线程与并发」原本是两张卡、各占一屏，现在合成一张「带宽与并发」。
 *  - 「链接体验」与「智能过滤」合到一张卡（二者互相引用同一套路由/过滤规则）。
 *  - 「更新」与「诊断」两张只有一两个控件的卡合成一张。
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { NIcon, NInput, NInputNumber, NProgress, NSelect, NSlider, NSwitch, useMessage } from 'naive-ui'
import { invoke } from '@tauri-apps/api/core'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import {
  AlertCircleOutline,
  BugOutline,
  CheckmarkCircleOutline,
  CloudDownloadOutline,
  ColorPaletteOutline,
  DocumentTextOutline,
  FlashOutline,
  FolderOpenOutline,
  FunnelOutline,
  GitNetworkOutline,
  GlobeOutline,
  PowerOutline,
  RefreshOutline,
  SpeedometerOutline,
  SyncOutline,
} from '@vicons/ionicons5'
import { useSettingsStore } from '@/stores/settings'
import { useTaskStore } from '@/stores/tasks'
import {
  applyCustomAccent,
  applyTheme,
  buildAccentScale,
  clearCustomAccent,
  isAccentAutoDarkened,
  isCustomAccentApplied,
  normalizeAccentHex,
  parseAccentHex,
  resolveTheme,
} from '@/services/theme'
import { formatBytes } from '@/services/utils'
import type { AppSettings, ToolStatus } from '@/types'

const props = defineProps<{ section: 'engine' | 'system' | 'accent' }>()

const { t: tr } = useI18n()
const settingsStore = useSettingsStore()
const store = useTaskStore()
const message = useMessage()

/* ============================================================
   1.4 新增设置字段（Rust 端 AppSettings 已实现并持久化；
   src/types 的 TS 类型表尚未合并这些字段，这里做局部声明，
   绑定照样直接落到 settingsStore.settings，保存走同一个 save_settings。
   ============================================================ */
interface V14Settings {
  engine?: string
  playlist_mode?: string
  speed_limit_enabled?: boolean
  speed_limit_kb?: number
  filter_ext_block?: string
  filter_domain_block?: string
  filter_domain_allow?: string
  filter_min_size_mb?: number
  /** 每任务限速（yt-dlp --limit-rate 语法，如 2M / 500K；留空 = 不额外限制） */
  rate_limit?: string | null
  capture_port?: number
  capture_token?: string
  capture_auto_queue?: boolean
  launch_at_login?: boolean
  shutdown_when_done?: boolean
  check_update_on_start?: boolean
  update_manifest_url?: string
  accent_custom?: string
  /** 多线程与并发（1.6 新增） */
  concurrency?: number
  aria2_connections?: number
  aria2_split?: number
  aria2_min_split_mb?: number
  ytdlp_concurrency?: number
}

type V14Key = keyof V14Settings

/** 后端默认值（设置首次加载前也能显示正确初值） */
const DEFAULTS: Required<V14Settings> = {
  engine: 'auto',
  playlist_mode: 'single',
  speed_limit_enabled: false,
  speed_limit_kb: 0,
  filter_ext_block: '',
  filter_domain_block: '',
  filter_domain_allow: '',
  filter_min_size_mb: 0,
  rate_limit: null,
  capture_port: 6970,
  capture_token: '',
  capture_auto_queue: true,
  launch_at_login: false,
  shutdown_when_done: false,
  check_update_on_start: true,
  update_manifest_url: '',
  accent_custom: '',
  concurrency: 3,
  aria2_connections: 16,
  aria2_split: 16,
  aria2_min_split_mb: 1,
  ytdlp_concurrency: 1,
}

/** 运行在 Tauri 容器内？（浏览器预览时 IPC 一律不可用） */
const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

const cfg = computed(() => settingsStore.settings as unknown as AppSettings & V14Settings)

function patchField(key: V14Key, value: unknown) {
  const part: Record<string, unknown> = {}
  part[key] = value
  settingsStore.patch(part as unknown as Partial<AppSettings>)
}

/** 首帧补齐缺失的新字段（load() 之前 / 旧配置文件里没有这些键） */
function ensureDefaults() {
  const part: Record<string, unknown> = {}
  const raw = settingsStore.settings as unknown as Record<string, unknown>
  for (const [k, v] of Object.entries(DEFAULTS)) {
    if (raw[k] === undefined) part[k] = v
  }
  if (Object.keys(part).length) settingsStore.patch(part as unknown as Partial<AppSettings>)
}

async function save() {
  try {
    await settingsStore.save()
  } catch (e: any) {
    // 浏览器预览（无桌面后端）时静默，桌面端才提示
    if (inTauri) message.error(String(e?.message ?? e))
  }
}

/** 统一的命令调用：返回 { ok, data }，失败时给出提示（silent 用于挂载时的探测） */
async function callInvoke<T>(
  cmd: string,
  args?: Record<string, unknown>,
  silent = false,
): Promise<{ ok: boolean; data: T | null }> {
  if (!inTauri) {
    if (!silent) message.warning(tr('该功能需要在桌面客户端中运行'))
    return { ok: false, data: null }
  }
  try {
    return { ok: true, data: await invoke<T>(cmd, args) }
  } catch (e: any) {
    if (!silent) message.error(String(e?.message ?? e))
    return { ok: false, data: null }
  }
}

/* ============================================================
   A 引擎卡：engine_info / install_tool
   ============================================================ */
interface EngineInfo {
  engine: string
  aria2_ready: boolean
  aria2_path?: string | null
  split: number
  speed_limit_kb: number
  playlist_mode: string
  proxy_mode: string
}

const engineInfo = ref<EngineInfo | null>(null)

const engineOptions = computed(() => [
  { label: tr('自动（推荐 · 按链接类型选择）'), value: 'auto' },
  { label: tr('yt-dlp · 站点解析引擎'), value: 'ytdlp' },
  { label: tr('aria2 · 分段并行引擎'), value: 'aria2' },
])

const playlistOptions = computed(() => [
  { label: tr('单个视频（分P 只取当前一集）'), value: 'single' },
  { label: tr('整列表 / 合集（下载全部）'), value: 'playlist' },
])

async function refreshEngineInfo(silent = false): Promise<void> {
  const r = await callInvoke<EngineInfo>('engine_info', undefined, silent)
  if (r.ok) engineInfo.value = r.data
}

/** aria2 状态副标题（新文案未进语言包前不做 {占位符} 插值，改用拼接，避免显示成字面量 {x}） */
const aria2Subtitle = computed(() => {
  const info = engineInfo.value
  if (!info?.aria2_ready) return tr('安装后直链 / BT 磁力 / FTP 可走 16 连接分段并行，支持断点续传')
  const bits = [`${info.split ?? 16} ${tr('连接并行分段 · 断点续传 · BT 磁力 FTP')}`]
  if (info.aria2_path) bits.push(info.aria2_path)
  return bits.join(' · ')
})

/** aria2 的安装 / 指定路径统一收口在「依赖工具」的工具行里，这里只做状态回显 */

/* ============================================================
   B 链接体验卡：explain_route（输入防抖 400ms）
   ============================================================ */
interface RouteVerdict {
  engine: string
  engine_label: string
  blocked: boolean
  reason?: string | null
}

const routeInput = ref('')
const routeBusy = ref(false)
const routeVerdict = ref<RouteVerdict | null>(null)
const routeError = ref('')
let routeTimer: ReturnType<typeof setTimeout> | undefined

/** 磁力 / ed2k / http(s) / ftp 才值得问后端 */
function looksLikeUrl(u: string): boolean {
  return /^(https?:\/\/|magnet:|ed2k:|ftp:\/\/)\S+$/i.test(u)
}

async function explainRoute() {
  const url = routeInput.value.trim()
  if (!looksLikeUrl(url)) {
    routeBusy.value = false
    return
  }
  routeBusy.value = true
  try {
    routeVerdict.value = await invoke<RouteVerdict>('explain_route', { url })
    routeError.value = ''
  } catch (e: any) {
    routeVerdict.value = null
    routeError.value = String(e?.message ?? e)
  } finally {
    routeBusy.value = false
  }
}

watch(routeInput, (v) => {
  routeVerdict.value = null
  routeError.value = ''
  if (routeTimer) clearTimeout(routeTimer)
  const url = (v || '').trim()
  if (!looksLikeUrl(url)) {
    routeBusy.value = false
    return
  }
  routeBusy.value = true
  routeTimer = setTimeout(() => void explainRoute(), 400)
})

/* ============================================================
   C 全局限速（合进「带宽与并发」卡）
   ============================================================ */
const speedSlider = computed(() => Math.min(102400, Math.max(1, Number(cfg.value.speed_limit_kb ?? 0) || 1)))

async function onSpeedToggle(v: unknown) {
  const enabled = !!v
  patchField('speed_limit_enabled', enabled)
  // 打开但没填过数值时给个可感知的默认值（1 MB/s），避免「开了却没效果」
  if (enabled && Number(cfg.value.speed_limit_kb ?? 0) <= 0) patchField('speed_limit_kb', 1024)
  await save()
}

/* ============================================================
   D 智能过滤卡：文本输入走 computed，失焦（change）时保存
   ============================================================ */
function textModel(key: V14Key) {
  return computed<string>({
    get: () => String(cfg.value[key] ?? ''),
    set: (v) => patchField(key, v),
  })
}

const filterExtBlock = textModel('filter_ext_block')
const filterDomainBlock = textModel('filter_domain_block')
const filterDomainAllow = textModel('filter_domain_allow')
const captureToken = textModel('capture_token')
const updateManifest = textModel('update_manifest_url')

/** 数值绑定（naive-ui 的 number 控件可能给 null，统一在这里收敛） */
function bindNum(key: V14Key, v: unknown, min = 0, max = 1_000_000) {
  const n = Number(v)
  if (!Number.isFinite(n)) return
  patchField(key, Math.min(max, Math.max(min, Math.round(n))))
  void save()
}

function bindBool(key: V14Key, v: unknown) {
  patchField(key, !!v)
  void save()
}

/** 文本字段：提交（blur / Enter）后落盘；空串归一到 null（留空 = 不设置） */
function bindText(key: V14Key, v: unknown) {
  const s = typeof v === 'string' ? v.trim() : ''
  patchField(key, s ? s : null)
  void save()
}

/* ============================================================
   E 浏览器捕获卡：capture_status / capture_restart
   ============================================================ */
interface CaptureStatus {
  running: boolean
  port: number
  /** 最近一次启动失败原因（端口被占用等）；null / 缺省 = 无错误（BUG-11） */
  error?: string | null
}

const capture = ref<CaptureStatus | null>(null)
const restarting = ref(false)

/** 启动失败且当前未监听 → 卡片里显示可见警告（而不是只写日志） */
const captureWarning = computed(() =>
  !capture.value?.running && capture.value?.error ? String(capture.value.error) : '',
)

const capturePort = computed(() => {
  const p = Number(cfg.value.capture_port ?? DEFAULTS.capture_port)
  if (Number.isFinite(p) && p > 0) return p
  return capture.value?.port || DEFAULTS.capture_port
})

const captureExample = computed(() => {
  const token = String(cfg.value.capture_token ?? '').trim()
  const suffix = token ? `（需带请求头 X-Umidl-Token: ${token}，或 ?token=${token}）` : ''
  return `http://127.0.0.1:${capturePort.value}/capture?url=<链接>${suffix}`
})

async function loadCaptureStatus(silent = true) {
  const r = await callInvoke<CaptureStatus>('capture_status', undefined, silent)
  if (r.ok) capture.value = r.data
}

/** 生成随机捕获令牌（与后端同规格：32 位十六进制 = 128 bit） */
function randomCaptureToken(): string {
  const bytes = new Uint8Array(16)
  if (typeof crypto !== 'undefined' && typeof crypto.getRandomValues === 'function') {
    crypto.getRandomValues(bytes)
  } else {
    for (let i = 0; i < bytes.length; i += 1) bytes[i] = Math.floor(Math.random() * 256)
  }
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('')
}

/** 保存 + 重启捕获服务；重启后回读设置，保证界面上的令牌就是服务正在校验的那一个 */
async function restartCapture() {
  restarting.value = true
  try {
    const r = await callInvoke<CaptureStatus>('capture_restart')
    if (r.ok && r.data) {
      capture.value = r.data
      if (r.data.error) message.error(`${tr('捕获服务启动失败')}：${r.data.error}`)
      else if (r.data.running) message.success(`${tr('捕获服务已监听')} 127.0.0.1:${r.data.port}`)
      else message.info(tr('捕获服务已关闭（端口为 0）'))
    }
    await settingsStore.load()
  } finally {
    restarting.value = false
  }
}

async function saveCapture() {
  await save()
  await restartCapture()
}

/** 重置令牌：本地生成新令牌 → 保存 → 重启（旧令牌立即失效） */
async function resetCaptureToken() {
  patchField('capture_token', randomCaptureToken())
  await saveCapture()
  message.success(tr('已重置捕获令牌（旧令牌立即失效）'))
}

/** 换端口并重启：端口被占用时的快捷操作（端口 +1，改完立即重启） */
async function changeCapturePort() {
  const cur = Number(cfg.value.capture_port ?? DEFAULTS.capture_port)
  const next = !Number.isFinite(cur) || cur <= 0 || cur >= 65535 ? 6971 : Math.round(cur) + 1
  patchField('capture_port', next)
  await saveCapture()
}

/** 令牌是必填（后端强制校验）：清空提交时自动补一个新的随机令牌 */
async function onTokenChange() {
  const v = String(cfg.value.capture_token ?? '').trim()
  patchField('capture_token', v || randomCaptureToken())
  await saveCapture()
}

async function copyCaptureExample() {
  const text = captureExample.value
  try {
    await navigator.clipboard.writeText(text)
    message.success(tr('示例已复制到剪贴板'))
  } catch {
    message.info(text)
  }
}

/* ============================================================
   F 系统与电源卡：set_autostart / get_autostart / cancel_shutdown / shutdown_system
   ============================================================ */
const autostartBusy = ref(false)
const shutdownArmed = ref(false)
let armTimer: ReturnType<typeof setTimeout> | undefined

async function loadAutostart() {
  const r = await callInvoke<boolean>('get_autostart', undefined, true)
  if (r.ok && typeof r.data === 'boolean') {
    patchField('launch_at_login', r.data)
    await save()
  }
}

async function onAutostartToggle(v: unknown) {
  const enabled = !!v
  autostartBusy.value = true
  try {
    patchField('launch_at_login', enabled)
    const r = await callInvoke<boolean>('set_autostart', { enabled })
    if (r.ok) {
      // 以系统实际状态为准回填（可能被系统策略拒绝）
      patchField('launch_at_login', typeof r.data === 'boolean' ? r.data : enabled)
      message.success(r.data ? tr('已开启开机自启动') : tr('已关闭开机自启动'))
    } else {
      patchField('launch_at_login', !enabled)
    }
    await save()
  } finally {
    autostartBusy.value = false
  }
}

async function cancelShutdown() {
  const r = await callInvoke<null>('cancel_shutdown')
  if (r.ok) message.success(tr('已取消计划的关机'))
}

/** 测试按钮：第一次点击待确认，第二次才真的计划 60 秒后关机 */
async function planShutdown() {
  if (!shutdownArmed.value) {
    shutdownArmed.value = true
    if (armTimer) clearTimeout(armTimer)
    armTimer = setTimeout(() => (shutdownArmed.value = false), 5000)
    return
  }
  if (armTimer) clearTimeout(armTimer)
  shutdownArmed.value = false
  const r = await callInvoke<null>('shutdown_system', { delaySecs: 60 })
  if (r.ok) message.warning(tr('已计划 60 秒后关机，可用「取消已计划的关机」撤回'))
}

/* ============================================================
   G 更新卡：check_update
   ============================================================ */
interface UpdateCheck {
  ok: boolean
  current?: string
  latest?: string
  has_update?: boolean
  url?: string | null
  notes?: string | null
  error?: string | null
}

const checking = ref(false)
const updateResult = ref<UpdateCheck | null>(null)

async function checkUpdate() {
  checking.value = true
  try {
    await save()
    const r = await callInvoke<UpdateCheck>('check_update')
    if (r.ok && r.data) updateResult.value = r.data
  } finally {
    checking.value = false
  }
}

/* ============================================================
   H 诊断卡：export_logs（与更新合成一张「更新与诊断」）
   ============================================================ */
const exporting = ref(false)
const exportedPath = ref('')

async function exportLogs() {
  try {
    const picked = await openDialog({ directory: true, multiple: false })
    if (typeof picked !== 'string') return
    exporting.value = true
    const r = await callInvoke<string>('export_logs', { dest: picked })
    if (r.ok && r.data) {
      exportedPath.value = r.data
      message.success(tr('日志包已导出'))
    }
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  } finally {
    exporting.value = false
  }
}

/* ============================================================
   I 自定义强调色：hex → CSS 变量的算法住在 services/theme.ts
   （applyCustomAccent / clearCustomAccent / buildAccentScale），这里只做取色、预览与落盘。
   --umi-accent-text 在本项目里是 "R G B" 三元组（.text-accent 用 rgb(var(...)) 取色），
   按当前明暗主题取亮档(a300)/深档(a700)，避免白字白底。
   ============================================================ */

const initialCustom = String(cfg.value.accent_custom ?? '')
const hexText = ref(parseAccentHex(initialCustom) ? normalizeAccentHex(initialCustom) : '#7c4dff')
const hexValid = computed(() => !!parseAccentHex(hexText.value))

const previewScale = computed(() => buildAccentScale(hexValid.value ? hexText.value : '#7c4dff'))

/** 预览：应用后会是什么样子（深色主题按钮白字压在 a600→a400 渐变上） */
const previewPrimary = computed(() => {
  const sc = previewScale.value
  return sc ? `linear-gradient(90deg, rgb(${sc['600']}), rgb(${sc['400']}))` : 'transparent'
})

const previewAccentText = computed(() => {
  const sc = previewScale.value
  if (!sc) return 'inherit'
  return `rgb(${isDarkTheme() ? sc['300'] : sc['700']})`
})

/** 基础色偏亮时会被自动加深（用于按钮渐变），给用户一句提示 */
const autoDarkened = computed(() => isAccentAutoDarkened(hexText.value))

/** 当前主题是否为深色：决定 --umi-accent-text 取亮档(a300) 还是深档(a700) */
function isDarkTheme(): boolean {
  return resolveTheme(settingsStore.settings.theme) === 'dark'
}

function onColorPick(e: Event) {
  const t = e.target as HTMLInputElement | null
  if (t) hexText.value = t.value
}

async function applyAccent() {
  if (!hexValid.value) return
  const hex = normalizeAccentHex(hexText.value)
  // 应用 / 清除都走 services/theme.ts，与启动时恢复自定义色共用同一套算法
  if (!applyCustomAccent(hex, isDarkTheme())) return
  patchField('accent_custom', hex)
  await save()
  message.success(tr('自定义强调色已应用'))
}

async function resetAccent() {
  clearCustomAccent()
  patchField('accent_custom', '')
  await save()
  // 交回预设色（data-accent 仍是用户选的预设）
  applyTheme(settingsStore.settings.theme, settingsStore.settings.accent)
  message.success(tr('已恢复预设强调色'))
}

/* 主题切换：自定义色在亮/暗下要取不同的档位，重算一次 accent-text */
watch(
  () => settingsStore.settings.theme,
  () => {
    if (isCustomAccentApplied() && parseAccentHex(hexText.value)) applyCustomAccent(hexText.value, isDarkTheme())
  },
)

/* 点了预设色 → 撤掉自定义覆盖，否则预设看起来「点了没反应」 */
watch(
  () => settingsStore.settings.accent,
  (v) => {
    if (!isCustomAccentApplied()) return
    clearCustomAccent()
    patchField('accent_custom', '')
    void save()
    message.info(`${tr('已切回预设强调色')} ${String(v)}`)
  },
)

/* 外部清空自定义色（例如设置被重置）时同步撤掉覆盖 */
watch(
  () => cfg.value.accent_custom,
  (v) => {
    if (isCustomAccentApplied() && !parseAccentHex(String(v ?? ''))) clearCustomAccent()
  },
)

/* ============================================================
   J 多线程与并发（合进「带宽与并发」卡）
   5 个参数：同时下载任务数 / aria2 单服务器连接数 / aria2 分段数 / 最小分片 MB / yt-dlp 分片并发
   范围（限幅）与默认值集中在一张表里，滑块与数字输入共用同一份绑定（写入即夹幅并落盘）。
   ============================================================ */
type PerfKey = 'concurrency' | 'aria2_connections' | 'aria2_split' | 'aria2_min_split_mb' | 'ytdlp_concurrency'

/** [最小值, 最大值]：超出范围一律夹到边界 */
const PERF_RANGE: Record<PerfKey, [number, number]> = {
  concurrency: [1, 8],
  aria2_connections: [1, 16],
  aria2_split: [1, 16],
  aria2_min_split_mb: [1, 64],
  ytdlp_concurrency: [1, 16],
}

const PERF_KEYS = Object.keys(PERF_RANGE) as PerfKey[]

/** 页面上按顺序渲染的并发字段（label / 单位 / 说明 全部走语言包） */
const PERF_FIELDS = computed<{ key: PerfKey; label: string; unit: string; hint: string }[]>(() => [
  { key: 'concurrency', label: tr('同时下载任务数'), unit: tr('个'), hint: tr('1-8，默认 3') },
  { key: 'aria2_connections', label: tr('aria2 单服务器连接数'), unit: tr('连接'), hint: tr('1-16，默认 16') },
  { key: 'aria2_split', label: tr('aria2 分段数'), unit: tr('段'), hint: tr('1-16，默认 16') },
  { key: 'aria2_min_split_mb', label: tr('最小分片（MB）'), unit: 'MB', hint: tr('1-64，默认 1；低于 1 时 aria2 会拒启') },
  { key: 'ytdlp_concurrency', label: tr('yt-dlp 分片并发'), unit: tr('并发'), hint: tr('HLS / DASH 片段，1-16，默认 1') },
])

/** 当前值：缺失 / 非数字回退默认值，越界夹到范围（滑块与输入框显示同一个数） */
function perfValue(key: PerfKey): number {
  const [min, max] = PERF_RANGE[key]
  const n = Number(cfg.value[key])
  if (!Number.isFinite(n)) return Number(DEFAULTS[key])
  return Math.min(max, Math.max(min, Math.round(n)))
}

/** 写入：夹幅 + 立即保存（沿用 bindNum 的落盘路径） */
function setPerf(key: PerfKey, v: unknown) {
  const [min, max] = PERF_RANGE[key]
  bindNum(key, v, min, max)
}

/** 恢复默认：五个字段一起回默认值并落盘 */
async function restoreConcurrency() {
  const part: Record<string, unknown> = {}
  for (const k of PERF_KEYS) part[k] = DEFAULTS[k]
  settingsStore.patch(part as unknown as Partial<AppSettings>)
  await save()
  message.success(tr('已恢复默认并发参数'))
}

/** 某一并发字段的 [min, max]（模板里给滑块与输入框用） */
function perfRange(key: PerfKey): [number, number] {
  return PERF_RANGE[key]
}

/* ============================================================
   挂载：按页签懒加载即可，避免一次打一堆 IPC
   ============================================================ */
onMounted(async () => {
  if (!settingsStore.loaded) {
    try {
      await settingsStore.load()
    } catch {
      /* 浏览器预览时忽略 */
    }
  }
  ensureDefaults()

  if (props.section === 'engine') await refreshEngineInfo(true)

  if (props.section === 'system') {
    await loadCaptureStatus(true)
    await loadAutostart()
  }

  if (props.section === 'accent') {
    const saved = String(cfg.value.accent_custom ?? '')
    if (parseAccentHex(saved)) {
      hexText.value = normalizeAccentHex(saved)
      applyCustomAccent(hexText.value, isDarkTheme())
    }
  }
})

onBeforeUnmount(() => {
  if (routeTimer) clearTimeout(routeTimer)
  if (armTimer) clearTimeout(armTimer)
})
</script>

<template>
  <div class="space-y-3">
    <!-- ==================== 下载引擎页签 ==================== -->
    <template v-if="section === 'engine'">
      <!-- A 引擎卡 -->
      <div class="umi-card p-4" data-test="engine-card">
        <div class="umi-card-title">
          <NIcon :size="14" :component="FlashOutline" class="text-accent" />{{ $t('下载引擎') }}
          <span class="umi-spacer" />
          <button class="umi-btn umi-btn-sm" @click="refreshEngineInfo()">
            <span class="flex items-center gap-1"><NIcon :size="12" :component="RefreshOutline" />{{ $t('重新检测') }}</span>
          </button>
        </div>

        <div class="grid gap-3 sm:grid-cols-2">
          <div>
            <label class="umi-label">{{ $t('引擎选择') }}</label>
            <NSelect
              :value="cfg.engine ?? 'auto'"
              :options="engineOptions"
              size="small"
              @update:value="(v) => { patchField('engine', v); save() }"
            />
          </div>
          <div>
            <label class="umi-label">{{ $t('分P / 合集模式') }}</label>
            <NSelect
              :value="cfg.playlist_mode ?? 'single'"
              :options="playlistOptions"
              size="small"
              @update:value="(v) => { patchField('playlist_mode', v); save() }"
            />
          </div>
        </div>

        <!-- aria2 状态一行回显：安装 / 指定路径在「依赖工具」的工具行里 -->
        <div class="mt-2.5 flex items-center gap-2 text-[11px]">
          <NIcon
            :size="13"
            class="shrink-0"
            :component="engineInfo?.aria2_ready ? CheckmarkCircleOutline : AlertCircleOutline"
            :class="engineInfo?.aria2_ready ? 'text-emerald-600 dark:text-emerald-400' : 'text-amber-600 dark:text-amber-400'"
          />
          <span class="s-text-2 truncate">{{ engineInfo?.aria2_ready ? $t('aria2 分段引擎已就绪') : $t('未检测到 aria2 分段引擎') }}</span>
          <span class="umi-hint truncate">{{ aria2Subtitle }}</span>
        </div>

        <div class="umi-hint mt-2.5 border-t s-border-soft pt-2.5">
          {{ $t('自动：按链接类型选择 —— 站点视频走 yt-dlp（1000+ 站点解析 / HLS / DASH），直链、BT 磁力、FTP 走 aria2（16 连接分段并行 + 断点续传）。强制选定后所有链接都按该引擎处理。') }}
        </div>
      </div>

      <!-- B+D 链接路由与过滤卡（原「链接体验」「智能过滤」两张卡合并） -->
      <div class="umi-card p-4" data-test="route-filter-card">
        <div class="umi-card-title">
          <NIcon :size="14" :component="GitNetworkOutline" class="text-accent" />{{ $t('链接路由与过滤') }}
        </div>
        <div class="grid gap-3 sm:grid-cols-3">
          <div class="sm:col-span-2">
            <label class="umi-label">{{ $t('链接体验（会走哪个引擎）') }}</label>
            <input
              v-model="routeInput"
              class="umi-input !text-[11.5px]"
              :placeholder="$t('粘贴链接，例如 https://… 或 magnet:?xt=…')"
              spellcheck="false"
              data-test="route-input"
            />
            <div class="mt-1.5 min-h-[18px] text-[11px]">
              <span v-if="routeBusy" class="s-text-3 flex items-center gap-1.5">
                <span class="h-1.5 w-1.5 animate-ping rounded-full bg-umi-400" />{{ $t('检测中…') }}
              </span>
              <span v-else-if="routeVerdict?.blocked" class="text-rose-500">
                {{ $t('已被过滤：') }}{{ routeVerdict.reason || $t('命中过滤规则') }}
              </span>
              <span v-else-if="routeVerdict" class="s-text-3">
                {{ $t('将使用：') }}<span class="text-accent font-medium">{{ routeVerdict.engine_label }}</span>
              </span>
              <span v-else-if="routeError" class="s-text-3">{{ routeError }}</span>
              <span v-else class="umi-hint">{{ $t('输入链接后自动预览：走哪个引擎、是否被过滤规则拦下（无需开始下载）。') }}</span>
            </div>
          </div>
          <div>
            <label class="umi-label">{{ $t('最小体积（MB，0 = 不过滤）') }}</label>
            <NInputNumber
              :value="cfg.filter_min_size_mb ?? 0"
              :min="0"
              :max="1048576"
              size="small"
              @update:value="(v) => bindNum('filter_min_size_mb', v, 0, 1048576)"
            />
          </div>
          <div>
            <label class="umi-label">{{ $t('扩展名黑名单') }}</label>
            <input
              v-model="filterExtBlock"
              class="umi-input !text-[11.5px]"
              placeholder=".iso, .exe, .apk"
              spellcheck="false"
              @change="save"
            />
          </div>
          <div>
            <label class="umi-label">{{ $t('域名黑名单') }}</label>
            <input
              v-model="filterDomainBlock"
              class="umi-input !text-[11.5px]"
              placeholder="ads.example.com, *.doubleclick.net"
              spellcheck="false"
              @change="save"
            />
          </div>
          <div>
            <label class="umi-label">{{ $t('域名白名单（非空时只允许名单内域名）') }}</label>
            <input
              v-model="filterDomainAllow"
              class="umi-input !text-[11.5px]"
              :placeholder="$t('留空 = 不限制')"
              spellcheck="false"
              @change="save"
            />
          </div>
        </div>
        <div class="umi-hint mt-2 flex flex-wrap items-center gap-x-2 gap-y-1">
          <NIcon :size="12" :component="FunnelOutline" class="shrink-0 text-accent" />
          <span>{{ $t('多条用英文逗号分隔；域名支持通配符 *.example.com。') }}</span>
          <span>{{ $t('命中黑名单、不在白名单或体积不足的文件会被拦下（「链接体验」会实时提示）。磁力与 ed2k 链接不受扩展名 / 域名规则限制。') }}</span>
        </div>
      </div>

      <!-- C+J 带宽与并发卡（原「全局限速」「多线程与并发」两张卡合并） -->
      <div class="umi-card p-4" data-role="concurrency-card">
        <div class="umi-card-title">
          <NIcon :size="14" :component="SpeedometerOutline" class="text-accent" />{{ $t('带宽与并发') }}
          <span class="umi-spacer" />
          <button class="umi-btn umi-btn-sm" data-role="perf-reset" @click="restoreConcurrency">
            <span class="flex items-center gap-1">
              <NIcon :size="12" :component="RefreshOutline" />{{ $t('恢复默认') }}</span>
          </button>
        </div>

        <!-- 全局限速 -->
        <div class="umi-inner flex flex-wrap items-center gap-x-4 gap-y-2">
          <label class="s-text-2 flex cursor-pointer items-center gap-2 text-[12px]">
            <NSwitch :value="!!cfg.speed_limit_enabled" size="small" data-test="speed-limit-switch" @update:value="onSpeedToggle" />{{ $t('启用全局限速') }}
          </label>
          <div class="w-40">
            <NInputNumber
              :value="cfg.speed_limit_kb ?? 0"
              :min="0"
              :max="102400"
              size="small"
              :disabled="!cfg.speed_limit_enabled"
              @update:value="(v) => bindNum('speed_limit_kb', v, 0, 102400)"
            >
              <template #suffix>{{ $t('KB/s') }}</template>
            </NInputNumber>
          </div>
          <div class="min-w-[180px] max-w-xs flex-1">
            <NSlider
              :value="speedSlider"
              :min="1"
              :max="102400"
              :step="64"
              :disabled="!cfg.speed_limit_enabled"
              @update:value="(v) => bindNum('speed_limit_kb', v, 1, 102400)"
            />
          </div>
          <span class="s-text-3 text-[11px] tabular-nums">
            <template v-if="cfg.speed_limit_enabled && Number(cfg.speed_limit_kb ?? 0) > 0">
              {{ $t('约') }} {{ formatBytes(Number(cfg.speed_limit_kb ?? 0) * 1024) }}/s
            </template>
            <template v-else>{{ $t('当前不限速') }}</template>
          </span>
          <!-- 每任务限速：原「通用设置 → 限速（留空不限速）」搬到这里，和全局限速同卡同源 -->
          <div class="flex items-center gap-2">
            <label class="umi-label !mb-0 shrink-0">{{ $t('限速（留空不限速）') }}</label>
            <NInput
              class="w-40"
              :value="cfg.rate_limit ?? ''"
              size="small"
              :placeholder="$t('如 2M / 500K')"
              data-test="rate-limit-input"
              @change="(v) => bindText('rate_limit', v)"
            />
          </div>
        </div>

        <div class="umi-hint mt-2">
          {{ $t('令牌桶算法，即改即生效（对 yt-dlp 与 aria2 同时生效）；0 或关闭 = 不限速。范围 64 KB/s – 100 MB/s。') }}
        </div>

        <!-- 并发：字段、限幅、落盘逻辑与原来完全一致，只是从独立卡片挪到本卡下半区 -->
        <div class="mt-3 grid gap-x-5 gap-y-3 border-t s-border-soft pt-3 sm:grid-cols-2">
          <div v-for="f in PERF_FIELDS" :key="f.key" :data-field="f.key" class="flex items-center gap-2.5">
            <label class="umi-label !mb-0 w-[128px] shrink-0 truncate" :title="`${f.label}（${f.hint}）`">{{ f.label }}</label>
            <NSlider
              class="min-w-0 flex-1"
              :value="perfValue(f.key)"
              :min="perfRange(f.key)[0]"
              :max="perfRange(f.key)[1]"
              :step="1"
              @update:value="(v) => setPerf(f.key, v)"
            />
            <NInputNumber
              class="w-[92px] shrink-0"
              size="small"
              :value="perfValue(f.key)"
              :min="perfRange(f.key)[0]"
              :max="perfRange(f.key)[1]"
              @update:value="(v) => setPerf(f.key, v)"
            >
              <template #suffix>{{ f.unit }}</template>
            </NInputNumber>
          </div>
        </div>

        <div class="umi-hint mt-2.5">
          {{ $t('并发越高越快，也越吃带宽与磁盘；改完即时生效，下次任务按新参数执行。最小分片低于 1 MB 时 aria2 会拒启，所以最小值锁在 1 MB。') }}
        </div>
      </div>
    </template>

    <!-- ==================== 系统与集成页签 ==================== -->
    <template v-else-if="section === 'system'">
      <!-- E 浏览器捕获卡 -->
      <div class="umi-card p-4" data-test="capture-card">
        <div class="umi-card-title">
          <NIcon :size="14" :component="GlobeOutline" class="text-accent" />{{ $t('浏览器捕获') }}
          <span class="umi-spacer" />
          <span
            class="umi-chip !text-[10.5px]"
            :class="
              capture?.running
                ? '!border-emerald-500/30 text-emerald-600 dark:text-emerald-400'
                : captureWarning
                  ? '!border-rose-500/30 text-rose-500'
                  : ''
            "
          >
            <template v-if="capture?.running">{{ $t('监听中') }} 127.0.0.1:{{ capture.port }}</template>
            <template v-else-if="captureWarning">{{ $t('捕获服务启动失败') }}</template>
            <template v-else>{{ $t('捕获服务未开启') }}</template>
          </span>
        </div>

        <!-- 启动失败的可见警告（BUG-11）：端口被占用时不必再翻日志 -->
        <div
          v-if="captureWarning"
          class="mt-2.5 rounded-lg border border-rose-500/25 bg-rose-500/10 px-2.5 py-2 text-[11px] text-rose-500"
          data-test="capture-warning"
        >
          <div class="flex items-start gap-2">
            <NIcon :size="13" :component="AlertCircleOutline" class="mt-[1px] shrink-0" />
            <div class="space-y-1">
              <div>{{ $t('捕获服务启动失败') }}</div>
              <div class="break-all font-mono text-[10.5px]">{{ captureWarning }}</div>
              <div>{{ $t('端口可能已被其它程序占用，请换一个端口后点「保存并重启捕获」。') }}</div>
            </div>
          </div>
          <div class="mt-2 flex flex-wrap gap-2">
            <button class="umi-btn umi-btn-sm" :disabled="restarting" data-test="capture-restart" @click="restartCapture">
              <span class="flex items-center gap-1">
                <NIcon :size="12" :component="SyncOutline" />{{ $t('重新启动捕获服务') }}</span>
            </button>
            <button class="umi-btn umi-btn-sm" :disabled="restarting" data-test="capture-change-port" @click="changeCapturePort">
              <span class="flex items-center gap-1">
                <NIcon :size="12" :component="RefreshOutline" />{{ $t('换端口并重启') }}</span>
            </button>
          </div>
        </div>

        <div class="grid gap-3 sm:grid-cols-3">
          <div>
            <label class="umi-label">{{ $t('监听端口（0 = 关闭）') }}</label>
            <NInputNumber
              :value="cfg.capture_port ?? 6970"
              :min="0"
              :max="65535"
              size="small"
              @update:value="(v) => bindNum('capture_port', v, 0, 65535)"
            />
          </div>
          <div>
            <label class="umi-label">{{ $t('访问令牌（自动生成，必填）') }}</label>
            <input
              v-model="captureToken"
              class="umi-input !text-[11.5px]"
              placeholder="umidl-secret"
              spellcheck="false"
              data-test="capture-token"
              @change="onTokenChange"
            />
          </div>
          <div>
            <label class="umi-label">{{ $t('捕获后自动入队') }}</label>
            <div class="flex h-[34px] items-center gap-2">
              <NSwitch
                :value="cfg.capture_auto_queue !== false"
                size="small"
                @update:value="(v) => bindBool('capture_auto_queue', v)"
              />
              <span class="umi-hint">{{ $t('关闭则只在界面提示') }}</span>
            </div>
          </div>
        </div>

        <div class="mt-3 flex flex-wrap items-center gap-2">
          <button class="umi-btn-primary umi-btn-sm" :disabled="restarting" @click="saveCapture">
            <span class="flex items-center gap-1">
              <NIcon :size="12" :component="SyncOutline" />{{ restarting ? $t('重启中…') : $t('保存并重启捕获') }}</span>
          </button>
          <button class="umi-btn umi-btn-sm" :disabled="restarting" data-test="capture-token-reset" @click="resetCaptureToken">
            <span class="flex items-center gap-1">
              <NIcon :size="12" :component="RefreshOutline" />{{ $t('重置令牌') }}</span>
          </button>
          <button class="umi-btn umi-btn-sm" @click="loadCaptureStatus(false)">
            <span class="flex items-center gap-1">
              <NIcon :size="12" :component="RefreshOutline" />{{ $t('刷新状态') }}</span>
          </button>
          <button class="umi-btn umi-btn-sm" @click="copyCaptureExample">
            <span class="flex items-center gap-1">
              <NIcon :size="12" :component="DocumentTextOutline" />{{ $t('复制示例链接') }}</span>
          </button>
        </div>

        <div class="umi-inner mt-2.5 space-y-1">
          <div class="umi-hint">
            {{ $t('用法：浏览器扩展 / 书签向以下地址发一个请求即可把链接推给 Umidl：') }}
          </div>
          <div class="s-text break-all font-mono text-[11px]">
            http://127.0.0.1:{{ capturePort }}/capture?url=&lt;{{ $t('链接') }}&gt;
          </div>
          <div class="umi-hint">
            {{ $t('所有请求都必须携带令牌（自动生成）：请求头 X-Umidl-Token: <令牌>，或用同名查询参数 ?token=；浏览器扩展 / 书签脚本同样适用，清空令牌保存会自动生成新的。') }}
          </div>
          <div class="umi-hint">
            {{ $t('安全检查：响应不含 CORS 通配头，网页（非本机 Origin / Referer）没有正确令牌会被 403 拒绝并在响应里说明原因。') }}
          </div>
          <div class="umi-hint">
            {{ $t('GET /ping 可握手自检；POST /capture（body 为 url=<链接>&referer=<来源>）同样支持；仅监听本机回环地址。') }}
          </div>
        </div>
      </div>

      <!-- F 系统与电源卡 -->
      <div class="umi-card p-4" data-test="power-card">
        <div class="umi-card-title">
          <NIcon :size="14" :component="PowerOutline" class="text-accent" />{{ $t('系统与电源') }}
        </div>
        <div class="flex flex-wrap gap-x-6 gap-y-3">
          <label class="s-text-2 flex cursor-pointer items-center gap-2 text-[12px]">
            <NSwitch
              :value="!!cfg.launch_at_login"
              size="small"
              :disabled="autostartBusy"
              @update:value="onAutostartToggle"
            />{{ $t('开机自启动') }}
          </label>
          <label class="s-text-2 flex cursor-pointer items-center gap-2 text-[12px]">
            <NSwitch
              :value="!!cfg.shutdown_when_done"
              size="small"
              @update:value="(v) => bindBool('shutdown_when_done', v)"
            />{{ $t('全部任务完成后关机') }}
          </label>
        </div>

        <div
          v-if="cfg.shutdown_when_done"
          class="mt-2.5 rounded-xl border border-amber-500/25 bg-amber-500/10 px-3 py-2"
        >
          <div class="text-[11px] leading-relaxed text-amber-700 dark:text-amber-400">
            {{ $t('已开启：所有任务完成后会计划 60 秒后关机（留出取消时间）。关机期间可随时撤回。') }}
          </div>
          <div class="mt-2 flex flex-wrap gap-2">
            <button class="umi-btn umi-btn-sm" @click="cancelShutdown">{{ $t('取消已计划的关机') }}</button>
            <button
              class="umi-btn umi-btn-sm"
              :class="shutdownArmed ? '!border-rose-400/60 !text-rose-500' : ''"
              @click="planShutdown"
            >
              {{ shutdownArmed ? $t('再次点击确认关机') : $t('测试：60 秒后关机') }}
            </button>
          </div>
        </div>
        <div class="umi-hint mt-2.5">
          {{ $t('开机自启动由系统计划项托管，此处状态以系统实际状态回填；关机只在全部任务成功结束时触发。') }}
        </div>
      </div>

      <!-- G+H 更新与诊断（原「更新」「诊断」两张卡合并） -->
      <div class="umi-card p-4" data-test="update-card">
        <div class="umi-card-title">
          <NIcon :size="14" :component="CloudDownloadOutline" class="text-accent" />{{ $t('更新与诊断') }}
        </div>

        <div class="grid gap-3 sm:grid-cols-2">
          <div>
            <label class="umi-label">{{ $t('启动时检查更新') }}</label>
            <div class="flex h-[34px] items-center gap-2">
              <NSwitch
                :value="cfg.check_update_on_start !== false"
                size="small"
                @update:value="(v) => bindBool('check_update_on_start', v)"
              />
              <span class="umi-hint">{{ $t('每次启动静默比对更新清单') }}</span>
            </div>
          </div>
          <div>
            <label class="umi-label">{{ $t('更新清单地址（JSON，含 version / url / notes）') }}</label>
            <input
              v-model="updateManifest"
              class="umi-input !text-[11.5px]"
              placeholder="https://example.com/umidl/update.json"
              spellcheck="false"
              @change="save"
            />
          </div>
        </div>

        <div v-if="updateResult" class="umi-inner mt-2.5 text-[11px]">
          <div v-if="!updateResult.ok" class="text-rose-500">
            {{ $t('检查失败：') }}{{ updateResult.error || $t('未知错误') }}
          </div>
          <template v-else>
            <div class="s-text flex flex-wrap items-center gap-x-3 gap-y-1">
              <span>{{ $t('当前版本') }} {{ updateResult.current || '-' }}</span>
              <span class="s-text-3">·</span>
              <span>{{ $t('最新版本') }} {{ updateResult.latest || '-' }}</span>
              <span class="umi-chip !text-[10px]" :class="updateResult.has_update ? 'text-accent' : ''">
                {{ updateResult.has_update ? $t('有新版本可用') : $t('已是最新版本') }}
              </span>
            </div>
            <div v-if="updateResult.notes" class="s-text-3 mt-1.5 leading-relaxed">{{ updateResult.notes }}</div>
            <div v-if="updateResult.url" class="s-text-3 mt-1 break-all">{{ updateResult.url }}</div>
          </template>
        </div>

        <div class="mt-3 flex flex-wrap items-center gap-2 border-t s-border-soft pt-3">
          <button class="umi-btn umi-btn-sm" :disabled="checking" @click="checkUpdate">
            <span class="flex items-center gap-1">
              <NIcon :size="12" :component="RefreshOutline" />{{ checking ? $t('检查中…') : $t('检查更新') }}</span>
          </button>
          <button class="umi-btn umi-btn-sm" :disabled="exporting" @click="exportLogs">
            <span class="flex items-center gap-1">
              <NIcon :size="12" :component="BugOutline" />{{ exporting ? $t('导出中…') : $t('导出日志包') }}</span>
          </button>
          <span v-if="exportedPath" class="s-text-3 break-all text-[10.5px]">{{ $t('已导出到') }} {{ exportedPath }}</span>
        </div>
        <div class="umi-hint mt-2">
          {{ $t('导出日志包：把运行日志与环境摘要（版本 / 平台 / 工具路径 / 引擎与限速 / 过滤规则 / 捕获端口）打包到指定目录，排查问题时直接附带该文件。') }}
        </div>
      </div>
    </template>

    <!-- ==================== 外观 → 自定义强调色（嵌在「主题与强调色」卡内，无独立卡片） ==================== -->
    <div v-else class="mt-3 border-t s-border-soft pt-3">
      <div class="s-text-2 mb-2 flex items-center gap-2 text-[12px] font-medium">
        <NIcon :size="14" :component="ColorPaletteOutline" class="text-accent" />{{ $t('自定义强调色') }}
      </div>

      <div class="flex flex-wrap items-end gap-3">
        <div>
          <label class="umi-label">{{ $t('选择颜色') }}</label>
          <input
            type="color"
            class="h-[34px] w-16 cursor-pointer rounded-xl border bg-transparent p-0.5 s-border-soft"
            :value="hexValid ? hexText : '#7c4dff'"
            @input="onColorPick"
          />
        </div>
        <div>
          <label class="umi-label">{{ $t('色值') }}</label>
          <input
            v-model="hexText"
            class="umi-input !w-36 !text-[11.5px]"
            placeholder="#7c4dff"
            spellcheck="false"
          />
        </div>
        <button class="umi-btn-primary umi-btn-sm" :disabled="!hexValid" @click="applyAccent">
          {{ $t('应用') }}
        </button>
        <button class="umi-btn umi-btn-sm" @click="resetAccent">{{ $t('恢复预设') }}</button>
      </div>

      <div class="umi-inner mt-3">
        <div class="umi-hint mb-2">{{ $t('预览（按当前色值推导的按钮与强调色文字）') }}</div>
        <div class="flex flex-wrap items-center gap-3">
          <span
            class="rounded-xl px-3.5 py-1.5 text-[11.5px] font-semibold text-white shadow-glow"
            :style="{ backgroundImage: previewPrimary }"
          >{{ $t('主要按钮') }}</span>
          <span class="text-[11.5px] font-medium" :style="{ color: previewAccentText }">{{ $t('强调色文字示例') }}</span>
          <span v-if="previewScale" class="flex overflow-hidden rounded-lg border s-border-soft">
            <span
              v-for="step in ['50', '200', '300', '500', '700', '900']"
              :key="step"
              class="h-6 w-6"
              :style="{ background: `rgb(${previewScale[step]})` }"
              :title="step"
            />
          </span>
        </div>
      </div>

      <div class="umi-hint mt-2.5">
        {{ $t('点击「应用」会写入 --umi-accent / --umi-accent-text 及 50–950 色阶（按钮、进度条、滚动条、背景光晕一起跟着变）。文字色按当前明暗主题自动取亮档或深档，浅色主题不会出现白字白底。') }}
      </div>
      <div v-if="autoDarkened" class="umi-hint mt-1.5">
        {{ $t('提示：所选颜色偏亮，已自动加深用于按钮渐变，保证白字对比度。') }}
      </div>
      <div class="umi-hint mt-1.5">
        <template v-if="cfg.accent_custom">
          {{ $t('当前生效：自定义') }} {{ cfg.accent_custom }}
          <span class="ml-1">{{ $t('（点「恢复预设」可撤销）') }}</span>
        </template>
        <template v-else>{{ $t('当前生效：预设强调色') }}</template>
      </div>
    </div>
  </div>
</template>
