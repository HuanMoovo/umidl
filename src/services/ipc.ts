/**
 * Tauri IPC 封装：所有后端调用集中在此，便于测试时 mock。
 */
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type {
  MediaInfo,
  DownloadTask,
  ConvertTask,
  SubtitleTask,
  ToolStatus,
  ToolInstallOutcome,
  ToolDirInfo,
  ToolDirChange,
  WhisperModelInfo,
  AppSettings,
  MediaProbe,
  TestReport,
  TestCase,
  DownloadRequest,
  ConvertRequest,
  SubtitleRequest,
} from '@/types'

/** 判断是否运行在 Tauri 容器内（浏览器预览时为 false） */
export const isTauri = (): boolean =>
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) {
    throw new Error(`[browser] 命令 ${cmd} 需要在桌面客户端中运行`)
  }
  return invoke<T>(cmd, args)
}

/* ------------------------- 工具检测 ------------------------- */
export const detectTools = () => call<ToolStatus[]>('detect_tools')
export const installTool = (name: string) => call<ToolStatus>('install_tool', { name })
/** 批量安装（「下载所选」）：逐个装、互不影响；返回逐条结果 */
export const installTools = (names: string[]) =>
  call<ToolInstallOutcome[]>('install_tools', { names })
/** 校验所选工具：后端绕过版本缓存真的启动一次程序 */
export const verifyTools = (names: string[]) => call<ToolInstallOutcome[]>('verify_tools', { names })
/** 受管工具目录现状（生效目录 / 默认目录 / 是否自定义） */
export const toolDirInfo = () => call<ToolDirInfo>('tool_dir_info')
/** 更换工具目录；migrate = true 时先复制校验、成功才删旧目录 */
export const setToolDir = (dir: string, migrate: boolean) =>
  call<ToolDirChange>('set_tool_dir', { dir, migrate })
export const installWhisperModel = (model: string) =>
  call<ToolStatus>('install_whisper_model', { model })
/** 自定义模型：http(s) 链接 / 本地 .bin 路径 / 官方仓库文件名 */
export const installWhisperCustom = (spec: string) =>
  call<ToolStatus>('install_whisper_custom', { spec })
export const listWhisperModels = () => call<WhisperModelInfo[]>('list_whisper_models')
export const deleteWhisperModel = (name: string) => call<void>('delete_whisper_model', { name })
/** 窗口主题：light/dark 强制窗口，system 交还系统（昼夜跟随才能真正生效） */
export const setWindowTheme = (mode: string) => call<void>('set_window_theme', { mode })

/** 自定义 LOGO：后端会复制图片到应用数据目录并落盘设置，返回保存后的路径 */
export const setCustomLogo = (path: string) => call<string>('set_custom_logo', { path })
export const clearCustomLogo = () => call<void>('clear_custom_logo')

/* ------------------------- 下载 ------------------------- */
export const probeUrl = (url: string, playlist = false) =>
  call<MediaInfo>('probe_url', { url, playlist })
export const startDownload = (req: DownloadRequest) =>
  call<DownloadTask>('start_download', { req })
export const listDownloads = () => call<DownloadTask[]>('list_downloads')
/** 给历史任务补封面（从本地视频抽帧），返回补齐条数 */
export const backfillCovers = () => call<number>('backfill_covers')
export const pauseDownload = (id: string) => call<void>('pause_download', { id })
export const resumeDownload = (id: string) => call<void>('resume_download', { id })
export const cancelDownload = (id: string) => call<void>('cancel_download', { id })
export const removeDownload = (id: string, deleteFile = false) =>
  call<void>('remove_download', { id, deleteFile })
export const clearDownloads = (which: 'done' | 'all') => call<void>('clear_downloads', { which })

/* ------------------------- 转换 ------------------------- */
export const probeMedia = (path: string) => call<MediaProbe>('probe_media', { path })
export const startConvert = (req: ConvertRequest) => call<ConvertTask>('start_convert', { req })
export const listConverts = () => call<ConvertTask[]>('list_converts')
export const cancelConvert = (id: string) => call<void>('cancel_convert', { id })
export const removeConvert = (id: string) => call<void>('remove_convert', { id })

/* ------------------------- 字幕 ------------------------- */
export const startSubtitle = (req: SubtitleRequest) => call<SubtitleTask>('start_subtitle', { req })
export const listSubtitles = () => call<SubtitleTask[]>('list_subtitles')
export const cancelSubtitle = (id: string) => call<void>('cancel_subtitle', { id })
export const removeSubtitle = (id: string) => call<void>('remove_subtitle', { id })
export const readSubtitle = (path: string) => call<string>('read_text_file', { path })

/* ------------------------- 设置 / 系统 ------------------------- */
export const getSettings = () => call<AppSettings>('get_settings')
export const saveSettings = (settings: AppSettings) => call<AppSettings>('save_settings', { settings })
export const defaultDownloadDir = () => call<string>('default_download_dir')
export const appDataDir = () => call<string>('app_data_dir')
export const openPath = (path: string) => call<void>('open_path', { path })
export const revealPath = (path: string) => call<void>('reveal_path', { path })
export const appVersion = () => call<string>('app_version')

/* ------------------------- 自检 ------------------------- */
export const runSelftest = (deep: boolean) => call<TestReport>('run_selftest', { deep })
export const listTestCases = () => call<TestCase[]>('list_test_cases')

/* ------------------------- 事件 ------------------------- */
type EventMap = {
  'download://update': DownloadTask
  'download://removed': { id?: string; all?: boolean }
  'convert://update': ConvertTask
  'subtitle://update': SubtitleTask
  'selftest://update': TestCase
  'tool://progress': {
    name: string
    percent: number
    message: string
    /** 实际下载源主机名（模型/工具下载才有） */
    source?: string
    /** 已下载 / 总字节数（模型/工具下载才有，用于本地化进度文案） */
    downloaded?: number
    total?: number
    /** 本次是不是断点续传 */
    resumed?: boolean
  }
}

export function onEvent<K extends keyof EventMap>(
  event: K,
  handler: (payload: EventMap[K]) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return Promise.resolve(() => {})
  return listen<EventMap[K]>(event, (e) => handler(e.payload))
}
