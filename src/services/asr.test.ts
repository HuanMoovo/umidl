import { afterEach, describe, expect, it, vi } from 'vitest'
import { asr, CLOUD_PROVIDERS, cloudAllowedLanguages, cloudConfigValid, cloudSupportsTranslation, parseCloudTaskModel, type CloudProvider } from './asr'

afterEach(() => { delete (window as any).__TAURI_INTERNALS__ })

describe('ASR service boundary', () => {
  it('maps a Chinese transcription to the shared subtitle request without a cloud config', async () => {
    const invoke = vi.fn().mockResolvedValue({ id: 'local-1' })
    ;(window as any).__TAURI_INTERNALS__ = { invoke }
    await asr.start({ engine: 'sensevoice', videoPath: 'C:\\video.mp4', language: 'zh', outputFormat: 'vtt', outputDir: 'C:\\Subs' })
    expect(invoke).toHaveBeenCalledWith('start_asr', { req: {
      engine: 'sensevoice', subtitle: { video_path: 'C:\\video.mp4', language: 'zh', model: null, translate_to: null, output_format: 'vtt', output_dir: 'C:\\Subs' },
    } }, undefined)
  })

  it('sends cloud credentials only in the live invocation and redacts provider failures', async () => {
    ;(window as any).__TAURI_INTERNALS__ = { invoke: vi.fn().mockRejectedValue(new Error('Rejected secret-key-123 twice: secret-key-123')) }
    await expect(asr.start({ engine: 'cloud', videoPath: 'a.wav', cloud: { base_url: 'https://api.openai.com/v1', model: 'whisper-1', api_key: 'secret-key-123' } })).rejects.toThrow('Rejected [REDACTED] twice: [REDACTED]')
  })

  it('does not invoke native commands from the browser preview', async () => {
    await expect(asr.status('sensevoice')).rejects.toThrow('桌面客户端')
    await expect(asr.install('paraformer')).rejects.toThrow('桌面客户端')
  })

  it.each([
    ['https://api.openai.com/v1', 'key', true],
    ['https://api.openai.com/v1', '', false],
    ['http://localhost:8080/v1', '', true],
    ['http://127.0.0.1:8080/v1', '', true],
    ['http://[::1]:8080/v1', '', true],
    ['http://example.com/v1', 'key', false],
    ['https://key@example.com/v1', 'key', false],
    ['https://example.com/v1?api_key=key', 'key', false],
    ['https://example.com/v1#key', 'key', false],
    ['not a URL', 'key', false],
  ])('validates cloud endpoint %s with the credential boundary', (base_url, api_key, valid) => {
    expect(cloudConfigValid({ base_url, api_key, model: 'whisper-1' })).toBe(valid)
  })

  it('requires a model and limits translation to the compatible Whisper model', () => {
    expect(cloudConfigValid({ base_url: 'https://example.com/v1', api_key: 'key', model: ' ' })).toBe(false)
    expect(cloudSupportsTranslation('whisper-1')).toBe(true)
    expect(cloudSupportsTranslation('other-model')).toBe(false)
  })

  it('requires each Tencent credential before enabling cloud submissions', () => {
    const config = { provider: 'tencent', base_url: 'https://asr.cloud.tencent.com', model: '16k_zh', api_key: 'unused', app_id: '1234', secret_id: 'id', secret_key: '' }
    expect(cloudConfigValid(config as any)).toBe(false)
    expect(cloudConfigValid({ ...config, secret_key: 'secret' } as any)).toBe(true)
  })

  it.each([
    ['groq', ['api_key']], ['openai', ['api_key']], ['fish', ['api_key']], ['deepgram', ['api_key']],
    ['assemblyai', ['api_key']], ['azure', ['api_key']], ['tencent', ['app_id', 'secret_id', 'secret_key']],
    ['baidu', ['api_key', 'secret_key']], ['xunfei', ['app_id', 'api_key', 'secret_key']],
  ] as const)('validates the required credentials for %s', (provider, fields) => {
    const preset = CLOUD_PROVIDERS[provider]
    const config = { provider, base_url: preset.baseUrl || 'https://resource.cognitiveservices.azure.com',
      model: preset.model, api_key: 'key', app_id: '1234', secret_id: 'id', secret_key: 'secret' }
    expect(cloudConfigValid(config)).toBe(true)
    for (const field of fields) expect(cloudConfigValid({ ...config, [field]: '' })).toBe(false)
  })

  it('restores old and new cloud task identifiers without discarding colons in custom model names', () => {
    expect(parseCloudTaskModel('cloud:whisper-1')).toEqual({ provider: 'compatible', model: 'whisper-1' })
    expect(parseCloudTaskModel('cloud:local:model')).toEqual({ provider: 'compatible', model: 'local:model' })
    expect(parseCloudTaskModel('cloud:fish:transcribe-1-pro')).toEqual({ provider: 'fish', model: 'transcribe-1-pro' })
    expect(parseCloudTaskModel('cloud:groq:vendor:model')).toEqual({ provider: 'groq', model: 'vendor:model' })
  })

  it('filters language choices for providers with narrower model support', () => {
    expect(cloudAllowedLanguages('tencent', '16k_zh')).not.toContain('auto')
    expect(cloudAllowedLanguages('baidu', '80006')).toEqual(['zh'])
    expect(cloudAllowedLanguages('xunfei', 'autodialect')).toEqual(['auto'])
    expect(cloudAllowedLanguages('assemblyai', 'universal-3-pro')).not.toContain('zh')
    expect(cloudAllowedLanguages('groq', 'whisper-large-v3-turbo')).toBeNull()
    expect(cloudSupportsTranslation('whisper-1', 'fish' as CloudProvider)).toBe(false)
  })

  it('redacts every session credential if the provider echoes it in an error', async () => {
    ;(window as any).__TAURI_INTERNALS__ = { invoke: vi.fn().mockRejectedValue(new Error('a-key secret-id secret-key app-id')) }
    await expect(asr.start({ engine: 'cloud', videoPath: 'a.wav', cloud: {
      provider: 'tencent', base_url: 'https://asr.cloud.tencent.com', model: '16k_zh',
      api_key: 'a-key', app_id: 'app-id', secret_id: 'secret-id', secret_key: 'secret-key',
    } as any })).rejects.toThrow('[REDACTED] [REDACTED] [REDACTED] [REDACTED]')
  })
})
