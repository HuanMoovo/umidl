/**
 * Umidl —— 前后端共享类型定义
 * 字段命名与 Rust 端 serde 序列化保持一致（snake_case）
 */

export type TaskStatus =
  | 'pending'
  | 'parsing'
  | 'downloading'
  | 'paused'
  | 'converting'
  | 'extracting'
  | 'transcribing'
  | 'done'
  | 'error'
  | 'canceled'

/** 下载队列的展示布局：card = 卡片流（默认），table = 详细表格 */
export type QueueLayout = 'card' | 'table'

export interface VideoFormat {
  format_id: string
  ext: string
  resolution: string
  fps?: number | null
  vcodec?: string | null
  acodec?: string | null
  filesize?: number | null
  filesize_text?: string | null
  note?: string | null
  /** 是否包含视频轨 */
  has_video: boolean
  /** 是否包含音频轨 */
  has_audio: boolean
}

export interface SubtitleTrack {
  lang: string
  name?: string | null
  ext: string
  /** 平台自动生成的字幕 */
  auto: boolean
  url?: string | null
}

export interface MediaInfo {
  id: string
  title: string
  /** 远程封面地址 */
  thumbnail?: string | null
  /** 已缓存到本地的封面路径（离线可显示 / 可直接另存） */
  thumbnail_local?: string | null
  thumbnail_width?: number | null
  thumbnail_height?: number | null
  uploader?: string | null
  duration?: number | null
  webpage_url: string
  extractor?: string | null
  description?: string | null
  filesize_approx?: number | null
  formats: VideoFormat[]
  /** 字幕语言列表（含自动字幕） */
  subtitles: string[]
  /** 字幕轨道明细 */
  subtitle_tracks: SubtitleTrack[]
  is_playlist: boolean
  playlist_count?: number | null
  view_count?: number | null
  like_count?: number | null
  upload_date?: string | null
  probe_ms?: number | null
}

export interface DownloadTask {
  id: string
  title: string
  url: string
  thumbnail?: string | null
  duration?: number | null
  uploader?: string | null
  status: TaskStatus
  progress: number
  speed?: string | null
  eta?: string | null
  downloaded?: number | null
  total?: number | null
  file_path?: string | null
  error?: string | null
  format_note?: string | null
  created_time: number
  /** 运行时标注：文件是否还在磁盘上 */
  file_exists?: boolean
}

export interface WhisperModelInfo {
  name: string
  file: string
  builtin: boolean
  downloaded: boolean
  size_bytes: number
  size_hint: string
  quality: string
  current: boolean
}

export interface ConvertTask {
  id: string
  input_file: string
  output_file: string
  format: string
  codec?: string | null
  status: TaskStatus
  progress: number
  error?: string | null
  created_time: number
}

export interface SubtitleTask {
  id: string
  video_path: string
  language: string
  model: string
  subtitle_path?: string | null
  /** 「额外生成一份英文字幕」的译文目标语言（后端回填，重启后仍能标出「→ 英文」） */
  translate_to?: string | null
  status: TaskStatus
  progress: number
  error?: string | null
  created_time: number
}

/** 受管工具 / 模型的下载进度（tool://progress 事件） */
export interface ToolProgress {
  percent: number
  message: string
  /** 实际使用的下载源主机名（模型/工具下载才有） */
  source?: string
  /** 已下载 / 总字节数 */
  downloaded?: number
  total?: number
  /** 本次是不是断点续传 */
  resumed?: boolean
}

export interface ToolStatus {
  name: string
  found: boolean
  path?: string | null
  version?: string | null
  source: 'bundled' | 'system' | 'managed' | 'missing'
  managed_path?: string | null
  hint: string
  size_hint?: string | null
  /** 实际使用的下载源主机名（一键下载成功时后端回填，用于提示「走的哪个源」） */
  origin?: string | null
  /** 本平台是否可自动下载安装（false = 无官方静态包，不显示「下载」按钮） */
  installable?: boolean
}

/** 单个工具的安装 / 校验结果（批量「下载所选」逐条返回，一条失败不影响其它） */
export interface ToolInstallOutcome {
  name: string
  ok: boolean
  status?: ToolStatus | null
  /** 失败原因（带稳定错误码，可本地化） */
  error?: string | null
}

/** 受管工具目录现状（设置页回显「生效目录」） */
export interface ToolDirInfo {
  /** 生效目录（绝对路径） */
  dir: string
  /** 默认目录（数据目录下的 bin/） */
  default_dir: string
  /** 设置里写的原始值（空 = 使用默认目录） */
  configured: string
  custom: boolean
  /** 配置值是否合法（非法时后端已回退默认目录） */
  valid: boolean
  error?: string | null
}

/** 更换工具目录的结果（含迁移报告） */
export interface ToolDirChange {
  info: ToolDirInfo
  previous_dir: string
  migrated: boolean
  files_copied: number
  bytes_copied: number
  already_present: number
  removed_old: number
  leftovers: string[]
  skipped: string[]
}

export interface AppSettings {
  download_dir: string
  ytdlp_path?: string | null
  ffmpeg_path?: string | null
  whisper_path?: string | null
  whisper_model: string
  /** dark | light（「跟随系统」已移除：历史 'system' 值读取时回退 dark） */
  theme: 'dark' | 'light'
  /** 强调色 */
  accent: string
  /** 界面语言：zh | en | ja | fr */
  ui_language?: string
  /** 自定义 LOGO 图片路径（空 = 内置） */
  custom_logo?: string | null
  concurrency: number
  proxy?: string | null
  cookies_file?: string | null
  keep_original: boolean
  /* 下载默认值 */
  default_container: string
  default_audio_format: string
  default_quality: string
  filename_template?: string | null
  rate_limit?: string | null
  auto_start: boolean
  open_folder_when_done: boolean
  notify_on_finish: boolean
  /* 转换默认值 */
  default_resolution: string
  default_crf: number
  auto_detect_format: boolean
  hwaccel: boolean
  /* 字幕默认值 */
  subtitle_language: string
  subtitle_format: string
  /* 界面 / 性能 */
  animation: boolean
  animation_quality: 'low' | 'medium' | 'high' | string
  /** 背景粒子形状：circle 圆形 | diamond 棱形 | square 正方形 */
  particle_shape?: 'circle' | 'diamond' | 'square' | string
  compact_cards: boolean
  /** 下载队列布局：card 卡片流 | table 详细表格（默认 card；后端结构体暂未声明该字段，前端本地兜底持久化） */
  queue_layout?: QueueLayout

  /* ── 1.4 新增（与后端 AppSettings 字段一一对应） ── */
  /** 下载引擎：auto 自动路由 | ytdlp 站点解析 | aria2 分段并行 */
  engine?: 'auto' | 'ytdlp' | 'aria2' | string
  /** 分P / 合集：single 单个视频 | playlist 整个合集 */
  playlist_mode?: 'single' | 'playlist' | string
  /** 全局限速（令牌桶）：开关 + 上限 KB/s，0 表示不限 */
  speed_limit_enabled?: boolean
  speed_limit_kb?: number
  /** 代理模式：system | none | custom */
  proxy_mode?: string
  /** 系统集成：开机自启动 / 启动时检查更新 */
  launch_at_login?: boolean
  check_update_on_start?: boolean
  /** 自定义强调色（hex，空 = 用预设） */
  accent_custom?: string

  /* ── 1.6 新增：多线程与并发（设置页「下载引擎 → 多线程与并发」卡片） ── */
  /** aria2 单服务器连接数（1-16，默认 16） */
  aria2_connections?: number
  /** aria2 分段数（1-16，默认 16） */
  aria2_split?: number
  /** 最小分片 MB（1-64，默认 1；小于 1 MB 时 aria2 会拒启） */
  aria2_min_split_mb?: number
  /** yt-dlp 分片并发（HLS / DASH，1-16，默认 1） */
  ytdlp_concurrency?: number

  /* ── 1.9 新增：受管工具目录 ── */
  /** 受管工具安装目录（绝对路径；空 = 数据目录下的 bin/）。所有工具解析/调用只认它 */
  tool_dir?: string

  /* ── 1.10 新增：界面模式 / 外观风格 / 自定义渐变 ── */
  /** 界面模式：simple 简单（默认，只显示常用项） | advanced 高级（全部可见） */
  ui_mode?: UiMode | string
  /** 外观风格：glass 玻璃拟态（默认） | mono 黑白简约 */
  appearance_style?: AppearanceStyle | string
  /** 自定义渐变强调色 "起色,止色"（两个十六进制色；空 = 不用渐变） */
  accent_gradient?: string
}

/** 界面模式（1.10）：简单只显示常用项，高级可见全部 */
export type UiMode = 'simple' | 'advanced'

/** 外观风格（1.10）：glass = 玻璃拟态（现状），mono = 黑白简约 */
export type AppearanceStyle = 'glass' | 'mono'

export interface MediaProbe {
  path: string
  format_name?: string | null
  duration?: number | null
  size?: number | null
  bit_rate?: number | null
  video_codec?: string | null
  audio_codec?: string | null
  width?: number | null
  height?: number | null
  fps?: number | null
}

export interface TestCase {
  id: string
  group: string
  name: string
  status: 'pending' | 'running' | 'pass' | 'fail' | 'skip'
  detail?: string | null
  duration_ms?: number | null
}

export interface TestReport {
  started_at: number
  finished_at: number
  total: number
  passed: number
  failed: number
  skipped: number
  cases: TestCase[]
}

export interface DownloadRequest {
  url: string
  format_id?: string | null
  /** video | audio | best | thumb（封面） | subs（字幕） */
  mode: 'video' | 'audio' | 'best' | 'thumb' | 'subs'
  output_dir?: string | null
  audio_format?: string | null
  merge_container?: string | null
  embed_thumbnail: boolean
  embed_subs: boolean
  filename_template?: string | null
  playlist: boolean
  rate_limit?: string | null
  /** 单独下载字幕时的语言 */
  subtitle_langs?: string[]
  save_thumbnail?: boolean
  thumbnail_url?: string | null
  title_hint?: string | null
}

export interface ConvertRequest {
  input_file: string
  output_dir?: string | null
  format: string
  video_codec?: string | null
  audio_codec?: string | null
  bitrate?: string | null
  resolution?: string | null
  crf?: number | null
  extract_audio: boolean
  mute: boolean
  audio_bitrate?: string | null
  sample_rate?: number | null
  channels?: number | null
  fps?: number | null
  speed?: number | null
  hwaccel?: boolean
  faststart?: boolean
  remove_metadata?: boolean
  auto_format?: string | null
}

export interface SubtitleRequest {
  video_path: string
  language: string
  model?: string | null
  translate_to?: string | null
  output_format: string
  output_dir?: string | null
}
