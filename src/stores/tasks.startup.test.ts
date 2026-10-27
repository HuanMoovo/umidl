import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import { useTaskStore } from '@/stores/tasks'
import * as ipc from '@/services/ipc'
import type { ConvertTask, DownloadTask, SubtitleTask } from '@/types'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue(undefined) }))
vi.mock('@/services/ipc', () => ({
  listDownloads: vi.fn(),
  listConverts: vi.fn(),
  listSubtitles: vi.fn(),
  onEvent: vi.fn(),
  detectTools: vi.fn().mockResolvedValue([]),
  backfillCovers: vi.fn().mockResolvedValue(0),
}))

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}

const download = (id: string, progress = 0): DownloadTask => ({
  id, title: id, url: `https://example.com/${id}`, status: 'downloading', progress, created_time: 1,
})
const convert = (id: string): ConvertTask => ({
  id, input_file: 'in.mp4', output_file: 'out.mkv', format: 'mkv', status: 'converting', progress: 0, created_time: 1,
})
const subtitle = (id: string): SubtitleTask => ({
  id, video_path: 'in.mp4', model: 'base', language: 'auto', status: 'transcribing', progress: 0, created_time: 1,
})
type Handler = (payload: any) => void
let listeners: Map<string, Set<Handler>>

function emit(event: string, payload: unknown) {
  listeners.get(event)?.forEach((handler) => handler(payload))
}

beforeEach(() => {
  vi.clearAllMocks()
  setActivePinia(createPinia())
  listeners = new Map()
  vi.mocked(invoke).mockResolvedValue(undefined)
  vi.mocked(ipc.detectTools).mockResolvedValue([])
  vi.mocked(ipc.backfillCovers).mockResolvedValue(0)
  vi.mocked(ipc.listDownloads).mockReset().mockResolvedValue([])
  vi.mocked(ipc.listConverts).mockReset().mockResolvedValue([])
  vi.mocked(ipc.listSubtitles).mockReset().mockResolvedValue([])
  vi.mocked(ipc.onEvent).mockReset().mockImplementation(async (event, handler) => {
    const handlers = listeners.get(event) ?? new Set<Handler>()
    handlers.add(handler)
    listeners.set(event, handlers)
    return () => { handlers.delete(handler) }
  })
})

afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe('startup queue recovery', () => {
  it('keeps successful queues and existing failed-queue rows when one read rejects', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    vi.mocked(ipc.listDownloads).mockResolvedValue([download('download')])
    vi.mocked(ipc.listConverts).mockRejectedValue(new Error('database unavailable'))
    vi.mocked(ipc.listSubtitles).mockResolvedValue([subtitle('subtitle')])
    const store = useTaskStore()
    store.converts = [convert('existing')]

    await store.bootstrap()

    expect(store.downloads.map((task) => task.id)).toEqual(['download'])
    expect(store.converts.map((task) => task.id)).toEqual(['existing'])
    expect(store.subtitles.map((task) => task.id)).toEqual(['subtitle'])
    expect(store.ready).toBe(true)
    expect(store.startupFailed).toBe(true)
  })

  it('keeps progress events that arrive while a stale snapshot is pending', async () => {
    const pending = deferred<DownloadTask[]>()
    vi.mocked(ipc.listDownloads).mockReturnValue(pending.promise)
    const store = useTaskStore()
    const boot = store.bootstrap()
    await vi.waitFor(() => expect(ipc.listDownloads).toHaveBeenCalledTimes(1))

    emit('download://update', download('active', 75))
    emit('download://update', download('new', 10))
    pending.resolve([download('active', 5), download('history', 100)])
    await boot

    expect(store.downloads.map((task) => [task.id, task.progress])).toEqual([
      ['new', 10], ['active', 75], ['history', 100],
    ])
  })

  it('does not resurrect rows removed while startup is reading history', async () => {
    const pending = deferred<DownloadTask[]>()
    vi.mocked(ipc.listDownloads).mockReturnValue(pending.promise)
    const store = useTaskStore()
    const boot = store.bootstrap()
    await vi.waitFor(() => expect(ipc.listDownloads).toHaveBeenCalledTimes(1))

    emit('download://removed', { id: 'removed' })
    pending.resolve([download('removed'), download('kept')])
    await boot

    expect(store.downloads.map((task) => task.id)).toEqual(['kept'])
  })

  it('replays clear and later updates in order instead of restoring cleared history', async () => {
    const pending = deferred<DownloadTask[]>()
    vi.mocked(ipc.listDownloads).mockReturnValue(pending.promise)
    const store = useTaskStore()
    const boot = store.bootstrap()
    await vi.waitFor(() => expect(ipc.listDownloads).toHaveBeenCalledTimes(1))

    emit('download://removed', { all: true })
    emit('download://update', download('new'))
    pending.resolve([download('cleared')])
    await boot

    expect(store.downloads.map((task) => task.id)).toEqual(['new'])
  })

  it('preserves local enqueue and delete actions during a pending snapshot', async () => {
    const pending = deferred<ConvertTask[]>()
    vi.mocked(ipc.listConverts).mockReturnValue(pending.promise)
    const store = useTaskStore()
    const boot = store.bootstrap()
    await vi.waitFor(() => expect(ipc.listConverts).toHaveBeenCalledTimes(1))

    store.dropTask('removed')
    store.enqueueConvert(convert('new'))
    pending.resolve([convert('removed'), convert('history')])
    await boot

    expect(store.converts.map((task) => task.id)).toEqual(['new', 'history'])
  })

  it('retries a failed queue without reloading successful queues or losing live events', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    vi.mocked(ipc.listDownloads).mockResolvedValue([download('active', 10)])
    vi.mocked(ipc.listConverts).mockRejectedValueOnce(new Error('temporary failure')).mockResolvedValue([convert('recovered')])
    const store = useTaskStore()
    await store.bootstrap()
    emit('download://update', download('active', 90))

    await store.bootstrap()

    expect(store.converts.map((task) => task.id)).toEqual(['recovered'])
    expect(store.downloads[0].progress).toBe(90)
    expect(ipc.listDownloads).toHaveBeenCalledTimes(1)
  })
})

describe('startup subscriptions', () => {
  it('resyncs conversion state missed before a failed subscription is recovered', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const register = vi.mocked(ipc.onEvent).getMockImplementation()!
    let fail = true
    vi.mocked(ipc.onEvent).mockImplementation((event, handler) => {
      if (event === 'convert://update' && fail) {
        fail = false
        return Promise.reject(new Error('listener unavailable'))
      }
      return register(event, handler)
    })
    vi.mocked(ipc.listConverts).mockResolvedValueOnce([convert('gap')])
      .mockResolvedValue([{ ...convert('gap'), status: 'done', progress: 100 }])
    const store = useTaskStore()
    await store.bootstrap()
    emit('convert://update', { ...convert('gap'), status: 'done', progress: 100 })
    expect(store.converts[0].status).toBe('converting')

    await store.bootstrap()

    expect(store.converts[0].status).toBe('done')
    expect(store.activeConverts).toEqual([])
    expect(store.startupFailed).toBe(false)
    expect(ipc.listDownloads).toHaveBeenCalledTimes(1)
    expect(ipc.listSubtitles).toHaveBeenCalledTimes(1)
    expect(listeners.get('convert://update')?.size).toBe(1)
  })

  it('resyncs downloaded rows removed while the removal subscription was unavailable', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const register = vi.mocked(ipc.onEvent).getMockImplementation()!
    let fail = true
    vi.mocked(ipc.onEvent).mockImplementation((event, handler) => {
      if (event === 'download://removed' && fail) {
        fail = false
        return Promise.reject(new Error('listener unavailable'))
      }
      return register(event, handler)
    })
    vi.mocked(ipc.listDownloads).mockResolvedValueOnce([download('removed')]).mockResolvedValue([])
    const store = useTaskStore()
    await store.bootstrap()
    emit('download://removed', { id: 'removed' })
    expect(store.downloads).toHaveLength(1)

    await store.bootstrap()

    expect(store.downloads).toEqual([])
    expect(store.startupFailed).toBe(false)
    expect(ipc.listConverts).toHaveBeenCalledTimes(1)
    expect(ipc.listSubtitles).toHaveBeenCalledTimes(1)
  })

  it('preserves live events while resyncing after recovering a subtitle subscription', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const register = vi.mocked(ipc.onEvent).getMockImplementation()!
    let fail = true
    vi.mocked(ipc.onEvent).mockImplementation((event, handler) => {
      if (event === 'subtitle://update' && fail) {
        fail = false
        return Promise.reject(new Error('listener unavailable'))
      }
      return register(event, handler)
    })
    const pending = deferred<SubtitleTask[]>()
    vi.mocked(ipc.listSubtitles).mockResolvedValueOnce([subtitle('active')]).mockReturnValue(pending.promise)
    const store = useTaskStore()
    await store.bootstrap()
    const retry = store.bootstrap()
    await vi.waitFor(() => expect(listeners.get('subtitle://update')?.size).toBe(1))
    emit('subtitle://update', { ...subtitle('active'), progress: 75 })
    pending.resolve([{ ...subtitle('active'), progress: 10 }, subtitle('missed')])
    await retry

    expect(store.subtitles.map((task) => [task.id, task.progress])).toEqual([['active', 75], ['missed', 0]])
    expect(store.startupFailed).toBe(false)
  })

  it('keeps recovery available if the resync fails after a listener recovers', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const register = vi.mocked(ipc.onEvent).getMockImplementation()!
    let fail = true
    vi.mocked(ipc.onEvent).mockImplementation((event, handler) => {
      if (event === 'download://update' && fail) {
        fail = false
        return Promise.reject(new Error('listener unavailable'))
      }
      return register(event, handler)
    })
    vi.mocked(ipc.listDownloads).mockResolvedValueOnce([download('active')])
      .mockRejectedValueOnce(new Error('database unavailable'))
      .mockResolvedValue([{ ...download('active'), status: 'done', progress: 100 }])
    const store = useTaskStore()
    await store.bootstrap()
    await store.bootstrap()

    expect(store.startupFailed).toBe(true)
    expect(store.downloads[0].status).toBe('downloading')
    await store.bootstrap()
    expect(store.startupFailed).toBe(false)
    expect(store.downloads[0].status).toBe('done')
    expect(listeners.get('download://update')?.size).toBe(1)
  })

  it('settles a stalled queue within the startup budget and ignores its late snapshot', async () => {
    vi.useFakeTimers()
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const pending = deferred<DownloadTask[]>()
    vi.mocked(ipc.listDownloads).mockReturnValueOnce(pending.promise).mockResolvedValue([download('recovered')])
    vi.mocked(ipc.listConverts).mockResolvedValue([convert('available')])
    const store = useTaskStore()
    let settled = false
    const boot = store.bootstrap().then(() => { settled = true })
    await vi.advanceTimersByTimeAsync(10_000)
    expect(settled).toBe(true)
    await boot

    expect(store.ready).toBe(true)
    expect(store.startupFailed).toBe(true)
    expect(store.converts[0].id).toBe('available')
    await store.bootstrap()
    expect(store.startupFailed).toBe(false)
    pending.resolve([download('stale')])
    await Promise.resolve()
    expect(store.downloads.map((task) => task.id)).toEqual(['recovered'])
  })

  it('settles a stalled subscription and unregisters it if it resolves after a retry', async () => {
    vi.useFakeTimers()
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const pending = deferred<() => void>()
    const register = vi.mocked(ipc.onEvent).getMockImplementation()!
    let first = true
    vi.mocked(ipc.onEvent).mockImplementation((event, handler) => {
      if (event === 'convert://update' && first) {
        first = false
        return pending.promise
      }
      return register(event, handler)
    })
    const store = useTaskStore()
    let settled = false
    const boot = store.bootstrap().then(() => { settled = true })
    await vi.advanceTimersByTimeAsync(10_000)
    expect(settled).toBe(true)
    await boot
    expect(store.ready).toBe(true)
    expect(store.startupFailed).toBe(true)

    await store.bootstrap()
    const lateUnlisten = vi.fn()
    pending.resolve(lateUnlisten)
    await Promise.resolve()
    await Promise.resolve()
    expect(lateUnlisten).toHaveBeenCalledTimes(1)
    expect(store.startupFailed).toBe(false)
    expect(listeners.get('convert://update')?.size).toBe(1)
  })

  it('keeps events received while another subscription is still registering', async () => {
    const pending = deferred<() => void>()
    const register = vi.mocked(ipc.onEvent).getMockImplementation()!
    vi.mocked(ipc.onEvent).mockImplementation((event, handler) => event === 'selftest://update'
      ? pending.promise
      : register(event, handler))
    vi.mocked(ipc.listDownloads).mockResolvedValue([download('active', 5)])
    const store = useTaskStore()
    const boot = store.bootstrap()
    await vi.waitFor(() => expect(listeners.get('download://update')?.size).toBe(1))
    emit('download://update', download('active', 75))
    pending.resolve(() => {})
    await boot

    expect(store.downloads[0].progress).toBe(75)
  })

  it('settles startup after a subscription fails and retries only missing listeners', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const register = vi.mocked(ipc.onEvent).getMockImplementation()!
    let fail = true
    vi.mocked(ipc.onEvent).mockImplementation((event, handler) => {
      if (event === 'convert://update' && fail) {
        fail = false
        return Promise.reject(new Error('listener unavailable'))
      }
      return register(event, handler)
    })
    const store = useTaskStore()

    await expect(store.bootstrap()).resolves.toBeUndefined()
    expect(store.ready).toBe(true)
    expect(store.startupFailed).toBe(true)
    await store.bootstrap()
    emit('convert://update', convert('recovered'))

    expect(store.converts.map((task) => task.id)).toEqual(['recovered'])
    expect(store.ready).toBe(true)
    expect(store.startupFailed).toBe(false)
    expect(listeners.get('download://update')?.size).toBe(1)
    expect(listeners.get('convert://update')?.size).toBe(1)
    expect(ipc.onEvent).toHaveBeenCalledTimes(7)
  })

  it('shares concurrent startup and releases subscriptions with the store', async () => {
    const pending = deferred<DownloadTask[]>()
    vi.mocked(ipc.listDownloads).mockReturnValue(pending.promise)
    const store = useTaskStore()
    const first = store.bootstrap()
    const second = store.bootstrap()
    pending.resolve([])
    await Promise.all([first, second])

    expect(ipc.listDownloads).toHaveBeenCalledTimes(1)
    expect(ipc.onEvent).toHaveBeenCalledTimes(6)
    store.$dispose()
    emit('download://update', download('after-dispose'))
    expect(store.downloads).toEqual([])
  })
})
