<script setup lang="ts">
/**
 * 可折叠区组（1.10）：设置页的「高级」区组与功能页的「高级选项」行共用。
 *
 *  - 默认折叠（open=false），展开状态记忆在组件内部（不必持久化）；
 *  - 「简单」模式（settings.ui_mode === 'simple'）下整体不渲染（不是折叠，是不显示）；
 *  - bare = 内容本身就是卡片时用（区组只出一行标题，不再套一层框）。
 */
import { ref } from 'vue'
import { NIcon } from 'naive-ui'
import { ChevronDownOutline } from '@vicons/ionicons5'
import { useSettingsStore } from '@/stores/settings'

const props = defineProps<{
  /** 区组标题（中文原文即 key 的 t() 结果由调用方传入） */
  title: string
  /** 标题左侧图标（可选） */
  icon?: unknown
  /** 初始是否展开（默认 false = 收起） */
  open?: boolean
  /** 轻量样式：内容本身是卡片时不套外框 */
  bare?: boolean
}>()

const settingsStore = useSettingsStore()
const expanded = ref(!!props.open)
</script>

<template>
  <div v-if="!settingsStore.isSimple" class="umi-group" :class="bare ? 'umi-group-bare' : ''">
    <button
      type="button"
      class="umi-group-head"
      :aria-expanded="expanded"
      data-test="group-toggle"
      @click="expanded = !expanded"
    >
      <NIcon v-if="icon" :size="13" :component="icon as any" class="text-accent shrink-0" />
      <span class="s-text-2 text-[12px] font-medium">{{ title }}</span>
      <slot name="extra" />
      <span class="umi-spacer" />
      <NIcon
        :size="14"
        :component="ChevronDownOutline"
        class="umi-group-chevron s-text-3 shrink-0"
        :class="expanded ? 'is-open' : ''"
      />
    </button>
    <div v-show="expanded" class="umi-group-body">
      <slot />
    </div>
  </div>
</template>
