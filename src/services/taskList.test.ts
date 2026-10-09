import { describe, expect, it } from 'vitest'
import {
  FINISHED_DOWNLOAD_STATUSES,
  dropById,
  dropWhere,
  enqueueOnce,
  isFinishedDownload,
} from '@/services/taskList'

type T = { id: string; status: string; progress?: number }

const t = (id: string, status = 'pending', progress = 0): T => ({ id, status, progress })

describe('enqueueOnce —— 入队时同一条任务只能出现一份', () => {
  it('列表里没有同 id → 插到最前面', () => {
    const list: T[] = [t('a'), t('b')]
    enqueueOnce(list, t('c'))
    expect(list.map((x) => x.id)).toEqual(['c', 'a', 'b'])
  })

  it('事件已经 upsert 过同 id → 不再插第二条（复现过的重复卡片场景）', () => {
    // 事件先到：任务已经在列表里且是较新的状态
    const list: T[] = [t('x', 'downloading', 12.5)]
    // 命令返回值带着「入队瞬间快照」后到
    enqueueOnce(list, t('x', 'pending', 0))
    expect(list.length, '同一条任务不该出现两条').toBe(1)
    expect(list[0].status, '不能被旧快照覆盖回 pending').toBe('downloading')
    expect(list[0].progress).toBe(12.5)
  })

  it('连续入队同一条链接 5 次也只会有一份', () => {
    const list: T[] = []
    for (let i = 0; i < 5; i++) enqueueOnce(list, t('same'))
    expect(list.length).toBe(1)
  })

  it('空值/无 id 直接忽略，不污染列表', () => {
    const list: T[] = [t('a')]
    enqueueOnce(list, null)
    enqueueOnce(list, undefined)
    enqueueOnce(list, { id: '', status: 'pending' })
    expect(list.map((x) => x.id)).toEqual(['a'])
  })

  it('新任务仍然立即上屏（事件丢失时的兜底路径）', () => {
    const list: T[] = []
    enqueueOnce(list, t('fresh'))
    expect(list.map((x) => x.id)).toEqual(['fresh'])
  })
})

describe('dropById / dropWhere —— 后端删了，界面就得跟着少一行', () => {
  it('命中 id → 只少这一行，其余顺序不变', () => {
    const list: T[] = [t('a'), t('b'), t('c')]
    dropById(list, 'b')
    expect(list.map((x) => x.id)).toEqual(['a', 'c'])
  })

  it('id 不在列表里 → 一行都不动（重复调用安全）', () => {
    const list: T[] = [t('a'), t('b')]
    dropById(list, 'zzz')
    dropById(list, 'zzz')
    expect(list.map((x) => x.id)).toEqual(['a', 'b'])
  })

  it('空 id / null / undefined → 一行都不动（不误删）', () => {
    const list: T[] = [t('a')]
    dropById(list, '')
    dropById(list, null)
    dropById(list, undefined)
    expect(list.length).toBe(1)
  })

  it('dropWhere：只摘谓词命中的行（清理已完成 = 只摘终态）', () => {
    const list: T[] = [t('done1', 'done'), t('run', 'downloading'), t('err', 'error'), t('idle', 'paused')]
    dropWhere(list, (x) => ['done', 'error'].includes(x.status))
    expect(list.map((x) => x.id)).toEqual(['run', 'idle'])
  })

  it('dropWhere：谓词恒真 → 整列清空；空列表安全', () => {
    const list: T[] = [t('a'), t('b')]
    dropWhere(list, () => true)
    expect(list.length).toBe(0)
    const empty: T[] = []
    dropWhere(empty, () => true)
    expect(empty.length).toBe(0)
  })

  it('终态口径与后端 clear_downloads("done") 一致：done/error/canceled', () => {
    expect([...FINISHED_DOWNLOAD_STATUSES]).toEqual(['done', 'error', 'canceled'])
    for (const s of FINISHED_DOWNLOAD_STATUSES) expect(isFinishedDownload(t('x', s)), s).toBe(true)
    for (const s of ['pending', 'parsing', 'downloading', 'paused', 'converting', 'extracting', 'transcribing']) {
      expect(isFinishedDownload(t('x', s)), s).toBe(false)
    }
    expect(isFinishedDownload(null)).toBe(false)
  })
})
