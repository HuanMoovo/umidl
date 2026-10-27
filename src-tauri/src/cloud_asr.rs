//! Timestamped OpenAI/Whisper-compatible audio uploads. Credentials stay in memory.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use reqwest::blocking::{multipart, Client};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;
use crate::subtitle::Cue;

#[derive(Clone, Deserialize, Serialize)]
pub struct CloudAsrConfig {
    #[serde(default = "default_provider")]
    pub provider: String,
    pub base_url: String,
    pub model: String,
    #[serde(default, skip_serializing)]
    pub api_key: String,
    #[serde(default, skip_serializing)]
    pub app_id: String,
    #[serde(default, skip_serializing)]
    pub secret_id: String,
    #[serde(default, skip_serializing)]
    pub secret_key: String,
}

fn default_provider() -> String { "compatible".into() }

impl Default for CloudAsrConfig {
    fn default() -> Self {
        Self { provider: default_provider(), base_url: String::new(), model: String::new(),
            api_key: String::new(), app_id: String::new(), secret_id: String::new(), secret_key: String::new() }
    }
}

impl CloudAsrConfig {
    pub fn redact(&self, text: &str) -> String {
        [self.api_key.as_str(), self.secret_key.as_str(), self.secret_id.as_str(), self.app_id.as_str()]
            .into_iter().filter(|value| !value.is_empty())
            .fold(text.to_string(), |text, value| redact_provider_text(&text, value))
    }
}

impl fmt::Debug for CloudAsrConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CloudAsrConfig")
            .field("base_url", &"[configured]")
            .field("model", &"[configured]")
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

// A 10-minute 16 kHz mono s16 chunk is 19.2 MB, below the 25 MB upload limit
// including its WAV header. Neither the source recording nor upload is buffered.
const CHUNK_PCM_BYTES: u64 = 19_200_000;
const MAX_UPLOAD_BYTES: u64 = 25_000_000;
const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;
const PCM_BYTES_PER_SECOND: u64 = 32_000;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

struct ValidatedConfig {
    base: Url,
    model: String,
    api_key: String,
    loopback: bool,
    diarized: bool,
}

/// Validate without network access, before a queued task is persisted or started.
pub fn validate_config(config: &CloudAsrConfig) -> anyhow::Result<()> {
    match config.provider.as_str() {
        "compatible" | "openai" | "groq" => validated_config(config).map(|_| ()),
        "fish" | "deepgram" | "assemblyai" | "azure" | "tencent" | "baidu" | "xunfei" => validate_native_config(config),
        _ => anyhow::bail!("未知的云端语音识别服务"),
    }
}

fn validate_native_config(config: &CloudAsrConfig) -> anyhow::Result<()> {
    let base = Url::parse(config.base_url.trim()).map_err(|_| anyhow::anyhow!("服务地址无效"))?;
    let host = base.host_str().ok_or_else(|| anyhow::anyhow!("服务地址缺少主机"))?;
    let loopback = host.eq_ignore_ascii_case("localhost") || host.trim_start_matches('[').trim_end_matches(']')
        .parse::<IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false);
    let raw_authority = config.base_url.trim().split_once("://").map(|(_, rest)| rest.split(['/', '?', '#', '\\']).next().unwrap_or("")).unwrap_or("");
    anyhow::ensure!((base.scheme() == "https" || (base.scheme() == "http" && loopback))
        && base.username().is_empty() && base.password().is_none() && !raw_authority.contains('@')
        && base.query().is_none() && base.fragment().is_none(), "云端服务地址必须使用 HTTPS，且不能包含凭证或查询参数");
    let expected_host = match config.provider.as_str() {
        "fish" => Some("api.fish.audio"), "deepgram" => Some("api.deepgram.com"),
        "assemblyai" => Some("api.assemblyai.com"), "tencent" => Some("asr.cloud.tencent.com"),
        "baidu" => Some("aip.baidubce.com"), "xunfei" => Some("office-api-ist-dx.iflyaisol.com"),
        "azure" => None, _ => anyhow::bail!("未知的云端语音识别服务"),
    };
    if !loopback {
        if let Some(expected) = expected_host { anyhow::ensure!(host.eq_ignore_ascii_case(expected), "该服务只能使用其官方接口地址"); }
    }
    let present = |value: &str| !value.trim().is_empty() && value.len() <= 8192
        && !value.bytes().any(|byte| byte <= 0x20 || byte == 0x7f);
    let valid = match config.provider.as_str() {
        "tencent" => present(&config.app_id) && present(&config.secret_id) && present(&config.secret_key),
        "baidu" => present(&config.api_key) && present(&config.secret_key),
        "xunfei" => present(&config.app_id) && present(&config.api_key) && present(&config.secret_key),
        _ => present(&config.api_key) || loopback,
    };
    anyhow::ensure!(valid, "请填写当前服务商所需的完整凭证");
    anyhow::ensure!(config.model.len() <= 200 && !config.model.chars().any(char::is_control), "云端模型名称无效");
    Ok(())
}

fn validated_config(config: &CloudAsrConfig) -> anyhow::Result<ValidatedConfig> {
    // Do not attach parser errors or user input: a malformed URL can itself
    // contain credentials. The same rule applies to every network error below.
    let raw_url = config.base_url.trim();
    let base = Url::parse(raw_url)
        .map_err(|_| anyhow::anyhow!("云端 Base URL 无效，请填写完整的 HTTPS 服务地址"))?;
    let authority_has_userinfo = raw_url
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#', '\\']).next().unwrap_or("").contains('@'))
        .unwrap_or(false);
    if !base.username().is_empty() || base.password().is_some() || authority_has_userinfo {
        anyhow::bail!("云端 Base URL 不能包含用户名或密码，请把 API Key 填入专用输入框");
    }
    if base.query().is_some() || base.fragment().is_some() {
        anyhow::bail!("云端 Base URL 不能包含查询参数或片段，请把 API Key 填入专用输入框");
    }
    let host = base.host_str().ok_or_else(|| anyhow::anyhow!("云端 Base URL 缺少主机地址"))?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host.trim_start_matches('[').trim_end_matches(']')
            .parse::<IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false);
    if base.scheme() != "https" && !(base.scheme() == "http" && loopback) {
        anyhow::bail!("远程云端服务必须使用 HTTPS；HTTP 只允许本机回环地址");
    }
    let model = config.model.trim();
    if model.is_empty() || model.len() > 200 || model.chars().any(char::is_control) {
        anyhow::bail!("请填写有效的云端模型名称（例如支持分段时间戳的 whisper-1）");
    }
    let diarized = model == "gpt-4o-transcribe-diarize" || model.starts_with("gpt-4o-transcribe-diarize-");
    let official_openai = host == "api.openai.com" || host.ends_with(".api.openai.com");
    if official_openai && !diarized
        && (model.starts_with("gpt-4o-transcribe") || model.starts_with("gpt-4o-mini-transcribe")
            || model == "gpt-transcribe" || model.starts_with("gpt-live-transcribe"))
    {
        anyhow::bail!("所选 OpenAI 模型未提供本字幕流程所需的分段时间戳；请使用 whisper-1，或支持 diarized_json 的 gpt-4o-transcribe-diarize");
    }
    let key = config.api_key.trim();
    if key.is_empty() && !loopback {
        anyhow::bail!("请填写云端 API Key；密钥只在本次会话中使用");
    }
    if key.len() > 8192 || key.bytes().any(|byte| byte <= 0x20 || byte == 0x7f) {
        anyhow::bail!("API Key 格式无效，请检查是否粘贴了空格或换行");
    }
    if !key.is_empty() {
        HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|_| anyhow::anyhow!("API Key 无法用于认证请求头，请检查格式"))?;
    }
    Ok(ValidatedConfig { base, model: model.into(), api_key: key.into(), loopback, diarized })
}

pub fn transcribe(
    ctx: &Ctx,
    wav: &Path,
    config: &CloudAsrConfig,
    language: &str,
    translate_to: Option<&str>,
    is_cancelled: &dyn Fn() -> bool,
) -> anyhow::Result<Vec<Cue>> {
    validate_config(config)?;
    if matches!(config.provider.as_str(), "compatible" | "openai" | "groq") {
        return transcribe_in_dir(&ctx.dirs.cache, wav, config, language, translate_to, is_cancelled, CHUNK_PCM_BYTES);
    }
    anyhow::ensure!(translate_to.map(str::trim).filter(|s| !s.is_empty()).is_none(), "当前服务不支持直接翻译字幕");
    native_chunks_in_dir(&ctx.dirs.cache, wav, CHUNK_PCM_BYTES, is_cancelled, &|chunk| {
        let result = match config.provider.as_str() {
            "fish" | "deepgram" | "azure" => crate::cloud_vendor_sync::transcribe(ctx, chunk, config, language, is_cancelled),
            "tencent" | "baidu" | "xunfei" => crate::cloud_vendor_cn::transcribe(ctx, chunk, config, language, is_cancelled),
            "assemblyai" => crate::cloud_vendor_assembly::transcribe(ctx, chunk, config, language, is_cancelled),
            _ => anyhow::bail!("未知的云端语音识别服务"),
        };
        result.map(|mut cues| {
            for cue in &mut cues { cue.text = config.redact(&cue.text); }
            cues
        }).map_err(|error| anyhow::anyhow!(config.redact(&format!("{error:#}"))))
    })
}

fn transcribe_in_dir(
    cache: &Path,
    wav: &Path,
    config: &CloudAsrConfig,
    language: &str,
    translate_to: Option<&str>,
    is_cancelled: &dyn Fn() -> bool,
    chunk_pcm_bytes: u64,
) -> anyhow::Result<Vec<Cue>> {
    check_cancelled(is_cancelled)?;
    let config = validated_config(config)?;
    let translation = match translate_to {
        None => false,
        Some(target) if matches!(target.trim().to_ascii_lowercase().as_str(), "en" | "english") => {
            if config.model != "whisper-1" {
                anyhow::bail!("云端音频翻译只支持 whisper-1 模型翻译为英文，请关闭翻译或切换模型");
            }
            true
        }
        Some(_) => anyhow::bail!("云端音频翻译接口只支持翻译为英文，请关闭翻译或选择英文"),
    };
    let language = language_hint(language)?;
    let mut endpoint = config.base.clone();
    let base_path = endpoint.path().trim_end_matches('/');
    let base_path = base_path.strip_suffix("/audio/transcriptions")
        .or_else(|| base_path.strip_suffix("/audio/translations")).unwrap_or(base_path);
    let endpoint_path = format!("{base_path}/audio/{}", if translation { "translations" } else { "transcriptions" });
    endpoint.set_path(&endpoint_path);
    let mut builder = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(REQUEST_TIMEOUT);
    // System proxy variables must not forward localhost audio/credentials away.
    if config.loopback { builder = builder.no_proxy(); }
    let client = builder.build().map_err(|_| anyhow::anyhow!("无法初始化云端连接"))?;
    let mut input = File::open(wav).map_err(|_| anyhow::anyhow!("无法读取提取后的音频文件"))?;
    let layout = wav_layout(&mut input)?;
    input.seek(SeekFrom::Start(layout.data_offset))
        .map_err(|_| anyhow::anyhow!("无法定位音频数据"))?;
    let chunk_pcm_bytes = chunk_pcm_bytes.min(MAX_UPLOAD_BYTES - 44) / 2 * 2;
    if chunk_pcm_bytes < 2 { anyhow::bail!("云端音频分块大小无效"); }
    let mut consumed = 0u64;
    let mut cues = Vec::new();
    while consumed < layout.data_bytes {
        check_cancelled(is_cancelled)?;
        let bytes = chunk_pcm_bytes.min(layout.data_bytes - consumed);
        let chunk = TempChunk::create(cache, &mut input, bytes, is_cancelled)?;
        let response = wait_for_chunk(
            client.clone(), endpoint.clone(), &config, language.clone(), translation,
            chunk, is_cancelled,
        )?;
        check_cancelled(is_cancelled)?;
        let chunk_cues = parse_segments(&response, consumed, bytes, &config.api_key)?;
        for mut cue in chunk_cues {
            cue.index = cues.len() + 1;
            cues.push(cue);
        }
        consumed += bytes;
    }
    check_cancelled(is_cancelled)?;
    if cues.is_empty() { anyhow::bail!("云端未识别到可用语音，未生成字幕"); }
    Ok(cues)
}

/// Run each native provider on a bounded WAV chunk and preserve its real offsets.
fn native_chunks_in_dir(
    cache: &Path, wav: &Path, chunk_pcm_bytes: u64, cancelled: &dyn Fn() -> bool,
    transcribe_chunk: &dyn Fn(&Path) -> anyhow::Result<Vec<Cue>>,
) -> anyhow::Result<Vec<Cue>> {
    check_cancelled(cancelled)?;
    let mut input = File::open(wav).map_err(|_| anyhow::anyhow!("无法读取提取后的音频文件"))?;
    let layout = wav_layout(&mut input)?;
    input.seek(SeekFrom::Start(layout.data_offset)).map_err(|_| anyhow::anyhow!("无法定位音频数据"))?;
    let chunk_size = chunk_pcm_bytes.min(MAX_UPLOAD_BYTES - 44) / 2 * 2;
    anyhow::ensure!(chunk_size >= 2, "云端音频分块大小无效");
    let mut consumed = 0u64;
    let mut output = Vec::new();
    while consumed < layout.data_bytes {
        check_cancelled(cancelled)?;
        let bytes = chunk_size.min(layout.data_bytes - consumed);
        let chunk = TempChunk::create(cache, &mut input, bytes, cancelled)?;
        let local = transcribe_chunk(&chunk.path)?;
        check_cancelled(cancelled)?;
        let duration_ms = bytes * 1000 / PCM_BYTES_PER_SECOND;
        let offset_ms = consumed * 1000 / PCM_BYTES_PER_SECOND;
        let mut previous_start = 0u64;
        for mut cue in local {
            anyhow::ensure!(!cue.text.trim().is_empty() && cue.end_ms > cue.start_ms
                && cue.start_ms >= previous_start && cue.start_ms < duration_ms
                && cue.end_ms <= duration_ms.saturating_add(10), "云端返回了无效的分段时间戳");
            previous_start = cue.start_ms;
            cue.start_ms += offset_ms;
            cue.end_ms += offset_ms;
            cue.index = output.len() + 1;
            output.push(cue);
        }
        consumed += bytes;
    }
    check_cancelled(cancelled)?;
    anyhow::ensure!(!output.is_empty(), "云端未识别到有效语音，未生成字幕");
    Ok(output)
}

fn check_cancelled(is_cancelled: &dyn Fn() -> bool) -> anyhow::Result<()> {
    if is_cancelled() { anyhow::bail!("云端字幕识别已取消"); }
    Ok(())
}

fn language_hint(language: &str) -> anyhow::Result<Option<String>> {
    let language = language.trim().to_ascii_lowercase();
    if language.is_empty() || language == "auto" { return Ok(None); }
    let code = language.split(['-', '_']).next().unwrap_or("");
    if !(2..=3).contains(&code.len()) || !code.bytes().all(|b| b.is_ascii_alphabetic()) {
        anyhow::bail!("云端识别语言必须使用语言代码（例如 zh、en）或自动检测");
    }
    Ok(Some(code.into()))
}

struct WavLayout { data_offset: u64, data_bytes: u64 }

fn wav_layout(file: &mut File) -> anyhow::Result<WavLayout> {
    let invalid = || anyhow::anyhow!("音频 WAV 无效；云端识别需要已提取的 16kHz 单声道 16 位 PCM WAV");
    let file_len = file.metadata().map_err(|_| invalid())?.len();
    let mut header = [0; 12];
    file.read_exact(&mut header).map_err(|_| invalid())?;
    if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" { return Err(invalid()); }
    let riff_end = u32::from_le_bytes(header[4..8].try_into().unwrap()) as u64 + 8;
    if riff_end > file_len || riff_end < 12 { return Err(invalid()); }
    let mut offset = 12u64;
    let mut pcm_format = false;
    let mut data = None;
    while offset.checked_add(8).filter(|end| *end <= riff_end).is_some() {
        file.seek(SeekFrom::Start(offset)).map_err(|_| invalid())?;
        let mut chunk_header = [0; 8];
        file.read_exact(&mut chunk_header).map_err(|_| invalid())?;
        let size = u32::from_le_bytes(chunk_header[4..].try_into().unwrap()) as u64;
        let payload = offset + 8;
        let end = payload.checked_add(size).filter(|end| *end <= riff_end).ok_or_else(invalid)?;
        if &chunk_header[..4] == b"fmt " {
            if size < 16 { return Err(invalid()); }
            let mut fmt = [0; 16];
            file.read_exact(&mut fmt).map_err(|_| invalid())?;
            pcm_format = u16::from_le_bytes(fmt[..2].try_into().unwrap()) == 1
                && u16::from_le_bytes(fmt[2..4].try_into().unwrap()) == 1
                && u32::from_le_bytes(fmt[4..8].try_into().unwrap()) == 16_000
                && u32::from_le_bytes(fmt[8..12].try_into().unwrap()) == 32_000
                && u16::from_le_bytes(fmt[12..14].try_into().unwrap()) == 2
                && u16::from_le_bytes(fmt[14..16].try_into().unwrap()) == 16;
            if !pcm_format { return Err(invalid()); }
        } else if &chunk_header[..4] == b"data" && data.is_none() {
            if size % 2 != 0 { return Err(invalid()); }
            data = Some(WavLayout { data_offset: payload, data_bytes: size });
        }
        if pcm_format && data.is_some() { break; }
        offset = end + size % 2;
    }
    if !pcm_format { return Err(invalid()); }
    let data = data.ok_or_else(invalid)?;
    if data.data_bytes == 0 { anyhow::bail!("音频为空，未上传云端或生成字幕"); }
    Ok(data)
}

/// Owned by the request worker so cancellation can return promptly without
/// removing a file that a still-finishing HTTP upload has open on Windows.
struct TempChunk { dir: PathBuf, path: PathBuf, bytes: u64 }

impl TempChunk {
    fn create(cache: &Path, input: &mut File, pcm_bytes: u64, cancelled: &dyn Fn() -> bool) -> anyhow::Result<Self> {
        let parent = cache.join("cloud-asr");
        fs::create_dir_all(&parent).map_err(|_| anyhow::anyhow!("无法创建云端音频临时目录"))?;
        let dir = parent.join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&dir).map_err(|_| anyhow::anyhow!("无法创建云端音频分块目录"))?;
        let chunk = Self { path: dir.join("audio.wav"), dir, bytes: pcm_bytes + 44 };
        let mut output = OpenOptions::new().write(true).create_new(true).open(&chunk.path)
            .map_err(|_| anyhow::anyhow!("无法创建云端音频分块"))?;
        let mut header = Vec::with_capacity(44);
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&(pcm_bytes as u32 + 36).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
        header.extend_from_slice(&16_000u32.to_le_bytes());
        header.extend_from_slice(&32_000u32.to_le_bytes());
        header.extend_from_slice(&2u16.to_le_bytes());
        header.extend_from_slice(&16u16.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&(pcm_bytes as u32).to_le_bytes());
        output.write_all(&header).map_err(|_| anyhow::anyhow!("无法写入云端音频分块"))?;
        let mut buffer = [0u8; 65_536];
        let mut remaining = pcm_bytes;
        while remaining > 0 {
            check_cancelled(cancelled)?;
            let size = remaining.min(buffer.len() as u64) as usize;
            input.read_exact(&mut buffer[..size]).map_err(|_| anyhow::anyhow!("提取后的音频文件不完整"))?;
            output.write_all(&buffer[..size]).map_err(|_| anyhow::anyhow!("无法写入云端音频分块"))?;
            remaining -= size as u64;
        }
        output.flush().map_err(|_| anyhow::anyhow!("无法保存云端音频分块"))?;
        Ok(chunk)
    }
}

impl Drop for TempChunk {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_dir(&self.dir);
    }
}

struct CancelReader {
    // Fields drop in declaration order: close the file before its last guard
    // can remove it, including when a provider replies before consuming upload.
    file: File,
    cancelled: Arc<AtomicBool>,
    _chunk: Arc<TempChunk>,
}
impl Read for CancelReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.cancelled.load(Ordering::Relaxed) {
            // Interrupted is retryable to standard I/O copying helpers.
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "cancelled"));
        }
        self.file.read(buffer)
    }
}

struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) { self.0.store(true, Ordering::Relaxed); }
}

#[allow(clippy::too_many_arguments)]
fn wait_for_chunk(
    client: Client, endpoint: Url, config: &ValidatedConfig, language: Option<String>,
    translation: bool, chunk: TempChunk, is_cancelled: &dyn Fn() -> bool,
) -> anyhow::Result<Vec<u8>> {
    // Reqwest blocking cannot interrupt a pending response read. Cancellation
    // stops upload readers and returns here promptly; the owned worker/chunk
    // finishes or times out within REQUEST_TIMEOUT, then deletes the chunk.
    check_cancelled(is_cancelled)?;
    let cancel = CancelOnDrop(Arc::new(AtomicBool::new(false)));
    let cancelled = cancel.0.clone();
    let model = config.model.clone();
    let api_key = config.api_key.clone();
    let diarized = config.diarized;
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::Builder::new().name("umi-cloud-asr".into()).spawn(move || {
        let response = request_chunk(client, endpoint, model, api_key, language, translation, diarized, chunk, cancelled);
        let _ = tx.send(response);
    }).map_err(|_| anyhow::anyhow!("无法启动云端识别请求"))?;
    loop {
        check_cancelled(is_cancelled)?;
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => { check_cancelled(is_cancelled)?; return result; }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => anyhow::bail!("云端识别请求意外结束，请重试"),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn request_chunk(
    client: Client, endpoint: Url, model: String, api_key: String, language: Option<String>,
    translation: bool, diarized: bool, chunk: TempChunk, cancelled: Arc<AtomicBool>,
) -> anyhow::Result<Vec<u8>> {
    let chunk = Arc::new(chunk);
    let file = File::open(&chunk.path).map_err(|_| anyhow::anyhow!("无法打开云端音频分块"))?;
    let reader = CancelReader { file, cancelled: cancelled.clone(), _chunk: chunk.clone() };
    let part = multipart::Part::reader_with_length(reader, chunk.bytes)
        .file_name("audio.wav").mime_str("audio/wav")
        .map_err(|_| anyhow::anyhow!("无法构造音频上传请求"))?;
    let mut form = multipart::Form::new().part("file", part).text("model", model)
        .text("response_format", if diarized { "diarized_json" } else { "verbose_json" });
    if !translation {
        if diarized { form = form.text("chunking_strategy", "auto"); }
        else { form = form.text("timestamp_granularities[]", "segment"); }
        if let Some(language) = language { form = form.text("language", language); }
    }
    let mut headers = HeaderMap::new();
    if !api_key.is_empty() {
        let mut authorization = HeaderValue::from_str(&format!("Bearer {api_key}"))
            .map_err(|_| anyhow::anyhow!("API Key 格式无效"))?;
        authorization.set_sensitive(true);
        headers.insert(AUTHORIZATION, authorization);
    }
    if cancelled.load(Ordering::Relaxed) { anyhow::bail!("云端字幕识别已取消"); }
    let mut response = client.post(endpoint).headers(headers).multipart(form).send().map_err(|error| {
        if error.is_timeout() { anyhow::anyhow!("云端识别请求超时，请检查网络或稍后重试") }
        else if error.is_connect() { anyhow::anyhow!("无法连接云端识别服务，请检查地址和网络") }
        else { anyhow::anyhow!("云端识别请求失败，请检查服务配置或网络") }
    })?;
    let status = response.status();
    if !status.is_success() {
        // Never render provider bodies, Location headers, request URLs or the
        // reqwest error chain: providers sometimes echo Authorization/API keys.
        let reason = if status.is_redirection() { "服务返回重定向；请直接填写最终 API 地址" }
            else { match status.as_u16() {
                401 | 403 => "认证失败，请检查 API Key 和服务权限",
                413 => "服务拒绝音频大小，请使用支持至少 20MB 上传的兼容服务",
                429 => "服务限流或额度不足，请检查额度后重试",
                400 | 404 | 422 => "服务不支持当前接口、模型或时间戳格式，请检查兼容设置",
                _ => "服务处理失败，请稍后重试",
            } };
        anyhow::bail!("云端识别失败（HTTP {}）：{}", status.as_u16(), reason);
    }
    if response.content_length().map(|len| len > MAX_RESPONSE_BYTES).unwrap_or(false) {
        anyhow::bail!("云端识别响应过大，已停止处理");
    }
    let mut body = Vec::new();
    let mut buffer = [0u8; 65_536];
    loop {
        if cancelled.load(Ordering::Relaxed) { anyhow::bail!("云端字幕识别已取消"); }
        let size = response.read(&mut buffer).map_err(|_| anyhow::anyhow!("读取云端识别响应失败，请重试"))?;
        if size == 0 { break; }
        if body.len() as u64 + size as u64 > MAX_RESPONSE_BYTES { anyhow::bail!("云端识别响应过大，已停止处理"); }
        body.extend_from_slice(&buffer[..size]);
    }
    Ok(body)
}

fn parse_segments(body: &[u8], offset_bytes: u64, chunk_bytes: u64, api_key: &str) -> anyhow::Result<Vec<Cue>> {
    let missing = || anyhow::anyhow!("云端未返回有效的分段时间戳；字幕需要 verbose_json 的 segments，或 diarized_json。请使用支持时间戳的模型（例如 whisper-1），普通 JSON 文本无法生成可靠字幕");
    // Malformed JSON diagnostics can contain response text, so never chain them.
    let response: serde_json::Value = serde_json::from_slice(body).map_err(|_| missing())?;
    let segments = response.get("segments").and_then(|v| v.as_array()).ok_or_else(missing)?;
    if segments.len() > 50_000 { anyhow::bail!("云端返回的时间戳分段数量异常"); }
    if segments.is_empty() {
        if response.get("text").and_then(|v| v.as_str()).map(|s| !s.trim().is_empty()).unwrap_or(false) {
            return Err(missing());
        }
        return Ok(Vec::new());
    }
    let duration = chunk_bytes as f64 / PCM_BYTES_PER_SECOND as f64;
    let offset = offset_bytes as f64 / PCM_BYTES_PER_SECOND as f64;
    let mut previous_start = 0.0;
    let mut cues = Vec::new();
    for segment in segments {
        let start = segment.get("start").and_then(|v| v.as_f64()).ok_or_else(missing)?;
        let end = segment.get("end").and_then(|v| v.as_f64()).ok_or_else(missing)?;
        let text = segment.get("text").and_then(|v| v.as_str()).ok_or_else(missing)?.trim();
        if !start.is_finite() || !end.is_finite() || start < 0.0 || end <= start
            || start < previous_start || start >= duration || end > duration + 0.01
        { return Err(missing()); }
        previous_start = start;
        let start_ms = ((offset + start) * 1000.0).round() as u64;
        let end_ms = ((offset + end) * 1000.0).round() as u64;
        if end_ms <= start_ms { return Err(missing()); }
        if text.is_empty() { continue; }
        cues.push(Cue { index: cues.len() + 1, start_ms, end_ms, text: redact_provider_text(text, api_key) });
    }
    Ok(cues)
}

fn redact_provider_text(text: &str, api_key: &str) -> String {
    if api_key.is_empty() { return text.into(); }
    let mut text = text.replace(api_key, "[REDACTED]");
    // A malicious/misconfigured compatible provider must not put an echoed
    // percent-encoded key into an otherwise valid subtitle response either.
    let encoded: String = api_key.bytes().map(|byte| format!("%{byte:02X}")).collect();
    text = text.replace(&encoded, "[REDACTED]");
    text.replace(&encoded.to_ascii_lowercase(), "[REDACTED]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::fs;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    const KEY: &str = "sk-synthetic-cloud-asr-test-key";

    #[test]
    fn vendor_requires_its_own_credentials_before_network() {
        let config: CloudAsrConfig = serde_json::from_value(serde_json::json!({
            "provider":"tencent", "base_url":"https://asr.cloud.tencent.com", "model":"16k_zh", "api_key":"not-the-required-credentials"})).unwrap();
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn vendor_chunks_keep_real_offsets_and_reject_empty_output() {
        let dir = TempDir::new();
        let input = dir.wav(64_000);
        let calls = std::cell::Cell::new(0usize);
        let cues = native_chunks_in_dir(&dir.0.join("cache"), &input, 32_000, &|| false, &|chunk| {
            assert!(chunk.is_file());
            calls.set(calls.get() + 1);
            Ok(vec![Cue { index: 1, start_ms: 100, end_ms: 700, text: format!("segment {}", calls.get()) }])
        }).unwrap();
        assert_eq!(calls.get(), 2);
        assert_eq!((cues[0].start_ms, cues[0].end_ms), (100, 700));
        assert_eq!((cues[1].start_ms, cues[1].end_ms), (1100, 1700));
        assert!(native_chunks_in_dir(&dir.0.join("cache"), &input, 32_000, &|| false, &|_| Ok(vec![])).is_err());
    }

    #[test]
    fn vendor_dispatch_sends_fish_to_its_native_endpoint() {
        let dir = TempDir::new();
        let server = Server::new(vec![Reply::json(r#"{"text":"你好","segments":[{"text":"你好","start":0.1,"end":0.7}]}"#)]);
        let config = CloudAsrConfig { provider: "fish".into(), base_url: server.url.trim_end_matches("/v1").into(),
            model: "transcribe-1-pro".into(), api_key: KEY.into(), ..Default::default() };
        let dirs = crate::ctx::AppDirs { data: dir.0.clone(), bin: dir.0.join("bin"), models: dir.0.join("models"),
            cache: dir.0.join("cache"), downloads: dir.0.join("downloads"), db_file: dir.0.join("umi.db"),
            settings_file: dir.0.join("settings.json") };
        let ctx = crate::ctx::Ctx::new(dirs, Default::default(), Default::default());
        let cues = transcribe(&ctx, &dir.wav(32_000), &config, "zh", None, &|| false).unwrap();
        assert_eq!(cues[0].text, "你好");
        assert!(server.requests.lock().unwrap()[0].line.starts_with("POST /v1/asr "));
    }

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("umi-cloud-asr-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn wav(&self, pcm_bytes: u32) -> PathBuf {
            let path = self.0.join("input.wav");
            let mut file = fs::File::create(&path).unwrap();
            file.write_all(b"RIFF").unwrap();
            file.write_all(&(36 + pcm_bytes).to_le_bytes()).unwrap();
            file.write_all(b"WAVEfmt \x10\0\0\0\x01\0\x01\0").unwrap();
            file.write_all(&16_000u32.to_le_bytes()).unwrap();
            file.write_all(&32_000u32.to_le_bytes()).unwrap();
            file.write_all(&2u16.to_le_bytes()).unwrap();
            file.write_all(&16u16.to_le_bytes()).unwrap();
            file.write_all(b"data").unwrap();
            file.write_all(&pcm_bytes.to_le_bytes()).unwrap();
            let mut left = pcm_bytes as usize;
            let block = [0u8; 4096];
            while left > 0 {
                let size = left.min(block.len());
                file.write_all(&block[..size]).unwrap();
                left -= size;
            }
            path
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    #[derive(Debug)]
    struct Request { line: String, headers: String, body: Vec<u8> }
    struct Reply { status: u16, body: String, headers: String, delay: Duration }
    impl Reply {
        fn json(body: &str) -> Self {
            Self { status: 200, body: body.into(), headers: String::new(), delay: Duration::ZERO }
        }
    }
    struct Server {
        url: String,
        stop: Arc<AtomicBool>,
        requests: Arc<Mutex<Vec<Request>>>,
        worker: Option<thread::JoinHandle<()>>,
    }
    impl Server {
        fn new(replies: Vec<Reply>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/v1", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop2 = stop.clone();
            let requests2 = requests.clone();
            let worker = thread::spawn(move || {
                let mut replies: VecDeque<_> = replies.into();
                let deadline = Instant::now() + Duration::from_secs(10);
                while !stop2.load(Ordering::SeqCst) && Instant::now() < deadline {
                    let (mut socket, _) = match listener.accept() {
                        Ok(s) => s,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(_) => break,
                    };
                    socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                    let mut reader = BufReader::new(socket.try_clone().unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 { continue; }
                    let mut headers = String::new();
                    let mut length = 0usize;
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" { break; }
                        if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                            length = value.trim().parse().unwrap();
                        }
                        headers.push_str(&header);
                    }
                    assert!(length < 25_000_000);
                    let mut body = vec![0; length];
                    if reader.read_exact(&mut body).is_err() { continue; }
                    requests2.lock().unwrap().push(Request { line, headers, body });
                    let Some(reply) = replies.pop_front() else { break; };
                    thread::sleep(reply.delay);
                    let response = format!(
                        "HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                        reply.status, reply.body.len(), reply.headers, reply.body
                    );
                    let _ = socket.write_all(response.as_bytes());
                    if replies.is_empty() { break; }
                }
            });
            Self { url, stop, requests, worker: Some(worker) }
        }
        fn config(&self) -> CloudAsrConfig {
            CloudAsrConfig { base_url: self.url.clone(), model: "whisper-1".into(), api_key: KEY.into(), ..Default::default() }
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(worker) = self.worker.take() { worker.join().unwrap(); }
        }
    }

    fn run(dir: &TempDir, server: &Server, pcm_bytes: u32) -> anyhow::Result<Vec<Cue>> {
        transcribe_in_dir(&dir.0.join("cache"), &dir.wav(pcm_bytes), &server.config(), "zh", None, &|| false, 32_000)
    }

    #[test]
    fn config_accepts_https_and_loopback_but_rejects_credential_leaks() {
        let mut config = CloudAsrConfig { base_url: "https://api.openai.com/v1".into(), model: "whisper-1".into(), api_key: KEY.into(), ..Default::default() };
        validate_config(&config).unwrap();
        for good in ["http://127.0.0.1:8080/v1", "http://localhost/v1", "http://[::1]:8000/custom"] {
            config.base_url = good.into();
            validate_config(&config).unwrap();
        }
        for bad in ["http://example.com/v1", "ftp://example.com/v1", "https://user:secret@example.com/v1", "https://@example.com/v1", "https://example.com/v1?key=secret", "https://example.com/v1#secret", "not a URL"] {
            config.base_url = bad.into();
            let error = validate_config(&config).unwrap_err();
            assert!(!format!("{error:?}").contains("secret"));
        }
    }

    #[test]
    fn config_debug_and_serialization_never_reveal_key() {
        let config = CloudAsrConfig { base_url: format!("https://{KEY}@example.com"), model: KEY.into(), api_key: KEY.into(), ..Default::default() };
        assert!(!format!("{config:?}").contains(KEY));
        let safe = CloudAsrConfig { base_url: "https://example.com/v1".into(), model: "whisper-1".into(), api_key: KEY.into(), ..Default::default() };
        assert!(!serde_json::to_string(&safe).unwrap().contains(KEY));
    }

    #[test]
    fn known_plain_text_only_openai_models_fail_preflight_but_custom_providers_are_allowed() {
        let mut config = CloudAsrConfig { base_url: "https://api.openai.com/v1".into(), model: "gpt-4o-transcribe".into(), api_key: KEY.into(), ..Default::default() };
        for model in ["gpt-4o-transcribe", "gpt-4o-mini-transcribe", "gpt-transcribe"] {
            config.model = model.into();
            assert!(validate_config(&config).unwrap_err().to_string().contains("时间戳"));
        }
        config.base_url = "https://whisper.example.com/custom/v1".into();
        validate_config(&config).unwrap();
        config.api_key.clear();
        assert!(validate_config(&config).is_err());
        config.base_url = "http://127.0.0.1:8000/v1".into();
        validate_config(&config).unwrap();
        config.api_key = "invalid\nheader".into();
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn empty_truncated_and_wrong_format_wav_never_uploads() {
        let dir = TempDir::new();
        let server = Server::new(vec![]);
        let cache = dir.0.join("cache");
        let empty = dir.wav(0);
        assert!(transcribe_in_dir(&cache, &empty, &server.config(), "auto", None, &|| false, 32_000).is_err());
        let truncated = dir.wav(32_000);
        fs::OpenOptions::new().write(true).open(&truncated).unwrap().set_len(100).unwrap();
        assert!(transcribe_in_dir(&cache, &truncated, &server.config(), "auto", None, &|| false, 32_000).is_err());
        let wrong_rate = dir.wav(32_000);
        let mut file = fs::OpenOptions::new().write(true).open(&wrong_rate).unwrap();
        std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(24)).unwrap();
        file.write_all(&48_000u32.to_le_bytes()).unwrap();
        drop(file);
        assert!(transcribe_in_dir(&cache, &wrong_rate, &server.config(), "auto", None, &|| false, 32_000).is_err());
        assert!(server.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn silent_chunks_keep_original_offsets_and_all_silent_audio_is_not_success() {
        let dir = TempDir::new();
        let server = Server::new(vec![Reply::json(r#"{"text":"","segments":[]}"#), Reply::json(r#"{"segments":[{"start":0.1,"end":0.8,"text":"after silence"}]}"#)]);
        let cues = run(&dir, &server, 64_000).unwrap();
        assert_eq!((cues[0].start_ms, cues[0].end_ms), (1100, 1800));
        let silent = Server::new(vec![Reply::json(r#"{"text":"","segments":[]}"#)]);
        assert!(run(&dir, &silent, 32_000).unwrap_err().to_string().contains("未识别"));
    }

    #[test]
    fn multipart_uploads_real_wav_chunks_and_keeps_sample_offsets() {
        let dir = TempDir::new();
        let server = Server::new(vec![
            Reply::json(r#"{"text":"一","segments":[{"start":0.125,"end":0.875,"text":" 一 "}]}"#),
            Reply::json(r#"{"text":"二","segments":[{"start":0.125,"end":0.875,"text":"二"}]}"#),
            Reply::json(r#"{"text":"三","segments":[{"start":0,"end":0.25,"text":"三"}]}"#),
        ]);
        let cues = run(&dir, &server, 80_000).unwrap();
        assert_eq!(cues.iter().map(|c| (c.index, c.start_ms, c.end_ms, c.text.as_str())).collect::<Vec<_>>(),
            vec![(1, 125, 875, "一"), (2, 1125, 1875, "二"), (3, 2000, 2250, "三")]);
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        for (index, request) in requests.iter().enumerate() {
            assert_eq!(request.line, "POST /v1/audio/transcriptions HTTP/1.1\r\n");
            assert!(request.headers.to_ascii_lowercase().contains(&format!("authorization: bearer {KEY}")));
            assert!(request.headers.contains("multipart/form-data; boundary="));
            let body = String::from_utf8_lossy(&request.body);
            for expected in ["name=\"model\"\r\n\r\nwhisper-1", "name=\"response_format\"\r\n\r\nverbose_json", "name=\"timestamp_granularities[]\"\r\n\r\nsegment", "name=\"language\"\r\n\r\nzh", "filename=\"audio.wav\""] {
                assert!(body.contains(expected), "missing multipart field {expected}");
            }
            let wave_at = request.body.windows(4).position(|w| w == b"RIFF").unwrap();
            assert_eq!(&request.body[wave_at + 8..wave_at + 12], b"WAVE");
            let pcm_len = u32::from_le_bytes(request.body[wave_at + 40..wave_at + 44].try_into().unwrap());
            assert_eq!(pcm_len, if index == 2 { 16_000 } else { 32_000 });
        }
        assert_eq!(fs::read_dir(dir.0.join("cache/cloud-asr")).unwrap().count(), 0);
    }

    #[test]
    fn provider_errors_never_echo_response_or_credentials() {
        for status in [401, 429, 500] {
            let dir = TempDir::new();
            let mut reply = Reply::json(&format!(r#"{{"error":{{"message":"Bearer {KEY} is invalid"}}}}"#));
            reply.status = status;
            let server = Server::new(vec![reply]);
            let error = run(&dir, &server, 32_000).unwrap_err();
            let rendered = format!("{error:?}");
            assert!(!rendered.contains(KEY));
            assert!(!rendered.contains("Bearer"));
            assert!(rendered.contains(&status.to_string()));
        }
    }

    #[test]
    fn missing_invalid_or_nonmonotonic_timestamps_fail_without_fabricating_cues() {
        for response in [
            r#"{"text":"hello"}"#,
            r#"{"text":"hello","segments":[]}"#,
            r#"{"segments":[{"start":0.1,"text":"hello"}]}"#,
            r#"{"segments":[{"start":0.8,"end":0.2,"text":"hello"}]}"#,
            r#"{"segments":[{"start":-1,"end":0.2,"text":"hello"}]}"#,
            r#"{"segments":[{"start":0,"end":5,"text":"hello"}]}"#,
            r#"{"segments":[{"start":0.5,"end":0.7,"text":"a"},{"start":0.1,"end":0.3,"text":"b"}]}"#,
        ] {
            let dir = TempDir::new();
            let server = Server::new(vec![Reply::json(response)]);
            let error = run(&dir, &server, 32_000).unwrap_err();
            assert!(error.to_string().contains("时间戳"), "{error:?}");
        }
    }

    #[test]
    fn partial_chunk_failure_returns_error_and_cleans_temporary_audio() {
        let dir = TempDir::new();
        let server = Server::new(vec![
            Reply::json(r#"{"segments":[{"start":0,"end":0.8,"text":"first"}]}"#),
            Reply::json(r#"{"text":"missing timestamps"}"#),
        ]);
        assert!(run(&dir, &server, 64_000).is_err());
        assert_eq!(server.requests.lock().unwrap().len(), 2);
        assert_eq!(fs::read_dir(dir.0.join("cache/cloud-asr")).unwrap().count(), 0);
    }

    #[test]
    fn redirect_is_rejected_without_forwarding_authorization() {
        let dir = TempDir::new();
        let target = Server::new(vec![]);
        let mut reply = Reply::json("");
        reply.status = 307;
        reply.headers = format!("Location: {}/audio/transcriptions\r\n", target.url);
        let server = Server::new(vec![reply]);
        assert!(run(&dir, &server, 32_000).is_err());
        assert_eq!(server.requests.lock().unwrap().len(), 1);
        assert!(target.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn english_translation_uses_translation_endpoint_and_no_transcription_only_fields() {
        let dir = TempDir::new();
        let server = Server::new(vec![Reply::json(r#"{"segments":[{"start":0,"end":0.8,"text":"hello"}]}"#)]);
        let wav = dir.wav(32_000);
        let cues = transcribe_in_dir(&dir.0.join("cache"), &wav, &server.config(), "zh", Some("en"), &|| false, 32_000).unwrap();
        assert_eq!(cues[0].text, "hello");
        let requests = server.requests.lock().unwrap();
        assert!(requests[0].line.starts_with("POST /v1/audio/translations "));
        let body = String::from_utf8_lossy(&requests[0].body);
        assert!(!body.contains("name=\"language\""));
        assert!(!body.contains("timestamp_granularities"));
        drop(requests);
        assert!(transcribe_in_dir(&dir.0.join("cache"), &wav, &server.config(), "zh", Some("zh"), &|| false, 32_000).is_err());
    }

    #[test]
    fn diarization_model_requests_segmented_format_and_auto_chunking() {
        let dir = TempDir::new();
        let server = Server::new(vec![Reply::json(r#"{"segments":[{"id":"seg_1","start":0,"end":0.8,"speaker":"A","text":"hello","type":"transcript.text.segment"}]}"#)]);
        let mut config = server.config();
        config.model = "gpt-4o-transcribe-diarize".into();
        let cues = transcribe_in_dir(&dir.0.join("cache"), &dir.wav(32_000), &config, "auto", None, &|| false, 32_000).unwrap();
        assert_eq!(cues[0].text, "hello");
        let requests = server.requests.lock().unwrap();
        let body = String::from_utf8_lossy(&requests[0].body);
        assert!(body.contains("name=\"response_format\"\r\n\r\ndiarized_json"));
        assert!(body.contains("name=\"chunking_strategy\"\r\n\r\nauto"));
        assert!(!body.contains("timestamp_granularities"));
    }

    #[test]
    fn cancellation_while_waiting_returns_promptly_and_worker_cleans_chunk() {
        let dir = TempDir::new();
        let mut reply = Reply::json(r#"{"segments":[{"start":0,"end":0.8,"text":"hello"}]}"#);
        reply.delay = Duration::from_secs(1);
        let server = Server::new(vec![reply]);
        let started = Instant::now();
        let error = transcribe_in_dir(&dir.0.join("cache"), &dir.wav(32_000), &server.config(), "auto", None, &|| started.elapsed() > Duration::from_millis(150), 32_000).unwrap_err();
        assert!(error.to_string().contains("取消"));
        assert!(started.elapsed() < Duration::from_millis(800));
        drop(server);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && fs::read_dir(dir.0.join("cache/cloud-asr")).unwrap().count() > 0 {
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(fs::read_dir(dir.0.join("cache/cloud-asr")).unwrap().count(), 0);
    }

    #[test]
    fn cancelled_upload_reader_stops_without_retryable_interrupt() {
        let dir = TempDir::new();
        let mut source = File::open(dir.wav(32_000)).unwrap();
        source.seek(SeekFrom::Start(44)).unwrap();
        let chunk = Arc::new(TempChunk::create(&dir.0.join("cache"), &mut source, 32_000, &|| false).unwrap());
        let file = File::open(&chunk.path).unwrap();
        let mut reader = CancelReader {
            file,
            cancelled: Arc::new(AtomicBool::new(true)),
            _chunk: chunk,
        };
        let error = reader.read(&mut [0u8; 32]).unwrap_err();
        // std::io copying retries Interrupted, which would keep a cancelled
        // upload worker alive instead of closing its file and deleting it.
        assert_ne!(error.kind(), std::io::ErrorKind::Interrupted);
    }

    #[test]
    fn temporary_chunk_lives_until_upload_reader_file_is_closed() {
        let dir = TempDir::new();
        let mut source = File::open(dir.wav(32_000)).unwrap();
        source.seek(SeekFrom::Start(44)).unwrap();
        let chunk = Arc::new(TempChunk::create(&dir.0.join("cache"), &mut source, 32_000, &|| false).unwrap());
        let path = chunk.path.clone();
        let reader = CancelReader {
            file: File::open(&path).unwrap(),
            cancelled: Arc::new(AtomicBool::new(false)),
            _chunk: chunk.clone(),
        };
        drop(chunk);
        assert!(path.is_file());
        drop(reader);
        assert!(!path.exists());
        assert_eq!(fs::read_dir(dir.0.join("cache/cloud-asr")).unwrap().count(), 0);
    }

    #[test]
    fn successful_provider_text_cannot_echo_api_key_into_subtitle() {
        let dir = TempDir::new();
        let server = Server::new(vec![Reply::json(&format!(r#"{{"segments":[{{"start":0,"end":0.8,"text":"hello {KEY}"}}]}}"#))]);
        let cues = run(&dir, &server, 32_000).unwrap();
        assert!(!cues[0].text.contains(KEY));
    }
}
