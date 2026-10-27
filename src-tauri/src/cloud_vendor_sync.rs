//! Native Fish, Deepgram and Azure adapters. All returned times are chunk-local.
use std::io::{Cursor, Read};
use std::net::IpAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use reqwest::blocking::{multipart, Body, Client};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Url;
use serde_json::{json, Value};
use crate::cloud_asr::CloudAsrConfig;
use crate::ctx::Ctx;
use crate::subtitle::Cue;

const MAX_WAV_BYTES: u64 = 20_000_000;
const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SEGMENTS: usize = 50_000;

pub fn transcribe(_ctx: &Ctx, wav: &Path, config: &CloudAsrConfig, language: &str,
                  cancelled: &dyn Fn() -> bool) -> anyhow::Result<Vec<Cue>> {
    transcribe_chunk(wav, config, language, cancelled)
}

fn transcribe_chunk(wav: &Path, config: &CloudAsrConfig, language: &str,
                    cancelled: &dyn Fn() -> bool) -> anyhow::Result<Vec<Cue>> {
    if cancelled() { anyhow::bail!("云端字幕识别已取消"); }
    anyhow::ensure!(matches!(config.provider.as_str(), "fish" | "deepgram" | "azure"), "未知的云端服务商");
    let (endpoint, loopback) = checked_endpoint(config, language)?;
    let key = config.api_key.trim();
    anyhow::ensure!(loopback || !key.is_empty(), "请填写当前服务商的 API 密钥");
    anyhow::ensure!(key.len() <= 8192 && !key.bytes().any(|b| b <= 0x20 || b == 0x7f), "API 密钥格式无效");
    let metadata = std::fs::metadata(wav).map_err(|_| anyhow::anyhow!("无法读取音频文件"))?;
    anyhow::ensure!(metadata.len() <= MAX_WAV_BYTES, "音频分块过大，请减小分块长度后重试");
    let source = std::fs::File::open(wav).map_err(|_| anyhow::anyhow!("无法读取音频文件"))?;
    let mut bytes = Vec::new();
    source.take(MAX_WAV_BYTES + 1).read_to_end(&mut bytes).map_err(|_| anyhow::anyhow!("无法读取音频文件"))?;
    anyhow::ensure!(bytes.len() as u64 <= MAX_WAV_BYTES, "音频分块过大，请减小分块长度后重试");
    let duration_ms = wav_duration_ms(&bytes)?;
    if cancelled() { anyhow::bail!("云端字幕识别已取消"); }
    let mut builder = Client::builder().redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(180));
    if loopback { builder = builder.no_proxy(); }
    let client = builder.build().map_err(|_| anyhow::anyhow!("无法初始化云端连接"))?;
    let stop = CancelGuard(Arc::new(AtomicBool::new(false)));
    let stop_worker = stop.0.clone();
    let provider = config.provider.clone();
    let key = key.to_owned();
    let model = config.model.trim().to_owned();
    let language = language.to_owned();
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::Builder::new().name("umi-native-cloud-asr".into()).spawn(move || {
        let result = request(client, endpoint, &provider, &key, &model, &language, bytes, stop_worker);
        let _ = tx.send(result);
    }).map_err(|_| anyhow::anyhow!("无法启动云端识别请求"))?;
    loop {
        if cancelled() { anyhow::bail!("云端字幕识别已取消"); }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => {
                if cancelled() { anyhow::bail!("云端字幕识别已取消"); }
                return parse_response(&config.provider, &result?, duration_ms, config.api_key.trim());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => anyhow::bail!("云端识别请求意外结束，请重试"),
        }
    }
}

struct CancelGuard(Arc<AtomicBool>);
impl Drop for CancelGuard {
    fn drop(&mut self) { self.0.store(true, Ordering::Relaxed); }
}

struct CancelReader { data: Cursor<Vec<u8>>, cancelled: Arc<AtomicBool> }
impl Read for CancelReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "cancelled"));
        }
        self.data.read(buf)
    }
}

fn checked_endpoint(config: &CloudAsrConfig, language: &str) -> anyhow::Result<(Url, bool)> {
    let mut endpoint = Url::parse(config.base_url.trim()).map_err(|_| anyhow::anyhow!("服务地址无效，请检查 HTTPS 地址"))?;
    let has_userinfo = config.base_url.trim().split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#', '\\']).next().unwrap_or("").contains('@')).unwrap_or(false);
    anyhow::ensure!(endpoint.username().is_empty() && endpoint.password().is_none() && !has_userinfo,
                    "服务地址不能包含用户名或密码");
    anyhow::ensure!(endpoint.query().is_none() && endpoint.fragment().is_none(), "服务地址不能包含查询参数或片段");
    let host = endpoint.host_str().ok_or_else(|| anyhow::anyhow!("服务地址缺少主机"))?;
    let loopback = host.eq_ignore_ascii_case("localhost") || host.trim_matches(['[', ']'])
        .parse::<IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false);
    anyhow::ensure!(endpoint.scheme() == "https" || (endpoint.scheme() == "http" && loopback), "远程云端服务必须使用 HTTPS");
    let model = config.model.trim();
    anyhow::ensure!(model.len() <= 200 && !model.chars().any(char::is_control), "云端模型名称无效");
    let path = endpoint.path().trim_end_matches('/');
    let path = match config.provider.as_str() {
        "fish" => format!("{path}/v1/asr"),
        "deepgram" => format!("{path}/v1/listen"),
        "azure" => format!("{path}/speechtotext/transcriptions:transcribe"),
        _ => anyhow::bail!("未知的云端服务商"),
    };
    endpoint.set_path(&path);
    match config.provider.as_str() {
        "deepgram" => {
            let model = if model.is_empty() { "nova-3" } else { model };
            let mut query = endpoint.query_pairs_mut();
            query.append_pair("model", model).append_pair("utterances", "true").append_pair("smart_format", "true");
            if language.eq_ignore_ascii_case("auto") || language.is_empty() {
                if model.starts_with("nova-3") { query.append_pair("language", "multi"); }
                else { query.append_pair("detect_language", "true"); }
            } else {
                let language = if language == "yue" { "zh-HK" } else { language };
                anyhow::ensure!(valid_language(language), "Deepgram 语言代码无效");
                query.append_pair("language", language);
            }
        }
        "azure" => { endpoint.query_pairs_mut().append_pair("api-version", "2025-10-15"); }
        _ => {}
    }
    Ok((endpoint, loopback))
}

fn valid_language(language: &str) -> bool {
    !language.is_empty() && language.len() <= 16 && language.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn azure_locale(language: &str) -> anyhow::Result<Option<String>> {
    if language.is_empty() || language.eq_ignore_ascii_case("auto") { return Ok(None); }
    let locale = match language {
        "zh" => "zh-CN", "yue" => "zh-HK", "en" => "en-US", "ja" => "ja-JP", "ko" => "ko-KR",
        "fr" => "fr-FR", "de" => "de-DE", "es" => "es-ES", "ru" => "ru-RU", "it" => "it-IT", "pt" => "pt-BR",
        value if value.contains('-') && valid_language(value) => value,
        _ => anyhow::bail!("Azure 需要支持的语言地区代码，例如 zh-CN"),
    };
    Ok(Some(locale.to_owned()))
}

fn wav_duration_ms(wav: &[u8]) -> anyhow::Result<u64> {
    let invalid = || anyhow::anyhow!("音频 WAV 无效，需要 16kHz 单声道 16 位 PCM");
    if wav.len() < 12 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" { return Err(invalid()); }
    let riff_end = u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize + 8;
    if riff_end > wav.len() || riff_end < 12 { return Err(invalid()); }
    let mut offset = 12usize; let mut correct_format = false; let mut data_size = None;
    while offset + 8 <= riff_end {
        let tag = &wav[offset..offset+4];
        let length = u32::from_le_bytes(wav[offset+4..offset+8].try_into().unwrap()) as usize;
        let start = offset + 8; let end = start.checked_add(length).ok_or_else(invalid)?;
        if end > riff_end { return Err(invalid()); }
        if tag == b"fmt " && length >= 16 {
            correct_format = u16::from_le_bytes(wav[start..start+2].try_into().unwrap()) == 1
                && u16::from_le_bytes(wav[start+2..start+4].try_into().unwrap()) == 1
                && u32::from_le_bytes(wav[start+4..start+8].try_into().unwrap()) == 16000
                && u16::from_le_bytes(wav[start+14..start+16].try_into().unwrap()) == 16;
        } else if tag == b"data" { data_size = Some(length); }
        offset = end.checked_add(length % 2).ok_or_else(invalid)?;
    }
    let data_size = data_size.ok_or_else(invalid)?;
    if !correct_format || data_size == 0 || data_size % 2 != 0 { return Err(invalid()); }
    Ok((data_size as u64 * 1000) / 32000)
}

fn request(client: Client, endpoint: Url, provider: &str, key: &str, model: &str, language: &str,
           bytes: Vec<u8>, cancelled: Arc<AtomicBool>) -> anyhow::Result<Vec<u8>> {
    let length = bytes.len() as u64;
    let reader = CancelReader { data: Cursor::new(bytes), cancelled: cancelled.clone() };
    let mut headers = HeaderMap::new();
    let auth = match provider { "fish" => format!("Bearer {key}"), "deepgram" => format!("Token {key}"), _ => key.to_owned() };
    let mut auth_value = HeaderValue::from_str(&auth).map_err(|_| anyhow::anyhow!("API 密钥格式无效"))?;
    auth_value.set_sensitive(true);
    if provider == "azure" { headers.insert("ocp-apim-subscription-key", auth_value); }
    else { headers.insert(AUTHORIZATION, auth_value); }
    let mut builder = client.post(endpoint).headers(headers);
    if provider == "deepgram" {
        builder = builder.header(CONTENT_TYPE, "audio/wav").body(Body::sized(reader, length));
    } else {
        let part = multipart::Part::reader_with_length(reader, length).file_name("audio.wav")
            .mime_str("audio/wav").map_err(|_| anyhow::anyhow!("无法创建音频请求"))?;
        let mut form = multipart::Form::new().part("audio", part);
        if provider == "fish" {
            let model = if model.is_empty() { "transcribe-1-pro" } else { model };
            anyhow::ensure!(matches!(model, "transcribe-1-pro" | "transcribe-1"), "Fish 语音识别模型无效，请选择 transcribe-1-pro 或 transcribe-1");
            builder = builder.header("model", HeaderValue::from_str(model).map_err(|_| anyhow::anyhow!("模型名称无效"))?);
            form = form.text("ignore_timestamps", "false");
            if model == "transcribe-1-pro" { form = form.text("diarize", "false").text("tag_audio_events", "false"); }
            if !language.is_empty() && !language.eq_ignore_ascii_case("auto") {
                let code = language.split('-').next().unwrap_or("").to_ascii_lowercase();
                anyhow::ensure!(code.len() == 2 && code.bytes().all(|b| b.is_ascii_lowercase()), "Fish 需要两位语言代码，例如 zh 或 en");
                form = form.text("language", code);
            }
        } else if provider == "azure" {
            let definition = match azure_locale(language)? {
                Some(locale) => json!({"locales": [locale]}), None => json!({}),
            };
            form = form.text("definition", definition.to_string());
        }
        builder = builder.multipart(form);
    }
    if cancelled.load(Ordering::Relaxed) { anyhow::bail!("云端字幕识别已取消"); }
    let mut response = builder.send().map_err(|error| {
        if error.is_timeout() { anyhow::anyhow!("云端识别请求超时，请检查网络或稍后重试") }
        else if error.is_connect() { anyhow::anyhow!("无法连接云端识别服务，请检查地址和网络") }
        else { anyhow::anyhow!("云端识别请求失败，请检查服务配置或网络") }
    })?;
    let status = response.status();
    if !status.is_success() {
        let reason = if status.is_redirection() { "服务返回重定向，请检查服务地址" }
            else { match status.as_u16() {
                401 | 403 => "认证失败，请检查 API 密钥和服务权限",
                402 => "API 额度不足，请检查该服务的 API 余额",
                413 => "音频分块超过服务商允许的大小",
                429 => "服务限流或额度不足，请检查额度后重试",
                400 | 404 | 422 => "服务不支持当前接口、模型或语言，请检查配置",
                _ => "服务处理失败，请稍后重试",
            } };
        anyhow::bail!("云端识别失败（HTTP {}）：{}", status.as_u16(), reason);
    }
    if response.content_length().map(|len| len > MAX_RESPONSE_BYTES).unwrap_or(false) {
        anyhow::bail!("云端识别响应过大，已停止处理");
    }
    let mut body = Vec::new(); let mut buffer = [0u8; 65536];
    loop {
        if cancelled.load(Ordering::Relaxed) { anyhow::bail!("云端字幕识别已取消"); }
        let size = response.read(&mut buffer).map_err(|_| anyhow::anyhow!("读取云端识别响应失败，请重试"))?;
        if size == 0 { break; }
        if body.len() as u64 + size as u64 > MAX_RESPONSE_BYTES { anyhow::bail!("云端识别响应过大，已停止处理"); }
        body.extend_from_slice(&buffer[..size]);
    }
    Ok(body)
}

fn clean_text(text: &str, key: &str) -> String {
    if key.is_empty() { return text.to_owned(); }
    let encoded: String = key.bytes().map(|byte| format!("%{byte:02X}")).collect();
    text.replace(key, "[REDACTED]").replace(&encoded, "[REDACTED]").replace(&encoded.to_ascii_lowercase(), "[REDACTED]")
}

fn millis(value: Option<&Value>, multiplier: f64, duration_ms: u64) -> anyhow::Result<u64> {
    let seconds = value.and_then(Value::as_f64).ok_or_else(|| anyhow::anyhow!("云端返回的时间戳无效"))?;
    anyhow::ensure!(seconds.is_finite() && seconds >= 0.0 && seconds * multiplier <= duration_ms as f64 + 10.0,
                    "云端返回的时间戳超出音频范围");
    Ok((seconds * multiplier).round() as u64)
}

fn append_text(out: &mut String, next: &str) {
    let next = next.trim(); if next.is_empty() { return; }
    if out.chars().last().map(|c| c.is_ascii_alphanumeric() || matches!(c, ',' | '.' | '!' | '?' | ';' | ':' | ')')).unwrap_or(false)
        && next.chars().next().map(|c| c.is_ascii_alphanumeric()).unwrap_or(false) { out.push(' '); }
    out.push_str(next);
}

fn words_to_cues(words: &[Value], text_key: &str, duration: u64, key: &str) -> anyhow::Result<Vec<Cue>> {
    anyhow::ensure!(words.len() <= MAX_SEGMENTS, "云端返回的时间戳分段数量异常");
    let mut output = Vec::new();
    let mut text = String::new(); let mut start = 0u64; let mut end = 0u64; let mut last = 0u64;
    for item in words {
        let a = millis(item.get("start"), 1000.0, duration)?;
        let b = millis(item.get("end"), 1000.0, duration)?;
        anyhow::ensure!(b >= a && (text.is_empty() || a >= last), "云端返回的时间戳顺序无效");
        last = a;
        let part = item.get(text_key).and_then(Value::as_str).or_else(|| item.get("word").and_then(Value::as_str))
            .ok_or_else(|| anyhow::anyhow!("云端返回的分段文本无效"))?;
        if part.trim().is_empty() { continue; }
        if !text.is_empty() && end > start && (a > end.saturating_add(800) || b.saturating_sub(start) > 5000 || text.chars().count() >= 45) {
            output.push(Cue { index: output.len()+1, start_ms: start, end_ms: end, text: clean_text(&text, key) });
            text.clear();
        }
        if text.is_empty() { start = a; end = b; } else { end = end.max(b); }
        append_text(&mut text, part);
    }
    if !text.is_empty() {
        anyhow::ensure!(end > start, "云端未返回有效的分段时间戳");
        output.push(Cue { index: output.len()+1, start_ms: start, end_ms: end, text: clean_text(&text, key) });
    }
    Ok(output)
}

fn parse_response(provider: &str, body: &[u8], duration_ms: u64, key: &str) -> anyhow::Result<Vec<Cue>> {
    let result: Value = serde_json::from_slice(body).map_err(|_| anyhow::anyhow!("云端返回的识别结果格式无效"))?;
    let mut cues = Vec::new();
    match provider {
        "fish" => {
            let words = result.get("segments").and_then(Value::as_array).ok_or_else(|| anyhow::anyhow!("Fish 未返回时间戳"))?;
            cues = words_to_cues(words, "text", duration_ms, key)?;
            if cues.is_empty() && result.get("text").and_then(Value::as_str).map(|s| !s.trim().is_empty()).unwrap_or(false) {
                anyhow::bail!("Fish 未返回可用的时间戳");
            }
        }
        "deepgram" => {
            if let Some(utterances) = result.pointer("/results/utterances").and_then(Value::as_array).filter(|u| !u.is_empty()) {
                anyhow::ensure!(utterances.len() <= MAX_SEGMENTS, "云端返回的时间戳分段数量异常");
                let mut last = 0;
                for item in utterances {
                    let start = millis(item.get("start"), 1000.0, duration_ms)?;
                    let end = millis(item.get("end"), 1000.0, duration_ms)?;
                    let text = item.get("transcript").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("Deepgram 未返回分段文本"))?.trim();
                    anyhow::ensure!(end > start && start >= last, "Deepgram 返回的时间戳无效"); last = start;
                    if !text.is_empty() { cues.push(Cue { index: cues.len()+1, start_ms: start, end_ms: end, text: clean_text(text, key) }); }
                }
            } else if let Some(alternative) = result.pointer("/results/channels/0/alternatives/0") {
                if let Some(words) = alternative.get("words").and_then(Value::as_array) {
                    cues = words_to_cues(words, "punctuated_word", duration_ms, key)?;
                }
            } else { anyhow::bail!("Deepgram 未返回有效识别结果"); }
            if cues.is_empty() && result.pointer("/results/channels/0/alternatives/0/transcript")
                .and_then(Value::as_str).map(|s| !s.trim().is_empty()).unwrap_or(false) {
                anyhow::bail!("Deepgram 未返回可用的时间戳");
            }
        }
        "azure" => {
            let phrases = result.get("phrases").and_then(Value::as_array).ok_or_else(|| anyhow::anyhow!("Azure 未返回分段时间戳"))?;
            anyhow::ensure!(phrases.len() <= MAX_SEGMENTS, "云端返回的时间戳分段数量异常");
            let mut last = 0;
            for item in phrases {
                let start = millis(item.get("offsetMilliseconds"), 1.0, duration_ms)?;
                let length = millis(item.get("durationMilliseconds"), 1.0, duration_ms)?;
                let end = start.checked_add(length).ok_or_else(|| anyhow::anyhow!("Azure 返回的时间戳无效"))?;
                let text = item.get("text").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("Azure 未返回分段文本"))?.trim();
                anyhow::ensure!(end > start && end <= duration_ms + 10 && start >= last, "Azure 返回的时间戳无效"); last = start;
                if !text.is_empty() { cues.push(Cue { index: cues.len()+1, start_ms: start, end_ms: end, text: clean_text(text, key) }); }
            }
            if cues.is_empty() && result.get("combinedPhrases").and_then(Value::as_array).map(|a| a.iter().any(|p| p.get("text").and_then(Value::as_str).map(|t| !t.trim().is_empty()).unwrap_or(false))).unwrap_or(false) {
                anyhow::bail!("Azure 未返回可用的时间戳");
            }
        }
        _ => anyhow::bail!("未知的云端服务商"),
    }
    Ok(cues)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    const KEY: &str = "synthetic-western-test-key";

    fn config(provider: &str, base_url: String) -> CloudAsrConfig {
        serde_json::from_value(serde_json::json!({
            "provider": provider, "base_url": base_url,
            "model": match provider { "fish" => "transcribe-1-pro", "deepgram" => "nova-3", _ => "" },
            "api_key": KEY, "app_id":"", "secret_id":"", "secret_key":""
        })).unwrap()
    }

    fn wav() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("western-asr-test-{}-{:?}.wav", std::process::id(), std::thread::current().id()));
        let mut bytes = b"RIFF".to_vec(); bytes.extend_from_slice(&32036u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0"); bytes.extend_from_slice(&16000u32.to_le_bytes());
        bytes.extend_from_slice(&32000u32.to_le_bytes()); bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes()); bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&32000u32.to_le_bytes()); bytes.resize(32044, 0);
        std::fs::write(&path, bytes).unwrap(); path
    }

    fn mock(status: &str, response: &str) -> (String, std::sync::mpsc::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let (send, recv) = std::sync::mpsc::channel();
        let status = status.to_owned(); let response = response.to_owned();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut request = Vec::new(); let mut block = [0u8; 8192];
            let (head_end, length) = loop {
                let n = socket.read(&mut block).unwrap(); assert_ne!(n, 0);
                request.extend_from_slice(&block[..n]);
                if let Some(pos) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head_end = pos + 4;
                    let header = String::from_utf8_lossy(&request[..head_end]);
                    let length = header.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").map(|n| n.trim().parse::<usize>().unwrap())).unwrap_or(0);
                    break (head_end, length);
                }
            };
            while request.len() < head_end + length {
                let n = socket.read(&mut block).unwrap(); if n == 0 { break; } request.extend_from_slice(&block[..n]);
            }
            let response_wire = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nLocation: http://127.0.0.1/never\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.as_bytes().len());
            socket.write_all(response_wire.as_bytes()).unwrap(); let _ = send.send(request);
        });
        (address, recv)
    }

    #[test]
    fn fish_merges_zero_duration_words_without_fabricating_time() {
        let json = r#"{"text":"你好，","segments":[{"text":"你","start":0.1,"end":0.1},{"text":"好","start":0.1,"end":0.4},{"text":"，","start":0.4,"end":0.4}]}"#.as_bytes();
        let cues = parse_response("fish", json, 1000, KEY).unwrap();
        assert_eq!(cues.len(), 1); assert_eq!(cues[0].text, "你好，");
        assert_eq!((cues[0].start_ms, cues[0].end_ms), (100, 400));
    }

    #[test]
    fn deepgram_utterance_and_azure_phrase_keep_real_time() {
        let deep = r#"{"results":{"utterances":[{"start":0.1,"end":0.8,"transcript":"你好"}]}}"#.as_bytes();
        let azure = r#"{"phrases":[{"offsetMilliseconds":100,"durationMilliseconds":700,"text":"你好"}]}"#.as_bytes();
        for (provider, json) in [("deepgram", &deep[..]), ("azure", &azure[..])] {
            let cues = parse_response(provider, json, 1000, KEY).unwrap();
            assert_eq!(cues.len(), 1); assert_eq!(cues[0].text, "你好");
            assert_eq!((cues[0].start_ms, cues[0].end_ms), (100, 800));
        }
    }

    #[test]
    fn text_without_timestamps_is_rejected_not_fake_success() {
        let fish = br#"{"text":"spoken words","segments":[]}"#;
        let azure = br#"{"combinedPhrases":[{"text":"spoken words"}],"phrases":[]}"#;
        let deep = br#"{"results":{"channels":[{"alternatives":[{"transcript":"spoken words"}]}],"utterances":[]}}"#;
        for (provider, json) in [("fish", &fish[..]), ("azure", &azure[..]), ("deepgram", &deep[..])] {
            assert!(parse_response(provider, json, 1000, KEY).is_err());
        }
    }

    #[test]
    fn deepgram_nonempty_transcript_with_blank_utterances_fails_loudly() {
        let json = br#"{"results":{"utterances":[{"start":0.1,"end":0.5,"transcript":""}],"channels":[{"alternatives":[{"transcript":"Words without timestamps"}]}]}}"#;
        assert!(parse_response("deepgram", json, 1000, KEY).is_err());
    }

    #[test]
    fn fish_posts_official_fields_and_parses_mock_result() {
        let (base, request) = mock("200 OK", r#"{"text":"你好","segments":[{"text":"你好","start":0.1,"end":0.4}]}"#);
        let path = wav(); let cues = transcribe_chunk(&path, &config("fish", base), "zh", &|| false).unwrap();
        let raw = String::from_utf8_lossy(&request.recv_timeout(Duration::from_secs(2)).unwrap()).into_owned();
        assert!(raw.starts_with("POST /v1/asr ")); assert!(raw.to_ascii_lowercase().contains("authorization: bearer synthetic-western-test-key"));
        assert!(raw.contains("transcribe-1-pro")); assert!(raw.contains("name=\"audio\""));
        assert!(raw.contains("name=\"ignore_timestamps\"")); assert_eq!(cues[0].text, "你好");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn errors_do_not_echo_provider_secrets() {
        let (base, _) = mock("401 Unauthorized", &format!(r#"{{"message":"bad {KEY}"}}"#));
        let path = wav(); let err = transcribe_chunk(&path, &config("fish", base), "auto", &|| false).unwrap_err().to_string();
        assert!(!err.contains(KEY)); assert!(err.contains("401")); let _ = std::fs::remove_file(path);
    }

    #[test]
    fn english_word_timestamps_preserve_spacing_and_punctuation() {
        let json = br#"{"text":"Hello, world","segments":[{"text":"Hello,","start":0.1,"end":0.4},{"text":"world","start":0.4,"end":0.8}]}"#;
        let cues = parse_response("fish", json, 1000, KEY).unwrap();
        assert_eq!(cues[0].text, "Hello, world");
    }

    #[test]
    fn deepgram_posts_audio_and_language_with_token_authorization() {
        let body = r#"{"results":{"utterances":[{"start":0.1,"end":0.8,"transcript":"hi"}]}}"#;
        let (base, request) = mock("200 OK", body);
        let path = wav(); let cues = transcribe_chunk(&path, &config("deepgram", base), "yue", &|| false).unwrap();
        let bytes = request.recv_timeout(Duration::from_secs(2)).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("POST /v1/listen?"));
        assert!(text.contains("model=nova-3") && text.contains("utterances=true") && text.contains("language=zh-HK"));
        assert!(text.to_ascii_lowercase().contains("authorization: token synthetic-western-test-key"));
        assert!(bytes.windows(12).any(|part| part.starts_with(b"RIFF") && &part[8..12] == b"WAVE"));
        assert_eq!((cues[0].start_ms, cues[0].end_ms), (100, 800)); let _ = std::fs::remove_file(path);
    }

    #[test]
    fn azure_posts_resource_key_audio_and_selected_locale() {
        let body = r#"{"phrases":[{"offsetMilliseconds":100,"durationMilliseconds":700,"text":"hi"}]}"#;
        let (base, request) = mock("200 OK", body);
        let path = wav(); let cues = transcribe_chunk(&path, &config("azure", base), "ru", &|| false).unwrap();
        let raw = String::from_utf8_lossy(&request.recv_timeout(Duration::from_secs(2)).unwrap()).into_owned();
        assert!(raw.starts_with("POST /speechtotext/transcriptions:transcribe?api-version=2025-10-15 "));
        assert!(raw.to_ascii_lowercase().contains("ocp-apim-subscription-key: synthetic-western-test-key"));
        assert!(raw.contains("name=\"audio\"") && raw.contains("name=\"definition\"") && raw.contains("ru-RU"));
        assert_eq!((cues[0].start_ms, cues[0].end_ms), (100, 800)); let _ = std::fs::remove_file(path);
    }

    #[test]
    fn credential_leaks_and_invalid_or_redirecting_urls_are_rejected() {
        let secret = format!(r#"{{"text":"{KEY}","segments":[{{"text":"{KEY}","start":0.1,"end":0.8}}]}}"#);
        let cues = parse_response("fish", secret.as_bytes(), 1000, KEY).unwrap();
        assert!(!cues[0].text.contains(KEY));
        for url in ["http://example.com", "https://user:pass@example.com", "https://example.com/?key=secret", "https://@example.com"] {
            assert!(checked_endpoint(&config("fish", url.into()), "auto").is_err());
        }
        let (base, _) = mock("302 Found", "{}");
        let path = wav(); let error = transcribe_chunk(&path, &config("fish", base), "auto", &|| false).unwrap_err().to_string();
        assert!(error.contains("302")); let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cancellation_does_not_wait_for_slow_server_or_retain_source_file() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let canceled = Arc::new(AtomicBool::new(false)); let background = canceled.clone();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut block = [0u8; 1024]; let _ = socket.read(&mut block);
            background.store(true, Ordering::SeqCst);
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
        });
        let path = wav(); let thread_path = path.clone(); let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let error = transcribe_chunk(&thread_path, &config("fish", base), "auto", &|| canceled.load(Ordering::SeqCst)).unwrap_err().to_string();
            let _ = done_tx.send(error);
        });
        let error = done_rx.recv_timeout(Duration::from_secs(3)).expect("cancellation should not await server");
        assert!(error.contains("取消")); assert!(std::fs::remove_file(&path).is_ok());
        release_tx.send(()).unwrap(); server.join().unwrap(); worker.join().unwrap();
    }
}
