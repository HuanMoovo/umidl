<script setup lang="ts">
/**
 * 插件管理（1.6：从独立页面搬进「设置 → 插件」页签，逻辑与事件订阅保持不变）
 *
 *  - plugin_sandbox_info()  沙箱信息（引擎 / 内存上限 / 脚本超时）
 *  - plugin_list()          已安装插件（启用开关 / 测试 / 卸载）
 *  - plugin_market_list()   内容寻址市场（sha256 + 大小 + 安装，安装后比对摘要）
 *  - plugin_run_resolvers() 解析器试跑（可选）
 *  - plugin://event         事件日志（追加，最多保留 200 条）
 *
 * 旧入口 `#/plugins` 由路由重定向到 `#/settings?tab=plugins`，本组件不受影响。
 * 浏览器预览（非 Tauri）下会读不到任何数据，这里用 isTauri() 守卫优雅降级。
 */
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { NIcon, NProgress, NSwitch, useMessage } from 'naive-ui'
import {
  AlertCircleOutline,
  CheckmarkCircleOutline,
  CloudDownloadOutline,
  CubeOutline,
  ExtensionPuzzleOutline,
  InformationCircleOutline,
  ListOutline,
  RefreshOutline,
  SparklesOutline,
} from '@vicons/ionicons5'
import { isTauri } from '@/services/ipc'
import { useTaskStore } from '@/stores/tasks'
import { formatBytes } from '@/services/utils'
import {
  onPluginEvent,
  onPluginInstalled,
  pluginInstall,
  pluginList,
  pluginMarketList,
  pluginRunResolvers,
  pluginSandboxInfo,
  pluginSetEnabled,
  pluginTest,
  pluginUninstall,
  sameSha,
  shortSha,
  type PluginInfo,
  type PluginMarketEntry,
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
const market = ref<PluginMarketEntry[]>([])
const loading = ref(false)
const busy = ref('')
const testResults = ref<Record<string, PluginTestResult>>({})
/** 安装后的摘要校验结果（插件 id → 是否与市场一致） */
const verify = ref<Record<string, { ok: boolean; sha256: string }>>({})
const error = ref('')

/* ------------------------- 事件日志 ------------------------- */
interface LogLine {
  time: string
  level: string
  id: string
  event: string
  message: string
}
const MAX_LOGS = 200
const logs = ref<LogLine[]>([])

function pushLog(level: string, id: string, event: string, text: string) {
  const time = new Date().toLocaleTimeString()
  logs.value = [{ time, level, id, event, message: text }, ...logs.value].slice(0, MAX_LOGS)
}

function levelClass(level: string): string {
  const l = (level || '').toLowerCase()
  if (l === 'error') return 'text-rose-500'
  if (l === 'warn' || l === 'warning') return 'text-amber-600 dark:text-amber-400'
  if (l === 'debug') return 's-text-3'
  return 'text-emerald-600 dark:text-emerald-400'
}

/* ------------------------- 数据加载 ------------------------- */
async function refreshAll(silent = false) {
  if (!inTauri) {
    if (!silent) message.warning(tr('该功能需要在桌面客户端中运行'))
    return
  }
  loading.value = true
  error.value = ''
  try {
    const [s, list, m] = await Promise.all([pluginSandboxInfo(), pluginList(), pluginMarketList()])
    sandbox.value = s
    installed.value = Array.isArray(list) ? list : []
    market.value = Array.isArray(m) ? m : []
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

/* ------------------------- 安装 / 卸载 / 启用 / 测试 ------------------------- */
async function install(entry: PluginMarketEntry) {
  if (!inTauri) {
    message.warning(tr('该功能需要在桌面客户端中运行'))
    return
  }
  busy.value = `install:${entry.id}`
  try {
    await pluginInstall(entry.id)
    message.success(`${tr('插件已安装')}：${entry.name || entry.id}`)
    await reloadInstalled()
    // 安装后显示校验结果：用装完的实际 sha256 与市场条目比对
    const got = installed.value.find((p) => p.id === entry.id)
    verify.value = { ...verify.value, [entry.id]: { ok: sameSha(got?.sha256, entry.sha256), sha256: String(got?.sha256 ?? '') } }
  } catch (e: any) {
    message.error(String(e?.message ?? e))
  } finally {
    busy.value = ''
  }
}

async function uninstall(p: PluginInfo) {
  if (!inTauri) {
    message.warning(tr('该功能需要在桌面客户端中运行'))
    return
  }
  busy.value = `uninstall:${p.id}`
  try {
    await pluginUninstall(p.id)
    message.success(`${tr('插件已卸载')}：${p.name || p.id}`)
    const next = { ...verify.value }
    delete next[p.id]
    verify.value = next
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
  offEvent = await onPluginEvent((p) => {
    pushLog(p.level || 'info', p.id || '', p.event || '', p.message || '')
    if ((p.level || '').toLowerCase() === 'error') message.error(`${p.id ? `${p.id} · ` : ''}${p.message}`)
  })
  offInstalled = await onPluginInstalled(async () => {
    pushLog('info', '', 'installed', tr('插件列表已更新'))
    await reloadInstalled()
  })
})

onUnmounted(() => {
  offEvent?.()
  offInstalled?.()
})

const sandboxReady = computed(() => !!sandbox.value?.available)
const marketCount = computed(() => market.value.length)
</script>

<template>
  <div class="space-y-3" data-test="settings-plugins">
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
        {{ $t('插件跑在受限沙箱里，只提供解析器扩展；安装来源为内容寻址市场（按 sha256 校验）。沙箱限制单个插件的内存与单次脚本执行时间，超限即被中止，不会拖垮主程序。') }}
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

    <!-- 插件市场 -->
    <div class="umi-card p-4" data-test="plugin-market">
      <div class="umi-card-title">
        <NIcon :size="14" :component="CloudDownloadOutline" class="text-accent" />{{ $t('插件市场') }}
        <span class="s-surface-2 s-text-3 rounded-md px-1.5 py-0.5 text-[10px] tabular-nums">{{ marketCount }}</span>
      </div>

      <div class="space-y-2">
        <div
          v-for="m in market"
          :key="m.id"
          class="umi-inner"
        >
          <div class="flex items-start gap-2.5">
            <NIcon :size="16" :component="SparklesOutline" class="mt-[1px] shrink-0 text-accent" />
            <div class="min-w-0 flex-1">
              <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
                <span class="s-text text-[12.5px] font-medium">{{ m.name || m.id }}</span>
                <span v-if="m.version" class="umi-chip !text-[10px]">v{{ m.version }}</span>
                <span v-if="m.size" class="s-text-3 text-[10px]">{{ formatBytes(m.size) }}</span>
              </div>
              <div v-if="m.description" class="s-text-3 mt-0.5 text-[10.5px] leading-relaxed">{{ m.description }}</div>
              <div class="s-text-3 mt-1 break-all font-mono text-[10px]" :title="m.sha256">sha256 {{ m.sha256 || '--' }}</div>

              <div
                v-if="verify[m.id]"
                class="mt-2 inline-flex flex-wrap items-center gap-x-2 rounded-lg border px-2.5 py-1.5 text-[10.5px]"
                :class="verify[m.id].ok ? 'border-emerald-500/25 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400' : 'border-rose-500/25 bg-rose-500/10 text-rose-500'"
              >
                <NIcon :size="12" :component="verify[m.id].ok ? CheckmarkCircleOutline : AlertCircleOutline" />
                <span>{{ verify[m.id].ok ? $t('校验通过：与市场摘要一致') : $t('校验不一致：摘要与市场不一致') }}</span>
                <span class="break-all font-mono">{{ shortSha(verify[m.id].sha256) }}</span>
              </div>
            </div>

            <button
              class="umi-btn-primary umi-btn-sm shrink-0"
              :disabled="busy === `install:${m.id}`"
              @click="install(m)"
            >
              {{ busy === `install:${m.id}` ? $t('安装中…') : $t('安装') }}
            </button>
          </div>
        </div>

        <div v-if="!marketCount" class="s-text-3 flex items-center gap-2 py-6 text-[11.5px]">
          <NIcon :size="14" :component="InformationCircleOutline" />{{ $t('市场暂无可用插件') }}
        </div>
      </div>

      <div v-if="loading" class="mt-2.5 max-w-[220px]">
        <NProgress type="line" :percentage="100" :height="3" :show-indicator="false" color="rgb(var(--umi-a500))" />
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

      <!-- 事件日志 -->
      <div class="umi-head mt-3 border-t s-border-soft pt-3">
        <div class="umi-card-title !mb-0">
          <NIcon :size="14" :component="ListOutline" class="text-accent" />{{ $t('事件日志') }}
          <span class="s-surface-2 s-text-3 rounded-md px-1.5 py-0.5 text-[10px] tabular-nums">{{ logs.length }}</span>
        </div>
        <span class="umi-spacer" />
        <button class="umi-btn umi-btn-sm" :disabled="!logs.length" @click="logs = []">
          <span class="flex items-center gap-1">
            <NIcon :size="12" :component="RefreshOutline" />{{ $t('清空日志') }}</span>
        </button>
      </div>

      <div class="umi-inner max-h-64 space-y-0.5 overflow-auto font-mono text-[10.5px]">
        <div v-for="(l, i) in logs" :key="i" class="flex flex-wrap items-baseline gap-x-2">
          <span class="s-text-3">{{ l.time }}</span>
          <span :class="levelClass(l.level)">{{ l.level || 'info' }}</span>
          <span v-if="l.id" class="s-text-3">{{ l.id }}</span>
          <span v-if="l.event" class="s-text-3">{{ l.event }}</span>
          <span class="s-text-2 break-all">{{ l.message }}</span>
        </div>
        <div v-if="!logs.length" class="s-text-3 py-3 text-center">{{ $t('暂无事件（最多保留 200 条）') }}</div>
      </div>
    </div>
  </div>
</template>
