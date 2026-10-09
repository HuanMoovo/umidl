<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import { computed, h } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import { NIcon, NTooltip } from 'naive-ui'
import {
  HomeOutline,
  CloudDownloadOutline,
  SwapHorizontalOutline,
  ChatbubblesOutline,
  SettingsOutline,
} from '@vicons/ionicons5'
import { useTaskStore } from '@/stores/tasks'
import { useBranding } from '@/services/branding'
const { t: tr } = useI18n()
const branding = useBranding()

const route = useRoute()
const store = useTaskStore()

const items = computed(() => [
  { to: '/', label: tr('首页'), icon: HomeOutline, key: 'home' },
  { to: '/download', label: tr('下载'), icon: CloudDownloadOutline, key: 'download' },
  { to: '/converter', label: tr('转换'), icon: SwapHorizontalOutline, key: 'converter' },
  { to: '/subtitle', label: tr('字幕'), icon: ChatbubblesOutline, key: 'subtitle' },
  // 插件（1.6）已并入「设置 → 插件」页签，侧边栏不再单独入口
  { to: '/settings', label: tr('设置'), icon: SettingsOutline, key: 'settings' },
])

const badges = computed<Record<string, number>>(() => ({
  download: store.activeDownloads.length,
  converter: store.activeConverts.length,
  subtitle: store.activeSubtitles.length,
}))
</script>

<template>
  <aside class="w-[188px] shrink-0 border-r s-border-soft s-surface-2 backdrop-blur-xl">
    <div class="flex h-full flex-col">
      <div class="flex items-center gap-2.5 px-5 pb-5 pt-6 select-none">
        <div class="relative">
          <div class="absolute inset-0 rounded-xl bg-umi-500/60 blur-md" />
          <img
            :src="branding.markUrl.value"
            alt="Umidl"
            class="relative h-9 w-9 rounded-xl object-contain p-0.5 s-surface-3 select-none"
            draggable="false"
          />
        </div>
        <div class="leading-tight">
          <div class="text-[13px] font-bold tracking-wide s-text">Umidl</div>
          <div class="text-[10px] tracking-[0.18em] s-text-3">DOWNLOADER</div>
        </div>
      </div>

      <nav class="flex-1 space-y-1 px-2.5">
        <RouterLink
          v-for="it in items"
          :key="it.key"
          :to="it.to"
          class="group relative flex items-center gap-3 rounded-xl px-3 py-2.5 text-sm transition-all duration-200"
          :class="
            route.path === it.to
              ? 'umi-nav-active bg-gradient-to-r from-umi-600/35 to-transparent s-text shadow-[inset_0_0_0_1px_rgba(124,77,255,0.35)]'
              : 's-text-3 hover:s-surface-2 hover:s-text'
          "
        >
          <span
            v-if="route.path === it.to"
            class="umi-nav-bar absolute left-0 top-1/2 h-5 w-[3px] -translate-y-1/2 rounded-r bg-gradient-to-b from-umi-400 to-accent-cyan shadow-glow"
          />
          <NIcon :size="18" :component="it.icon" :class="route.path === it.to ? 'text-accent' : ''" />
          <span class="font-medium">{{ it.label }}</span>
          <span
            v-if="badges[it.key]"
            class="ml-auto rounded-full bg-umi-500 px-1.5 py-px text-[10px] font-bold leading-tight text-white"
          >
            {{ badges[it.key] }}
          </span>
        </RouterLink>
      </nav>
    </div>
  </aside>
</template>
