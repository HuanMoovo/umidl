<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { NConfigProvider, NMessageProvider, NDialogProvider, darkTheme, zhCN, dateZhCN, enUS, dateEnUS, jaJP, dateJaJP, frFR, dateFrFR } from 'naive-ui'
import { useI18n } from 'vue-i18n'
import { normalizeLocale, setLocale } from '@/i18n'
import { setLogoPath } from '@/services/branding'
import SideBar from '@/components/SideBar.vue'
import CaptureWatcher from '@/components/CaptureWatcher.vue'
import ParticleBg from '@/components/ParticleBg.vue'
import TitleBar from '@/components/TitleBar.vue'
import { useTaskStore } from '@/stores/tasks'
import { useSettingsStore } from '@/stores/settings'
import { applyCustomAccent, applyTheme, clearCustomAccent, naiveOverrides, startSystemThemeWatch } from '@/services/theme'
import { syncWindowTheme, watchWindowTheme } from '@/services/windowTheme'
import type { GlobalThemeOverrides } from 'naive-ui'
import { invoke } from '@tauri-apps/api/core'

/** 启动性能埋点：耗时写入后端 stderr（[umi][fe] 前缀） */
const flog = (m: string) => {
  invoke('frontend_log', { msg: `+${performance.now().toFixed(0)}ms ${m}` }).catch(() => {})
}

const taskStore = useTaskStore()
const settingsStore = useSettingsStore()
const booted = ref(false)
const resolved = ref<'dark' | 'light'>('dark')
let stopSystemWatch: (() => void) | null = null
let stopWindowWatch: (() => void) | null = null

const themeOverrides = computed<GlobalThemeOverrides>(
  () =>
    naiveOverrides(
      // 自定义强调色优先，没设时回退到预设名（violet / cyan …）
      settingsStore.settings.accent_custom || settingsStore.settings.accent,
      resolved.value === 'dark',
    ) as GlobalThemeOverrides,
)

/* ------------------------- 界面语言 ------------------------- */
const { locale } = useI18n()
const NAIVE_LOCALE = {
  zh: [zhCN, dateZhCN],
  en: [enUS, dateEnUS],
  ja: [jaJP, dateJaJP],
  fr: [frFR, dateFrFR],
} as const
const naiveLocale = computed(() => NAIVE_LOCALE[normalizeLocale(locale.value)][0])
const naiveDateLocale = computed(() => NAIVE_LOCALE[normalizeLocale(locale.value)][1])

function sync(mode?: string, accent?: string) {
  const m = mode ?? settingsStore.settings.theme
  resolved.value = applyTheme(m, accent ?? settingsStore.settings.accent)
  // 窗口主题跟着走：system 时交还系统，昼夜跟随才能真正生效
  void syncWindowTheme(m || 'system')
}

/**
 * 自定义强调色：把 settings.accent_custom 写回 <html> 内联 CSS 变量
 * （主题切换时 accent-text 要取不同的档位，所以这里要重算一次）；清空时撤掉覆盖。
 */
function syncCustomAccent() {
  const hex = settingsStore.settings.accent_custom
  if (!applyCustomAccent(hex, resolved.value === 'dark')) clearCustomAccent()
}

// 系统昼夜变化时，若为“跟随系统”则实时切换
stopSystemWatch = null

onMounted(async () => {
  const nav = performance.getEntriesByType('navigation')[0] as PerformanceNavigationTiming | undefined
  if (nav) flog(`WebView 首字节 ${nav.responseStart.toFixed(0)}ms · DOM 就绪 ${nav.domContentLoadedEventEnd.toFixed(0)}ms`)
  flog('Vue 挂载完成，开始加载设置')
  applyTheme(settingsStore.settings.theme, settingsStore.settings.accent)
  const onSys = () => {
    if ((settingsStore.settings.theme || 'system') === 'system') sync()
  }
  // 跟随系统：媒体查询事件 + 轮询 + 回窗口复核（三条路兜底，见 startSystemThemeWatch 注释）
  stopSystemWatch = startSystemThemeWatch(onSys)
  // 双保险：Tauri 窗口主题变化事件（WebView2 的媒体查询偶有不触发的情况）
  stopWindowWatch = await watchWindowTheme(onSys)
  await settingsStore.load()
  flog('设置加载完成')
  setLocale(settingsStore.settings.ui_language)
  setLogoPath(settingsStore.settings.custom_logo)
  sync()
  // 上次保存的自定义强调色随启动立即生效，无需再进「设置 → 外观」
  syncCustomAccent()
  flog(settingsStore.settings.accent_custom ? '自定义强调色已恢复' : '主题已应用')
  await taskStore.bootstrap()
  flog('任务数据 + 工具检测完成，隐藏加载遮罩')
  booted.value = true
})

onUnmounted(() => {
  stopSystemWatch?.()
  stopWindowWatch?.()
})

watch(() => [settingsStore.settings.theme, settingsStore.settings.accent], () => sync(), { deep: true })

// 自定义强调色变化：立即写回 <html> 上的内联 CSS 变量（清空 → 撤掉覆盖，回到预设色）
watch(() => settingsStore.settings.accent_custom, () => syncCustomAccent())

// 明暗变化（含「跟随系统」的昼夜切换）：--umi-accent-text 要在亮档(a300)/深档(a700) 之间重算
watch(resolved, () => syncCustomAccent())

// 语言变化：切换 vue-i18n（模板里的 $t 自动重渲染）
watch(
  () => settingsStore.settings.ui_language,
  (v) => {
    setLocale(v)
  },
)

// 自定义 LOGO 变化：同步给界面各处（侧栏 / 首页 / 标题栏）
watch(
  () => settingsStore.settings.custom_logo,
  (v) => setLogoPath(v),
)
</script>

<template>
  <NConfigProvider
    :theme="resolved === 'dark' ? darkTheme : null"
    :theme-overrides="themeOverrides"
    :locale="naiveLocale"
    :date-locale="naiveDateLocale"
  >
    <NMessageProvider :max="4">
      <NDialogProvider>
        <CaptureWatcher />
        <div class="relative h-screen w-screen overflow-hidden">
          <ParticleBg
            :enabled="settingsStore.settings.animation !== false"
            :quality="settingsStore.settings.animation_quality || 'medium'"
            :shape="settingsStore.settings?.particle_shape ?? 'circle'"
          />
          <div class="relative z-10 flex h-full">
            <SideBar />
            <div class="flex min-w-0 flex-1 flex-col">
              <TitleBar />
              <main class="flex-1 overflow-y-auto px-6 pb-8 pt-4">
                <RouterView v-slot="{ Component }">
                  <Transition name="fade" mode="out-in">
                    <component :is="Component" :key="$route.path" />
                  </Transition>
                </RouterView>
              </main>
            </div>
          </div>
          <div
            v-if="!booted"
            class="absolute inset-0 z-20 flex items-center justify-center backdrop-blur-sm"
            style="background: var(--umi-sunken)"
          >
            <div class="s-text-2 flex items-center gap-3">
              <span class="h-3 w-3 animate-ping rounded-full bg-umi-400" />{{ $t('正在加载 Umidl…') }}</div>
          </div>
        </div>
      </NDialogProvider>
    </NMessageProvider>
  </NConfigProvider>
</template>
