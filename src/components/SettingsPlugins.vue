<script setup lang="ts">
/**
 * 插件管理（1.6：从独立页面搬进「设置 → 插件」页签，逻辑与事件订阅保持不变）
 *
 *  - plugin_sandbox_info()      沙箱信息（引擎 / 内存上限 / 脚本超时）
 *  - plugin_install_from_url()  从 GitHub 仓库 / https 直链安装（极简入口卡片）
 *  - plugin_list()              已安装插件（启用开关 / 测试 / 卸载）
 *  - plugin_run_resolvers()     解析器试跑（可选）
 *  - plugin://event             插件事件（仅错误级别转提示；事件日志 UI 已移除）
 *
 * 插件页极简（1.11）：市场浏览卡片与事件日志块已移除；安装入口只保留「从 GitHub 安装」
 * （后端 plugin_install_from_url 负责：仅 https、≤ 5 MB、超时 60 s、只落盘不执行）。
 * 旧入口 `#/plugins` 由路由重定向到 `#/settings?tab=plugins`，本组件不受影响。
 * 浏览器预览（非 Tauri）下会读不到任何数据，这里用 isTauri() 守卫优雅降级。
 */
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { NIcon, NSwitch, useMessage } from 'naive-ui'
import {
  AlertCircleOutline,
  CheckmarkCircleOutline,
  CloudDownloadOutline,
  CubeOutline,
  ExtensionPuzzleOutline,
  ListOutline,
  RefreshOutline,
  SparklesOutline,
} from '@vicons/ionicons5'
import { isTauri } from '@/services/ipc'
import { useTaskStore } from '@/stores/tasks'
import {
  onPluginEvent,
  onPluginInstalled,
  pluginInstallFromUrl,
  pluginList,
  pluginRunResolvers,
  pluginSandboxInfo,
  pluginSetEnabled,
  pluginTest,
  pluginUninstall,
  shortSha,
  type PluginInfo,
  type PluginSandboxInfo,
  type PluginTestResult,
} from '@/services/plugins'

const { t: tr } = useI18n()
const message = useMessage()
const store = useTaskStore()

/** 运行在 Tauri 容器内？（浏览器预览时 IPC 一律不可用） */
const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

const sandbox = ref<PluginSandboxInfo | null>(null)
const installed = ref<PluginInfo[]>([])
const loading = ref(false)
const busy = ref('')
const testResults = ref<Record<string, PluginTestResult>>({})
const error = ref('')

/* ------------------------- 数据加载 ------------------------- */
async function refreshAll(silent = false) {
  if (!inTauri) {
    if (!silent) message.warning(tr('该功能需要在桌面客户端中运行'))
    return
  }
  loading.value = true
  error.value = ''
  try {
    const [s, list] = await Promise.all([pluginSandboxInfo(), pluginList()])
    sandbox.value = s
    installed.value = Array.isArray(list) ? list : []
  } catch (e: any) {
    error.value = String(e?.message ?? e)
    if (!silent) message.error(error.value)
  } finally {
    loading.value = false
  }
}

async function reloadInstalled() {
  try {
    const list = await pluginList()
    installed.value = Array.isArray(list) ? list : []
  } catch (e: any) {
    error.value = String(e?.message ?? e)
  }
}

/* ------------------------- 从 GitHub / https 直链安装 ------------------------- */
const installUrl = ref('')
const installBusy = ref(false)
const installError = ref('')
const installOk = ref('')

async function installFromUrl() {
  const url = installUrl.value.trim()
  if (!url) return
  if (!inTauri) {
    message.warning(tr('该功能需要在桌面客户端中运行'))
    return
  }
  installBusy.value = true
  installError.value = ''
  installOk.value = ''
  try {
    const info = await pluginInstallFromUrl(url)
    const name = info?.name || info?.id || url
    installOk.value = String(name)
    message.success(`${tr('插件已安装')}：${name}`)
    installUrl.value = ''
    await reloadInstalled()
  } catch (e: any) {
    installError.value = String(e?.message ?? e)
    message.error(installError.value)
  } finally {
    installBusy.value = false
  }
}

/* ------------------------- 卸载 / 启用 / 测试 ------------------------- */
async function uninstall(p: PluginInfo) {
  if (!inTauri) {
    message.warning(tr('该功能需要在桌面客户端中运行'))
    return
  }
  busy.value = `uninstall:${p.id}`
  try {
    await pluginUninstall(p.id)
    message.success(`${tr('插件已卸载')}：${p.name || p.id}`)
    const tr2 = { ...testResults.value }
    delete tr2[p.id]
    testResults.value = tr2
    await reloadInstalled()
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  } finally {
    busy.value = ''
  }
}

async function toggle(p: PluginInfo, enabled: boolean) {
  if (!inTauri) {
    message.warning(tr('该功能需要在桌面客户端中运行'))
    return
  }
  busy.value = `toggle:${p.id}`
  try {
    await pluginSetEnabled(p.id, enabled)
    message.success(`${enabled ? tr('已启用') : tr('已停用')}：${p.name || p.id}`)
    await reloadInstalled()
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  } finally {
    busy.value = ''
  }
}

async function runTest(p: PluginInfo) {
  if (!inTauri) {
    message.warning(tr('该功能需要在桌面客户端中运行'))
    return
  }
  busy.value = `test:${p.id}`
  try {
    const r = await pluginTest(p.id)
    testResults.value = { ...testResults.value, [p.id]: r }
  } catch (e: any) {
    testResults.value = {
      ...testResults.value,
      [p.id]: { ok: false, logs: [], error: String(e?.message ?? e) },
    }
  } finally {
    busy.value = ''
  }
}

/* ------------------------- 解析器试跑 ------------------------- */
const resolverUrl = ref('')
const resolverBusy = ref(false)
const resolverResult = ref('')
const resolverError = ref('')

async function runResolvers() {
  const url = resolverUrl.value.trim()
  if (!url) return
  if (!inTauri) {
    message.warning(tr('该功能需要在桌面客户端中运行'))
    return
  }
  resolverBusy.value = true
  resolverResult.value = ''
  resolverError.value = ''
  try {
    const r = await pluginRunResolvers(url)
    resolverResult.value = typeof r === 'string' ? r : JSON.stringify(r, null, 2)
  } catch (e: any) {
    resolverError.value = String(e?.message ?? e)
  } finally {
    resolverBusy.value = false
  }
}

/* ------------------------- 事件订阅 ------------------------- */
let offEvent: (() => void) | null = null
let offInstalled: (() => void) | null = null

onMounted(async () => {
  await refreshAll(true)
  // 事件日志 UI 已移除（插件页极简）：仍订阅事件，只把错误级别转成一条提示
  offEvent = await onPluginEvent((p) => {
    if ((p.level || '').toLowerCase() === 'error') message.error(`${p.id ? `${p.id} · ` : ''}${p.message}`)
  })
  offInstalled = await onPluginInstalled(async () => {
    await reloadInstalled()
  })
})

onUnmounted(() => {
  offEvent?.()
  offInstalled?.()
})

const sandboxReady = computed(() => !!sandbox.value?.available)
</script>

<template>
  <div class="space-y-3" data-test="settings-plugins">
    <!-- 从 GitHub 安装：地址输入 + 安装按钮（仅 https；体积 / 超时 / 净化由后端负责） -->
    <div class="umi-card p-4" data-test="plugin-install-url">
      <div class="umi-card-title">
        <NIcon :size="14" :component="CloudDownloadOutline" class="text-accent" />{{ $t('从 GitHub 安装') }}
      </div>
      <div class="flex gap-2">
        <input
          v-model="installUrl"
          class="umi-input flex-1 !text-[11.5px]"
          :placeholder="$t('粘贴 GitHub 仓库地址（https://github.com/owner/repo）或 .js / 清单 JSON 的 https 直链')"
          spellcheck="false"
          :disabled="installBusy"
          @keyup.enter="installFromUrl"
        />
        <button
          class="umi-btn-primary umi-btn-sm whitespace-nowrap"
          :disabled="installBusy || !installUrl.trim()"
          @click="installFromUrl"
        >
          {{ installBusy ? $t('安装中…') : $t('安装') }}
        </button>
      </div>
      <div
        v-if="installError"
        class="mt-2 break-all rounded-lg border border-rose-500/25 bg-rose-500/10 px-2.5 py-1.5 font-mono text-[10.5px] text-rose-500"
      >
        {{ installError }}
      </div>
      <div
        v-else-if="installOk"
        class="mt-2 rounded-lg border border-emerald-500/25 bg-emerald-500/10 px-2.5 py-1.5 text-[10.5px] text-emerald-600 dark:text-emerald-400"
      >
        {{ $t('插件已安装') }}：{{ installOk }}
      </div>
      <div v-else class="umi-hint mt-2">
        {{ $t('支持 GitHub 仓库地址（自动读取仓库根目录的 plugin.json / manifest.json）或任意 https 直链（.js / 清单 JSON）；仅 https、5 MB 以内；下载内容不执行，只写入插件目录。') }}
      </div>
    </div>

    <!-- 插件与沙箱：页头（说明 + 刷新）与沙箱四项指标合成一张卡 -->
    <div class="umi-card p-4" data-test="plugin-head">
      <div class="umi-card-title">
        <NIcon :size="14" :component="ExtensionPuzzleOutline" class="text-accent" />{{ $t('插件管理') }}
        <span
          class="umi-chip !text-[10px]"
          :class="sandboxReady ? '!border-emerald-500/30 text-emerald-600 dark:text-emerald-400' : ''"
        >
          {{ sandbox?.available ? $t('沙箱可用') : $t('沙箱不可用') }}
        </span>
        <span class="umi-spacer" />
        <button class="umi-btn umi-btn-sm" :disabled="loading" @click="refreshAll(false)">
          <span class="flex items-center gap-1">
            <NIcon :size="12" :component="RefreshOutline" />{{ loading ? $t('刷新中…') : $t('刷新列表') }}</span>
        </button>
      </div>

      <div class="grid grid-cols-2 gap-2 sm:grid-cols-4">
        <div class="umi-inner">
          <div class="s-text-3 text-[10px]">{{ $t('引擎') }}</div>
          <div class="s-text mt-0.5 text-[12px] font-medium">{{ sandbox?.engine || '--' }}</div>
        </div>
        <div class="umi-inner">
          <div class="s-text-3 text-[10px]">{{ $t('版本') }}</div>
          <div class="s-text mt-0.5 text-[12px] font-medium">{{ sandbox?.version || '--' }}</div>
        </div>
        <div class="umi-inner">
          <div class="s-text-3 text-[10px]">{{ $t('内存上限') }}</div>
          <div class="s-text mt-0.5 text-[12px] font-medium">
            {{ sandbox?.memory_limit_mb ? `${sandbox.memory_limit_mb} MB` : '--' }}
          </div>
        </div>
        <div class="umi-inner">
          <div class="s-text-3 text-[10px]">{{ $t('脚本超时') }}</div>
          <div class="s-text mt-0.5 text-[12px] font-medium">
            {{ sandbox?.script_timeout_ms ? `${sandbox.script_timeout_ms} ms` : '--' }}
          </div>
        </div>
      </div>

      <div class="umi-hint mt-2">
        {{ $t('插件跑在受限沙箱里，只提供解析器扩展；可从GitHub 地址 / https 直链安装（只落盘不执行）。沙箱限制单个插件的内存与单次脚本执行时间，超限即被中止，不会拖垮主程序。') }}
      </div>

      <div v-if="!inTauri" class="mt-2 rounded-lg border border-amber-500/25 bg-amber-500/10 px-2.5 py-1.5 text-[11px] text-amber-700 dark:text-amber-400">
        {{ $t('当前是浏览器预览：插件数据不可用，请在桌面客户端中打开本页。') }}
      </div>
      <div v-else-if="error" class="mt-2 rounded-lg border border-rose-500/25 bg-rose-500/10 px-2.5 py-1.5 text-[11px] text-rose-500">
        {{ error }}
      </div>
    </div>

    <!-- 已安装插件 -->
    <div class="umi-card p-4" data-test="plugin-installed">
      <div class="umi-card-title">
        <NIcon :size="14" :component="ListOutline" class="text-accent" />{{ $t('已安装插件') }}
        <span class="s-surface-2 s-text-3 rounded-md px-1.5 py-0.5 text-[10px] tabular-nums">{{ installed.length }}</span>
      </div>

      <div class="space-y-2">
        <div
          v-for="p in installed"
          :key="p.id"
          class="umi-inner"
        >
          <div class="flex items-start gap-2.5">
            <NIcon
              :size="16"
              class="mt-[1px] shrink-0"
              :component="p.enabled ? CheckmarkCircleOutline : AlertCircleOutline"
              :class="p.enabled ? 'text-emerald-600 dark:text-emerald-400' : 'text-amber-600 dark:text-amber-400'"
            />
            <div class="min-w-0 flex-1">
              <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
                <span class="s-text text-[12.5px] font-medium">{{ p.name || p.id }}</span>
                <span v-if="p.version" class="umi-chip !text-[10px]">v{{ p.version }}</span>
                <span v-if="p.source" class="s-text-3 s-surface-3 rounded-full px-1.5 py-px text-[10px]">{{ p.source }}</span>
                <span v-if="busy.endsWith(`:${p.id}`)" class="s-text-3 text-[10px]">{{ $t('处理中…') }}</span>
              </div>
              <div v-if="p.description" class="s-text-3 mt-0.5 text-[10.5px] leading-relaxed">{{ p.description }}</div>
              <div class="s-text-3 mt-1 flex flex-wrap items-center gap-x-3 gap-y-0.5 break-all font-mono text-[10px]">
                <span :title="p.sha256">sha256 {{ shortSha(p.sha256) }}</span>
                <span v-if="p.installed_at">{{ p.installed_at }}</span>
                <span>{{ p.id }}</span>
              </div>

              <!-- 测试结果 -->
              <div
                v-if="testResults[p.id]"
                class="mt-2 rounded-lg border px-2.5 py-2 text-[10.5px]"
                :class="testResults[p.id].ok ? 'border-emerald-500/25 bg-emerald-500/10' : 'border-rose-500/25 bg-rose-500/10'"
              >
                <div :class="testResults[p.id].ok ? 'text-emerald-600 dark:text-emerald-400' : 'text-rose-500'">
                  {{ testResults[p.id].ok ? $t('测试通过') : $t('测试失败') }}
                </div>
                <div v-if="testResults[p.id].error" class="mt-1 break-all font-mono text-rose-500">
                  {{ testResults[p.id].error }}
                </div>
                <div v-if="testResults[p.id].result !== undefined && testResults[p.id].result !== null" class="s-text-2 mt-1 break-all font-mono">
                  {{ typeof testResults[p.id].result === 'string' ? testResults[p.id].result : JSON.stringify(testResults[p.id].result) }}
                </div>
                <div v-if="testResults[p.id].logs?.length" class="s-text-3 mt-1 space-y-0.5">
                  <div v-for="(line, i) in testResults[p.id].logs" :key="i" class="break-all font-mono">{{ line }}</div>
                </div>
              </div>
            </div>

            <div class="flex shrink-0 flex-col items-end gap-1.5">
              <NSwitch
                :value="p.enabled"
                size="small"
                :disabled="busy === `toggle:${p.id}`"
                @update:value="(v) => toggle(p, !!v)"
              />
              <div class="flex gap-1.5">
                <button
                  class="umi-btn umi-btn-xs"
                  :disabled="busy === `test:${p.id}`"
                  @click="runTest(p)"
                >
                  {{ busy === `test:${p.id}` ? $t('测试中…') : $t('测试') }}
                </button>
                <button
                  class="umi-btn umi-btn-xs hover:!border-rose-400/40"
                  :disabled="busy === `uninstall:${p.id}`"
                  @click="uninstall(p)"
                >
                  {{ $t('卸载') }}
                </button>
              </div>
            </div>
          </div>
        </div>

        <!-- 空态直接内联，不再嵌一张 umi-card -->
        <div v-if="!installed.length" class="s-text-3 flex items-center gap-2 py-6 text-[11.5px]">
          <NIcon :size="15" :component="ExtensionPuzzleOutline" />{{ $t('还没有安装任何插件') }}
        </div>
      </div>
    </div>

    <!-- 解析器与日志：试跑 + 事件日志合成一张卡 -->
    <div class="umi-card p-4" data-test="plugin-resolver">
      <div class="umi-card-title">
        <NIcon :size="14" :component="SparklesOutline" class="text-accent" />{{ $t('解析器试跑') }}
      </div>
      <div class="flex gap-2">
        <input
          v-model="resolverUrl"
          class="umi-input flex-1 !text-[11.5px]"
          :placeholder="$t('粘贴链接，用已启用插件跑一遍解析器')"
          spellcheck="false"
          @keyup.enter="runResolvers"
        />
        <button
          class="umi-btn-primary umi-btn-sm whitespace-nowrap"
          :disabled="resolverBusy || !resolverUrl.trim()"
          @click="runResolvers"
        >
          {{ resolverBusy ? $t('解析中…') : $t('运行解析器') }}
        </button>
      </div>
      <div v-if="resolverError" class="mt-2 break-all rounded-lg border border-rose-500/25 bg-rose-500/10 px-2.5 py-1.5 font-mono text-[10.5px] text-rose-500">
        {{ resolverError }}
      </div>
      <pre
        v-else-if="resolverResult"
        class="umi-inner s-text-2 mt-2 max-h-64 overflow-auto whitespace-pre-wrap break-all font-mono text-[10.5px]"
      >{{ resolverResult }}</pre>
      <div v-else class="umi-hint mt-2">
        {{ $t('解析器由已启用的插件提供，试跑只做解析、不会入队下载，结果原样展示。') }}
      </div>
    </div>
  </div>
</template>
