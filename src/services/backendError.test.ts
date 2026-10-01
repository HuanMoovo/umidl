/**
 * 后端下载错误码 → 四语言文案（Whisper 模型下载修复）。
 *
 * 契约：后端报文 `[code] host :: detail`；前端按 code 选模板、用 host/detail 渲染，
 * 认不出格式时原样返回（绝不吞掉后端信息）。
 */
import { describe, expect, it } from 'vitest'
import { MODEL_ERROR_CODES, localizeBackendError, parseBackendError } from '@/services/backendError'
import { currentLocale, localeMessages, setLocale } from '@/i18n'

const ZH_KEYS = localeMessages.zh

describe('后端错误解析', () => {
  it('解析 [code] host :: detail', () => {
    const p = parseBackendError(
      '[model.dns] hf-mirror.com、huggingface.co :: dns error ← failed to lookup address information',
    )
    expect(p).toEqual({
      code: 'model.dns',
      host: 'hf-mirror.com、huggingface.co',
      detail: 'dns error ← failed to lookup address information',
    })
  })

  it('没有 :: 时 host 后面就是空 detail', () => {
    expect(parseBackendError('[model.timeout] huggingface.co')).toEqual({
      code: 'model.timeout',
      host: 'huggingface.co',
      detail: '',
    })
  })

  it('不是这个格式的报文返回 null（老后端 / 其它模块的报错不能被误读）', () => {
    expect(parseBackendError('缺少 Whisper 模型「base」')).toBeNull()
    expect(parseBackendError('')).toBeNull()
    expect(parseBackendError('[] 空码')).toBeNull()
    expect(parseBackendError('[有 空格 的码] x')).toBeNull()
    expect(parseBackendError(undefined as unknown)).toBeNull()
    expect(parseBackendError(42 as unknown)).toBeNull()
  })
})

describe('错误码 → 本地化文案', () => {
  it('DNS / 超时 / 连不上 / 代理 / TLS 各自给出可行动的说明（含主机名）', () => {
    const dns = localizeBackendError('[model.dns] huggingface.co :: dns error')
    expect(dns).toContain('huggingface.co')
    expect(dns).toContain('无法解析下载源地址')
    expect(dns).toContain('代理')

    expect(localizeBackendError('[model.timeout] hf-mirror.com :: timed out')).toContain('超时')
    expect(localizeBackendError('[model.connect] hf-mirror.com :: refused')).toContain('无法连接')
    const proxy = localizeBackendError('[model.proxy] 127.0.0.1:7892 :: tunnel error')
    expect(proxy).toContain('代理不可用')
    expect(proxy).toContain('设置')
    expect(localizeBackendError('[model.tls] huggingface.co :: invalid peer certificate')).toContain('TLS')
  })

  it('HTTP 状态码按原样带出（403 / 404 / 503 区分得开）', () => {
    expect(localizeBackendError('[model.http403] huggingface.co :: HTTP 403 Forbidden')).toContain('HTTP 403')
    expect(localizeBackendError('[model.http404] hf-mirror.com :: HTTP 404 Not Found')).toContain('HTTP 404')
    expect(localizeBackendError('[model.http503] hf-mirror.com :: HTTP 503')).toContain('HTTP 503')
  })

  it('校验失败 / 所有源都失败 / 续传失效各有专门文案', () => {
    const invalid = localizeBackendError('[model.invalid] hf-mirror.com :: 文件头不是 ggml 模型魔数')
    expect(invalid).toContain('不是有效的 Whisper 模型')
    const sources = localizeBackendError('[model.sources] hf-mirror.com、huggingface.co :: a ← b')
    expect(sources).toContain('hf-mirror.com、huggingface.co')
    expect(sources).toContain('都连不上')
    expect(localizeBackendError('[model.range] hf-mirror.com :: HTTP 416')).toContain('断点续传')
  })

  it('没见过的错误码也要显示原因（不吞信息）', () => {
    const msg = localizeBackendError('[model.weird] x 主机 :: 磁盘写满了')
    expect(msg).toContain('磁盘写满了')
  })

  it('非本格式的报错原样返回', () => {
    expect(localizeBackendError('不支持的模型：foo（自定义模型请用「自定义导入」）')).toBe(
      '不支持的模型：foo（自定义模型请用「自定义导入」）',
    )
    expect(localizeBackendError(new Error('缺少 Whisper 模型「base」'))).toBe('缺少 Whisper 模型「base」')
  })
})

describe('四语言覆盖（新增文案不许漏译）', () => {
  /** 本次新增的全部文案 key（中文原文即 key） */
  const NEW_KEYS = [
    '模型下载失败：无法解析下载源地址（{host}）—— 请检查网络或代理设置后重试',
    '模型下载失败：连接下载源超时（{host}）—— 请检查网络或代理设置后重试',
    '模型下载失败：无法连接下载源（{host}）—— 请检查网络或代理设置后重试',
    '模型下载失败：代理不可用（{host}）—— 请在设置里检查代理地址，或清空代理改为直连后重试',
    '模型下载失败：下载源 TLS 握手失败（{host}）—— 网络可能被干扰，请配置代理后重试',
    '模型下载失败：断点续传的位置已失效（{host}）—— 已清理残片，请重试',
    '模型下载失败：连接中断（{host}）—— 已保留已下载的部分，重试会从断点继续',
    '模型下载失败：写入或读取中断（{host}）—— 请检查磁盘空间后重试',
    '模型下载失败：下载内容不是有效的 Whisper 模型（{host}）—— 已清理，请重试或改用自定义模型直链',
    '模型下载失败：{host} 都连不上 —— 请检查网络或代理设置后重试（详细原因见日志）',
    '模型下载失败：下载源返回 HTTP {code}（{host}）—— 稍后重试，或改用自定义模型直链',
    '模型下载失败：{detail}',
    '正在从 {source} 下载 {done} / {total}',
    '继续上次未完成的下载：{done} / {total}（{source}）',
    '{name} 模型已下载并启用（来源：{source}）',
  ]

  it('每个错误码都能选到一个模板（模板数与错误码数对得上）', () => {
    for (const code of MODEL_ERROR_CODES) {
      expect(localizeBackendError(`[${code}] host :: detail`)).toContain('模型下载失败')
    }
    expect(MODEL_ERROR_CODES.length).toBe(10)
  })

  it('全部新增 key 在四种语言包里都有译文，且不是中文原文', () => {
    const zhKeys = Object.keys(ZH_KEYS)
    for (const k of NEW_KEYS) {
      expect(zhKeys, `中文语言包缺 key：${k}`).toContain(k)
      for (const lang of ['en', 'ja', 'fr'] as const) {
        const v = (localeMessages[lang] as Record<string, string>)[k]
        expect(v, `${lang} 缺 ${k}`).toBeTruthy()
        expect(v, `${lang} 的 ${k} 还是中文原文`).not.toBe(k)
      }
    }
    // 错误码模板 12 条 + 进度/成功文案 3 条
    expect(NEW_KEYS.length).toBe(15)
  })

  it('真的会跟着界面语言切换（en / fr 下不是中文）', () => {
    const prev = currentLocale()
    setLocale('zh')
    const zh = localizeBackendError('[model.dns] huggingface.co :: dns error')
    setLocale('en')
    const en = localizeBackendError('[model.dns] huggingface.co :: dns error')
    setLocale('fr')
    const fr = localizeBackendError('[model.sources] a、b :: x')
    setLocale(prev)
    expect(zh).toContain('无法解析下载源地址')
    expect(en).toContain('cannot resolve')
    expect(en).not.toContain('无法解析')
    expect(fr).toContain('aucun de')
  })
})
