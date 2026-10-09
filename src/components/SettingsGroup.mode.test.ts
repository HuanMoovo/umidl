/**
 * SettingsGroup（1.10）行为测试：
 *  - 简单模式（ui_mode 默认 simple）：区组整体不渲染（不是折叠，是不显示）；
 *  - 高级模式：标题行渲染、内容默认折叠，点击标题行展开 / 收起。
 * 组件与设置 store 的 ui_mode 是唯一状态源（设置页与功能页共用）。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, h, nextTick } from 'vue'
import { createPinia, setActivePinia } from 'pinia'
import { h as hh } from 'vue'
import SettingsGroup from '@/components/SettingsGroup.vue'
import { useSettingsStore } from '@/stores/settings'

vi.mock('@/services/ipc', () => ({
  getSettings: vi.fn(),
  saveSettings: vi.fn(),
  defaultDownloadDir: vi.fn(),
}))

async function mountGroup() {
  const host = document.createElement('div')
  document.body.appendChild(host)
  const app = createApp({
    setup: () => () =>
      h(SettingsGroup, { title: '高级选项', open: false }, { default: () => hh('span', { 'data-test': 'group-content' }, 'inner') }),
  })
  app.use(createPinia())
  app.mount(host)
  for (let i = 0; i < 4; i += 1) await nextTick()
  return { app, host }
}

describe('SettingsGroup：简单 / 高级模式与折叠', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('简单模式（默认）：区组整体不渲染，内容不存在于 DOM', async () => {
    const { app } = await mountGroup()
    const store = useSettingsStore()
    store.patch({ ui_mode: 'simple' })
    await nextTick()
    expect(document.querySelector('[data-test="group-toggle"]')).toBeNull()
    expect(document.querySelector('[data-test="group-content"]')).toBeNull()
    app.unmount()
  })

  it('高级模式：标题行渲染、默认折叠，点击后展开', async () => {
    const { app } = await mountGroup()
    const store = useSettingsStore()
    store.patch({ ui_mode: 'advanced' })
    await nextTick()

    const head = document.querySelector<HTMLButtonElement>('[data-test="group-toggle"]')
    expect(head, '高级模式下应出现可折叠标题行').toBeTruthy()
    expect(head!.getAttribute('aria-expanded')).toBe('false')
    // v-show：元素在 DOM 里但不可见（display:none）
    const body = document.querySelector('[data-test="group-content"]')
    expect(body).toBeTruthy()
    expect((body!.closest('.umi-group-body') as HTMLElement).style.display).toBe('none')

    head!.click()
    await nextTick()
    expect(head!.getAttribute('aria-expanded')).toBe('true')
    expect((document.querySelector('[data-test="group-content"]')!.closest('.umi-group-body') as HTMLElement).style.display).not.toBe('none')
    app.unmount()
  })
})
