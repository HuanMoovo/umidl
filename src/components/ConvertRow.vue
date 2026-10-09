<script setup lang="ts">
/**
 * ConvertRow.vue —— 任务队列的简约行（一行一个任务，行高 ≤ 40px）
 *
 * 用户要求：转换队列不要大卡片，只保留文件本身的信息 ——
 *   状态指示 / 文件名 / 元信息（源 → 目标）/ 进度 / 操作按钮。
 * 这里没有任何缩略图、预览按钮或预览弹窗入口，也没有卡片包裹：就是纯列表的一行。
 * 纯展示组件：动作全部 emit 给父级（Converter.vue / Subtitle.vue），由父级做真实打开 / 定位 / 删除。
 *
 * 字幕队列（Subtitle.vue）复用本行：`rowRole="subtitle-row"`，
 * name = 字幕文件名、srcText = 「语言 · 模型」、dstText 不传（元信息只有一段，无箭头）。
 */
import { useI18n } from 'vue-i18n'
import { computed } from 'vue'
import { NIcon } from 'naive-ui'
import {
  AlertCircleOutline,
  CheckmarkCircleOutline,
  CloseCircleOutline,
  FolderOpenOutline,
  PauseOutline,
  PlayCircleOutline,
  PlayOutline,
  RefreshOutline,
  TrashOutline,
} from '@vicons/ionicons5'
import { isActive } from '@/services/utils'
import type { TaskStatus } from '@/types'
const { t: tr } = useI18n()

const props = defineProps<{
  /**
   * 行的 data-role：默认 `convert-row`（转换页），字幕页传 `subtitle-row`。
   * 字幕队列复用本组件时只换这个 + name/srcText（元信息只有一段，dstText 留空即无箭头），
   * 转换页的调用与外观完全不变。
   */
  rowRole?: string
  /** 展示名：产物文件名，没有产物时退回源文件名（字幕页传字幕文件名 / 源视频名） */
  name: string
  status: TaskStatus
  progress: number
  /** 元信息第一段：转换页是 `源文件名 源大小`，字幕页是 `语言 · 模型` */
  srcText?: string
  /** 元信息第二段：`目标格式 产物大小`；留空则不渲染（中间的箭头也一并消失） */
  dstText?: string
  error?: string | null
  /** 产物路径（有且没丢才给「打开 / 在文件夹中显示」） */
  filePath?: string | null
  /** 产物是否还在磁盘上（false = 已丢失，只留删除） */
  fileExists?: boolean
}>()

const emit = defineEmits<{
  (e: 'open'): void
  (e: 'reveal'): void
  (e: 'remove'): void
  (e: 'cancel'): void
  (e: 'resume'): void
}>()

/** 状态 → 文案 / 颜色 / 图标（与 TaskCard.vue / TaskTable.vue 的映射保持一致） */
const STATUS_MAP: Record<TaskStatus, { label: string; color: string; icon: any }> = {
  pending: { label: tr('排队中'), color: 's-text-3', icon: RefreshOutline },
  parsing: { label: tr('解析中'), color: 'text-cyan-600 dark:text-cyan-400', icon: RefreshOutline },
  downloading: { label: tr('下载中'), color: 'text-accent', icon: PlayOutline },
  paused: { label: tr('已暂停'), color: 'text-amber-600 dark:text-amber-400', icon: PauseOutline },
  converting: { label: tr('转换中'), color: 'text-accent', icon: RefreshOutline },
  extracting: { label: tr('提取音频'), color: 'text-cyan-600 dark:text-cyan-400', icon: RefreshOutline },
  transcribing: { label: tr('AI 识别中'), color: 'text-pink-600 dark:text-pink-400', icon: RefreshOutline },
  done: { label: tr('已完成'), color: 'text-emerald-600 dark:text-emerald-400', icon: CheckmarkCircleOutline },
  error: { label: tr('失败'), color: 'text-rose-600 dark:text-rose-400', icon: AlertCircleOutline },
  canceled: { label: tr('已取消'), color: 's-text-3', icon: CloseCircleOutline },
}

const meta = computed(() => STATUS_MAP[props.status] ?? STATUS_MAP.pending)
const active = computed(() => isActive({ status: props.status } as any))
const pct = computed(() => Math.max(0, Math.min(100, props.progress || 0)))
const failed = computed(() => props.status === 'error')
/** 产物完成但已不在磁盘上：只留「删除记录」，不摆点不动的打开按钮 */
const lost = computed(() => props.status === 'done' && !!props.filePath && props.fileExists === false)
const openable = computed(() => !!props.filePath && props.fileExists !== false)
</script>

<template>
  <div
    :data-role="rowRole || 'convert-row'"
    :data-status="status"
    :title="filePath || name"
    class="s-border-soft group flex items-center gap-2.5 border-b px-2 py-1 transition-colors last:border-b-0 hover:bg-umi-400/[0.06]"
  >
    <!-- 状态指示：颜色 + 图标 + 短标签（进行中带呼吸点） -->
    <span class="flex w-[74px] shrink-0 items-center gap-1 text-[11px]" :class="meta.color">
      <span v-if="active" class="h-1.5 w-1.5 shrink-0 animate-pulse rounded-full bg-current" />
      <NIcon :size="12" :component="meta.icon" class="shrink-0" />
      <span class="truncate">{{ meta.label }}</span>
    </span>

    <!-- 文件名（唯一的主文案） -->
    <span class="s-text min-w-0 flex-1 truncate text-[12.5px] font-medium">{{ name }}</span>

    <!-- 元信息：转换页是 `源大小 → 目标格式 产物大小`；字幕页只有一段（语言 · 模型 / 语言 · 模型 · 产物大小） -->
    <span
      v-if="srcText || dstText"
      class="s-text-3 hidden shrink-0 items-center gap-1.5 text-[11px] tabular-nums sm:flex"
    >
      <span v-if="srcText" class="max-w-[200px] truncate" :title="srcText">{{ srcText }}</span>
      <span v-if="srcText && dstText">→</span>
      <span v-if="dstText" class="max-w-[150px] truncate" :title="dstText">{{ dstText }}</span>
    </span>

    <!-- 「文件已丢失」固定宽度槽位：有无都占位，右侧的进度 / 百分比 / 按钮列不会错位 -->
    <span class="flex w-[58px] shrink-0 justify-end">
      <span
        v-if="lost"
        class="rounded-full bg-amber-500/15 px-1.5 py-px text-[10px] text-amber-600 dark:text-amber-400"
        :title="$t('原文件已被移动或删除，可重新下载或删除该记录')"
      >{{ $t('文件已丢失') }}</span>
    </span>

    <!-- 进度：细进度条 + 百分比；失败就地把错误摊在这一格里 -->
    <span v-if="failed" class="s-text-3 w-[168px] shrink-0 truncate text-[10.5px] text-rose-500" :title="error || $t('未知错误')">
      {{ error || $t('未知错误') }}
    </span>
    <template v-else>
      <span class="h-[3px] w-[112px] shrink-0 overflow-hidden rounded-full bg-[var(--umi-panel-3)]">
        <span
          class="block h-full rounded-full"
          :style="{
            width: pct + '%',
            background:
              'linear-gradient(90deg, rgb(var(--umi-a500)), color-mix(in srgb, rgb(var(--umi-a500)) 55%, #22d3ee))',
          }"
        />
      </span>
      <span class="s-text-3 w-[40px] shrink-0 text-right text-[10.5px] tabular-nums">{{ pct.toFixed(1) }}%</span>
    </template>

    <!-- 操作：进行中可取消 / 失败可重来，其余只留 打开 · 在文件夹中显示 · 删除记录 -->
    <span class="flex shrink-0 items-center gap-1 opacity-70 transition-opacity group-hover:opacity-100">
      <button v-if="active" class="umi-btn umi-btn-xs" :title="$t('取消')" @click="emit('cancel')">
        <NIcon :size="12" :component="CloseCircleOutline" />
      </button>
      <button v-if="status === 'error' || status === 'paused'" class="umi-btn umi-btn-xs" :title="$t('继续')" @click="emit('resume')">
        <NIcon :size="12" :component="PlayOutline" />
      </button>
      <button v-if="openable" class="umi-btn umi-btn-xs" :title="$t('打开文件')" @click="emit('open')">
        <NIcon :size="12" :component="PlayCircleOutline" />
      </button>
      <button v-if="openable" class="umi-btn umi-btn-xs" :title="$t('在文件夹中显示')" @click="emit('reveal')">
        <NIcon :size="12" :component="FolderOpenOutline" />
      </button>
      <button
        class="umi-btn umi-btn-xs hover:!border-rose-400/40"
        :title="$t('删除记录')"
        @click="emit('remove')"
      >
        <NIcon :size="12" :component="TrashOutline" />
      </button>
    </span>
  </div>
</template>
