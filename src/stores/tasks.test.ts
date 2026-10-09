/**
 * tasks store —— 删除记录后的本地摘行（dropTask / dropDownloads）
 *
 * 背景（实机复现过的 bug）：字幕页 / 转换页的行内「删除记录」只 await 了后端命令，
 * store 从来不更新 —— 后端 list_subtitles 从 9 变 8，store.subtitles 还是 9，
 * DOM 还渲染 9 行，切页回来依旧，只有重启客户端才消失。
 *
 * 这里盯 store 这个唯一出口：后端返回 ok 之后调它，**对应队列必须正好少一行**；
 * 没删除成功的（视图侧 catch）不该动一行 —— 所以别的地方都不许再偷偷改这三条列表。
 */
import { beforeEach, describe, expect, it } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { useTaskStore } from '@/stores/tasks'
import type { ConvertTask, DownloadTask, SubtitleTask, TaskStatus } from '@/types'

let seq = 0

const dl = (id: string, status: TaskStatus = 'done'): DownloadTask => ({
  id,
  title: id,
  url: `https://example.com/${id}`,
  status,
  progress: 100,
  created_time: ++seq,
})
const cv = (id: string, status: TaskStatus = 'done'): ConvertTask => ({
  id,
  input_file: `C:/in/${id}.mp4`,
  output_file: `C:/out/${id}.mkv`,
  format: 'mkv',
  status,
  progress: 100,
  created_time: ++seq,
})
const sb = (id: string, status: TaskStatus = 'done'): SubtitleTask => ({
  id,
  video_path: `C:/in/${id}.mp4`,
  language: 'zh',
  model: 'base',
  subtitle_path: `C:/out/${id}.zh.srt`,
  status,
  progress: 100,
  created_time: ++seq,
})

/** 三条队列各放几条（含进行中的，用于验证「只摘该摘的」） */
function seedStore() {
  const store = useTaskStore()
  store.downloads = [dl('dl-1'), dl('dl-2', 'downloading'), dl('dl-3', 'error')]
  store.converts = [cv('cv-1'), cv('cv-2', 'converting')]
  store.subtitles = [sb('st-1'), sb('st-2', 'transcribing')]
  return store
}

const ids = <T extends { id: string }>(list: T[]) => list.map((x) => x.id)

beforeEach(() => setActivePinia(createPinia()))

describe('dropTask —— 后端删掉哪条，store 就摘哪条', () => {
  it('字幕：删一条 → store.subtitles 长度减一，另两条队列一行不动', () => {
    const store = seedStore()
    store.dropTask('st-1')
    expect(ids(store.subtitles)).toEqual(['st-2'])
    expect(store.downloads.length, '下载队列不该被牵连').toBe(3)
    expect(store.converts.length, '转换队列不该被牵连').toBe(2)
  })

  it('转换：删一条 → store.converts 长度减一', () => {
    const store = seedStore()
    store.dropTask('cv-1')
    expect(ids(store.converts)).toEqual(['cv-2'])
    expect(store.subtitles.length).toBe(2)
  })

  it('下载：删一条 → store.downloads 长度减一', () => {
    const store = seedStore()
    store.dropTask('dl-1')
    expect(ids(store.downloads)).toEqual(['dl-2', 'dl-3'])
    expect(store.subtitles.length).toBe(2)
  })

  it('进行中的那条也能被摘掉（删除「下载中 / 识别中」的任务同样是删记录）', () => {
    const store = seedStore()
    store.dropTask('dl-2')
    store.dropTask('cv-2')
    store.dropTask('st-2')
    expect(ids(store.downloads)).toEqual(['dl-1', 'dl-3'])
    expect(ids(store.converts)).toEqual(['cv-1'])
    expect(ids(store.subtitles)).toEqual(['st-1'])
  })

  it('不存在的 id / 空 id：空转，列表一行都不少（视图 + 事件双路径不会多删）', () => {
    const store = seedStore()
    store.dropTask('不存在')
    store.dropTask('')
    store.dropTask(null)
    store.dropTask(undefined)
    expect(store.downloads.length).toBe(3)
    expect(store.converts.length).toBe(2)
    expect(store.subtitles.length).toBe(2)
    // 同一条删两次（事件先到 + 视图再摘）也只会少一行
    store.dropTask('st-1')
    store.dropTask('st-1')
    expect(ids(store.subtitles)).toEqual(['st-2'])
  })
})

describe('dropDownloads —— 清理已完成 / 清空队列', () => {
  it("'done'：只摘终态行（done/error/canceled），进行中的留下", () => {
    const store = useTaskStore()
    store.downloads = [
      dl('d1', 'done'),
      dl('d2', 'downloading'),
      dl('d3', 'error'),
      dl('d4', 'canceled'),
      dl('d6', 'paused'),
      dl('d7', 'pending'),
    ]
    store.dropDownloads('done')
    expect(ids(store.downloads), '进行中/排队/暂停的行必须留下').toEqual(['d2', 'd6', 'd7'])
  })

  it("'all'：整列清空", () => {
    const store = seedStore()
    store.dropDownloads('all')
    expect(store.downloads.length).toBe(0)
    expect(store.converts.length, '清空下载队列不该动别的队列').toBe(2)
  })
})
