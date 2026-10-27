import { invoke } from '@tauri-apps/api/core'
import * as ipc from './ipc'
import type { SubtitleRequest, SubtitleTask } from '@/types'
import type { SubtitleOptions } from './whisper'

export type LocalAsrEngine = 'sensevoice' | 'paraformer'
export type AsrEngine = LocalAsrEngine | 'cloud'
export type CloudProvider = 'openai' | 'groq' | 'fish' | 'deepgram' | 'assemblyai' | 'azure' | 'tencent' | 'baidu' | 'xunfei' | 'compatible'
export type CloudCredentialField = 'api_key' | 'app_id' | 'secret_id' | 'secret_key'
export interface CloudProviderPreset {
  label: string
  baseUrl: string
  model: string
  credentials: CloudCredentialField[]
  editableUrl?: boolean
  showModel?: boolean
}
export const CLOUD_PROVIDERS: Record<CloudProvider, CloudProviderPreset> = {
  groq: { label: 'Groq', baseUrl: 'https://api.groq.com/openai/v1', model: 'whisper-large-v3-turbo', credentials: ['api_key'], showModel: true },
  openai: { label: 'OpenAI', baseUrl: 'https://api.openai.com/v1', model: 'whisper-1', credentials: ['api_key'], showModel: true },
  fish: { label: 'Fish Audio', baseUrl: 'https://api.fish.audio', model: 'transcribe-1-pro', credentials: ['api_key'], showModel: true },
  deepgram: { label: 'Deepgram', baseUrl: 'https://api.deepgram.com', model: 'nova-3', credentials: ['api_key'], showModel: true },
  assemblyai: { label: 'AssemblyAI', baseUrl: 'https://api.assemblyai.com', model: 'universal-2', credentials: ['api_key'], showModel: true },
  azure: { label: 'Azure Speech', baseUrl: '', model: 'fast-transcription', credentials: ['api_key'], editableUrl: true },
  tencent: { label: '腾讯云', baseUrl: 'https://asr.cloud.tencent.com', model: '16k_zh', credentials: ['app_id', 'secret_id', 'secret_key'] },
  baidu: { label: '百度智能云', baseUrl: 'https://aip.baidubce.com', model: '80006', credentials: ['api_key', 'secret_key'] },
  xunfei: { label: '讯飞开放平台', baseUrl: 'https://office-api-ist-dx.iflyaisol.com', model: 'autodialect', credentials: ['app_id', 'api_key', 'secret_key'] },
  compatible: { label: '自定义兼容', baseUrl: 'https://api.openai.com/v1', model: 'whisper-1', credentials: ['api_key'], editableUrl: true, showModel: true },
}

/** Old queue entries are cloud:<model>; new entries are cloud:<provider>:<model>. */
export function parseCloudTaskModel(taskModel: string): { provider: CloudProvider; model: string } {
  const saved = taskModel.startsWith('cloud:') ? taskModel.slice('cloud:'.length) : taskModel
  const divider = saved.indexOf(':')
  const candidate = divider > 0 ? saved.slice(0, divider) : ''
  if (Object.prototype.hasOwnProperty.call(CLOUD_PROVIDERS, candidate)) {
    return { provider: candidate as CloudProvider, model: saved.slice(divider + 1) }
  }
  return { provider: 'compatible', model: saved }
}

/** The values are sent as common language hints; provider adapters map their own codes. */
export function cloudAllowedLanguages(provider: CloudProvider, model: string): readonly string[] | null {
  if (provider === 'tencent') return ['zh', 'en', 'ja', 'ko', 'yue']
  if (provider === 'baidu') return ['zh']
  if (provider === 'xunfei') return ['auto']
  if (provider === 'deepgram' || provider === 'azure') return ['auto', 'zh', 'en', 'ja', 'ko', 'fr', 'de', 'es', 'ru', 'yue']
  if (provider === 'assemblyai' && model.trim().startsWith('universal-3-pro')) return ['auto', 'en', 'fr', 'de', 'es']
  return null
}
export interface AsrEngineStatus {
  engine: LocalAsrEngine
  runtime_ready: boolean
  model_ready: boolean
  ready: boolean
  message: string
  model_dir: string
}
export interface CloudAsrConfig {
  provider?: CloudProvider
  base_url: string
  model: string
  api_key: string
  app_id?: string
  secret_id?: string
  secret_key?: string
}
export interface AsrOptions extends SubtitleOptions {
  engine: AsrEngine
  cloud?: CloudAsrConfig
}

async function call<T>(command: string, args: Record<string, unknown>): Promise<T> {
  if (!ipc.isTauri()) throw new Error(`[browser] 命令 ${command} 需要在桌面客户端中运行`)
  return invoke<T>(command, args)
}

export function cloudConfigValid(config: CloudAsrConfig): boolean {
  const provider = config.provider ?? 'compatible'
  const preset = CLOUD_PROVIDERS[provider]
  if (!preset) return false
  if (!config.model.trim()) return false
  try {
    const rawUrl = config.base_url.trim()
    const url = new URL(rawUrl)
    const loopback = ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname)
    const credentialsPresent = preset.credentials.every((field) => {
      const value = String(config[field] ?? '').trim()
      return !!value && !/[\x00-\x20\x7f]/.test(value)
    })
    const anonymousLocalCompatible = provider === 'compatible' && loopback
    return !url.username && !url.password && !/^[A-Za-z][A-Za-z\d+.-]*:\/\/[^/?#]*@/.test(rawUrl)
      && !rawUrl.includes('?') && !rawUrl.includes('#')
      && (url.protocol === 'https:' || (url.protocol === 'http:' && loopback))
      && (credentialsPresent || anonymousLocalCompatible)
      && (provider !== 'tencent' || /^\d+$/.test(config.app_id?.trim() ?? ''))
  } catch { return false }
}

export function cloudSupportsTranslation(model: string, provider: CloudProvider = 'compatible'): boolean {
  return (provider === 'openai' || provider === 'compatible') && model.trim() === 'whisper-1'
}

export const asr = {
  status: (engine: LocalAsrEngine) => call<AsrEngineStatus>('asr_engine_status', { engine }),
  install: (engine: LocalAsrEngine) => call<void>('install_asr_engine', { engine }),
  async start(opts: AsrOptions): Promise<SubtitleTask> {
    const subtitle: SubtitleRequest = {
      video_path: opts.videoPath,
      language: opts.language ?? 'auto',
      model: opts.model ?? null,
      translate_to: opts.translateTo ?? null,
      output_format: opts.outputFormat ?? 'srt',
      output_dir: opts.outputDir ?? null,
    }
    const req = { engine: opts.engine, subtitle, ...(opts.cloud ? { cloud: { ...opts.cloud } } : {}) }
    try {
      return await call<SubtitleTask>('start_asr', { req })
    } catch (error) {
      // Providers can echo authorization details. Never pass the live key to UI/log errors.
      const secrets = [opts.cloud?.api_key, opts.cloud?.app_id, opts.cloud?.secret_id, opts.cloud?.secret_key]
        .filter((value): value is string => !!value).sort((a, b) => b.length - a.length)
      let detail = error instanceof Error ? error.message : String(error)
      for (const secret of secrets) detail = detail.split(secret).join('[REDACTED]')
      throw new Error(detail)
    }
  },
}
