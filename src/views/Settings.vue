<script setup lang="ts">
/**
 * 设置页（布局重构）：7 个页签 → 5 个页签
 *
 *  引擎  ← 依赖工具 + 下载引擎（aria2 状态/安装原本写了两遍，现在只留工具行一处）
 *  通用  ← 通用设置 + 默认参数（同一类「默认行为」配置不再分两个页签）
 *  系统  ← 系统与集成（浏览器捕获 / 系统与电源 / 更新与诊断）
 *  外观  ← 主题 + 强调色 + 自定义强调色合成一张卡
 *  插件  ← 插件与沙箱 / 已安装 / 市场 / 解析器与日志
 *
 * 旧深链 ?tab=tools / ?tab=defaults 仍然有效（映射到新页签），零入口丢失。
 */
import { useI18n } from 'vue-i18n'
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import { NIcon, NSelect, NInputNumber, NInput, NSwitch, NProgress, NCheckbox, useMessage } from 'naive-ui'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import {
  SettingsOutline,
  CheckmarkCircleOutline,
  CloseCircleOutline,
  RefreshOutline,
  FolderOpenOutline,
  ColorPaletteOutline,
  DownloadOutline,
  SparklesOutline,
  SunnyOutline,
  MoonOutline,
  ContrastOutline,
  LanguageOutline,
  ImageOutline,
  CloudUploadOutline,
  KeyOutline,
  FilmOutline,
  ChatbubblesOutline,
} from '@vicons/ionicons5'
import { useTaskStore } from '@/stores/tasks'
import { useSettingsStore } from '@/stores/settings'
import SettingsV14 from '@/components/SettingsV14.vue'
import SettingsPlugins from '@/components/SettingsPlugins.vue'
import { resolveSettingsTab, SETTINGS_TABS, type SettingsTab } from '@/services/settingsTabs'
import * as ipc from '@/services/ipc'
import { localizeBackendError } from '@/services/backendError'
import { formatBytes } from '@/services/utils'
import { accents, applyTheme, themeModes } from '@/services/theme'
import { LOCALES, normalizeLocale, setLocale } from '@/i18n'
import { currentLogoPath, useBranding } from '@/services/branding'
import type { LocaleName } from '@/i18n'
import { syncWindowTheme } from '@/services/windowTheme'
import type { AccentName, ThemeMode } from '@/services/theme'
import {
  installActionLabel,
  normalizeSelection,
  pendingInstalls,
  selectionFor,
  selectionSummary,
  statusLine,
  summarizeOutcomes,
} from '@/services/toolSelection'
import type { ToolDirInfo } from '@/types'
const { t: tr } = useI18n()
const branding = useBranding()

const store = useTaskStore()
const settingsStore = useSettingsStore()
const message = useMessage()

const installing = ref<string | null>(null)

/** 版本号缺失时给用户的解释（写在 title 上，不再是语焉不详的空白） */
function probeHint(t: { version?: string | null; probe?: 'exec' | 'exists' | null }): string {
  if ((t.version || '').trim()) return ''
  return t.probe === 'exists'
    ? tr('仅核对文件，不启动探测（GUI 程序）')
    : tr('探活超时，无法确认版本')
}

/* --------------- 依赖工具：按需勾选下载（纯逻辑见 services/toolSelection.ts） --------------- */
/** 勾选的工具名集合 */
const selected = ref<string[]>([])

const selSummary = computed(() => selectionSummary(selected.value, store.tools))

function toggleTool(name: string, checked: boolean) {
  selected.value = checked
    ? normalizeSelection([...selected.value, name], store.tools)
    : selected.value.filter((n) => n !== name)
}

/** 全选 / 只选未装 / 清空 */
function selectMode(mode: 'all' | 'missing' | 'none') {
  selected.value = selectionFor(store.tools, mode)
}

/** 单个工具按钮文案：已装 = 重装，未装 = 下载 */
function actionLabel(name: string): string {
  return installActionLabel(name, store.tools) === 'reinstall' ? tr('重装') : tr('下载')
}

/** 合并后的页签：5 个（名称表与旧深链映射见 services/settingsTabs.ts） */
const activeTab = ref<SettingsTab>('engine')

const route = useRoute()

/** 页签表：顺序与名称来自 services/settingsTabs.ts，这里只补图标与文案 */
const TAB_META = computed<Record<SettingsTab, { label: string; icon: any }>>(() => ({
  engine: { label: tr('引擎'), icon: SparklesOutline },
  general: { label: tr('通用'), icon: SettingsOutline },
  system: { label: tr('系统'), icon: FilmOutline },
  appearance: { label: tr('外观'), icon: ColorPaletteOutline },
  plugins: { label: tr('插件'), icon: KeyOutline },
}))
const TABS = computed(() => SETTINGS_TABS.map((k) => ({ k, ...TAB_META.value[k] })))

/** 深链：#/settings?tab=plugins（旧入口 #/plugins 与 ?tab=tools / ?tab=defaults 都能落位） */
function applyQueryTab() {
  const q = route.query.tab
  const name = Array.isArray(q) ? q[0] : q
  const tab = resolveSettingsTab(name)
  if (tab) activeTab.value = tab
}

const settings = computed(() => settingsStore.settings)

const themeIcons: Record<string, any> = { light: SunnyOutline, dark: MoonOutline, system: ContrastOutline }

async function save() {
  await settingsStore.save()
}

async function saveMsg(text = tr('已保存')) {
  await settingsStore.save()
  message.success(text)
}

/* ------------------------- 依赖工具 / Whisper 模型（引擎页签） ------------------------- */
/** 是否可下载/勾选由后端 ToolStatus.installable 决定（不再前端硬编码清单） */
/** 可手动指定路径的工具（后端只读设置里的这三个字段） */
const PATH_PICKABLE: Record<string, 'ytdlp' | 'ffmpeg' | 'whisper'> = {
  'yt-dlp': 'ytdlp',
  ffmpeg: 'ffmpeg',
  whisper: 'whisper',
}

/** 单个工具：下载 / 重装（后端会覆盖同名文件并重新校验） */
async function installTool(name: string) {
  installing.value = name
  try {
    await ipc.installTool(name)
    message.success(tr('{name} 安装完成', { name }))
    await store.refreshTools()
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  } finally {
    installing.value = null
  }
}

/** 「下载所选」：勾选集 → 待装列表（未装优先）→ 后端逐条装，互不影响 */
async function installSelected() {
  const names = pendingInstalls(selected.value, store.tools)
  if (!names.length) {
    message.info(tr('请先勾选要下载的工具'))
    return
  }
  installing.value = '__batch__'
  try {
    const outs = await ipc.installTools(names)
    const sum = summarizeOutcomes(outs)
    const err = outs.find((o) => !o.ok)?.error
    if (sum.failed.length) {
      message.warning(
        tr('下载完成：成功 {ok} 个，失败 {fail} 个（{names}）', {
          ok: sum.ok,
          fail: sum.failed.length,
          names: sum.failed.join('、'),
        }),
      )
      if (err) message.error(localizeBackendError(err))
    } else {
      message.success(tr('已安装 {n} 个工具', { n: sum.ok }))
    }
    await store.refreshTools()
  } catch (e: any) {
    message.error(localizeBackendError(e))
  } finally {
    installing.value = null
  }
}

/** 校验所选：后端绕过版本缓存真的启动一次程序，报告现在到底能不能跑 */
async function verifySelected() {
  const names = normalizeSelection(selected.value, store.tools)
  if (!names.length) {
    message.info(tr('请先勾选要下载的工具'))
    return
  }
  installing.value = '__batch__'
  try {
    const outs = await ipc.verifyTools(names)
    const sum = summarizeOutcomes(outs)
    if (sum.failed.length) {
      message.warning(
        tr('校验完成：{ok} 个可用，{fail} 个不可用（{names}）', {
          ok: sum.ok,
          fail: sum.failed.length,
          names: sum.failed.join('、'),
        }),
      )
    } else {
      message.success(tr('校验通过：{n} 个工具都可以正常运行', { n: sum.ok }))
    }
    await store.refreshTools()
  } catch (e: any) {
    message.error(localizeBackendError(e))
  } finally {
    installing.value = null
  }
}

/* --------------- 工具安装目录（可自定义 + 迁移） --------------- */
const toolDir = ref<ToolDirInfo | null>(null)
/** 输入框里的草稿；应用成功后回显后端返回的**实际生效目录** */
const dirDraft = ref('')
/** 换目录时是否把已有工具搬过去（默认搬，失败不丢文件） */
const dirMigrate = ref(true)
const dirBusy = ref(false)

async function loadToolDir() {
  try {
    toolDir.value = await ipc.toolDirInfo()
    dirDraft.value = toolDir.value.dir
  } catch {
    /* 浏览器预览 / 无桌面后端：静默（不打扰） */
  }
}

/** 原生目录选择器（拿不到就退回手填输入框） */
async function pickToolDir() {
  try {
    const picked = await openDialog({ directory: true, multiple: false })
    if (typeof picked === 'string') dirDraft.value = picked
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}

/** 应用目录：后端校验 + （可选）迁移，失败时旧目录与设置都不变 */
async function applyToolDir(dir: string) {
  dirBusy.value = true
  try {
    const ch = await ipc.setToolDir(dir, dirMigrate.value)
    toolDir.value = ch.info
    dirDraft.value = ch.info.dir
    if (!ch.migrated) {
      message.success(tr('工具目录已更新'))
    } else if (ch.files_copied > 0) {
      message.success(tr('已迁移 {n} 个文件（{size}）到新目录', { n: ch.files_copied, size: formatBytes(ch.bytes_copied) }))
    } else {
      message.info(tr('新目录暂无可迁移的工具'))
    }
    if (ch.leftovers.length) {
      message.warning(tr('旧目录仍有 {n} 项未能删除（可手动清理）', { n: ch.leftovers.length }))
    }
    if (ch.skipped.length) {
      message.info(tr('已跳过 {n} 个未完成的下载残片', { n: ch.skipped.length }))
    }
    await store.refreshTools()
  } catch (e: any) {
    message.error(localizeBackendError(e))
    await loadToolDir()
  } finally {
    dirBusy.value = false
  }
}

/* ------------------------- 外观：主题 / 强调色 / 语言 / LOGO ------------------------- */
/** 切换主题：立刻应用 + 同步窗口主题 + 保存 */
async function setTheme(mode: ThemeMode) {
  settingsStore.patch({ theme: mode })
  applyTheme(mode, settingsStore.settings.accent as AccentName)
  void syncWindowTheme(mode)
  await save()
}

const logoName = computed(() => {
  const p = currentLogoPath()
  return p ? p.split(/[\\/]/).pop() || p : ''
})

async function pickLogo() {
  try {
    const picked = await openDialog({
      multiple: false,
      directory: false,
      filters: [{ name: 'Logo', extensions: ['png', 'jpg', 'jpeg', 'webp', 'svg', 'ico'] }],
    })
    if (!picked || typeof picked !== 'string') return
    const saved = await ipc.setCustomLogo(picked)
    settingsStore.settings.custom_logo = saved
    message.success(tr('已应用自定义 LOGO'))
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}

async function resetLogo() {
  try {
    await ipc.clearCustomLogo()
    settingsStore.settings.custom_logo = ''
    message.success(tr('已恢复默认 LOGO'))
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}

const currentLocale = computed(() => normalizeLocale(settings.value.ui_language))

async function setLanguage(name: LocaleName) {
  settingsStore.patch({ ui_language: name })
  setLocale(name)
  await save()
  message.success(tr('界面语言已切换'))
}

async function setAccent(name: AccentName) {
  settingsStore.patch({ accent: name })
  applyTheme(settingsStore.settings.theme as ThemeMode, name)
  await save()
}

/* ------------------------- 通用：路径 / 行为 / 默认参数 ------------------------- */
async function pickDir() {
  try {
    const picked = await openDialog({ directory: true, multiple: false })
    if (typeof picked === 'string') {
      settingsStore.patch({ download_dir: picked })
      await saveMsg(tr('下载目录已更新'))
    }
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}

async function pickCookies() {
  try {
    const p = await openDialog({ multiple: false })
    if (typeof p === 'string') {
      settingsStore.patch({ cookies_file: p })
      await saveMsg(tr('Cookies 文件已设置'))
    }
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}

async function pickTool(name: 'ytdlp' | 'ffmpeg' | 'whisper') {
  try {
    const picked = await openDialog({ multiple: false, directory: false })
    if (typeof picked === 'string') {
      const key = name === 'ytdlp' ? 'ytdlp_path' : name === 'ffmpeg' ? 'ffmpeg_path' : 'whisper_path'
      settingsStore.patch({ [key]: picked } as any)
      await saveMsg(tr('路径已保存'))
      await store.refreshTools()
    }
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}

async function openDataDir() {
  try {
    const dir = await ipc.appDataDir()
    await ipc.openPath(dir)
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  }
}

onMounted(async () => {
  applyQueryTab()
  if (!store.ready) await store.bootstrap()
  if (!settingsStore.loaded) await settingsStore.load()
  await store.refreshTools()
  await loadToolDir()
})

// 从 #/plugins 重定向过来（或直接带 ?tab= 打开）时自动切到对应页签
watch(() => route.query.tab, applyQueryTab)
</script>

<template>
  <div class="umi-page">
    <!-- 分段导航：5 个页签 -->
    <div class="flex flex-wrap gap-1.5">
      <button
        v-for="t in TABS"
        :key="t.k"
        class="umi-btn umi-btn-sm"
        :class="activeTab === t.k ? 'chip-active' : ''"
        :aria-pressed="activeTab === t.k"
        :data-tab="t.k"
        @click="activeTab = t.k"
      >
        <span class="flex items-center gap-1.5">
          <NIcon :size="13" :component="t.icon" />{{ t.label }}
        </span>
      </button>
    </div>

    <!-- ============ 引擎：依赖工具 + Whisper 模型 + 引擎/路由/过滤/带宽 ============ -->
    <section v-if="activeTab === 'engine'" class="space-y-3">
      <div class="umi-card p-4" data-test="tools-card">
        <div class="umi-card-title">
          <NIcon :size="14" :component="SettingsOutline" class="text-accent" />{{ $t('依赖工具状态') }}
          <span class="umi-spacer" />
          <span class="umi-hint mr-1 hidden lg:inline">{{ $t('已选 {n} 个（已装 {have}）', { n: selSummary.total, have: selSummary.installed }) }}</span>
          <button class="umi-btn umi-btn-xs" data-test="tools-select-missing" :disabled="!!installing" @click="selectMode('missing')">
            {{ $t('只选未装') }}
          </button>
          <button class="umi-btn umi-btn-xs" data-test="tools-select-all" :disabled="!!installing" @click="selectMode('all')">
            {{ $t('全选') }}
          </button>
          <button class="umi-btn umi-btn-xs" data-test="tools-select-none" :disabled="!!installing" @click="selectMode('none')">
            {{ $t('清空') }}
          </button>
          <button class="umi-btn umi-btn-sm" data-test="tools-verify" :disabled="!!installing || !selSummary.total" @click="verifySelected">
            <span class="flex items-center gap-1"><NIcon :size="12" :component="CheckmarkCircleOutline" />{{ $t('校验所选') }}</span>
          </button>
          <button class="umi-btn-primary umi-btn-sm" data-test="tools-install-selected" :disabled="!!installing || !selSummary.total" @click="installSelected">
            <span class="flex items-center gap-1">
              <NIcon :size="12" :component="DownloadOutline" />{{ installing === '__batch__' ? $t('下载中…') : $t('下载所选（{n}）', { n: selSummary.total }) }}
            </span>
          </button>
          <button class="umi-btn umi-btn-sm" @click="store.refreshTools()">
            <span class="flex items-center gap-1"><NIcon :size="12" :component="RefreshOutline" />{{ $t('重新检测') }}</span>
          </button>
        </div>

        <!-- 两列网格：9 个工具从 9 行压到 5 行，首屏内能看全 -->
        <div class="grid gap-x-3 gap-y-1 sm:grid-cols-2">
          <div v-for="t in store.tools" :key="t.name" class="umi-row !py-1" :data-test="`tool-row-${t.name}`">
            <NCheckbox
              v-if="t.installable"
              size="small"
              :checked="selected.includes(t.name)"
              :disabled="!!installing"
              :data-test="`tool-check-${t.name}`"
              @update:checked="(v: boolean) => toggleTool(t.name, v)"
            />
            <span v-else class="s-text-3 w-4 shrink-0 text-center text-[11px]" :title="$t('需手动放置')">—</span>
            <NIcon
              :size="14"
              :component="t.found ? CheckmarkCircleOutline : CloseCircleOutline"
              :class="t.found ? 'text-emerald-600 dark:text-emerald-400' : 'text-amber-600 dark:text-amber-400'"
            />
            <span class="s-text shrink-0 text-[12px] font-medium">{{ t.name }}</span>
            <span
              class="shrink-0 rounded-md px-1.5 py-[1px] text-[10px]"
              :class="{
                'bg-emerald-500/15 text-emerald-500': t.source === 'managed',
                'bg-cyan-500/15 text-cyan-500': t.source === 'system',
                'bg-umi-500/15 text-accent': t.source === 'bundled',
                's-surface-3 s-text-3': t.source === 'missing',
              }"
            >
              {{ { managed: $t('已托管'), system: $t('系统'), bundled: $t('内置'), missing: $t('缺失') }[t.source] }}
            </span>
            <span class="s-text-3 min-w-0 flex-1 truncate text-[10px]" :title="t.path || t.hint">
              <template v-if="statusLine(t).installed">
                <span :title="probeHint(t)">{{
                  probeHint(t) ? $t('已装 · {version}', { version: $t('未知') }) : $t('已装 · {version}', { version: statusLine(t).version })
                }}</span>
              </template>
              <template v-else>{{ $t('未装 · {size}', { size: statusLine(t).sizeHint || t.hint }) }}</template><template v-if="t.path"> · {{ t.path }}</template>
            </span>
            <div v-if="store.toolProgress[t.name] && (installing === t.name || installing === '__batch__')" class="w-20 shrink-0">
              <NProgress
                type="line"
                :percentage="Math.round(store.toolProgress[t.name].percent)"
                :height="3"
                :show-indicator="false"
                color="rgb(var(--umi-a500))"
              />
            </div>
            <div class="flex shrink-0 gap-1.5">
              <button
                v-if="PATH_PICKABLE[t.name]"
                class="umi-btn umi-btn-xs"
                :data-test="`tool-path-${t.name}`"
                @click="pickTool(t.name as 'ytdlp' | 'ffmpeg' | 'whisper')"
              >{{ $t('指定路径') }}</button>
              <button
                v-if="t.installable"
                class="umi-btn-primary umi-btn-xs"
                :disabled="!!installing"
                :data-test="`tool-install-${t.name}`"
                @click="installTool(t.name)"
              >
                {{ installing === t.name ? $t('安装中…') : actionLabel(t.name) }}
              </button>
              <span v-else class="s-text-3 text-[10px]">{{ t.name === 'ffprobe' ? $t('随 ffmpeg') : $t('需手动放置') }}</span>
            </div>
          </div>
        </div>

      <!-- 工具安装目录：可自定义（原生目录选择器 + 手填校验），换目录时可迁移已有工具。
           与依赖工具同卡（不新增卡片：Settings.vue 的卡片预算由 uiLayout.test.ts 守住） -->
      <div class="umi-inner mt-3" data-test="tool-dir-card">
        <div class="s-text-2 flex items-center gap-2 text-[12px] font-medium">
          <NIcon :size="14" :component="FolderOpenOutline" class="text-accent" />{{ $t('工具安装目录') }}
          <span v-if="toolDir && !toolDir.custom" class="s-text-3 text-[10px]">{{ $t('当前使用默认目录') }}</span>
        </div>
        <div class="flex flex-wrap items-center gap-2">
          <input
            v-model="dirDraft"
            class="umi-input min-w-[220px] flex-1 !text-[11.5px]"
            :placeholder="toolDir ? toolDir.default_dir : ''"
            data-test="tool-dir-input"
          />
          <button class="umi-btn umi-btn-sm whitespace-nowrap" :disabled="dirBusy" data-test="tool-dir-browse" @click="pickToolDir">
            <span class="flex items-center gap-1"><NIcon :size="12" :component="FolderOpenOutline" />{{ $t('浏览…') }}</span>
          </button>
          <button class="umi-btn-primary umi-btn-sm whitespace-nowrap" :disabled="dirBusy" data-test="tool-dir-apply" @click="applyToolDir(dirDraft)">
            {{ dirBusy ? $t('处理中…') : $t('应用目录') }}
          </button>
          <button class="umi-btn umi-btn-sm whitespace-nowrap" :disabled="dirBusy || !toolDir || !toolDir.custom" data-test="tool-dir-reset" @click="applyToolDir('')">
            {{ $t('恢复默认') }}
          </button>
        </div>
        <label class="s-text-2 mt-2 flex cursor-pointer items-center gap-2 text-[12px]">
          <NSwitch v-model:value="dirMigrate" size="small" :disabled="dirBusy" />{{ $t('迁移已有工具（先复制校验，成功后才删旧目录）') }}
        </label>
        <div class="umi-hint mt-1.5">
          {{ $t('受管工具（yt-dlp / ffmpeg / whisper 等）安装在这里；解析与调用只认这个目录，不改系统 PATH。') }}
        </div>
        <div v-if="toolDir" class="umi-hint mt-1 truncate" :title="toolDir.dir">{{ $t('生效目录：{dir}', { dir: toolDir.dir }) }}</div>
        <div v-if="toolDir && !toolDir.valid" class="mt-1 text-[11px] text-amber-500">
          {{ $t('配置里的工具目录不是绝对路径，已回退到默认目录。') }}
        </div>
      </div>
      </div>

      <!-- 下载引擎 / 链接路由与过滤 / 带宽与并发（原「下载引擎」页签 5 张卡收成 3 张） -->
      <!-- 语音模型的下载与管理已挪到「字幕」页（选择模型的同一处），这里只留一行指引 -->
      <div class="umi-hint px-1" data-test="models-moved-hint">
        {{ $t('Whisper 语音模型的下载 / 自定义导入已挪到「字幕」页 —— 在模型下拉框下方即可直接下载或贴链接安装。') }}
      </div>

      <SettingsV14 section="engine" />
    </section>

    <!-- ============ 通用：存储与网络 + 下载行为 + 默认参数 ============ -->
    <section v-else-if="activeTab === 'general'" class="space-y-3">
      <div class="umi-card p-4" data-test="storage-card">
        <div class="umi-card-title">
          <NIcon :size="14" :component="FolderOpenOutline" class="text-accent" />{{ $t('存储与网络') }}
        </div>
        <label class="umi-label">{{ $t('默认下载目录') }}</label>
        <div class="flex gap-2">
          <input v-model="settings.download_dir" class="umi-input flex-1" />
          <button class="umi-btn whitespace-nowrap" @click="pickDir">{{ $t('浏览…') }}</button>
          <button class="umi-btn-primary whitespace-nowrap" @click="saveMsg()">{{ $t('保存') }}</button>
        </div>

        <div class="mt-3 grid gap-3 sm:grid-cols-2">
          <div>
            <label class="umi-label">{{ $t('代理服务器') }}</label>
            <NInput v-model:value="settings.proxy" size="small" placeholder="http://127.0.0.1:7890" @blur="save" />
            <div class="umi-hint mt-1">
              {{ $t('工具与模型下载都走这里填写的代理；留空时自动跟随系统代理（Windows 设置 → 网络和 Internet → 代理）') }}
            </div>
          </div>
          <div>
            <label class="umi-label">{{ $t('Cookies 文件（用于会员 / 受限内容）') }}</label>
            <div class="flex gap-2">
              <input
                v-model="settings.cookies_file"
                class="umi-input flex-1 !text-[11.5px]"
                :placeholder="$t('cookies.txt（Netscape 格式）')"
                @change="save"
              />
              <button class="umi-btn umi-btn-sm whitespace-nowrap" @click="pickCookies">{{ $t('选择') }}</button>
            </div>
          </div>
        </div>

        <div class="mt-3 border-t s-border-soft pt-2.5">
          <button class="umi-btn umi-btn-sm" @click="openDataDir">
            <span class="flex items-center gap-1">
              <NIcon :size="12" :component="FolderOpenOutline" />{{ $t('打开数据目录') }}</span>
          </button>
        </div>
      </div>

      <div class="umi-card p-4" data-test="behavior-card">
        <div class="umi-card-title">
          <NIcon :size="14" :component="DownloadOutline" class="text-accent" />{{ $t('下载行为') }}
        </div>
        <div class="grid gap-x-6 gap-y-2.5 sm:grid-cols-2">
          <label class="s-text-2 flex cursor-pointer items-center gap-2 text-[12px]">
            <NSwitch v-model:value="settings.keep_original" size="small" @update:value="save" />{{ $t('保留原始文件') }}</label>
          <label class="s-text-2 flex cursor-pointer items-center gap-2 text-[12px]">
            <NSwitch v-model:value="settings.notify_on_finish" size="small" @update:value="save" />{{ $t('完成后系统通知') }}</label>
          <label class="s-text-2 flex cursor-pointer items-center gap-2 text-[12px]">
            <NSwitch v-model:value="settings.open_folder_when_done" size="small" @update:value="save" />{{ $t('完成后打开所在文件夹') }}</label>
          <label class="s-text-2 flex cursor-pointer items-center gap-2 text-[12px]">
            <NSwitch v-model:value="settings.auto_start" size="small" @update:value="save" />{{ $t('解析后自动开始下载') }}</label>
        </div>
      </div>

      <!-- 默认参数：下载 / 转换 / 字幕三组收进一张卡（原来是三张各占一屏的卡） -->
      <div class="umi-card p-4" data-test="defaults-card">
        <div class="umi-card-title">
          <NIcon :size="14" :component="FilmOutline" class="text-accent" />{{ $t('默认参数') }}
        </div>

        <div class="s-text-2 mb-2 flex items-center gap-1.5 text-[11.5px] font-medium">
          <NIcon :size="13" :component="DownloadOutline" class="text-accent" />{{ $t('下载默认值') }}
        </div>
        <div class="grid gap-3 sm:grid-cols-3">
          <div>
            <label class="umi-label">{{ $t('默认容器') }}</label>
            <NSelect
              v-model:value="settings.default_container"
              :options="['mp4', 'mkv', 'webm'].map((v) => ({ label: v.toUpperCase(), value: v }))"
              size="small"
              @update:value="save"
            />
          </div>
          <div>
            <label class="umi-label">{{ $t('默认音频格式') }}</label>
            <NSelect
              v-model:value="settings.default_audio_format"
              :options="['mp3', 'm4a', 'flac', 'wav', 'opus'].map((v) => ({ label: v.toUpperCase(), value: v }))"
              size="small"
              @update:value="save"
            />
          </div>
          <div>
            <label class="umi-label">{{ $t('清晰度偏好') }}</label>
            <NSelect
              v-model:value="settings.default_quality"
              :options="[
                { label: $t('最佳画质'), value: 'best' },
                { label: $t('优先 1080p'), value: '1080' },
                { label: $t('优先 720p'), value: '720' },
                { label: $t('优先 480p'), value: '480' },
                { label: $t('仅音频'), value: 'audio' },
              ]"
              size="small"
              @update:value="save"
            />
          </div>
          <div class="sm:col-span-3">
            <label class="umi-label">{{ $t('文件名模板（yt-dlp 语法）') }}</label>
            <input
              v-model="settings.filename_template"
              class="umi-input"
              placeholder="%(title).150B [%(id)s].%(ext)s"
              @blur="save"
            />
            <div class="umi-hint mt-1">{{ $t('常用变量：%(title)s 标题 · %(id)s 视频 ID · %(uploader)s 作者 · %(ext)s 扩展名 · %(resolution)s 分辨率') }}</div>
          </div>
        </div>

        <div class="s-text-2 mb-2 mt-3 flex items-center gap-1.5 border-t s-border-soft pt-3 text-[11.5px] font-medium">
          <NIcon :size="13" :component="SparklesOutline" class="text-accent" />{{ $t('转换默认值') }}
        </div>
        <div class="grid gap-3 sm:grid-cols-2">
          <div>
            <label class="umi-label">{{ $t('默认分辨率') }}</label>
            <NSelect
              v-model:value="settings.default_resolution"
              :options="
                ['原分辨率', '1920x1080', '1280x720', '854x480'].map((r) => ({ label: r === '原分辨率' ? tr('原分辨率') : r, value: r }))
              "
              size="small"
              @update:value="save"
            />
          </div>
          <div>
            <label class="umi-label">{{ $t('默认质量（CRF）') }}</label>
            <NSelect
              v-model:value="settings.default_crf"
              :options="[
                { label: $t('接近原始画质（14）'), value: 14 },
                { label: $t('极速 · 体积大（18）'), value: 18 },
                { label: $t('标准 · 推荐（23）'), value: 23 },
                { label: $t('高压缩 · 体积小（28）'), value: 28 },
              ]"
              size="small"
              @update:value="save"
            />
          </div>
        </div>
        <div class="mt-2.5 grid gap-x-6 gap-y-2.5 sm:grid-cols-2">
          <label class="s-text-2 flex cursor-pointer items-center gap-2 text-[12px]">
            <NSwitch v-model:value="settings.auto_detect_format" size="small" @update:value="save" />{{ $t('载入文件后自动检测最佳输出格式') }}</label>
          <label class="s-text-2 flex cursor-pointer items-center gap-2 text-[12px]">
            <NSwitch v-model:value="settings.hwaccel" size="small" @update:value="save" />{{ $t('默认启用硬件加速解码') }}</label>
        </div>

        <div class="s-text-2 mb-2 mt-3 flex items-center gap-1.5 border-t s-border-soft pt-3 text-[11.5px] font-medium">
          <NIcon :size="13" :component="ChatbubblesOutline" class="text-accent" />{{ $t('字幕默认值') }}
        </div>
        <div class="grid gap-3 sm:grid-cols-2">
          <div>
            <label class="umi-label">{{ $t('默认识别语言') }}</label>
            <NSelect
              v-model:value="settings.subtitle_language"
              :options="[
                { label: $t('自动检测'), value: 'auto' },
                { label: $t('中文'), value: 'zh' },
                { label: $t('英语'), value: 'en' },
                { label: $t('日语'), value: 'ja' },
                { label: $t('韩语'), value: 'ko' },
              ]"
              size="small"
              @update:value="save"
            />
          </div>
          <div>
            <label class="umi-label">{{ $t('默认导出格式') }}</label>
            <NSelect
              v-model:value="settings.subtitle_format"
              :options="['srt', 'vtt', 'ass', 'txt'].map((v) => ({ label: v.toUpperCase(), value: v }))"
              size="small"
              @update:value="save"
            />
          </div>
        </div>
      </div>
    </section>

    <!-- ============ 系统：浏览器捕获 / 系统与电源 / 更新与诊断 ============ -->
    <section v-else-if="activeTab === 'system'" class="space-y-3">
      <SettingsV14 section="system" />
    </section>

    <!-- ============ 外观：主题与强调色 + 界面语言 + 品牌与动画 ============ -->
    <section v-else-if="activeTab === 'appearance'" class="space-y-3">
      <!-- 主题 + 预设强调色 + 自定义强调色，三张卡合成一张 -->
      <div class="umi-card p-4" data-test="theme-card">
        <div class="umi-card-title">
          <NIcon :size="14" :component="ContrastOutline" class="text-accent" />{{ $t('主题与强调色') }}
        </div>
        <div class="grid gap-2.5 sm:grid-cols-3">
          <button
            v-for="t in themeModes()"
            :key="t.value"
            class="umi-entry flex items-center gap-3"
            :class="settings.theme === t.value ? '!border-umi-400/60 !bg-umi-500/15' : ''"
            :data-test="`theme-${t.value}`"
            @click="setTheme(t.value as ThemeMode)"
          >
            <NIcon :size="18" :component="themeIcons[t.value]" class="text-accent" />
            <span>
              <span class="block text-[12.5px] font-medium s-text">{{ t.label }}</span>
              <span class="block text-[10.5px] s-text-3">{{ t.hint }}</span>
            </span>
          </button>
        </div>

        <div class="s-text-2 mb-2 mt-3 flex items-center gap-2 text-[12px] font-medium">
          <NIcon :size="14" :component="ColorPaletteOutline" class="text-accent" />{{ $t('强调色') }}
        </div>
        <div class="flex flex-wrap gap-2">
          <button
            v-for="a in accents()"
            :key="a.value"
            class="umi-entry flex items-center gap-2 !py-2"
            :class="settings.accent === a.value ? '!border-umi-400/60 !bg-umi-500/15' : ''"
            :data-test="`accent-${a.value}`"
            @click="setAccent(a.value as AccentName)"
          >
            <span class="h-4 w-4 rounded-full" :style="{ background: a.hex }" />
            <span class="s-text-2 text-[11.5px]">{{ a.label }}</span>
          </button>
        </div>

        <!-- 自定义强调色（原「外观」页签最后一张卡，现并入本卡） -->
        <SettingsV14 section="accent" />
      </div>

      <div class="umi-card p-4" data-test="language-card">
        <div class="umi-card-title">
          <NIcon :size="14" :component="LanguageOutline" class="text-accent" />{{ $t('界面语言') }}
          <span class="umi-spacer" />
          <span class="umi-hint">{{ $t('切换后立即生效，无需重启。') }}</span>
        </div>
        <div class="grid grid-cols-2 gap-2.5 sm:grid-cols-4">
          <button
            v-for="l in LOCALES"
            :key="l.value"
            class="umi-entry !py-2"
            :class="currentLocale === l.value ? '!border-umi-400/60 !bg-umi-500/15' : ''"
            :data-test="`locale-${l.value}`"
            @click="setLanguage(l.value)"
          >
            <span class="block text-[12.5px] font-medium s-text">{{ l.native }}</span>
            <span class="block text-[10.5px] s-text-3">{{ l.code }}</span>
          </button>
        </div>
      </div>

      <!-- 自定义 LOGO + 动画与性能，两张只有几个控件的卡合成一张「品牌与动画」 -->
      <div class="umi-card p-4" data-test="brand-card">
        <div class="umi-card-title">
          <NIcon :size="14" :component="ImageOutline" class="text-accent" />{{ $t('品牌与动画') }}
        </div>
        <div class="flex items-start gap-3.5">
          <div class="flex h-[68px] w-[68px] shrink-0 items-center justify-center rounded-2xl border s-border-soft s-surface-2 p-1.5">
            <img :src="branding.logoUrl.value" alt="logo" class="max-h-full max-w-full object-contain" draggable="false" />
          </div>
          <div class="min-w-0 flex-1">
            <div class="flex flex-wrap items-center gap-2">
              <button class="umi-btn umi-btn-sm" data-test="logo-pick" @click="pickLogo">
                <span class="flex items-center gap-1"><NIcon :size="13" :component="CloudUploadOutline" />{{ $t('选择图片…') }}</span>
              </button>
              <button class="umi-btn umi-btn-sm disabled:opacity-40" :disabled="!branding.isCustom.value" data-test="logo-reset" @click="resetLogo">
                {{ $t('恢复默认') }}
              </button>
            </div>
            <div class="umi-hint mt-1.5">
              {{ $t('支持 PNG / JPG / WEBP / SVG / ICO，4 MB 以内。PNG、ICO 会同时替换窗口与任务栏图标。') }}
            </div>
            <div v-if="branding.isCustom.value" class="umi-hint mt-1 truncate" :title="logoName">
              {{ $t('当前使用自定义 LOGO：') }}{{ logoName }}
            </div>
          </div>
        </div>

        <div class="mt-3 grid gap-x-6 gap-y-2.5 border-t s-border-soft pt-3 sm:grid-cols-2">
          <label class="s-text-2 flex cursor-pointer items-center gap-2 text-[12px]">
            <NSwitch v-model:value="settings.animation" size="small" @update:value="save" />{{ $t('背景粒子动画（关闭可明显降低 GPU / CPU 占用）') }}</label>
          <label class="s-text-2 flex cursor-pointer items-center gap-2 text-[12px]">
            <NSwitch v-model:value="settings.compact_cards" size="small" @update:value="save" />{{ $t('紧凑任务卡片') }}</label>
          <div>
            <label class="umi-label">{{ $t('动画质量') }}</label>
            <NSelect
              v-model:value="settings.animation_quality"
              :options="[
                { label: $t('低（省电，粒子最少）'), value: 'low' },
                { label: $t('中（推荐）'), value: 'medium' },
                { label: $t('高（粒子最多）'), value: 'high' },
              ]"
              size="small"
              @update:value="save"
            />
          </div>
          <div>
            <label class="umi-label">{{ $t('粒子形状') }}</label>
            <NSelect
              v-model:value="settings.particle_shape"
              :options="[
                { label: $t('圆形'), value: 'circle' },
                { label: $t('棱形（菱形）'), value: 'diamond' },
                { label: $t('正方形'), value: 'square' },
              ]"
              size="small"
              @update:value="save"
            />
          </div>
        </div>
        <div class="umi-hint mt-2">
          {{ $t('提示：粒子动画采用预渲染发光贴图 + 空间分桶连线 + 30fps 限帧，已大幅降低占用；低配设备建议选择「低」或直接关闭。') }}
        </div>
      </div>
    </section>

    <!-- ============ 插件 ============ -->
    <section v-else class="space-y-3">
      <SettingsPlugins />
    </section>
  </div>
</template>
