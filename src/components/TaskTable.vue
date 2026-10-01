<script setup lang="ts">
/**
 * TaskTable.vue —— 下载队列「详细列表」布局
 *
 * 一行一个任务，列：封面缩略图 + 标题 / 状态 / 进度 / 大小 / 速度 · 剩余 / 操作。
 * 与 TaskCard.vue 保持一致的能力：状态色、进度条样式、文件缺失提示、
 * 按钮显隐（开始/暂停/取消/打开文件/在文件夹中显示/预览/删除记录）。
 * 纯展示组件：所有动作 emit 给父级（Download.vue），删除确认对话框仍由父级负责。
 */
import { useI18n } from 'vue-i18n'
import { ref, watch } from 'vue'
import { NIcon, NProgress, NSpin } from 'naive-ui'
import { convertFileSrc } from '@tauri-apps/api/core'
import {
  AlertCircleOutline,
  CheckmarkCircleOutline,
  CloseCircleOutline,
  CloudDownloadOutline,
  EyeOutline,
  FolderOpenOutline,
  PauseOutline,
  PlayCircleOutline,
  PlayOutline,
  RefreshOutline,
  TrashOutline,
} from '@vicons/ionicons5'
import { formatBytes, formatDuration, isActive, translateFormatNote } from '@/services/utils'
import type { DownloadTask, TaskStatus } from '@/types'
const { t: tr } = useI18n()

const props = defineProps<{
  tasks: DownloadTask[]
  /** 首次加载中（还没拿到队列数据） */
  loading?: boolean
  canPause?: boolean
  canCancel?: boolean
}>()

const emit = defineEmits<{
  (e: 'pause', id: string): void
  (e: 'resume', id: string): void
  (e: 'cancel', id: string): void
  (e: 'remove', task: DownloadTask): void
  (e: 'open', task: DownloadTask): void
  (e: 'reveal', task: DownloadTask): void
  (e: 'preview', task: DownloadTask): void
}>()

/** 状态 → 文案 / 颜色 / 图标（与 TaskCard.vue 的 STATUS_MAP 保持一致） */
const STATUS_MAP: Record<TaskStatus, { label: string; color: string; icon: any }> = {
  pending: { label: tr('排队中'), color: 's-text-3', icon: RefreshOutline },
  parsing: { label: tr('解析中'), color: 'text-cyan-600 dark:text-cyan-400', icon: RefreshOutline },
  downloading: { label: tr('下载中'), color: 'text-accent', icon: PlayOutline },
  paused: { label: tr('已暂停'), color: 'text-amber-600 dark:text-amber-400', icon: PauseOutline },
  converting: { label: tr('转换中'), color: 'text-accent', icon: RefreshOutline },
  extracting: { label: tr('提取音频'), color: 'text-cyan-600 dark:text-cyan-400', icon: RefreshOutline },
  transcribing: { label: tr('AI 识别中'), color: 'text-pink-600 dark:text-pink-400', icon: RefreshOutline },
  done: { label: tr('已完成'), color: 'text-emerald-600 dark:text-emerald-400', icon: CheckmarkCircleOutline },
  // ED2K：交给 eMule 引擎接管 —— 与 TaskCard.vue 的映射保持一致
  handed_off: { label: tr('已交给引擎'), color: 'text-sky-600 dark:text-sky-400', icon: CheckmarkCircleOutline },
  error: { label: tr('失败'), color: 'text-rose-600 dark:text-rose-400', icon: AlertCircleOutline },
  canceled: { label: tr('已取消'), color: 's-text-3', icon: CloseCircleOutline },
}

function statusMeta(s: TaskStatus) {
  return STATUS_MAP[s] ?? STATUS_MAP.pending
}

function pct(t: DownloadTask) {
  return Math.max(0, Math.min(100, t.progress || 0))
}

/** 缩略图：本地缓存文件走 asset 协议，远程地址直接用 */
function thumbSrc(t: DownloadTask): string {
  const p = t.thumbnail
  if (!p) return ''
  if (/^[a-zA-Z]:[\\/]/.test(p) || p.startsWith('file:')) {
    try {
      return convertFileSrc(p)
    } catch {
      return ''
    }
  }
  return p
}

/** 远程图被热链保护挡住 / 本地文件丢失时回退占位图标，避免破图 */
const broken = ref<Record<string, boolean>>({})
function markBroken(id: string) {
  broken.value = { ...broken.value, [id]: true }
}
watch(
  () => props.tasks,
  () => {
    broken.value = {}
  },
)

/** 副标题：格式化备注（可翻译）→ 上传者 → 原始链接 */
function subtitle(t: DownloadTask) {
  return translateFormatNote(t.format_note) || t.uploader || t.url
}

/** 已下载 / 总大小 */
function sizeText(t: DownloadTask) {
  if (t.total && t.downloaded) return `${formatBytes(t.downloaded)} / ${formatBytes(t.total)}`
  if (t.total) return formatBytes(t.total)
  return '—'
}

/** 文件在磁盘上才给「打开文件 / 在文件夹中显示」 */
function fileReady(t: DownloadTask) {
  return Boolean(t.file_path && t.file_exists)
}

/** 下载完成但文件已不在磁盘上 */
function fileLost(t: DownloadTask) {
  return Boolean(t.file_path && !t.file_exists && t.status === 'done')
}
</script>

<template>
  <div class="umi-card overflow-hidden">
    <!-- 加载中 -->
    <div v-if="loading" class="s-text-3 flex flex-col items-center gap-2 py-12">
      <NSpin :size="18" />
      <div class="text-[12px]">{{ $t('加载中…') }}</div>
    </div>

    <!-- 空队列 -->
    <div v-else-if="!tasks.length" class="s-text-3 flex flex-col items-center gap-2 py-12">
      <NIcon :size="26" :component="CloudDownloadOutline" />
      <div class="text-[12px]">{{ $t('还没有下载任务') }}</div>
    </div>

    <!-- 详细表格：任务多时纵向滚动，表头吸顶 -->
    <div v-else class="max-h-[62vh] overflow-auto">
      <table class="w-full min-w-[900px] table-fixed border-collapse text-left">
        <thead>
          <tr class="s-text-3 text-[11px]">
            <th class="s-border-soft sticky top-0 z-10 bg-[var(--umi-panel)] border-b px-3 py-2 font-medium backdrop-blur">
              {{ $t('任务') }}
            </th>
            <th class="s-border-soft sticky top-0 z-10 bg-[var(--umi-panel)] w-[104px] whitespace-nowrap border-b px-3 py-2 font-medium backdrop-blur">
              {{ $t('状态') }}
            </th>
            <th class="s-border-soft sticky top-0 z-10 bg-[var(--umi-panel)] w-[176px] whitespace-nowrap border-b px-3 py-2 font-medium backdrop-blur">
              {{ $t('进度') }}
            </th>
            <th class="s-border-soft sticky top-0 z-10 bg-[var(--umi-panel)] w-[142px] whitespace-nowrap border-b px-3 py-2 font-medium backdrop-blur">
              {{ $t('大小') }}
            </th>
            <th class="s-border-soft sticky top-0 z-10 bg-[var(--umi-panel)] w-[136px] whitespace-nowrap border-b px-3 py-2 font-medium backdrop-blur">
              {{ $t('速度 / 剩余') }}
            </th>
            <th class="s-border-soft sticky top-0 z-10 bg-[var(--umi-panel)] w-[240px] whitespace-nowrap border-b px-3 py-2 text-right font-medium backdrop-blur">
              {{ $t('操作') }}
            </th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="t in tasks"
            :key="t.id"
            class="s-border-soft group border-b align-middle transition-colors last:border-b-0 hover:bg-umi-400/[0.06]"
          >
            <!-- 封面缩略图 + 标题 -->
            <td class="px-3 py-2">
              <div class="flex min-w-0 items-center gap-2.5">
                <div
                  class="s-sunken s-border-soft relative h-[38px] w-[68px] shrink-0 cursor-pointer overflow-hidden rounded-md border"
                  :title="$t('点击预览')"
                  @click="emit('preview', t)"
                >
                  <img
                    v-if="thumbSrc(t) && !broken[t.id]"
                    :src="thumbSrc(t)"
                    class="h-full w-full object-cover"
                    alt=""
                    loading="lazy"
                    @error="markBroken(t.id)"
                  />
                  <div v-else class="s-text-3 flex h-full w-full items-center justify-center">
                    <NIcon :size="15" :component="statusMeta(t.status).icon" />
                  </div>
                  <span
                    v-if="t.duration"
                    class="absolute bottom-0.5 right-0.5 rounded bg-black/75 px-1 py-px text-[9.5px] leading-tight tabular-nums text-white"
                  >
                    {{ formatDuration(t.duration) }}
                  </span>
                </div>
                <div class="min-w-0">
                  <div class="flex items-center gap-1.5">
                    <span class="s-text truncate text-[12.5px] font-medium" :title="t.title">
                      {{ t.title || $t('未命名任务') }}
                    </span>
                    <span
                      v-if="fileLost(t)"
                      class="shrink-0 rounded-full bg-amber-500/15 px-1.5 py-px text-[10px] text-amber-600 dark:text-amber-400"
                      :title="$t('原文件已被移动或删除，可重新下载或删除该记录')"
                    >{{ $t('文件已丢失') }}</span>
                  </div>
                  <div class="s-text-3 mt-0.5 truncate text-[11px]" :title="subtitle(t)">{{ subtitle(t) }}</div>
                </div>
              </div>
            </td>

            <!-- 状态 -->
            <td class="px-3 py-2">
              <span class="flex items-center gap-1.5 text-[11.5px]" :class="statusMeta(t.status).color">
                <span v-if="isActive(t)" class="h-1.5 w-1.5 shrink-0 animate-pulse rounded-full bg-current" />
                <NIcon :size="12" :component="statusMeta(t.status).icon" />
                {{ statusMeta(t.status).label }}
              </span>
            </td>

            <!-- 进度 -->
            <td class="px-3 py-2">
              <template v-if="t.status !== 'error'">
                <NProgress
                  type="line"
                  :percentage="pct(t)"
                  :show-indicator="false"
                  :height="5"
                  :border-radius="6"
                  color="linear-gradient(90deg, rgb(var(--umi-a500)), color-mix(in srgb, rgb(var(--umi-a500)) 55%, #22d3ee))"
                  rail-color="var(--umi-panel-3)"
                />
                <div class="s-text-3 mt-1 text-[10.5px] tabular-nums">{{ pct(t).toFixed(1) }}%</div>
              </template>
              <div
                v-else
                class="truncate rounded-md border border-rose-500/25 bg-rose-500/10 px-2 py-1 text-[10.5px] text-rose-500"
                :title="t.error || $t('未知错误')"
              >
                {{ t.error || $t('未知错误') }}
              </div>
            </td>

            <!-- 大小 -->
            <td class="s-text-2 px-3 py-2 text-[11.5px] tabular-nums">{{ sizeText(t) }}</td>

            <!-- 速度 / 剩余 -->
            <td class="px-3 py-2 text-[11px] tabular-nums">
              <div v-if="t.speed" class="s-text-2">↓ {{ t.speed }}</div>
              <div v-if="t.eta" class="s-text-3">{{ $t('剩余 {t}', { t: t.eta }) }}</div>
              <div v-if="!t.speed && !t.eta" class="s-text-3">—</div>
            </td>

            <!-- 操作 -->
            <td class="px-3 py-2">
              <div class="flex items-center justify-end gap-1 opacity-70 transition-opacity group-hover:opacity-100">
                <button
                  v-if="canPause !== false && ['downloading', 'parsing'].includes(t.status)"
                  class="umi-btn !px-2 !py-1"
                  :title="$t('暂停')"
                  @click="emit('pause', t.id)"
                >
                  <NIcon :size="13" :component="PauseOutline" />
                </button>
                <button
                  v-if="t.status === 'paused' || t.status === 'error'"
                  class="umi-btn !px-2 !py-1"
                  :title="$t('继续')"
                  @click="emit('resume', t.id)"
                >
                  <NIcon :size="13" :component="PlayOutline" />
                </button>
                <button
                  v-if="canCancel !== false && isActive(t)"
                  class="umi-btn !px-2 !py-1"
                  :title="$t('取消')"
                  @click="emit('cancel', t.id)"
                >
                  <NIcon :size="13" :component="CloseCircleOutline" />
                </button>
                <button
                  v-if="fileReady(t)"
                  class="umi-btn !px-2 !py-1"
                  :title="$t('打开文件')"
                  @click="emit('open', t)"
                >
                  <NIcon :size="13" :component="PlayCircleOutline" />
                </button>
                <button
                  v-if="fileReady(t)"
                  class="umi-btn !px-2 !py-1"
                  :title="$t('在文件夹中显示')"
                  @click="emit('reveal', t)"
                >
                  <NIcon :size="13" :component="FolderOpenOutline" />
                </button>
                <button v-if="t.file_path" class="umi-btn !px-2 !py-1" :title="$t('预览')" @click="emit('preview', t)">
                  <NIcon :size="13" :component="EyeOutline" />
                </button>
                <button
                  class="umi-btn !px-2 !py-1 hover:!border-rose-400/40"
                  :title="$t('删除记录')"
                  @click="emit('remove', t)"
                >
                  <NIcon :size="13" :component="TrashOutline" />
                </button>
              </div>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>
