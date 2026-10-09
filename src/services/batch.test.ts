import { describe, expect, it } from 'vitest'
import {
  BATCH_TEXT_MAX_CHARS,
  ENQUEUE_LIMIT,
  createParseCursor,
  parseChunk,
  parseLinks,
  truncateBatchText,
} from '@/services/batch'

describe('批量导入：链接识别 / 去重计数', () => {
  it('空文本 → 0 条', () => {
    expect(parseLinks('')).toEqual({ total: 0, unique: [], overflow: 0 })
    expect(parseLinks('   \n  \n\t').total).toBe(0)
  })

  it('多行粘贴：空行与 # / // 注释行整行忽略', () => {
    const p = parseLinks('https://a.example/1\n\n#注释\n//注释\n# 带空格的注释 也要忽略\nhttps://b.example/2\n')
    expect(p.total).toBe(2)
    expect(p.unique).toEqual(['https://a.example/1', 'https://b.example/2'])
  })

  it('注释行里的文字不会折算成链接（识别条数 = 用户真实粘贴的链接数）', () => {
    const p = parseLinks('# 这是注释\nhttps://a.example/1')
    expect(p.total).toBe(1)
    expect(p.unique).toEqual(['https://a.example/1'])
  })

  it('重复链接：识别条数含重复，去重后只留一条（顺序保持）', () => {
    const p = parseLinks('https://a.example/1\nhttps://b.example/2\n\nhttps://a.example/1\nhttps://c.example/3')
    expect(p.total).toBe(4)
    expect(p.unique).toEqual(['https://a.example/1', 'https://b.example/2', 'https://c.example/3'])
  })

  it('支持逗号（半角 / 全角）与空格分隔', () => {
    const p = parseLinks('https://a.example/1, https://b.example/2，https://c.example/3 https://a.example/1')
    expect(p.total).toBe(4)
    expect(p.unique.length).toBe(3)
  })

  it('超过单次上限时给出溢出条数', () => {
    const many = Array.from({ length: ENQUEUE_LIMIT + 7 }, (_, i) => `https://x.example/${i}`).join('\n')
    const p = parseLinks(many)
    expect(p.unique.length).toBe(ENQUEUE_LIMIT + 7)
    expect(p.overflow).toBe(7)
  })

  it('磁力 / ftp 等非 http 链接同样被识别（识别在前端，入队判定交给后端）', () => {
    const p = parseLinks('magnet:?xt=urn:btih:abc\nftp://a.example/x.iso')
    expect(p.unique).toEqual(['magnet:?xt=urn:btih:abc', 'ftp://a.example/x.iso'])
  })
})

describe('批量导入：超长粘贴保护（BUG-07）', () => {
  it('未超过上限的文本原样返回、不标记截断', () => {
    const r = truncateBatchText('https://a.example/1\nhttps://b.example/2')
    expect(r.truncated).toBe(false)
    expect(r.removedChars).toBe(0)
    expect(r.text).toBe('https://a.example/1\nhttps://b.example/2')
  })

  it('正好等于上限不算截断', () => {
    const text = 'x'.repeat(BATCH_TEXT_MAX_CHARS)
    expect(truncateBatchText(text).truncated).toBe(false)
  })

  it('超过上限：按行边界截断并给出被丢弃的字符数', () => {
    const line = `https://example.com/video_0.mp4\n`
    const text = line.repeat(60_000) // ≈ 2 MB
    const r = truncateBatchText(text)
    expect(r.truncated).toBe(true)
    expect(r.text.length).toBeLessThanOrEqual(BATCH_TEXT_MAX_CHARS)
    expect(r.removedChars).toBe(text.length - r.text.length)
    // 截断后最后一行仍是完整链接（没有被切成半截）
    const lines = r.text.split('\n')
    expect(lines[lines.length - 1]).toBe('https://example.com/video_0.mp4')
  })

  it('整段没有换行时按字符数硬截断', () => {
    const text = 'a'.repeat(BATCH_TEXT_MAX_CHARS + 10)
    const r = truncateBatchText(text)
    expect(r.truncated).toBe(true)
    expect(r.text.length).toBe(BATCH_TEXT_MAX_CHARS)
    expect(r.removedChars).toBe(10)
  })

  it('自定义上限可用于小规模测试', () => {
    const r = truncateBatchText('aa\nbb\ncc\n', 5)
    expect(r.text).toBe('aa\nbb')
    expect(r.truncated).toBe(true)
    expect(r.removedChars).toBe(4)
  })
})

describe('批量导入：分块解析（长输入不阻塞主线程）', () => {
  it('分块解析结果与一次性 parseLinks 完全一致', () => {
    const text = 'https://a.example/1\n#注释\nhttps://b.example/2, https://a.example/1\n\n//注释\nmagnet:?xt=urn:btih:abc'
    const cursor = createParseCursor()
    const seenSteps: number[] = []
    while (!cursor.done) {
      parseChunk(cursor, text, 1) // 每次只走一行，强制走多步
      seenSteps.push(cursor.total)
    }
    const oneShot = parseLinks(text)
    expect(cursor.total).toBe(oneShot.total)
    expect(cursor.unique).toEqual(oneShot.unique)
    expect(seenSteps.length).toBeGreaterThan(1)
  })

  it('每次步进最多处理 maxLines 行（不会一口气算完）', () => {
    const text = Array.from({ length: 10_000 }, (_, i) => `https://x.example/${i}`).join('\n')
    const cursor = createParseCursor()
    parseChunk(cursor, text, 100)
    expect(cursor.done).toBe(false)
    expect(cursor.total).toBe(100)
    expect(cursor.unique.length).toBe(100)
  })

  it('1e6 行 / ≈37 MB：单次步进耗时 ≤ 150ms，总步数 ≥ 400（主线程始终可响应）', () => {
    // ≈ 36.9 MB 的真实复现输入（与 BUG_SWEEP 报告的粘贴体积一致）
    const block = Array.from({ length: 1000 }, (_, i) => `https://example.com/video_${i}.mp4`).join('\n')
    const text = `${block}\n`.repeat(1000)
    expect(text.length).toBeGreaterThan(30 * 1024 * 1024)

    const cursor = createParseCursor()
    let steps = 0
    let maxStepMs = 0
    const t0 = performance.now()
    while (!cursor.done) {
      const s0 = performance.now()
      parseChunk(cursor, text)
      const dt = performance.now() - s0
      if (dt > maxStepMs) maxStepMs = dt
      steps += 1
      if (steps > 10_000) throw new Error('分块解析步数异常：疑似一次性算完')
    }
    const totalMs = performance.now() - t0

    expect(cursor.total).toBe(1_000_000) // 识别到 100 万条
    expect(cursor.unique.length).toBe(1000) // 去重后 1000 条
    expect(steps).toBeGreaterThanOrEqual(400) // 1e6 / PARSE_CHUNK_LINES = 500
    // 阈值放宽到 150ms：本机（Windows）实测远低于此，但 GitHub 的 macOS/共享 runner 会被
    // 限流且缓存冷 —— 曾量到 68.3ms 假红。真正的回归形态是「单步冻结上万毫秒」（BUG_SWEEP 复现
    // 过 10034ms 的整段解析），150ms 相对它仍有 66× 余量，足以守住“主线程可响应”。
    expect(maxStepMs, `单次步进耗时 ${maxStepMs.toFixed(1)}ms`).toBeLessThanOrEqual(150)
    // 分块的首步必然在毫秒级：这就是粘贴 1e6 行时主线程不被冻结的原因
    expect(totalMs).toBeLessThan(5000)
  })
})
