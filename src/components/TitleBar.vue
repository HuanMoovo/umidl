<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import { computed } from 'vue'
import { useRoute } from 'vue-router'
import { NIcon, NTooltip } from 'naive-ui'
import { ContrastOutline, MoonOutline, SunnyOutline } from '@vicons/ionicons5'
import { useTaskStore } from '@/stores/tasks'
import { useSettingsStore } from '@/stores/settings'
import { applyTheme, themeModes } from '@/services/theme'
import type { ThemeMode } from '@/services/theme'
import { syncWindowTheme } from '@/services/windowTheme'
const { t: tr } = useI18n()

const route = useRoute()
const store = useTaskStore()
const settingsStore = useSettingsStore()

const title = computed(() => (route.meta?.title as string) || 'Umidl')
const current = computed(() => settingsStore.settings.theme || 'system')
const running = computed(() => store.activeCount)

/** 昼夜切换：浅色 / 深色 / 跟随系统（图标 + 文字提示） */
const modes = computed(() =>
  themeModes().map((m) => ({
    ...m,
    icon: m.value === 'light' ? SunnyOutline : m.value === 'dark' ? MoonOutline : ContrastOutline,
  })),
)

async function setTheme(mode: ThemeMode) {
  settingsStore.patch({ theme: mode })
  applyTheme(mode, settingsStore.settings.accent)
  void syncWindowTheme(mode)
  await settingsStore.save()
}
</script>

<template>
  <header class="flex h-14 shrink-0 items-center justify-between border-b px-6" style="border-color: var(--umi-border-soft)">
    <div class="flex items-baseline gap-3">
      <h1 class="s-text text-[15px] font-semibold tracking-wide">{{ $t(title) }}</h1>
      <span v-if="running > 0" class="flex items-center gap-1.5 text-[11px] text-accent">
        <span class="h-1.5 w-1.5 animate-ping rounded-full bg-umi-400" />
        {{ $t('{n} 个任务进行中', { n: running }) }}
      </span>
    </div>

    <div class="flex items-center gap-2">
      <div class="umi-chip !text-[10.5px] tabular-nums">v1.8.10</div>

      <!-- 昼夜切换：图标 + 提示 -->
      <div class="umi-seg" role="group" :aria-label="$t('昼夜模式切换')">
        <NTooltip v-for="m in modes" :key="m.value" trigger="hover" placement="bottom">
          <template #trigger>
            <button
              class="flex h-7 w-7 items-center justify-center rounded-lg transition-all duration-200"
              :class="current === m.value ? 'text-white shadow-glow' : 's-text-3 hover:opacity-100'"
              :style="current === m.value ? 'background-image: linear-gradient(135deg, rgb(var(--umi-a600)), rgb(var(--umi-a400)))' : ''"
              :aria-pressed="current === m.value"
              :aria-label="m.hint"
              @click="setTheme(m.value)"
            >
              <NIcon :size="14" :component="m.icon" />
            </button>
          </template>
          {{ m.label }} · {{ m.hint }}
        </NTooltip>
      </div>
    </div>
  </header>
</template>
