/**
 * 后端错误文案本地化（Whisper 模型 / 工具下载）。
 *
 * 后端（Rust）下载失败时返回带**稳定错误码**的报文：
 *
 *   `[model.dns] hf-mirror.com、huggingface.co :: dns error ← failed to lookup address …`
 *   └ code ───┘ └ 试过的主机 ─────────────┘    └ 原始原因链（给日志/排查用）
 *
 * 前端只认 code 与两个参数（host / detail），完整句子走「中文原文即 key」的四语言包，
 * 所以英文 / 日文 / 法文界面下也能看懂「是 DNS 失败还是代理没开」。
 * 不是这个格式的报文原样返回 —— 绝不吞掉后端信息。
 */
import { tr } from '@/i18n'

export interface ParsedBackendError {
  /** 稳定错误码，如 model.dns / model.http403 */
  code: string
  /** 涉事主机（可能是「a、b」这种多源列表） */
  host: string
  /** 原始原因链，纯技术信息，不进主文案 */
  detail: string
}

/** 解析 `[code] host :: detail`；不是这个格式返回 null */
export function parseBackendError(msg: unknown): ParsedBackendError | null {
  const s = typeof msg === 'string' ? msg.trim() : ''
  if (!s.startsWith('[')) return null
  const end = s.indexOf(']')
  if (end <= 1) return null
  const code = s.slice(1, end).trim()
  if (!/^[a-z][a-z0-9_.]*$/i.test(code)) return null
  const rest = s.slice(end + 1).trim()
  const sep = rest.indexOf(' :: ')
  const host = (sep === -1 ? rest : rest.slice(0, sep)).trim()
  const detail = sep === -1 ? '' : rest.slice(sep + 4).trim()
  return { code, host, detail }
}

type Params = Record<string, unknown>

/**
 * 错误码 → 文案模板（中文原文即 key，四语言包里都有同 key 的译文）。
 * 每条都在 tr(…) 调用里出现，`scripts/check_i18n_catalog.py` 能对上账。
 */
const MODEL_TEMPLATES: Record<string, (p: Params) => string> = {
  'model.dns': (p) => tr('模型下载失败：无法解析下载源地址（{host}）—— 请检查网络或代理设置后重试', p),
  'model.timeout': (p) => tr('模型下载失败：连接下载源超时（{host}）—— 请检查网络或代理设置后重试', p),
  'model.connect': (p) => tr('模型下载失败：无法连接下载源（{host}）—— 请检查网络或代理设置后重试', p),
  'model.proxy': (p) =>
    tr('模型下载失败：代理不可用（{host}）—— 请在设置里检查代理地址，或清空代理改为直连后重试', p),
  'model.tls': (p) => tr('模型下载失败：下载源 TLS 握手失败（{host}）—— 网络可能被干扰，请配置代理后重试', p),
  'model.range': (p) => tr('模型下载失败：断点续传的位置已失效（{host}）—— 已清理残片，请重试', p),
  'model.truncated': (p) =>
    tr('模型下载失败：连接中断（{host}）—— 已保留已下载的部分，重试会从断点继续', p),
  'model.io': (p) => tr('模型下载失败：写入或读取中断（{host}）—— 请检查磁盘空间后重试', p),
  'model.invalid': (p) =>
    tr('模型下载失败：下载内容不是有效的 Whisper 模型（{host}）—— 已清理，请重试或改用自定义模型直链', p),
  'model.sources': (p) =>
    tr('模型下载失败：{host} 都连不上 —— 请检查网络或代理设置后重试（详细原因见日志）', p),
}

/** model.http403 / model.http500 … 共用一条模板，状态码原样展示 */
const HTTP_TEMPLATE = (p: Params) =>
  tr('模型下载失败：下载源返回 HTTP {code}（{host}）—— 稍后重试，或改用自定义模型直链', p)
/** 没见过的错误码：保住原文，别让用户看不到任何原因 */
const OTHER_TEMPLATE = (p: Params) => tr('模型下载失败：{detail}', p)

/** 错误码集合（供单测 / 排查用） */
export const MODEL_ERROR_CODES = Object.keys(MODEL_TEMPLATES)

/**
 * 工具目录 / 工具安装的错误码（后端 tools::validate_tool_dir、migrate_tool_dir 等发出）。
 * 与 model.* 分开一张表：认不出来的 tools.* 不会退化成「模型下载失败」这种错口径的文案。
 */
const TOOL_TEMPLATES: Record<string, (p: Params) => string> = {
  'tools.dir.empty': () => tr('工具目录不能为空 —— 请填写绝对路径（例如 D:\\Umidl\\tools）'),
  'tools.dir.relative': (p) =>
    tr('工具目录必须是绝对路径（{host}）—— 例如 D:\\Umidl\\tools，或点「浏览…」选择目录', p),
  'tools.dir.invalid': (p) =>
    tr('工具目录包含非法字符（{host}）—— 目录名不能出现 < > " | ? * 这几个字符', p),
  'tools.dir.not_dir': (p) => tr('这个路径是一个文件而不是目录（{host}）—— 请选择目录', p),
  'tools.dir.create_failed': (p) => tr('无法创建工具目录（{host}）：{detail}', p),
  'tools.dir.not_writable': (p) =>
    tr('工具目录不可写（{host}）：{detail} —— 请换一个可写目录（避免系统盘受保护目录）', p),
  'tools.dir.nested': (p) =>
    tr('新目录不能位于旧目录内部（{host}）—— 请选择另一个目录，否则复制与删除会互相覆盖', p),
  'tools.dir.copy_failed': (p) => tr('迁移失败：无法复制 {host}（{detail}）—— 旧目录保持原样，没有文件被删除', p),
  'tools.dir.copy_mismatch': (p) =>
    tr('迁移失败：{host} 复制后体积不一致（{detail}）—— 旧目录保持原样，没有文件被删除', p),
  'tools.dir.migrate_failed': (p) =>
    tr('迁移到新目录失败（{host}）：{detail} —— 旧目录仍然生效，工具目录未改动', p),
  'tools.install_failed': (p) => tr('{host} 安装失败：{detail}', p),
  'tools.verify_missing': (p) => tr('{host} 未找到可用的可执行文件（{detail}）', p),
  'tools.unknown': (p) => tr('未知的工具：{host}', p),
}

/** 没见过的 tools.* 错误码：保住原文，别让用户看不到原因 */
const TOOL_OTHER_TEMPLATE = (p: Params) => tr('操作失败：{detail}', p)

/** 工具错误码集合（供单测 / 排查用） */
export const TOOL_ERROR_CODES = Object.keys(TOOL_TEMPLATES)

/** 后端错误码 → 当前界面语言的完整说明；认不出格式就原样返回 */
export function localizeBackendError(e: unknown): string {
  const raw =
    typeof e === 'string'
      ? e
      : e && typeof e === 'object' && 'message' in e
        ? String((e as { message?: unknown }).message ?? '')
        : String(e ?? '')
  const parsed = parseBackendError(raw)
  if (!parsed) return raw
  const { code, host, detail } = parsed

  // 工具目录 / 工具安装：走 tools.* 模板（不与 model.* 混用，避免「模型下载失败」这种错口径）
  if (code.startsWith('tools.')) {
    const toolTpl = TOOL_TEMPLATES[code]
    return toolTpl ? toolTpl({ host, detail }) : TOOL_OTHER_TEMPLATE({ detail: detail || raw })
  }
  if (code.startsWith('model.http')) {
    return HTTP_TEMPLATE({ code: code.slice('model.http'.length), host, detail })
  }
  const tpl = MODEL_TEMPLATES[code]
  if (!tpl) return OTHER_TEMPLATE({ detail: detail || raw })
  return tpl({ host, detail })
}
