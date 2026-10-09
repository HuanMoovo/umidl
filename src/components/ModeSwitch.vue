<script setup lang="ts">
/**
 * 简单 / 高级 模式切换（1.10）
 *
 * 可复用组件：设置页顶部与功能页共用同一状态源 —— settings store 的 ui_mode。
 *  - 简单：任务 A 里的默认折叠区组整体隐藏，只留常用项；
 *  - 高级：全部可见（区组仍可手动展开 / 折叠）。
 * 保存失败（浏览器预览 / 无桌面后端）时静默，内存里仍会立即生效。
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useSettingsStore } from '@/stores/settings'
import { normalizeUiMode } from '@/utils/accent'
import type { UiMode } from '@/types'

const { t: tr } = useI18n()
const settingsStore = useSettingsStore()

const mode = computed<UiMode>(() => normalizeUiMode(settingsStore.settings.ui_mode))

async function pick(m: UiMode) {
  if (m === mode.value) return
  settingsStore.patch({ ui_mode: m })
  try {
    await settingsStore.save()
  } catch {
    /* 浏览器预览 / 无后端：忽略，内存里已生效 */
  }
}
</script>

<template>
  <div class="umi-seg shrink-0" role="group" :aria-label="tr('界面模式')" data-test="mode-switch">
    <button
      class="rounded-lg px-2.5 py-1 text-[11px] font-medium transition-all"
      :class="mode === 'simple' ? 'chip-active' : 's-text-3'"
      :aria-pressed="mode === 'simple'"
      data-test="mode-simple"
      @click="pick('simple')"
    >
      {{ tr('简单') }}
    </button>
    <button
      class="rounded-lg px-2.5 py-1 text-[11px] font-medium transition-all"
      :class="mode === 'advanced' ? 'chip-active' : 's-text-3'"
      :aria-pressed="mode === 'advanced'"
      data-test="mode-advanced"
      @click="pick('advanced')"
    >
      {{ tr('高级') }}
    </button>
  </div>
</template>
