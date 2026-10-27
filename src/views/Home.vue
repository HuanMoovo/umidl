<script setup lang="ts">
/**
 * 首页：常用操作与任务概览，两张卡提供与侧边栏一致的入口。
 */
import { useI18n } from 'vue-i18n'
import { computed, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useTaskStore } from '@/stores/tasks'
import { looksLikeUrl } from '@/services/utils'
import { supportedSites } from '@/services/downloader'
import { NIcon } from 'naive-ui'
import { ArrowForwardOutline, ChatbubblesOutline, DownloadOutline, SwapHorizontalOutline } from '@vicons/ionicons5'
const { t: tr } = useI18n()

const router = useRouter()
const store = useTaskStore()
const url = ref('')

const actions = computed(() => [
  { key: 'download', to: '/download', label: tr('下载视频'), hint: tr('粘贴链接，下载视频或音频'), icon: DownloadOutline },
  { key: 'converter', to: '/converter', label: tr('转换文件'), hint: tr('转换视频、音频、图片和文档'), icon: SwapHorizontalOutline },
  { key: 'subtitle', to: '/subtitle', label: tr('生成字幕'), hint: tr('从音视频生成字幕'), icon: ChatbubblesOutline },
])

/** 多个模块同时工作时，进入活跃任务最多的模块；同量时保持入口顺序。 */
const activeModule = computed(() => [
  { to: '/download', label: tr('下载任务'), count: store.activeDownloads.length },
  { to: '/converter', label: tr('转换任务'), count: store.activeConverts.length },
  { to: '/subtitle', label: tr('字幕任务'), count: store.activeSubtitles.length },
].sort((a, b) => b.count - a.count)[0])

/** 概览格：数字来自仓库，点击即进入对应模块 */
const stats = computed(() => [
  { key: 'download', to: '/download', label: tr('下载任务'), value: store.downloads.length, hue: 'text-accent' },
  { key: 'converter', to: '/converter', label: tr('转换任务'), value: store.converts.length, hue: 'text-cyan-600 dark:text-cyan-300' },
  { key: 'subtitle', to: '/subtitle', label: tr('字幕任务'), value: store.subtitles.length, hue: 'text-pink-600 dark:text-pink-300' },
  { key: 'active', to: activeModule.value.to, label: tr('进行中'), value: store.activeCount, hue: 'text-emerald-600 dark:text-emerald-300' },
])

function go() {
  if (!looksLikeUrl(url.value)) return
  router.push({ path: '/download', query: { url: url.value.trim() } })
}

onMounted(() => {
  if (!store.ready) void store.bootstrap()
})
</script>

<template>
  <div class="umi-page">
    <!-- Hero：链接入口 + 支持站点 -->
    <section class="umi-card relative overflow-hidden p-5">
      <div class="pointer-events-none absolute -right-16 -top-20 h-56 w-56 rounded-full bg-umi-500/15 blur-3xl" />
      <div class="relative">
        <div class="flex flex-wrap items-center gap-2.5">
          <h2 class="text-[22px] font-bold leading-none">
            <span class="text-gradient">Umidl</span>
          </h2>
          <span class="inline-flex items-center gap-1.5 rounded-full border s-border s-surface-2 px-2.5 py-0.5 text-[10.5px] s-text-3">
            <span class="h-1.5 w-1.5 animate-pulse rounded-full bg-emerald-400" />{{ $t('下载 · 转换 · 字幕，一站式搞定') }}
          </span>
          <span class="umi-hint hidden sm:inline">{{ $t('视频下载 / 格式转换 / AI 字幕桌面客户端。粘贴链接即可开始，全部处理在本机完成。') }}</span>
        </div>

        <div class="mt-4 grid gap-2 sm:grid-cols-3">
          <button
            v-for="action in actions"
            :key="action.key"
            class="umi-entry flex items-center gap-2.5 text-left"
            :data-test="`home-action-${action.key}`"
            @click="router.push(action.to)"
          >
            <NIcon :size="18" :component="action.icon" class="text-accent shrink-0" />
            <span class="min-w-0 flex-1">
              <span class="s-text block text-[12.5px] font-medium">{{ action.label }}</span>
              <span class="umi-hint block">{{ action.hint }}</span>
            </span>
            <NIcon :size="13" :component="ArrowForwardOutline" class="s-text-3 shrink-0" />
          </button>
        </div>

        <div class="mt-3.5 flex max-w-2xl gap-2">
          <input
            v-model="url"
            class="umi-input flex-1"
            :placeholder="$t('粘贴视频链接，例如 https://www.bilibili.com/video/BV...')"
            spellcheck="false"
            data-test="home-link"
            @keyup.enter="go"
          />
          <button class="umi-btn-primary whitespace-nowrap" :disabled="!looksLikeUrl(url)" data-test="home-go" @click="go">
            {{ $t('开始解析') }}
          </button>
        </div>

        <div class="mt-3 flex flex-wrap gap-1.5">
          <span v-for="s in supportedSites()" :key="s.name" class="umi-chip">
            <span class="mr-1">{{ s.icon }}</span>{{ s.name }}
          </span>
        </div>
      </div>
    </section>

    <!-- 概览：统计格（可点），一张卡收口 -->
    <section class="umi-card p-4">
      <div class="grid grid-cols-2 gap-2.5 sm:grid-cols-4">
        <button
          v-for="s in stats"
          :key="s.key"
          class="umi-entry"
          :data-test="`home-stat-${s.key}`"
          :disabled="s.key === 'active' && !s.value"
          @click="router.push(s.to)"
        >
          <div class="text-[10.5px] s-text-3">{{ s.label }}</div>
          <div class="mt-0.5 text-[20px] font-bold leading-none tabular-nums" :class="s.hue">{{ s.value }}</div>
          <div v-if="s.key === 'active' && s.value" class="s-text-3 mt-1 text-[10px]">{{ activeModule.label }}</div>
        </button>
      </div>
    </section>
  </div>
</template>
