/**
 * 字幕产物路径推导单测（对齐 Rust `subtitle.rs::fix18_tests` 的命名规则）
 *
 * 锁死的事：
 *   1. 原文 `<基名>.<源语言标签>.<格式>` / 译文 `<基名>.<源语言标签>-to-<目标语言>.<格式>` 的拼法；
 *   2. `auto` / 空值 → `detected`；`en` / `English` → `en`；脏值走文件名安全标签；
 *   3. 队列行的产物路径：非译文行优先用记录里那份、译文行永远以 `-to-en` 那份为准
 *      （旧记录把原文文件记在译文行上，直接信它就会「在文件夹中显示」到原文文件）。
 */
import { describe, expect, it } from 'vitest'
import {
  dirname,
  expectedProductPath,
  joinPath,
  languageTag,
  productExt,
  productTag,
  rowProductPath,
  sanitizeTag,
  translateTag,
  videoStem,
} from '@/services/subtitleProducts'

describe('字幕产物命名（与 Rust subtitle.rs 同一套规则）', () => {
  it('源语言标签：auto / 空值 → detected，其余原样（脏值走安全标签）', () => {
    expect(languageTag('auto')).toBe('detected')
    expect(languageTag('AUTO')).toBe('detected')
    expect(languageTag('')).toBe('detected')
    expect(languageTag('   ')).toBe('detected')
    expect(languageTag('zh')).toBe('zh')
    expect(languageTag('en')).toBe('en')
    expect(languageTag('zh-CN')).toBe('zh-CN')
    expect(languageTag('zh/../x')).toBe('zh_.._x')
    expect(languageTag('中文')).toBe('detected') // 非 ASCII 的标签字符会被安全标签清掉 → 回落 detected
    expect(sanitizeTag('')).toBe('detected')
    expect(sanitizeTag('...')).toBe('detected')
  })

  it('译文标签：只有 en / english 才算，没开译文是 null', () => {
    expect(translateTag(null)).toBeNull()
    expect(translateTag(undefined)).toBeNull()
    expect(translateTag('')).toBeNull()
    expect(translateTag('  ')).toBeNull()
    expect(translateTag('en')).toBe('en')
    expect(translateTag('English')).toBe('en')
    expect(translateTag('  en  ')).toBe('en')
    expect(translateTag('ja')).toBe('ja')
  })

  it('产物标签：原文 `<语言>`、译文 `<语言>-to-<目标>`（两份永不共名）', () => {
    expect(productTag('auto', null)).toBe('detected')
    expect(productTag('zh', null)).toBe('zh')
    expect(productTag('auto', 'en')).toBe('detected-to-en')
    expect(productTag('zh', 'en')).toBe('zh-to-en')
    // 原文语言就是 en 也不撞：`.en` vs `.en-to-en`
    expect(productTag('en', 'en')).toBe('en-to-en')
    expect(productTag('AUTO', 'English')).toBe('detected-to-en')
    expect(productTag('zh', '  en  ')).toBe('zh-to-en')
  })

  it('期望产物路径：`<视频目录>/<基名>.<标签>.<格式>`，空间 / 中文 / 引号都原样保留', () => {
    expect(expectedProductPath(String.raw`D:\Videos\clip.mp4`, 'zh')).toBe(String.raw`D:\Videos\clip.zh.srt`)
    expect(expectedProductPath(String.raw`D:\Videos\clip12s.mp4`, 'auto')).toBe(
      String.raw`D:\Videos\clip12s.detected.srt`,
    )
    expect(expectedProductPath(String.raw`D:\Videos\clip.mp4`, 'auto', 'en')).toBe(
      String.raw`D:\Videos\clip.detected-to-en.srt`,
    )
    // 目录名、文件名带空格 / 中文 / 单引号 / 方括号：绝不能再被拼坏
    const dir = String.raw`D:\Media Files\Videos\Umidl`
    const video = `${dir}\\Sample '샘플 영상' Official Special Clip [AbCdEfGhIjK].mp4`
    expect(expectedProductPath(video, 'auto')).toBe(
      `${dir}\\Sample '샘플 영상' Official Special Clip [AbCdEfGhIjK].detected.srt`,
    )
    expect(expectedProductPath(video, 'auto', 'en')).toBe(
      `${dir}\\Sample '샘플 영상' Official Special Clip [AbCdEfGhIjK].detected-to-en.srt`,
    )
    // 扩展名可换（vtt / ass…）
    expect(expectedProductPath(String.raw`D:\V\a.mp4`, 'zh', null, 'vtt')).toBe(String.raw`D:\V\a.zh.vtt`)
    // 没目录（相对路径）也能拼
    expect(expectedProductPath('a.mp4', 'zh')).toBe(String.raw`a.zh.srt`)
  })

  it('路径零件：dirname / videoStem / productExt 与后端同口径', () => {
    expect(dirname(String.raw`D:\a\b\c.srt`)).toBe(String.raw`D:\a\b`)
    expect(dirname(String.raw`D:\a\b\c.srt`.replace(/\\/g, '/'))).toBe('D:/a/b')
    expect(dirname('c.srt')).toBe('')
    expect(dirname(String.raw`D:\c.srt`)).toBe('D:')
    expect(joinPath('', 'a.srt')).toBe('a.srt')
    expect(joinPath(String.raw`D:\a`, 'b.srt')).toBe(String.raw`D:\a\b.srt`)
    expect(joinPath('D:/a/', 'b.srt')).toBe('D:/a/b.srt')
    expect(videoStem(String.raw`D:\a\clip12s.mp4`)).toBe('clip12s')
    expect(videoStem(String.raw`D:\a\a.b.mp4`)).toBe('a.b')
    expect(videoStem(String.raw`D:\a\.hidden`)).toBe('.hidden')
    expect(productExt(String.raw`D:\a\x.detected-to-en.vtt`)).toBe('vtt')
    expect(productExt(String.raw`D:\a\x.srt`)).toBe('srt')
    expect(productExt(null)).toBe('srt')
    expect(productExt(String.raw`D:\a\x`)).toBe('srt')
  })
})

describe('队列行的产物路径（「打开文件 / 在文件夹中显示」的目标）', () => {
  const DIR = String.raw`D:\Media Files\Videos\Umidl`

  it('普通行：用记录里那份（历史目录 / output_dir 都保留）', () => {
    expect(
      rowProductPath({
        video_path: `${DIR}\\clip.mp4`,
        language: 'auto',
        subtitle_path: `${DIR}\\clip.detected.srt`,
      }),
    ).toBe(`${DIR}\\clip.detected.srt`)
    // 记录里的产物在别的目录（历史上给过 output_dir）→ 仍然以记录为准
    expect(
      rowProductPath({
        video_path: String.raw`D:\Projects\umi-downloader\.tmp\subfix\manifest.wav`,
        language: 'zh',
        subtitle_path: String.raw`C:\Users\user\AppData\Roaming\umi-downloader\verify18\manifest.zh.srt`,
      }),
    ).toBe(String.raw`C:\Users\user\AppData\Roaming\umi-downloader\verify18\manifest.zh.srt`)
  })

  it('译文行：永远指向自己那份 `<基名>.<源语言>-to-en.<扩展名>`（哪怕记录里写的是原文文件）', () => {
    // 1.8.6 之前的真实记录：译文行的 subtitle_path 指着原文那份 → 必须纠正成 -to-en
    const stale = {
      video_path: `${DIR}\\「特级咒术师」乙骨初华 [BV17Gaa6TEvf].mp4`,
      language: 'auto',
      subtitle_path: `${DIR}\\「特级咒术师」乙骨初华 [BV17Gaa6TEvf].detected.srt`,
      translated: true,
    }
    expect(rowProductPath(stale)).toBe(`${DIR}\\「特级咒术师」乙骨初华 [BV17Gaa6TEvf].detected-to-en.srt`)
    expect(rowProductPath(stale)).not.toBe(stale.subtitle_path)

    // 记录已经是 -to-en（新记录）：推导结果与记录一致，不做多余改动
    const fresh = { ...stale, subtitle_path: `${DIR}\\「特级咒术师」乙骨初华 [BV17Gaa6TEvf].detected-to-en.srt` }
    expect(rowProductPath(fresh)).toBe(fresh.subtitle_path)

    // 记录的扩展名是 vtt → 推导也推 vtt（别把格式吞掉）
    expect(
      rowProductPath({
        video_path: `${DIR}\\clip.mp4`,
        language: 'zh',
        subtitle_path: `${DIR}\\clip.zh.vtt`,
        translated: true,
      }),
    ).toBe(`${DIR}\\clip.zh-to-en.vtt`)
    // 记录目录与视频目录不同（历史上给过 output_dir）→ 译文也落在记录那个目录里
    expect(
      rowProductPath({
        video_path: String.raw`D:\v\clip.mp4`,
        language: 'zh',
        subtitle_path: String.raw`E:\out\clip.zh.srt`,
        translated: true,
      }),
    ).toBe(String.raw`E:\out\clip.zh-to-en.srt`)
  })

  it('没有记录路径时按命名规则推导（新任务刚入队 / 记录被清过）', () => {
    expect(rowProductPath({ video_path: `${DIR}\\clip.mp4`, language: 'auto', subtitle_path: null })).toBe(
      `${DIR}\\clip.detected.srt`,
    )
    expect(
      rowProductPath({ video_path: `${DIR}\\clip.mp4`, language: 'zh', subtitle_path: null, translated: true }),
    ).toBe(`${DIR}\\clip.zh-to-en.srt`)
  })

  it('原文行与译文行永远不是同一个文件（回归：两份产物共名，后写的覆盖先写的）', () => {
    const video = `${DIR}\\song.mp4`
    const orig = rowProductPath({ video_path: video, language: 'auto', subtitle_path: null })
    const tr = rowProductPath({ video_path: video, language: 'auto', subtitle_path: null, translated: true })
    expect(orig).toBe(`${DIR}\\song.detected.srt`)
    expect(tr).toBe(`${DIR}\\song.detected-to-en.srt`)
    expect(orig).not.toBe(tr)
  })
})
