/**
 * 工具刷新（refreshTools）的前端兜底超时
 *
 * 背景（实机复现过的 bug）：换工具目录后 store.refreshTools() 触发后端探活，
 * eMule 这种 GUI 程序被 `--version` 启动探测时永不退出，探活进程不返回 →
 * 设置页永久停在「处理中…」，全部按钮禁用，只能 kill 客户端。
 * 后端已修（GUI 不启动探测 + 探活超时 + 总预算），前端这里再压一道保底：
 * 无论后端因为什么原因不返回，界面都必须在有限时间内解锁，
 * 并且解锁用的是**上一次的结果**，不能把已装工具当成没装。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

vi.mock('@/services/ipc', () => ({
  detectTools: vi.fn(),
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => undefined),
}))

import * as ipc from '@/services/ipc'
import { invoke } from '@tauri-apps/api/core'
import { useTaskStore } from '@/stores/tasks'
import type { ToolStatus } from '@/types'

const tool = (name: string, found: boolean, version: string | null): ToolStatus => ({
  name,
  found,
  path: found ? `C:/tools/${name}.exe` : null,
  version,
  source: found ? 'managed' : 'missing',
  managed_path: found ? `C:/tools/${name}.exe` : null,
  hint: name,
})

const LAST = [tool('aria2', true, '1.37.0'), tool('emule', true, null)]

describe('商店 refreshTools —— 前端兜底超时', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.useFakeTimers()
    vi.mocked(ipc.detectTools).mockReset()
    vi.mocked(invoke).mockClear()
  })

  it('后端一直不返回时，20s 后按「上次结果」解锁界面', async () => {
    const store = useTaskStore()
    store.tools = LAST
    // 探活吊死：既不 resolve 也不 reject
    vi.mocked(ipc.detectTools).mockReturnValue(new Promise<ToolStatus[]>(() => {}))

    const pending = store.refreshTools()
    // 19.9s 还在等：不能提前解锁、也不能报超时
    await vi.advanceTimersByTimeAsync(19_900)
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('frontend_log', expect.anything())

    await vi.advanceTimersByTimeAsync(200)
    const got = await pending

    expect(got).toEqual(LAST)
    expect(store.tools).toEqual(LAST)
    const logged = vi
      .mocked(invoke)
      .mock.calls.filter(([cmd]) => cmd === 'frontend_log')
      .map(([, args]) => String((args as { msg?: string })?.msg ?? ''))
    expect(logged.some((m) => m.includes('20s') && m.includes('未返回'))).toBe(true)
  })

  it('后端正常返回时用后端结果，且不触发超时日志', async () => {
    const store = useTaskStore()
    store.tools = LAST
    const fresh = [tool('aria2', true, '1.37.0'), tool('emule', true, null), tool('poppler', true, '26.09.0')]
    vi.mocked(ipc.detectTools).mockResolvedValue(fresh)

    const got = await store.refreshTools()
    await vi.advanceTimersByTimeAsync(60_000) // 定时器即使残留也不该再改状态

    expect(got).toEqual(fresh)
    expect(store.tools).toEqual(fresh)
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('frontend_log', expect.anything())
  })

  it('后端报错时保留上次结果（界面照常解锁）', async () => {
    const store = useTaskStore()
    store.tools = LAST
    vi.mocked(ipc.detectTools).mockRejectedValue(new Error('命令 detect_tools 需要在桌面客户端中运行'))

    const got = await store.refreshTools()

    expect(got).toEqual(LAST)
    const logged = vi
      .mocked(invoke)
      .mock.calls.filter(([cmd]) => cmd === 'frontend_log')
      .map(([, args]) => String((args as { msg?: string })?.msg ?? ''))
    expect(logged.some((m) => m.includes('工具刷新失败'))).toBe(true)
  })
})
