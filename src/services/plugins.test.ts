/**
 * 插件市场形状回归
 *
 * 后端 `plugin_market_list` 返回的是索引对象 `{ version, generated_at, description, plugins: [...] }`；
 * 页面按数组消费。曾经直接按数组解析 → `Array.isArray(m) ? m : []` 一律落成空数组，
 * 界面上「插件市场 0 条、市场暂无可用插件」，内置的两个插件根本装不到。
 *
 * 这里锁住：① 索引对象 → 取 plugins 数组；② 老形状（直接数组）→ 原样用；③ 异常形状 → 空数组不炸。
 */
import { describe, expect, it, beforeAll } from 'vitest'
import { pluginMarketList } from '@/services/plugins'

const ENTRY = {
  id: 'direct-link-sniffer',
  name: '直链嗅探器',
  version: '1.0.0',
  description: '用 umi.resolve 判断链接是否为可直下的直链',
  sha256: 'a8ac1782de52099456dd73203c0ae836e4b17265bf0a174630a8bceeed68e081',
  size: 2049,
}

/** 按需改写的 mock 应答 */
let response: unknown = null

function installStub() {
  ;(window as any).__TAURI_INTERNALS__ = {
    invoke: (cmd: string) => {
      if (cmd === 'plugin_market_list') return Promise.resolve(response)
      return Promise.reject(new Error(`[test] 未处理 ${cmd}`))
    },
    transformCallback: (cb: any) => cb,
    metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
  }
}

describe('插件市场列表形状', () => {
  beforeAll(installStub)

  it('索引对象：取出 plugins 数组（后端的真实形状）', async () => {
    response = {
      version: 1,
      generated_at: '2026-10-01T00:00:00+00:00',
      description: 'Umidl 内置插件市场（内容寻址：sha256 为文件真实摘要）',
      plugins: [ENTRY],
    }
    const list = await pluginMarketList()
    expect(list).toHaveLength(1)
    expect(list[0].id).toBe('direct-link-sniffer')
    expect(list[0].sha256).toBe(ENTRY.sha256)
  })

  it('老形状：直接返回数组时原样使用', async () => {
    response = [ENTRY]
    expect(await pluginMarketList()).toHaveLength(1)
  })

  it('异常形状：不抛错，退化为空数组', async () => {
    response = { version: 1 } // 没有 plugins 字段
    expect(await pluginMarketList()).toEqual([])
    response = null
    expect(await pluginMarketList()).toEqual([])
  })
})
