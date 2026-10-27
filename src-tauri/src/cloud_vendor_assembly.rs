//! AssemblyAI prerecorded transcription. Returns real chunk-local timestamps.
use std::io::{Cursor, Read};
use std::net::IpAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use reqwest::blocking::{Body, Client, Response};
use reqwest::header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Url;
use serde_json::{json, Value};
use crate::cloud_asr::CloudAsrConfig;
use crate::ctx::Ctx;
use crate::subtitle::Cue;

pub fn transcribe(_ctx: &Ctx, wav: &Path, config: &CloudAsrConfig, language: &str,
                  cancelled: &dyn Fn() -> bool) -> anyhow::Result<Vec<Cue>> {
    transcribe_chunk(wav, config, language, cancelled)
}

const MAX_AUDIO_BYTES: u64 = 20_000_000;
const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_WORDS: usize = 50_000;
const POLL_TIMEOUT: Duration = Duration::from_secs(900);
#[cfg(not(test))]
const POLL_INTERVAL: Duration = Duration::from_millis(1500);
#[cfg(test)]
const POLL_INTERVAL: Duration = Duration::from_millis(25);

fn transcribe_chunk(wav: &Path, config: &CloudAsrConfig, language: &str,
                    cancelled: &dyn Fn() -> bool) -> anyhow::Result<Vec<Cue>> {
    anyhow::ensure!(!cancelled(), "云端字幕识别已取消");
    anyhow::ensure!(config.provider == "assemblyai", "未知的云端服务商");
    let (base, loopback) = checked_base(&config.base_url)?;
    let key = config.api_key.trim();
    anyhow::ensure!(loopback || !key.is_empty(), "请填写 AssemblyAI API 密钥");
    anyhow::ensure!(key.len() <= 8192 && !key.bytes().any(|byte| byte <= 0x20 || byte == 0x7f), "API 密钥格式无效");
    let model = config.model.trim();
    anyhow::ensure!(!model.is_empty() && model.len() <= 200 && !model.chars().any(char::is_control), "请选择有效的 AssemblyAI 模型");
    let language = language.trim().to_ascii_lowercase();
    anyhow::ensure!(language == "auto" || (!language.is_empty() && language.len() <= 16
        && language.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'_' || byte == b'-')),
        "识别语言无效");

    // The owned request thread never holds a file handle. Cancellation can
    // therefore release the original temporary WAV immediately on Windows.
    let size = std::fs::metadata(wav).map_err(|_| anyhow::anyhow!("无法读取音频文件"))?.len();
    anyhow::ensure!(size <= MAX_AUDIO_BYTES, "音频分块过大，请减小分块长度");
    let mut audio = Vec::new();
    {
        let file = std::fs::File::open(wav).map_err(|_| anyhow::anyhow!("无法读取音频文件"))?;
        file.take(MAX_AUDIO_BYTES + 1).read_to_end(&mut audio).map_err(|_| anyhow::anyhow!("无法读取音频文件"))?;
    }
    anyhow::ensure!(audio.len() as u64 <= MAX_AUDIO_BYTES, "音频分块过大，请减小分块长度");
    let duration_ms = wav_duration_ms(&audio)?;
    anyhow::ensure!(!cancelled(), "云端字幕识别已取消");

    let mut builder = Client::builder().redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(180));
    if loopback { builder = builder.no_proxy(); }
    let client = builder.build().map_err(|_| anyhow::anyhow!("无法初始化云端连接"))?;
    let stop = CancelGuard(Arc::new(AtomicBool::new(false)));
    let worker_stop = stop.0.clone();
    let key = key.to_owned();
    let model = model.to_owned();
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new().name("umi-assembly-asr".into()).spawn(move || {
        let result = request(client, base, loopback, &key, &model, &language, audio, worker_stop);
        let _ = sender.send(result);
    }).map_err(|_| anyhow::anyhow!("无法启动云端识别请求"))?;
    loop {
        anyhow::ensure!(!cancelled(), "云端字幕识别已取消");
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => {
                anyhow::ensure!(!cancelled(), "云端字幕识别已取消");
                return parse_transcript(&result?, duration_ms, config);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => anyhow::bail!("云端识别请求意外结束，请重试"),
        }
    }
}

struct CancelGuard(Arc<AtomicBool>);
impl Drop for CancelGuard { fn drop(&mut self) { self.0.store(true, Ordering::Relaxed); } }
struct CancelReader { data: Cursor<Vec<u8>>, cancelled: Arc<AtomicBool> }
impl Read for CancelReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.cancelled.load(Ordering::Relaxed) { return Err(std::io::Error::new(std::io::ErrorKind::Other, "cancelled")); }
        self.data.read(buffer)
    }
}

fn checked_base(raw: &str) -> anyhow::Result<(Url, bool)> {
    let raw = raw.trim();
    let url = Url::parse(raw).map_err(|_| anyhow::anyhow!("AssemblyAI 服务地址无效"))?;
    let authority = raw.split_once("://").map(|(_, rest)| rest.split(['/', '?', '#', '\\']).next().unwrap_or("")).unwrap_or("");
    anyhow::ensure!(url.username().is_empty() && url.password().is_none() && !authority.contains('@')
        && url.query().is_none() && url.fragment().is_none() && matches!(url.path(), "" | "/"), "AssemblyAI 地址格式无效");
    let host = url.host_str().ok_or_else(|| anyhow::anyhow!("AssemblyAI 服务地址缺少主机"))?;
    let loopback = host.eq_ignore_ascii_case("localhost") || host.trim_matches(['[', ']'])
        .parse::<IpAddr>().map(|address| address.is_loopback()).unwrap_or(false);
    anyhow::ensure!(url.scheme() == "https" || (url.scheme() == "http" && loopback), "远程云端服务必须使用 HTTPS");
    anyhow::ensure!(loopback || host.eq_ignore_ascii_case("api.assemblyai.com"), "AssemblyAI 只能使用其官方接口地址");
    anyhow::ensure!(loopback || url.port().is_none(), "AssemblyAI 只能使用其官方接口地址");
    Ok((url, loopback))
}

fn wav_duration_ms(wav: &[u8]) -> anyhow::Result<u64> {
    let invalid = || anyhow::anyhow!("音频 WAV 无效，需要 16kHz 单声道 16 位 PCM");
    if wav.len() < 12 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" { return Err(invalid()); }
    let riff_end = (u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize).checked_add(8).ok_or_else(invalid)?;
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
    Ok(data_size as u64 * 1000 / 32000)
}

fn read_response(response: Response, cancelled: &AtomicBool) -> anyhow::Result<Value> {
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
    anyhow::ensure!(!cancelled.load(Ordering::Relaxed), "云端字幕识别已取消");
    if response.content_length().map(|length| length > MAX_RESPONSE_BYTES).unwrap_or(false) {
        anyhow::bail!("云端识别响应过大，已停止处理");
    }
    let mut body = Vec::new();
    response.take(MAX_RESPONSE_BYTES + 1).read_to_end(&mut body).map_err(|_| anyhow::anyhow!("读取云端识别响应失败，请重试"))?;
    anyhow::ensure!(body.len() as u64 <= MAX_RESPONSE_BYTES, "云端识别响应过大，已停止处理");
    anyhow::ensure!(!cancelled.load(Ordering::Relaxed), "云端字幕识别已取消");
    serde_json::from_slice(&body).map_err(|_| anyhow::anyhow!("云端返回的识别结果格式无效"))
}

fn send(builder: reqwest::blocking::RequestBuilder, cancelled: &AtomicBool) -> anyhow::Result<Value> {
    anyhow::ensure!(!cancelled.load(Ordering::Relaxed), "云端字幕识别已取消");
    let response = builder.send().map_err(|error| {
        if error.is_timeout() { anyhow::anyhow!("云端识别请求超时，请检查网络或稍后重试") }
        else if error.is_connect() { anyhow::anyhow!("无法连接云端识别服务，请检查地址和网络") }
        else { anyhow::anyhow!("云端识别请求失败，请检查服务配置或网络") }
    })?;
    read_response(response, cancelled)
}

fn endpoint(base: &Url, path: &str) -> Url { let mut url = base.clone(); url.set_path(path); url }

fn request(client: Client, base: Url, loopback: bool, key: &str, model: &str,
           language: &str, audio: Vec<u8>, cancelled: Arc<AtomicBool>) -> anyhow::Result<Vec<u8>> {
    let mut auth = HeaderValue::from_str(key).map_err(|_| anyhow::anyhow!("API 密钥格式无效"))?;
    auth.set_sensitive(true);
    let size = audio.len() as u64;
    let reader = CancelReader { data: Cursor::new(audio), cancelled: cancelled.clone() };
    let uploaded = send(client.post(endpoint(&base, "/v2/upload"))
        .header(AUTHORIZATION, auth.clone()).header(CONTENT_TYPE, "application/octet-stream")
        .body(Body::sized(reader, size)), &cancelled)?;
    let upload_url = uploaded.get("upload_url").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("AssemblyAI 未返回音频上传地址"))?;
    anyhow::ensure!(upload_url.len() <= 4096, "AssemblyAI 返回的音频地址无效");
    let returned = Url::parse(upload_url).map_err(|_| anyhow::anyhow!("AssemblyAI 返回的音频地址无效"))?;
    let returned_host = returned.host_str().unwrap_or("");
    let valid_host = if loopback { returned.origin() == base.origin() }
        else { returned.scheme() == "https" && (returned_host.eq_ignore_ascii_case("assemblyai.com")
            || returned_host.to_ascii_lowercase().ends_with(".assemblyai.com")) };
    anyhow::ensure!(valid_host && returned.username().is_empty() && returned.password().is_none() && returned.fragment().is_none(), "AssemblyAI 返回的音频地址无效");
    let mut options = json!({"audio_url": upload_url, "speech_models": [model]});
    if language == "auto" { options["language_detection"] = json!(true); }
    else { options["language_code"] = json!(language); }
    let submitted = send(client.post(endpoint(&base, "/v2/transcript"))
        .header(AUTHORIZATION, auth.clone()).header(CONTENT_TYPE, "application/json")
        .body(options.to_string()), &cancelled)?;
    let id = submitted.get("id").and_then(Value::as_str).unwrap_or("");
    anyhow::ensure!(!id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "AssemblyAI 未返回有效的任务编号");
    let poll = endpoint(&base, &format!("/v2/transcript/{id}"));
    let deadline = Instant::now() + POLL_TIMEOUT;
    loop {
        anyhow::ensure!(!cancelled.load(Ordering::Relaxed), "云端字幕识别已取消");
        anyhow::ensure!(Instant::now() < deadline, "AssemblyAI 识别等待超时，请稍后重试");
        let remaining = deadline.saturating_duration_since(Instant::now()).min(Duration::from_secs(180));
        let result = send(client.get(poll.clone()).header(AUTHORIZATION, auth.clone()).timeout(remaining), &cancelled)?;
        match result.get("status").and_then(Value::as_str).unwrap_or("") {
            "completed" => return serde_json::to_vec(&result).map_err(|_| anyhow::anyhow!("云端返回的识别结果格式无效")),
            "queued" | "processing" => {},
            "error" => anyhow::bail!("AssemblyAI 识别失败，请检查音频、模型权限和额度"),
            _ => anyhow::bail!("AssemblyAI 返回了未知的任务状态"),
        }
        let sleep_until = Instant::now() + POLL_INTERVAL;
        while Instant::now() < sleep_until {
            anyhow::ensure!(!cancelled.load(Ordering::Relaxed), "云端字幕识别已取消");
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

fn append_text(out: &mut String, next: &str) {
    if out.chars().last().map(|c| c.is_ascii_alphanumeric() || matches!(c, ',' | '.' | '!' | '?' | ';' | ':' | ')')).unwrap_or(false)
        && next.chars().next().map(|c| c.is_ascii_alphanumeric()).unwrap_or(false) { out.push(' '); }
    out.push_str(next);
}

fn parse_transcript(body: &[u8], duration_ms: u64, config: &CloudAsrConfig) -> anyhow::Result<Vec<Cue>> {
    let result: Value = serde_json::from_slice(body).map_err(|_| anyhow::anyhow!("云端返回的识别结果格式无效"))?;
    let words = result.get("words").and_then(Value::as_array);
    let has_text = result.get("text").and_then(Value::as_str).map(|s| !s.trim().is_empty()).unwrap_or(false);
    if words.map(|items| items.is_empty()).unwrap_or(true) {
        anyhow::ensure!(!has_text, "AssemblyAI 未返回可用的时间戳");
        return Ok(Vec::new());
    }
    let words = words.unwrap();
    anyhow::ensure!(words.len() <= MAX_WORDS, "云端返回的时间戳分段数量异常");
    let mut output = Vec::new(); let mut text = String::new();
    let mut start = 0u64; let mut end = 0u64; let mut last_start = 0u64;
    for word in words {
        let part = word.get("text").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("AssemblyAI 未返回分段文本"))?.trim();
        if part.is_empty() { continue; }
        let a = word.get("start").and_then(Value::as_u64).ok_or_else(|| anyhow::anyhow!("云端返回的时间戳无效"))?;
        let b = word.get("end").and_then(Value::as_u64).ok_or_else(|| anyhow::anyhow!("云端返回的时间戳无效"))?;
        anyhow::ensure!(a >= last_start && b >= a && b <= duration_ms.saturating_add(10), "云端返回的时间戳无效");
        last_start = a;
        let break_before = !text.is_empty() && (a > end.saturating_add(800) || b.saturating_sub(start) > 5000
            || text.chars().count() >= 45 || text.ends_with(['。', '！', '？', '.', '!', '?']));
        if break_before {
            anyhow::ensure!(end > start, "云端未返回有效的分段时间戳");
            output.push(Cue { index: output.len() + 1, start_ms: start, end_ms: end, text: config.redact(&text) });
            text.clear();
        }
        if text.is_empty() { start = a; end = b; } else { end = end.max(b); }
        append_text(&mut text, part);
    }
    if !text.is_empty() {
        anyhow::ensure!(end > start, "云端未返回有效的分段时间戳");
        output.push(Cue { index: output.len() + 1, start_ms: start, end_ms: end, text: config.redact(&text) });
    }
    anyhow::ensure!(!output.is_empty() || !has_text, "AssemblyAI 未返回可用的时间戳");
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{mpsc, Arc, atomic::{AtomicBool, Ordering}};
    use std::time::{Duration, Instant};

    fn config(base_url: String) -> CloudAsrConfig {
        CloudAsrConfig { provider: "assemblyai".into(), base_url, model: "universal-2".into(),
            api_key: "synthetic-assembly-key".into(), ..Default::default() }
    }
    fn wav() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("assembly-asr-{}-{:?}.wav", std::process::id(), std::thread::current().id()));
        let mut bytes = b"RIFF".to_vec(); bytes.extend_from_slice(&32036u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0"); bytes.extend_from_slice(&16000u32.to_le_bytes());
        bytes.extend_from_slice(&32000u32.to_le_bytes()); bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes()); bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&32000u32.to_le_bytes()); bytes.resize(32044, 0);
        std::fs::write(&path, bytes).unwrap(); path
    }
    fn read_request(socket: &mut std::net::TcpStream) -> Vec<u8> {
        socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut received = Vec::new(); let mut buf = [0; 8192];
        let (header_end, content_length) = loop {
            let n = socket.read(&mut buf).unwrap(); assert!(n > 0); received.extend_from_slice(&buf[..n]);
            if let Some(at) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&received[..at]);
                let length = header.lines().find_map(|line| line.split_once(':').filter(|(k,_)| k.eq_ignore_ascii_case("content-length"))
                    .map(|(_,v)| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                break (at + 4, length);
            }
        };
        while received.len() < header_end + content_length {
            let n = socket.read(&mut buf).unwrap(); assert!(n > 0); received.extend_from_slice(&buf[..n]);
        }
        received
    }
    fn reply(socket: &mut std::net::TcpStream, status: &str, body: &str, headers: &str) {
        write!(socket, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}", body.len()).unwrap();
    }

    #[test]
    fn upload_submit_poll_round_trip_preserves_real_word_times() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let upload = format!("{base}/media/demo");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let bodies = [format!(r#"{{"upload_url":"{upload}"}}"#), r#"{"id":"job-123","status":"queued"}"#.into(),
                r#"{"status":"processing"}"#.into(),
                r#"{"status":"completed","text":"你好 world!","words":[{"text":"你好","start":30,"end":400},{"text":"world!","start":410,"end":800}]}"#.into()];
            for body in bodies {
                let (mut socket, _) = listener.accept().unwrap();
                let request = read_request(&mut socket); tx.send(request).unwrap();
                reply(&mut socket, "200 OK", &body, "");
            }
        });
        let path = wav();
        let cues = transcribe_chunk(&path, &config(base), "zh", &|| false).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(cues, vec![Cue { index: 1, start_ms: 30, end_ms: 800, text: "你好world!".into() }]);
        let requests: Vec<String> = (0..4).map(|_| String::from_utf8_lossy(&rx.recv_timeout(Duration::from_secs(2)).unwrap()).into_owned()).collect();
        assert!(requests[0].starts_with("POST /v2/upload HTTP/1.1"));
        assert!(requests[0].to_ascii_lowercase().contains("authorization: synthetic-assembly-key"));
        assert!(requests[1].starts_with("POST /v2/transcript HTTP/1.1"));
        assert!(requests[1].contains("\"language_code\":\"zh\""));
        assert!(requests[1].contains("\"speech_models\":[\"universal-2\"]"));
        assert!(requests[2].starts_with("GET /v2/transcript/job-123 HTTP/1.1"));
        assert!(requests[3].starts_with("GET /v2/transcript/job-123 HTTP/1.1"));
    }

    #[test]
    fn missing_or_invalid_word_timestamps_never_become_subtitles() {
        let config = config("http://127.0.0.1:5".into());
        for body in [r#"{"text":"hello","words":[]}"#,
                     r#"{"text":"hello","words":[{"text":"hello"}]}"#,
                     r#"{"text":"hello","words":[{"text":"hello","start":900,"end":800}]}"#,
                     r#"{"text":"hello","words":[{"text":"hello","start":0,"end":2000}]}"#] {
            assert!(parse_transcript(body.as_bytes(), 1000, &config).is_err());
        }
        assert!(parse_transcript(br#"{"text":"","words":[]}"#, 1000, &config).unwrap().is_empty());
    }

    #[test]
    fn credential_and_origin_checks_reject_untrusted_addresses() {
        assert!(checked_base("https://api.assemblyai.com").is_ok());
        for raw in ["http://api.assemblyai.com", "https://api.assemblyai.com:8443", "https://api.assemblyai.com.evil.example",
                    "https://key@api.assemblyai.com", "https://api.assemblyai.com/v2", "https://api.assemblyai.com/?key=secret"] {
            assert!(checked_base(raw).is_err(), "unexpected acceptance for {raw}");
        }
        assert!(checked_base("http://127.0.0.1:4321").is_ok());
    }

    #[test]
    fn redirects_are_not_followed_and_errors_do_not_echo_credentials() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let destination = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let redirected = format!("http://{}/stolen", destination.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let _ = read_request(&mut socket);
            let body = r#"{"error":"synthetic-assembly-key"}"#;
            reply(&mut socket, "302 Found", body, &format!("Location: {redirected}\r\n"));
        });
        let path = wav();
        let result = transcribe_chunk(&path, &config(base), "zh", &|| false).unwrap_err().to_string();
        std::fs::remove_file(path).unwrap();
        assert!(result.contains("重定向"));
        assert!(!result.contains("synthetic-assembly-key"));
        destination.set_nonblocking(true).unwrap();
        assert_eq!(destination.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }

    #[test]
    fn echoed_credentials_in_provider_words_are_redacted() {
        let body = br#"{"text":"synthetic-assembly-key","words":[{"text":"synthetic-assembly-key","start":10,"end":100}]}"#;
        let cues = parse_transcript(body, 1000, &config("http://127.0.0.1:5".into())).unwrap();
        assert_eq!(cues[0].text, "[REDACTED]");
    }

    #[test]
    fn cancellation_during_upload_is_prompt_and_releases_wav_file() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (seen_tx, seen_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let _ = read_request(&mut socket); seen_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(600));
        });
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        std::thread::spawn(move || { seen_rx.recv_timeout(Duration::from_secs(4)).unwrap(); flag.store(true, Ordering::SeqCst); });
        let path = wav(); let started = Instant::now();
        let error = transcribe_chunk(&path, &config(base), "auto", &|| cancelled.load(Ordering::SeqCst)).unwrap_err();
        assert!(error.to_string().contains("取消"));
        assert!(started.elapsed() < Duration::from_secs(2));
        std::fs::remove_file(path).expect("upload must not retain the source file handle");
    }
}
