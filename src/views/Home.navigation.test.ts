import { afterEach, describe, expect, it } from 'vitest'
import { createApp, nextTick, type App } from 'vue'
import { createPinia } from 'pinia'
import { createMemoryHistory, createRouter } from 'vue-router'
import Home from './Home.vue'
import { i18n } from '@/i18n'
import { useTaskStore } from '@/stores/tasks'
import type { ConvertTask, DownloadTask, SubtitleTask } from '@/types'

let app: App | null = null
afterEach(() => {
  app?.unmount()
  app = null
  document.body.innerHTML = ''
})

async function mountHome(counts: { downloads?: number; converts?: number; subtitles?: number }) {
  const pinia = createPinia()
  const store = useTaskStore(pinia)
  store.ready = true
  store.downloads = Array.from({ length: counts.downloads ?? 0 }, (_, i) => ({ id: `d${i}`, status: 'downloading' } as DownloadTask))
  store.converts = Array.from({ length: counts.converts ?? 0 }, (_, i) => ({ id: `c${i}`, status: 'converting' } as ConvertTask))
  store.subtitles = Array.from({ length: counts.subtitles ?? 0 }, (_, i) => ({ id: `s${i}`, status: 'transcribing' } as SubtitleTask))
  const router = createRouter({
    history: createMemoryHistory(),
    routes: ['/', '/download', '/converter', '/subtitle'].map((path) => ({ path, component: Home })),
  })
  await router.push('/')
  const host = document.createElement('div')
  document.body.append(host)
  app = createApp(Home)
  app.use(pinia).use(i18n).use(router).mount(host)
  await nextTick()
  return { host, router }
}

describe('Home navigation', () => {
  it.each([
    [{ converts: 1 }, '/converter'],
    [{ subtitles: 1 }, '/subtitle'],
    [{ downloads: 1 }, '/download'],
    [{ downloads: 1, converts: 3, subtitles: 2 }, '/converter'],
  ] as const)('opens the module with the most active work: %j', async (counts, path) => {
    const { host, router } = await mountHome(counts)
    host.querySelector<HTMLButtonElement>('[data-test="home-stat-active"]')!.click()
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(router.currentRoute.value.path).toBe(path)
  })
})
