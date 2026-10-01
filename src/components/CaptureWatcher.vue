<script setup lang="ts">
/**
 * 浏览器捕获监听：浏览器扩展 / 书签脚本 / 命令行把链接推到本地捕获端口后，
 * 后端会 emit `capture://url`，这里负责刷新队列并给出提示。
 */
import { onMounted, onUnmounted } from 'vue'
import { useMessage } from 'naive-ui'
import { useI18n } from 'vue-i18n'
import { onEvent } from '@/services/ipc'
import { useTaskStore } from '@/stores/tasks'

const message = useMessage()
const { t } = useI18n()
const tasks = useTaskStore()

let off: (() => void) | null = null

onMounted(async () => {
  off = await onEvent('capture://url', async (p) => {
    await tasks.reload()
    if (p.blocked) {
      message.warning(t('已捕获链接，但被智能过滤规则拦截'))
    } else if (p.queued) {
      message.success(t('已捕获并加入下载队列'))
    } else {
      message.info(t('已捕获链接（自动入队已关闭）'))
    }
  })
})

onUnmounted(() => {
  if (off) off()
})
</script>

<template>
  <span class="hidden" aria-hidden="true" />
</template>
