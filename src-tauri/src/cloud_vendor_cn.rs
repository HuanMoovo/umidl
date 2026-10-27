//! Tencent, Baidu and iFlytek native prerecorded ASR adapters.
//!
//! Inputs are application-owned, bounded 16 kHz mono PCM WAV chunks. All
//! returned timestamps are relative to the chunk and come from the provider.
//! The background request owns memory rather than opening the chunk file, so a
//! canceled job can remove its temporary file even on Windows.
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Cursor};
use std::net::IpAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::blocking::{Body, Client, Response};
use reqwest::Url;
use ring::hmac;
use serde_json::{json, Value};

use crate::cloud_asr::CloudAsrConfig;
use crate::ctx::Ctx;
use crate::subtitle::Cue;

const MAX_CHUNK: u64 = 20 * 1024 * 1024;
const MAX_RESPONSE: u64 = 8 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const JOB_TIMEOUT: Duration = Duration::from_secs(20 * 60);

pub fn transcribe(
    _ctx: &Ctx,
    wav: &Path,
    config: &CloudAsrConfig,
    language: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<Cue>> {
    ensure!(!cancelled(), "任务已取消");
    validate_config(config, language)?;
    let mut file = File::open(wav).map_err(|_| anyhow!("无法读取音频分块"))?;
    let length = file.metadata().map_err(|_| anyhow!("无法读取音频分块"))?.len();
    ensure!(length > 44 && length <= MAX_CHUNK, "音频分块大小无效，请重新生成任务");
    let mut audio = Vec::with_capacity(length as usize);
    let mut buffer = [0u8; 65536];
    loop {
        ensure!(!cancelled(), "任务已取消");
        let count = file.read(&mut buffer).map_err(|_| anyhow!("无法读取音频分块"))?;
        if count == 0 { break; }
        ensure!((audio.len() + count) as u64 <= MAX_CHUNK, "音频分块过大");
        audio.extend_from_slice(&buffer[..count]);
    }
    drop(file);
    ensure!(audio.len() as u64 == length, "音频分块读取不完整");
    ensure!(!cancelled(), "任务已取消");
    let provider = config.provider.clone();
    let config = config.clone();
    let language = language.to_owned();
    let flag = Arc::new(AtomicBool::new(false));
    let worker_flag = flag.clone();
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::Builder::new().name("umi-cn-asr".into()).spawn(move || {
        let result = execute(&provider, &config, &language, audio, worker_flag);
        let _ = tx.send(result);
    }).map_err(|_| anyhow!("无法启动云端语音识别"))?;
    loop {
        if cancelled() {
            flag.store(true, Ordering::Relaxed);
            bail!("任务已取消");
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => { ensure!(!cancelled(), "任务已取消"); return result; }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => bail!("云端识别任务意外结束"),
        }
    }
}

/// Validate credentials, model, language and destination before extracting audio.
pub fn validate_config(config: &CloudAsrConfig, language: &str) -> Result<()> {
    fn valid(value: &str, label: &str) -> Result<()> {
        ensure!(!value.trim().is_empty() && value.len() <= 8192 && !value.bytes().any(|b| b <= 32 || b == 127),
            "请填写有效的{label}");
        Ok(())
    }
    match config.provider.as_str() {
        "tencent" => {
            valid(&config.app_id, "腾讯云 AppID")?;
            ensure!(config.app_id.bytes().all(|b| b.is_ascii_digit()) && config.app_id.len() <= 24, "腾讯云 AppID 应为数字");
            valid(&config.secret_id, "腾讯云 SecretID")?;
            valid(&config.secret_key, "腾讯云 SecretKey")?;
            tencent_engine(&config.model, language)?;
        }
        "baidu" => {
            valid(&config.api_key, "百度 API Key")?;
            valid(&config.secret_key, "百度 Secret Key")?;
            ensure!(config.model.trim().is_empty() || config.model.trim() == "80006", "百度极速版目前使用模型 80006");
            ensure!(matches!(language, "auto" | "zh" | "zh-CN"), "百度极速版请选择中文或自动识别");
        }
        "xunfei" => {
            valid(&config.app_id, "讯飞 AppID")?;
            valid(&config.api_key, "讯飞 APIKey")?;
            valid(&config.secret_key, "讯飞 APISecret")?;
            xunfei_language(&config.model, language)?;
        }
        _ => bail!("未知的国内语音识别服务"),
    }
    endpoint_base(config)?;
    Ok(())
}

fn endpoint_base(config: &CloudAsrConfig) -> Result<(Url, bool)> {
    let host = match config.provider.as_str() {
        "tencent" => "asr.cloud.tencent.com",
        "baidu" => "aip.baidubce.com",
        "xunfei" => "office-api-ist-dx.iflyaisol.com",
        _ => bail!("未知的国内语音识别服务"),
    };
    let raw = if config.base_url.trim().is_empty() { format!("https://{host}") } else { config.base_url.trim().to_owned() };
    let url = Url::parse(&raw).map_err(|_| anyhow!("服务地址无效"))?;
    let userinfo = raw.split_once("://").map(|(_, rest)| rest.split(['/', '?', '#', '\\']).next().unwrap_or("").contains('@')).unwrap_or(false);
    ensure!(!userinfo && url.username().is_empty() && url.password().is_none() && url.query().is_none() && url.fragment().is_none(), "服务地址不能包含凭证、参数或片段");
    let actual_host = url.host_str().ok_or_else(|| anyhow!("服务地址缺少域名"))?;
    let loopback = actual_host.eq_ignore_ascii_case("localhost") || actual_host.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false);
    ensure!((url.scheme() == "https" && actual_host.eq_ignore_ascii_case(host)) || (loopback && matches!(url.scheme(), "http" | "https")), "该服务只支持其官方 HTTPS 地址");
    ensure!(url.path().is_empty() || url.path() == "/", "服务地址仅填写域名，无需输入接口路径");
    Ok((url, loopback))
}

fn client(loopback: bool) -> Result<Client> {
    let mut builder = Client::builder().redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15)).timeout(REQUEST_TIMEOUT);
    if loopback { builder = builder.no_proxy(); }
    builder.build().map_err(|_| anyhow!("无法初始化云端连接"))
}

struct CancelReader {
    bytes: Cursor<Vec<u8>>,
    canceled: Arc<AtomicBool>,
}
impl Read for CancelReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.canceled.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "canceled"));
        }
        self.bytes.read(buf)
    }
}

fn send_bytes(client: &Client, url: Url, headers: &[(&str, &str)], bytes: Vec<u8>, cancel: &Arc<AtomicBool>) -> Result<Value> {
    ensure!(!cancel.load(Ordering::Relaxed), "任务已取消");
    let length = bytes.len();
    let reader = CancelReader { bytes: Cursor::new(bytes), canceled: cancel.clone() };
    let mut request = client.post(url).body(Body::sized(reader, length as u64));
    for (name, value) in headers { request = request.header(*name, *value); }
    let response = request.send().map_err(|_| anyhow!("云端连接失败或超时，请检查网络"))?;
    ensure!(!cancel.load(Ordering::Relaxed), "任务已取消");
    read_response(response)
}

fn read_response(response: Response) -> Result<Value> {
    let status = response.status();
    if !status.is_success() {
        match status.as_u16() {
            401 | 403 => bail!("云端鉴权失败或账号未开通该服务，请检查凭证和权限"),
            402 => bail!("云端额度不足，请检查服务商账户余额"),
            429 => bail!("云端请求过于频繁，请稍后重试"),
            code if code >= 500 => bail!("云端服务暂不可用，请稍后重试（HTTP {code}）"),
            code => bail!("云端服务拒绝请求（HTTP {code}），请检查模型和参数"),
        }
    }
    let mut bytes = Vec::new();
    response.take(MAX_RESPONSE + 1).read_to_end(&mut bytes).map_err(|_| anyhow!("读取云端识别结果失败"))?;
    ensure!(bytes.len() as u64 <= MAX_RESPONSE, "云端识别结果过大");
    serde_json::from_slice(&bytes).map_err(|_| anyhow!("云端识别结果格式无效"))
}

fn execute(provider: &str, cfg: &CloudAsrConfig, language: &str, audio: Vec<u8>, cancel: Arc<AtomicBool>) -> Result<Vec<Cue>> {
    let (base, loopback) = endpoint_base(cfg)?;
    let client = client(loopback)?;
    match provider {
        "tencent" => tencent(&client, base, cfg, language, audio, &cancel),
        "baidu" => baidu(&client, base, cfg, audio, &cancel),
        "xunfei" => xunfei(&client, base, cfg, language, audio, &cancel),
        _ => bail!("未知的国内语音识别服务"),
    }
}

fn sign_sha1(key: &[u8], message: &[u8]) -> String {
    let signing_key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, key);
    STANDARD.encode(hmac::sign(&signing_key, message).as_ref())
}

fn encoded_query(items: &[(&str, &str)]) -> String {
    let mut pairs = items.to_vec();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    let mut url = Url::parse("https://example.invalid/").expect("static URL");
    { let mut query = url.query_pairs_mut(); for (key, value) in pairs { if !value.is_empty() { query.append_pair(key, value); } } }
    url.query().unwrap_or("").to_owned()
}

fn tencent_signing_parts(app_id: &str, items: &[(&str, &str)]) -> (String, String) {
    let query = encoded_query(items);
    (format!("POSTasr.cloud.tencent.com/asr/flash/v1/{app_id}?{query}"), query)
}

fn tencent_engine(model: &str, language: &str) -> Result<String> {
    let model = model.trim();
    if !model.is_empty() && model != "16k_zh" {
        ensure!(model.len() <= 80 && model.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')), "腾讯云识别引擎名称无效");
        return Ok(model.to_owned());
    }
    let suffix = match language {
        "zh" | "zh-CN" => "zh", "en" => "en", "yue" => "yue", "ja" => "ja", "ko" => "ko",
        "vi" => "vi", "ms" => "ms", "id" => "id", "fil" => "fil", "th" => "th", "pt" => "pt",
        "tr" => "tr", "ar" => "ar", "es" => "es", "hi" => "hi", "fr" => "fr", "de" => "de",
        _ => bail!("腾讯云请先选择支持的识别语言"),
    };
    Ok(format!("16k_{suffix}"))
}

fn tencent(client: &Client, mut base: Url, cfg: &CloudAsrConfig, language: &str, audio: Vec<u8>, cancel: &Arc<AtomicBool>) -> Result<Vec<Cue>> {
    let engine = tencent_engine(&cfg.model, language)?;
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| anyhow!("系统时间无效"))?.as_secs().to_string();
    let items = [("engine_type", engine.as_str()), ("secretid", cfg.secret_id.as_str()), ("timestamp", timestamp.as_str()), ("voice_format", "wav"), ("word_info", "0")];
    let (canonical, query) = tencent_signing_parts(&cfg.app_id, &items);
    let signature = sign_sha1(cfg.secret_key.as_bytes(), canonical.as_bytes());
    base.set_path(&format!("/asr/flash/v1/{}", cfg.app_id));
    base.set_query(Some(&query));
    let result = send_bytes(client, base, &[("Authorization", &signature), ("Content-Type", "application/octet-stream")], audio, cancel)?;
    parse_tencent(&result)
}

fn baidu(client: &Client, mut base: Url, cfg: &CloudAsrConfig, audio: Vec<u8>, cancel: &Arc<AtomicBool>) -> Result<Vec<Cue>> {
    base.set_path("/oauth/2.0/token");
    // Baidu's documented OAuth endpoint requires URL query credentials. The
    // client restricts the destination, disables redirects and never exposes
    // request URLs or raw transport errors to logs or the UI.
    base.query_pairs_mut().append_pair("grant_type", "client_credentials")
        .append_pair("client_id", &cfg.api_key).append_pair("client_secret", &cfg.secret_key);
    let token_response = send_bytes(client, base.clone(), &[("Content-Type", "application/json")], Vec::new(), cancel)?;
    let token = token_response.get("access_token").and_then(Value::as_str).filter(|v| !v.is_empty() && v.len() <= 8192).ok_or_else(|| anyhow!("百度鉴权失败，请检查 API Key、Secret Key 和产品权限"))?;
    let body = serde_json::to_vec(&json!({"access_token":token,"rate":16000,"cuid":format!("umidl-{}", uuid::Uuid::new_v4()), "dev_pid":80006,
        "speech":STANDARD.encode(audio), "enable_subtitle":1, "subtitle_punc":1, "smooth_text":1}))
        .map_err(|_| anyhow!("无法准备百度识别请求"))?;
    base.set_path("/rest/2.0/speech/publiccloudspeech/v1/asr/topspeed");
    base.set_query(None);
    let result = send_bytes(client, base, &[("Content-Type", "application/json")], body, cancel)?;
    parse_baidu(&result)
}

fn xunfei_language(model: &str, language: &str) -> Result<&'static str> {
    let selected = match model.trim() { "" | "autodialect" => "autodialect", "autominor" => "autominor", _ => bail!("讯飞模型仅支持 autodialect 或 autominor") };
    if selected == "autodialect" { ensure!(matches!(language, "auto" | "zh" | "zh-CN" | "en"), "讯飞 autodialect 支持自动识别中英和方言"); }
    else { ensure!(language == "auto", "讯飞 autominor 请使用自动检测语种"); }
    Ok(selected)
}

fn xunfei_signing_query(items: &[(&str, &str)]) -> String { encoded_query(items) }

fn xunfei_date() -> String { chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%z").to_string() }

fn xunfei_url(mut base: Url, path: &str, entries: &BTreeMap<String, String>, secret: &str) -> (Url, String) {
    let items: Vec<(&str, &str)> = entries.iter().map(|(key,value)| (key.as_str(), value.as_str())).collect();
    let query = xunfei_signing_query(&items);
    let signature = sign_sha1(secret.as_bytes(), query.as_bytes());
    base.set_path(path);
    base.set_query(Some(&query));
    (base, signature)
}

fn xunfei(client: &Client, base: Url, cfg: &CloudAsrConfig, language: &str, audio: Vec<u8>, cancel: &Arc<AtomicBool>) -> Result<Vec<Cue>> {
    let selected_language = xunfei_language(&cfg.model, language)?;
    let random = uuid::Uuid::new_v4().simple().to_string()[..16].to_owned();
    let mut upload = BTreeMap::new();
    upload.insert("appId".into(), cfg.app_id.clone());
    upload.insert("accessKeyId".into(), cfg.api_key.clone());
    upload.insert("dateTime".into(), xunfei_date());
    upload.insert("signatureRandom".into(), random.clone());
    upload.insert("fileSize".into(), audio.len().to_string());
    upload.insert("fileName".into(), "audio.wav".into());
    upload.insert("durationCheckDisable".into(), "true".into());
    upload.insert("language".into(), selected_language.into());
    let (url, signature) = xunfei_url(base.clone(), "/v2/upload", &upload, &cfg.secret_key);
    let result = send_bytes(client, url, &[("signature", &signature), ("Content-Type", "application/octet-stream")], audio, cancel)?;
    xunfei_check_code(&result)?;
    let order = result.pointer("/content/orderId").and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 256 && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')))
        .ok_or_else(|| anyhow!("讯飞未返回有效的转写订单"))?.to_owned();
    let deadline = Instant::now() + JOB_TIMEOUT;
    loop {
        ensure!(!cancel.load(Ordering::Relaxed), "任务已取消");
        ensure!(Instant::now() < deadline, "讯飞转写等待超时，请检查服务后重试");
        let mut params = BTreeMap::new();
        params.insert("accessKeyId".into(), cfg.api_key.clone());
        params.insert("dateTime".into(), xunfei_date());
        params.insert("signatureRandom".into(), random.clone());
        params.insert("orderId".into(), order.clone());
        params.insert("resultType".into(), "transfer".into());
        let (url, signature) = xunfei_url(base.clone(), "/v2/getResult", &params, &cfg.secret_key);
        let result = send_bytes(client, url, &[("signature", &signature), ("Content-Type", "application/json")], b"{}".to_vec(), cancel)?;
        xunfei_check_code(&result)?;
        let state = result.pointer("/content/orderInfo/status").and_then(Value::as_i64).ok_or_else(|| anyhow!("讯飞返回的订单状态无效"))?;
        match state {
            4 => {
                let raw = result.pointer("/content/orderResult").and_then(Value::as_str).ok_or_else(|| anyhow!("讯飞未返回转写结果"))?;
                return parse_xunfei_result(raw);
            }
            -1 => bail!("讯飞转写失败，请检查音频、服务权限和账户额度"),
            0 | 1 | 2 | 3 => {
                for _ in 0..20 {
                    ensure!(!cancel.load(Ordering::Relaxed), "任务已取消");
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            _ => bail!("讯飞返回未知的订单状态"),
        }
    }
}

fn xunfei_check_code(value: &Value) -> Result<()> {
    let code = value.get("code");
    let success = code.and_then(Value::as_str) == Some("000000") || code.and_then(Value::as_i64) == Some(0);
    ensure!(success, "讯飞请求失败，请检查 API 凭证、产品开通状态和账户额度");
    Ok(())
}

fn timed_cue(start: Option<u64>, end: Option<u64>, text: &str, index: usize) -> Result<Cue> {
    let (start, end) = (start.ok_or_else(|| anyhow!("识别结果缺少时间戳"))?, end.ok_or_else(|| anyhow!("识别结果缺少时间戳"))?);
    ensure!(end > start && !text.trim().is_empty(), "识别结果的时间戳或文字无效");
    Ok(Cue { index, start_ms: start, end_ms: end, text: text.trim().to_owned() })
}

fn parse_tencent(value: &Value) -> Result<Vec<Cue>> {
    let code = value.get("code").and_then(Value::as_i64).ok_or_else(|| anyhow!("腾讯云返回格式无效"))?;
    ensure!(code == 0, "腾讯云识别失败（错误码 {code}），请检查账户权限、额度和模型");
    let channels = value.get("flash_result").and_then(Value::as_array).ok_or_else(|| anyhow!("腾讯云未返回识别结果"))?;
    let Some(channel) = channels.first() else { return Ok(Vec::new()); };
    let full_text = channel.get("text").and_then(Value::as_str).unwrap_or("").trim();
    let sentences = channel.get("sentence_list").and_then(Value::as_array).ok_or_else(|| anyhow!("腾讯云结果缺少分句时间戳"))?;
    let mut cues = Vec::new();
    for sentence in sentences {
        let text = sentence.get("text").and_then(Value::as_str).ok_or_else(|| anyhow!("腾讯云分句缺少文字"))?;
        if !text.trim().is_empty() {
            cues.push(timed_cue(sentence.get("start_time").and_then(Value::as_u64), sentence.get("end_time").and_then(Value::as_u64), text, cues.len() + 1)?);
        }
    }
    ensure!(!cues.is_empty() || full_text.is_empty(), "腾讯云返回文字但缺少有效时间戳");
    Ok(cues)
}

fn parse_baidu(value: &Value) -> Result<Vec<Cue>> {
    let code = value.get("error_code").and_then(Value::as_i64).unwrap_or(0);
    ensure!(code == 0, "百度转写失败（错误码 {code}），请检查极速版邀测权限、额度和凭证");
    ensure!(value.get("result").and_then(Value::as_array).is_some() || value.get("detailed_result").and_then(Value::as_array).is_some(), "百度未返回有效的识别结果");
    let full_text = value.pointer("/result/0").and_then(Value::as_str).unwrap_or("").trim();
    let mut cues = Vec::new();
    if let Some(sentences) = value.get("detailed_result").and_then(Value::as_array) {
        for sentence in sentences {
            let text = sentence.pointer("/res/0").and_then(Value::as_str).ok_or_else(|| anyhow!("百度分句缺少文字"))?;
            if !text.trim().is_empty() {
                cues.push(timed_cue(sentence.get("begin_time").and_then(Value::as_u64), sentence.get("end_time").and_then(Value::as_u64), text, cues.len() + 1)?);
            }
        }
    }
    ensure!(!cues.is_empty() || full_text.is_empty(), "百度返回文字但缺少有效分句时间戳");
    Ok(cues)
}

fn parse_xunfei_result(raw: &str) -> Result<Vec<Cue>> {
    let result: Value = serde_json::from_str(raw).map_err(|_| anyhow!("讯飞转写结果格式无效"))?;
    let lattice = result.get("lattice").and_then(Value::as_array).ok_or_else(|| anyhow!("讯飞转写结果缺少分句时间戳"))?;
    let mut cues = Vec::new();
    for element in lattice {
        let raw = element.get("json_1best").and_then(Value::as_str).ok_or_else(|| anyhow!("讯飞分句格式无效"))?;
        let item: Value = serde_json::from_str(raw).map_err(|_| anyhow!("讯飞分句格式无效"))?;
        let st = item.get("st").ok_or_else(|| anyhow!("讯飞分句格式无效"))?;
        let mut text = String::new();
        let results = st.get("rt").and_then(Value::as_array).ok_or_else(|| anyhow!("讯飞分句文字缺失"))?;
        for result in results {
            let words = result.get("ws").and_then(Value::as_array).ok_or_else(|| anyhow!("讯飞分句文字格式无效"))?;
            for word in words {
                let value = word.pointer("/cw/0/w").and_then(Value::as_str).ok_or_else(|| anyhow!("讯飞分词文字缺失"))?;
                text.push_str(value);
            }
        }
        if !text.trim().is_empty() {
            let time = |name: &str| st.get(name).and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok())));
            cues.push(timed_cue(time("bg"), time("ed"), &text, cues.len() + 1)?);
        }
    }
    Ok(cues)
}

#[cfg(test)]
#[path = "cloud_vendor_cn_tests.rs"]
mod tests;
