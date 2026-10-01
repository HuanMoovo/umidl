<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import { computed, ref, watch } from 'vue'
import { NIcon, NProgress } from 'naive-ui'
import { convertFileSrc } from '@tauri-apps/api/core'
import {
  PlayOutline,
  PauseOutline,
  PlayCircleOutline,
  TrashOutline,
  FolderOpenOutline,
  RefreshOutline,
  CloseCircleOutline,
  CheckmarkCircleOutline,
  AlertCircleOutline,
  EyeOutline,
} from '@vicons/ionicons5'
import { formatBytes, formatDuration, isActive, basename } from '@/services/utils'
import type { TaskStatus } from '@/types'
const { t: tr } = useI18n()

const props = defineProps<{
  title: string
  subtitle?: string
  status: TaskStatus
  progress: number
  speed?: string | null
  eta?: string | null
  downloaded?: number | null
  total?: number | null
  error?: string | null
  thumbnail?: string | null
  filePath?: string | null
  duration?: number | null
  canPause?: boolean
  canCancel?: boolean
  /** 文件是否还在磁盘上（false 时隐藏"打开/显示"按钮并提示已丢失） */
  fileExists?: boolean
  /**
   * 是否提供「预览」入口（缩略图点击 + 预览按钮）。
   * 格式转换模块按用户要求已移除预览功能，传 false 即整卡不再有预览入口。
   */
  canPreview?: boolean
}>()

const emit = defineEmits<{
  (e: 'pause'): void
  (e: 'resume'): void
  (e: 'cancel'): void
  (e: 'remove'): void
  (e: 'open'): void
  (e: 'reveal'): void
  (e: 'preview'): void
}>()

/** 缩略图：本地缓存文件走 asset 协议，远程地址直接用 */
const thumbSrc = computed(() => {
  const t = props.thumbnail
  if (!t) return ''
  if (/^[a-zA-Z]:[\\/]/.test(t) || t.startsWith('file:')) {
    try {
      return convertFileSrc(t)
    } catch {
      return ''
    }
  }
  return t
})

/** 远程图被热链保护挡住 / 文件丢失时，回退显示占位图标而不是破图 */
const imgFailed = ref(false)
watch(
  () => props.thumbnail,
  () => {
    imgFailed.value = false
  },
)

const STATUS_MAP: Record<TaskStatus, { label: string; color: string; icon: any }> = {
  pending: { label: tr('排队中'), color: 's-text-3', icon: RefreshOutline },
  parsing: { label: tr('解析中'), color: 'text-cyan-600 dark:text-cyan-400', icon: RefreshOutline },
  downloading: { label: tr('下载中'), color: 'text-accent', icon: PlayOutline },
  paused: { label: tr('已暂停'), color: 'text-amber-600 dark:text-amber-400', icon: PauseOutline },
  converting: { label: tr('转换中'), color: 'text-accent', icon: RefreshOutline },
  extracting: { label: tr('提取音频'), color: 'text-cyan-600 dark:text-cyan-400', icon: RefreshOutline },
  transcribing: { label: tr('AI 识别中'), color: 'text-pink-600 dark:text-pink-400', icon: RefreshOutline },
  done: { label: tr('已完成'), color: 'text-emerald-600 dark:text-emerald-400', icon: CheckmarkCircleOutline },
  // ED2K：交给 eMule 引擎接管 —— 不再显示「完成 / 100%」（进度在引擎窗口看）
  handed_off: { label: tr('已交给引擎'), color: 'text-sky-600 dark:text-sky-400', icon: CheckmarkCircleOutline },
  error: { label: tr('失败'), color: 'text-rose-600 dark:text-rose-400', icon: AlertCircleOutline },
  canceled: { label: tr('已取消'), color: 's-text-3', icon: CloseCircleOutline },
}

const meta = computed(() => STATUS_MAP[props.status] ?? STATUS_MAP.pending)
const active = computed(() => isActive({ status: props.status } as any))
const pct = computed(() => Math.max(0, Math.min(100, props.progress || 0)))
/** 预览入口是否可用（格式转换模块传 canPreview=false → 缩略图与预览按钮都不再可点） */
const previewable = computed(() => props.canPreview !== false)
</script>

<template>
  <div class="umi-card group relative overflow-hidden p-3 transition-all hover:border-umi-400/30">
    <div
      v-if="active"
      class="pointer-events-none absolute inset-x-0 top-0 h-[2px] bg-gradient-to-r from-transparent via-umi-400 to-transparent opacity-70"
    />
    <div class="flex gap-3.5">
      <!-- 缩略图 -->
      <div
        class="s-sunken s-border-soft group relative h-[62px] w-[110px] shrink-0 overflow-hidden rounded-lg border"
        :class="previewable ? 'cursor-pointer' : ''"
        :title="previewable ? (filePath ? $t('点击预览') : $t('点击查看封面')) : ''"
        @click="previewable && emit('preview')"
      >
        <img
          v-if="thumbSrc && !imgFailed"
          :src="thumbSrc"
          class="h-full w-full object-cover"
          alt=""
          loading="lazy"
          @error="imgFailed = true"
        />
        <div v-else class="flex h-full w-full items-center justify-center text-lg s-text-3">
          <NIcon :component="meta.icon" :size="20" />
        </div>
        <div
          v-if="previewable"
          class="absolute inset-0 flex items-center justify-center bg-black/45 opacity-0 transition-opacity group-hover:opacity-100"
        >
          <NIcon class="text-white" :size="16" :component="EyeOutline" />
        </div>
        <span
          v-if="duration"
          class="absolute bottom-1 right-1 rounded-md bg-black/75 px-1.5 py-px text-[10px] leading-tight tabular-nums text-white"
        >
          {{ formatDuration(duration) }}
        </span>
      </div>

      <!-- 主体 -->
      <div class="min-w-0 flex-1">
        <div class="flex items-start justify-between gap-3">
          <div class="min-w-0">
            <div class="truncate text-[13px] font-medium s-text" :title="title">{{ title }}</div>
            <div class="mt-0.5 flex items-center gap-2 text-[11px]">
              <span class="flex items-center gap-1" :class="meta.color">
                <span v-if="active" class="h-1.5 w-1.5 animate-pulse rounded-full bg-current" />
                {{ meta.label }}
              </span>
              <span v-if="subtitle" class="truncate s-text-3">{{ subtitle }}</span>
              <span
                v-if="filePath && !fileExists && status === 'done'"
                class="shrink-0 rounded-full bg-amber-500/15 px-1.5 py-px text-[10px] text-amber-600 dark:text-amber-400"
                :title="$t('原文件已被移动或删除，可重新下载或删除该记录')"
              >{{ $t('文件已丢失') }}</span>
            </div>
          </div>

          <div class="flex shrink-0 items-center gap-1 opacity-70 transition-opacity group-hover:opacity-100">
            <button
              v-if="canPause !== false && ['downloading', 'parsing'].includes(status)"
              class="umi-btn umi-btn-xs"
              :title="$t('暂停')"
              @click="emit('pause')"
            >
              <NIcon :size="13" :component="PauseOutline" />
            </button>
            <button
              v-if="status === 'paused' || status === 'error'"
              class="umi-btn umi-btn-xs"
              :title="$t('继续')"
              @click="emit('resume')"
            >
              <NIcon :size="13" :component="PlayOutline" />
            </button>
            <button
              v-if="canCancel !== false && active"
              class="umi-btn umi-btn-xs"
              :title="$t('取消')"
              @click="emit('cancel')"
            >
              <NIcon :size="13" :component="CloseCircleOutline" />
            </button>
            <button
              v-if="filePath && fileExists"
              class="umi-btn umi-btn-xs"
              :title="$t('打开文件')"
              @click="emit('open')"
            >
              <NIcon :size="13" :component="PlayCircleOutline" />
            </button>
            <button
              v-if="filePath && fileExists"
              class="umi-btn umi-btn-xs"
              :title="$t('在文件夹中显示')"
              @click="emit('reveal')"
            >
              <NIcon :size="13" :component="FolderOpenOutline" />
            </button>
            <button
              v-if="filePath && previewable"
              class="umi-btn umi-btn-xs"
              :title="$t('预览')"
              @click="emit('preview')"
            >
              {{ $t('预览') }}
            </button>
            <button
              class="umi-btn umi-btn-xs hover:!border-rose-400/40"
              :title="$t('删除记录')"
              @click="emit('remove')"
            >
              <NIcon :size="13" :component="TrashOutline" />
            </button>
          </div>
        </div>

        <!-- 进度（已交给外部引擎的任务没有本机进度，不显示进度条以免误读成 0%） -->
        <div v-if="status !== 'error' && status !== 'handed_off'" class="mt-2.5">
          <NProgress
            type="line"
            :percentage="pct"
            :show-indicator="false"
            :height="5"
            :border-radius="6"
            color="linear-gradient(90deg, rgb(var(--umi-a500)), color-mix(in srgb, rgb(var(--umi-a500)) 55%, #22d3ee))"
            rail-color="var(--umi-panel-3)"
          />
          <div class="mt-1.5 flex items-center justify-between text-[11px] tabular-nums s-text-3">
            <span>{{ pct.toFixed(1) }}%</span>
            <span class="flex gap-3">
              <span v-if="speed">↓ {{ speed }}</span>
              <span v-if="eta">{{ $t('剩余 {t}', { t: eta }) }}</span>
              <span v-if="total">{{ formatBytes(downloaded) }} / {{ formatBytes(total) }}</span>
            </span>
          </div>
        </div>

        <!-- 错误 -->
        <div v-else class="mt-2 rounded-lg border border-rose-500/25 bg-rose-500/10 px-2.5 py-1.5 text-[11px] text-rose-500">
          {{ error || $t('未知错误') }}
        </div>
      </div>
    </div>
  </div>
</template>
