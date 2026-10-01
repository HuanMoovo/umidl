//! 浏览器下载捕获接收端（IDM 风格本地接口）
//!
//! 在本机 127.0.0.1 上开一个极小的 HTTP 服务，浏览器扩展 / 书签脚本 /
//! 命令行把要下载的链接 POST 过来即可入队：
//!
//! - `GET  /ping`                              握手，返回应用名与版本
//! - `GET  /capture?url=<enc>&referer=<enc>`   捕获一个链接
//! - `POST /capture`  body: `url=<enc>&referer=<enc>`
//! - `OPTIONS *`                               CORS 预检（不回 CORS 头）
//!
//! # 安全（1.8 加固，BUG-06）
//!
//! - 只监听回环地址；
//! - **令牌强制校验**：`X-Umidl-Token` 头（或同名查询参数 `?token=`）必须等于设置里的
//!   `capture_token`；令牌为空时 [`start`] 直接拒绝启动 —— 不再有「空令牌 = 不校验」的路径。
//!   令牌由应用自动生成（32 位十六进制随机串，`capture::generate_token`），
//!   首启写入设置，UI 可看 / 可重置。
//! - 响应**不带** `Access-Control-Allow-Origin`：任意网页既触发不了预检放行，也读不到响应；
//! - `/capture` 额外校验 `Origin` / `Referer`：非本机来源（网页）必须带上正确令牌才受理，
//!   否则 403 并在响应里说明原因 —— 防 drive-by 越站入队；
//! - 响应体如实反映结果：`queued` / `skipped`（含原因）/ `blocked`（含规则），见 [`CaptureOutcome`]。
//!
//! 用法（浏览器扩展 / 书签脚本 / 命令行）：
//! ```text
//! GET http://127.0.0.1:6970/capture?url=<编码后的链接>&token=<设置里的令牌>
//! 或带请求头 X-Umidl-Token: <设置里的令牌>（扩展 / curl 均可用）
//! 缺失或错误的令牌一律 401；网页来源缺少令牌一律 403。
//! ```

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

/// 读请求 / 读响应的墙钟上限。服务端读不到完整请求就回 408；测试客户端读不到响应就报错，
/// 两边共用同一个数，避免「服务端还在等、客户端已经不等了」的口径差。
const READ_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub token: Option<String>,
    /// `Origin` 请求头（网页来源判定）
    pub origin: Option<String>,
    /// `Referer` 请求头（Origin 缺失时的兜底判定）
    pub referer: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CapturePayload {
    pub url: String,
    pub referer: Option<String>,
    pub source: String,
}

/// 捕获回调的处理结果：决定 `/capture` 的响应体（不再一律 `{"ok":true,"queued":true}`，BUG-12）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureOutcome {
    /// 已入队（随后自动开始下载）
    Queued,
    /// 未入队：自动入队关闭 / 重复链接 / 入队失败等，附原因
    Skipped(String),
    /// 被智能过滤规则拦截，附命中的规则说明
    Blocked(String),
}

impl CaptureOutcome {
    pub fn status(&self) -> &'static str {
        match self {
            CaptureOutcome::Queued => "queued",
            CaptureOutcome::Skipped(_) => "skipped",
            CaptureOutcome::Blocked(_) => "blocked",
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            CaptureOutcome::Queued => None,
            CaptureOutcome::Skipped(r) | CaptureOutcome::Blocked(r) => Some(r.as_str()),
        }
    }

    /// 响应体（JSON 文本）：`status` 是唯一权威字段，`queued` / `blocked` 便于老客户端过渡
    pub fn to_body(&self) -> String {
        let (queued, blocked) = match self {
            CaptureOutcome::Queued => (true, false),
            CaptureOutcome::Skipped(_) => (false, false),
            CaptureOutcome::Blocked(_) => (false, true),
        };
        let mut v = serde_json::json!({
            "ok": true,
            "status": self.status(),
            "queued": queued,
            "blocked": blocked,
        });
        if let Some(r) = self.reason() {
            if let serde_json::Value::Object(map) = &mut v {
                map.insert("reason".to_string(), serde_json::Value::String(r.to_string()));
            }
        }
        v.to_string()
    }
}

/// 解析请求行与查询串（纯函数，便于单测）
pub fn parse_request_line(line: &str) -> Option<(String, String, Vec<(String, String)>)> {
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?;
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), parse_form(q)),
        None => (target.to_string(), Vec::new()),
    };
    Some((method, path, query))
}

/// `a=1&b=hello%20world` → [(a,1),(b,hello world)]
pub fn parse_form(s: &str) -> Vec<(String, String)> {
    s.split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (decode(k), decode(v)),
            None => (decode(pair), String::new()),
        })
        .collect()
}

pub fn lookup<'a>(q: &'a [(String, String)], key: &str) -> Option<&'a str> {
    q.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// 百分号解码（含 UTF-8 多字节）
pub fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 从 `Referer` / 链接里取出 `scheme://host[:port]`（解析不出来返回 None）
pub fn origin_of(url: &str) -> Option<String> {
    let u = url.trim();
    let idx = u.find("://")?;
    let scheme = &u[..idx];
    if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.') {
        return None;
    }
    let rest = &u[idx + 3..];
    let end = rest
        .find(|c| c == '/' || c == '?' || c == '#')
        .unwrap_or(rest.len());
    let host = rest[..end].trim();
    if host.is_empty() {
        return None;
    }
    Some(format!("{}://{}", scheme.to_ascii_lowercase(), host.to_ascii_lowercase()))
}

/// 本机回环 / 应用自身 / 浏览器扩展来源？（网页来源一律 false）
///
/// 授权来源（无需再看 Origin）靠令牌表达：外来网页带上正确令牌才被受理，
/// 见 `start` 里的处理顺序。
pub fn is_trusted_origin(origin: &str) -> bool {
    let o = origin.trim().to_ascii_lowercase();
    if o.is_empty() || o == "null" {
        return false;
    }
    // 浏览器扩展 / 应用自身协议（扩展拥有 host 权限时由浏览器放行，不经 CORS 预检）
    const TRUSTED_PREFIXES: [&str; 10] = [
        "chrome-extension://",
        "moz-extension://",
        "safari-web-extension://",
        "ms-browser-extension://",
        "edge-extension://",
        "tauri://",
        "asset://",
        "http://tauri.localhost",
        "https://tauri.localhost",
        "http://ipc.localhost",
    ];
    if TRUSTED_PREFIXES.iter().any(|p| o.starts_with(p)) {
        return true;
    }
    let Some(rest) = o.strip_prefix("http://").or_else(|| o.strip_prefix("https://")) else {
        return false;
    };
    let host_port = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = match host_port.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => h,
        _ => host_port,
    };
    matches!(
        host,
        "127.0.0.1" | "localhost" | "::1" | "[::1]" | "[0:0:0:0:0:0:0:1]"
    )
}

/// 读一个 HTTP 请求（请求行 + 头 + 可选 body）
///
/// 直接借用连接读，**不 dup**（旧写法 `stream.try_clone()?`）：dup 只是为了一层 BufReader 的借用，
/// 却多占一个文件描述符，而且 dup 失败（fd 紧张时 EMFILE）会把整条请求连带连接一起丢掉 ——
/// 客户端只会看到「连上了但没有任何响应」。
pub fn read_request(stream: &mut TcpStream) -> std::io::Result<(Parsed, String)> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    let mut reader = BufReader::new(&mut *stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let (method, path, query) = parse_request_line(line.trim_end()).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "bad request line")
    })?;
    let mut token = None;
    let mut origin = None;
    let mut referer = None;
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_string());
            if k == "x-umidl-token" {
                token = Some(v.clone());
            } else if k == "content-length" {
                len = v.parse().unwrap_or(0);
            } else if k == "origin" {
                origin = Some(v.clone());
            } else if k == "referer" {
                referer = Some(v.clone());
            }
        }
    }
    let mut body = String::new();
    if len > 0 && len <= 64 * 1024 {
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf)?;
        body = String::from_utf8_lossy(&buf).to_string();
    }
    Ok((
        Parsed {
            method,
            path,
            query,
            token,
            origin,
            referer,
        },
        body,
    ))
}

fn respond(stream: &mut TcpStream, code: u16, body: &str) -> std::io::Result<()> {
    let reason = match code {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        _ => "Error",
    };
    // 注意：**不返回任何 CORS 头**。旧版固定 `Access-Control-Allow-Origin: *`，
    // 任意网页都能读取 /ping 与 /capture 的响应并触发跨站入队（BUG-06）。
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\n\
         Content-Type: application/json; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n",
        body.as_bytes().len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

/// 拒绝请求：`{"ok":false,"status":"rejected","queued":false,"error":<原因>}` + 附加说明
fn reject(stream: &mut TcpStream, code: u16, error: &str, extra: serde_json::Value) {
    let mut v = serde_json::json!({
        "ok": false,
        "status": "rejected",
        "queued": false,
        "blocked": false,
        "error": error,
    });
    if let (serde_json::Value::Object(map), serde_json::Value::Object(m)) = (&mut v, extra) {
        for (k, val) in m {
            map.insert(k, val);
        }
    }
    let _ = respond(stream, code, &v.to_string());
}

/// 生成随机捕获令牌：32 位十六进制（128 bit 熵），首启写入设置
pub fn generate_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[derive(Debug)]
pub struct CaptureServer {
    stop: Arc<AtomicBool>,
    port: u16,
    /// 后台线程句柄：`stop_and_wait` 用它等线程真正退出（退出即释放监听套接字）
    join: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl CaptureServer {
    pub fn port(&self) -> u16 {
        self.port
    }

    /// 只置停止标志（不等待线程退出）。自测用；应用内重启请用 [`Self::stop_and_wait`]。
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// 置停止标志并**等后台线程真正退出**。
    ///
    /// 为什么必须等：线程退出才会丢掉 `TcpListener`。不等就重新 bind 同一端口，
    /// 真机上会拿到 `os error 10048`（WSAEADDRINUSE）—— 表现就是「端口没变却报端口被占用」，
    /// 设置页据此弹出「换端口」的误导性警告。
    pub fn stop_and_wait(self) {
        self.stop.store(true, Ordering::SeqCst);
        let handle = self.join.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(h) = handle {
            let _ = h.join();
        }
    }
}

/// 启动捕获服务。<param: on_capture> 收到链接时回调（返回值决定 HTTP 响应体）。
///
/// `token` 为空时直接返回 `Err`：**没有任何“无令牌放行”的模式**（BUG-06）。
pub fn start<F>(port: u16, token: String, app_name: String, app_version: String, on_capture: F) -> std::io::Result<CaptureServer>
where
    F: Fn(CapturePayload) -> CaptureOutcome + Send + 'static,
{
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "捕获令牌为空：拒绝以「不校验令牌」的模式启动捕获服务",
        ));
    }
    // 端口刚被上一个监听器释放时，立即 bind 可能短暂失败：macOS 上 close 后端口回收有极短
    // 延迟（Windows 上则可能撞 TIME_WAIT）。真机「端口没变，点保存并重启」曾因此拿到
    // os error 10048，被误报成「端口可能已被其它程序占用」。这里做**有界**重试（最多 2 秒）：
    // 端口确实被别的进程长期占用时，仍然如实返回最后一次的错误。
    let mut last_err: Option<std::io::Error> = None;
    let mut bound_listener: Option<TcpListener> = None;
    for attempt in 0..100u32 {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => {
                bound_listener = Some(l);
                break;
            }
            Err(e) => {
                last_err = Some(e);
                if attempt < 99 {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }
    let listener = match bound_listener {
        Some(l) => l,
        None => {
            return Err(last_err.unwrap_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::Other, "bind 127.0.0.1 失败")
            }))
        }
    };
    let bound = listener
        .local_addr()
        .map(|a| a.port())
        .unwrap_or(port);
    listener.set_nonblocking(true)?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();

    let handle = std::thread::spawn(move || {
        while !flag.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _addr)) => {
                    // macOS（BSD 系）的 accept 会**继承监听套接字的非阻塞标志**（Linux 不会继承），
                    // 于是 read_request 里的 read_line 立刻拿到 EAGAIN —— 在错误分支被当成
                    // 「读超时」回 408，真机上表现为 macOS 上每一个请求都 408。
                    // 这里显式改回阻塞读；读超时仍由 read_request 里的 SO_RCVTIMEO 兜底。
                    let _ = stream.set_nonblocking(false);
                    let (req, body) = match read_request(&mut stream) {
                        Ok(v) => v,
                        Err(e) => {
                            // 读不出完整请求也必须回一个响应，**不能静默断开**：
                            // 客户端（扩展 / curl / 单测）否则只会看到「连接上但零字节响应」，
                            // 真实原因（请求没发完 / 连接被重置）完全看不到，排查方向会被带错。
                            // 读超时 → 408；其余（请求行 / 头部畸形、连接被重置等）→ 400。
                            let timed_out = matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            );
                            let (code, error) = if timed_out {
                                (408u16, "request read timed out")
                            } else {
                                (400u16, "bad request")
                            };
                            reject(
                                &mut stream,
                                code,
                                error,
                                serde_json::json!({ "detail": e.to_string() }),
                            );
                            continue;
                        }
                    };
                    if req.method == "OPTIONS" {
                        // 不回 CORS 头 → 网页预检拿不到放行，跨站自定义头请求被浏览器拦下；
                        // 扩展（host 权限）不受影响
                        let _ = respond(&mut stream, 204, "");
                        continue;
                    }
                    // 令牌：请求头优先，其次查询参数（老用法 ?token= 继续可用）
                    let provided = req
                        .token
                        .clone()
                        .or_else(|| lookup(&req.query, "token").map(|s| s.to_string()));
                    let token_ok = provided.as_deref() == Some(token.as_str());
                    // 来源判定：Origin 优先，缺失时退回 Referer 的 origin（网页 fetch / <img> 会带 Referer）
                    let source = req
                        .origin
                        .clone()
                        .or_else(|| req.referer.as_deref().and_then(origin_of));
                    let from_web = matches!(source.as_deref(), Some(o) if !is_trusted_origin(o));
                    if from_web && !token_ok {
                        // drive-by 越站入队：非本机来源又没有正确令牌 → 拒绝并说明原因
                        reject(
                            &mut stream,
                            403,
                            "origin not allowed",
                            serde_json::json!({
                                "origin": source,
                                "hint": "网页（非本机）来源的捕获请求必须携带正确的 X-Umidl-Token 令牌；令牌见 Umidl 设置 › 系统 › 浏览器捕获。",
                            }),
                        );
                        continue;
                    }
                    match req.path.as_str() {
                        "/ping" => {
                            let body = serde_json::json!({
                                "ok": true,
                                "app": app_name,
                                "version": app_version,
                                "capture": true,
                            })
                            .to_string();
                            let _ = respond(&mut stream, 200, &body);
                        }
                        "/capture" => {
                            if !token_ok {
                                reject(
                                    &mut stream,
                                    401,
                                    "bad or missing token",
                                    serde_json::json!({
                                        "hint": "请求需带 X-Umidl-Token 头（或 ?token= 查询参数）；令牌见 Umidl 设置 › 系统 › 浏览器捕获。",
                                    }),
                                );
                                continue;
                            }
                            let form = if req.method == "POST" { parse_form(&body) } else { req.query.clone() };
                            match lookup(&form, "url").filter(|u| !u.trim().is_empty()) {
                                Some(url) => {
                                    let payload = CapturePayload {
                                        url: url.trim().to_string(),
                                        referer: lookup(&form, "referer").map(|s| s.to_string()),
                                        source: lookup(&form, "source")
                                            .unwrap_or("browser")
                                            .to_string(),
                                    };
                                    let outcome = on_capture(payload);
                                    let _ = respond(&mut stream, 200, &outcome.to_body());
                                }
                                None => reject(
                                    &mut stream,
                                    400,
                                    "missing url",
                                    serde_json::json!({}),
                                ),
                            }
                        }
                        _ => reject(&mut stream, 404, "not found", serde_json::json!({})),
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(60));
                }
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(60));
                }
            }
        }
    });

    Ok(CaptureServer {
        stop,
        port: bound,
        join: std::sync::Mutex::new(Some(handle)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn parses_request_line_and_query() {
        let (m, p, q) = parse_request_line("POST /capture?url=https%3A%2F%2Fa.com%2Fx%20y&t=1 HTTP/1.1").unwrap();
        assert_eq!(m, "POST");
        assert_eq!(p, "/capture");
        assert_eq!(lookup(&q, "url"), Some("https://a.com/x y"));
        assert_eq!(lookup(&q, "t"), Some("1"));
    }

    #[test]
    fn decodes_utf8() {
        assert_eq!(decode("%E4%B8%AD%E6%96%87"), "中文");
        assert_eq!(decode("a+b"), "a b");
        assert_eq!(decode("100%25"), "100%");
    }

    #[test]
    fn form_parsing_handles_empty_values() {
        let f = parse_form("url=x&referer=&flag");
        assert_eq!(lookup(&f, "referer"), Some(""));
        assert_eq!(lookup(&f, "flag"), Some(""));
    }

    #[test]
    fn origin_of_extracts_scheme_host_port() {
        assert_eq!(origin_of("https://evil.example/x?y=1").as_deref(), Some("https://evil.example"));
        assert_eq!(origin_of("http://127.0.0.1:6970/a").as_deref(), Some("http://127.0.0.1:6970"));
        assert_eq!(origin_of("about:blank"), None);
        assert_eq!(origin_of(""), None);
        assert_eq!(origin_of("  https://a.example  ").as_deref(), Some("https://a.example"));
    }

    #[test]
    fn trusted_origins_local_and_extension_only() {
        for ok in [
            "http://127.0.0.1:6970",
            "http://localhost",
            "https://localhost:8443",
            "http://[::1]:6970",
            "chrome-extension://abcdefghijklmnop",
            "moz-extension://something",
            "tauri://localhost",
            "http://tauri.localhost",
        ] {
            assert!(is_trusted_origin(ok), "{ok} 应视为本机/扩展来源");
        }
        for bad in [
            "https://evil.example",
            "http://evil.example:6970",
            "https://127.0.0.1.evil.example",
            "null",
            "",
            "https://localhost.evil.example",
        ] {
            assert!(!is_trusted_origin(bad), "{bad} 不应视为本机来源");
        }
    }

    #[test]
    fn outcome_bodies_reflect_actual_result() {
        let q: serde_json::Value = serde_json::from_str(&CaptureOutcome::Queued.to_body()).unwrap();
        assert_eq!(q["status"], "queued");
        assert_eq!(q["queued"], true);
        assert_eq!(q["blocked"], false);
        assert!(q.get("reason").is_none());

        let s: serde_json::Value =
            serde_json::from_str(&CaptureOutcome::Skipped("自动入队已关闭".into()).to_body()).unwrap();
        assert_eq!(s["status"], "skipped");
        assert_eq!(s["queued"], false);
        assert_eq!(s["reason"], "自动入队已关闭");

        let b: serde_json::Value =
            serde_json::from_str(&CaptureOutcome::Blocked("域名黑名单命中：*.evil.com".into()).to_body()).unwrap();
        assert_eq!(b["status"], "blocked");
        assert_eq!(b["blocked"], true);
        assert_eq!(b["queued"], false);
        assert_eq!(b["reason"], "域名黑名单命中：*.evil.com");
    }

    #[test]
    fn start_refuses_empty_token() {
        let e = start(0, String::new(), "Umidl".into(), "test".into(), |_| CaptureOutcome::Queued)
            .expect_err("空令牌必须拒绝启动");
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput);
        let e2 = start(0, "   ".into(), "Umidl".into(), "test".into(), |_| CaptureOutcome::Queued)
            .expect_err("空白令牌必须拒绝启动");
        assert_eq!(e2.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn token_is_hex_and_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "两次生成的令牌不能相同");
    }

    /// 发一个 HTTP 请求并读回响应。
    ///
    /// 旧写法把读错误整个吞掉（`let _ = stream.read_to_string(&mut out)`），于是**任何**传输层问题
    /// （服务器没回响应就断开 / 连接被重置 / 读超时）都会退化成「响应：」为空的断言失败，
    /// 看上去像状态码不对，真正的原因被掩盖了。这里把原因直接报出来，并保证返回的确实是一段 HTTP 响应。
    fn http_request(port: u16, request: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream.set_read_timeout(Some(READ_TIMEOUT)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut out = String::new();
        if let Err(e) = stream.read_to_string(&mut out) {
            assert!(
                !out.is_empty(),
                "读响应失败（{e:?}）且一个字节都没收到：服务器没有回响应（不能当成状态码不对）"
            );
        }
        assert!(
            out.starts_with("HTTP/1.1 "),
            "响应不是一段完整的 HTTP 状态行（读到 {} 字节）：{out:?}",
            out.len()
        );
        out
    }

    #[test]
    fn end_to_end_enforces_token_and_origin_without_cors_wildcard() {
        let hits: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let hits2 = hits.clone();
        let server = start(
            0,
            "unit-test-token".to_string(),
            "Umidl".to_string(),
            "9.9.9".to_string(),
            move |p| {
                hits2.lock().unwrap().push(p.url.clone());
                CaptureOutcome::Queued
            },
        )
        .expect("启动捕获服务");
        let port = server.port();
        assert!(port > 0, "port=0 时应返回真实端口");

        // 1) 恶意外站 Origin + 无令牌 → 403 origin not allowed，且不入队
        let r = http_request(
            port,
            "GET /capture?url=http%3A%2F%2F127.0.0.1%3A9%2Fdriveby.bin&source=web HTTP/1.1\r\n\
             Host: 127.0.0.1\r\nOrigin: https://evil.example\r\nConnection: close\r\n\r\n",
        );
        assert!(r.starts_with("HTTP/1.1 403"), "响应：{r}");
        assert!(r.contains("origin not allowed"), "响应：{r}");
        assert!(!r.to_ascii_lowercase().contains("access-control-allow-origin"), "不得回 CORS 通配：{r}");
        assert!(hits.lock().unwrap().is_empty(), "被拒绝的请求不得入队");

        // 2) 只有 Referer（fetch no-cors / <img> 场景）+ 无令牌 → 同样 403
        let r = http_request(
            port,
            "GET /capture?url=http%3A%2F%2F127.0.0.1%3A9%2Fref.bin HTTP/1.1\r\n\
             Host: 127.0.0.1\r\nReferer: https://evil.example/page.html\r\nConnection: close\r\n\r\n",
        );
        assert!(r.starts_with("HTTP/1.1 403"), "响应：{r}");
        assert!(hits.lock().unwrap().is_empty(), "被拒绝的请求不得入队");

        // 3) 无来源（curl / 扩展）+ 无令牌 → 401
        let r = http_request(
            port,
            "GET /capture?url=http%3A%2F%2F127.0.0.1%3A9%2Fnotoken.bin HTTP/1.1\r\n\
             Host: 127.0.0.1\r\nConnection: close\r\n\r\n",
        );
        assert!(r.starts_with("HTTP/1.1 401"), "响应：{r}");
        assert!(r.contains("bad or missing token"), "响应：{r}");
        assert!(hits.lock().unwrap().is_empty(), "被拒绝的请求不得入队");

        // 4) 无来源 + 正确令牌 → 200 queued，回调收到链接（扩展 / 命令行用法保持可用）
        let r = http_request(
            port,
            "GET /capture?url=http%3A%2F%2F127.0.0.1%3A9%2Fok.bin&token=unit-test-token HTTP/1.1\r\n\
             Host: 127.0.0.1\r\nConnection: close\r\n\r\n",
        );
        assert!(r.starts_with("HTTP/1.1 200"), "响应：{r}");
        assert!(r.contains("\"status\":\"queued\""), "响应：{r}");
        assert_eq!(hits.lock().unwrap().len(), 1);
        assert_eq!(hits.lock().unwrap()[0], "http://127.0.0.1:9/ok.bin");

        // 5) 恶意外站 Origin + 正确令牌 → 授权来源，受理（书签脚本里带令牌的用法）
        let r = http_request(
            port,
            "GET /capture?url=http%3A%2F%2F127.0.0.1%3A9%2Fbookmark.bin&token=unit-test-token HTTP/1.1\r\n\
             Host: 127.0.0.1\r\nOrigin: https://evil.example\r\nConnection: close\r\n\r\n",
        );
        assert!(r.starts_with("HTTP/1.1 200"), "响应：{r}");
        assert_eq!(hits.lock().unwrap().len(), 2, "带令牌的授权来源应受理");

        // 6) /ping：本机可握手，恶意外站 403，且响应永不带 CORS 通配
        let ping = http_request(port, "GET /ping HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        assert!(ping.starts_with("HTTP/1.1 200"), "响应：{ping}");
        assert!(ping.contains("\"version\":\"9.9.9\""), "响应：{ping}");
        assert!(!ping.to_ascii_lowercase().contains("access-control-allow-origin"), "响应：{ping}");
        let ping_evil = http_request(
            port,
            "GET /ping HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: https://evil.example\r\nConnection: close\r\n\r\n",
        );
        assert!(ping_evil.starts_with("HTTP/1.1 403"), "响应：{ping_evil}");
        assert!(!ping_evil.to_ascii_lowercase().contains("access-control-allow-origin"), "响应：{ping_evil}");

        // 7) 缺 url → 400
        let r = http_request(
            port,
            "GET /capture?token=unit-test-token HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        );
        assert!(r.starts_with("HTTP/1.1 400"), "响应：{r}");
        assert!(r.contains("missing url"), "响应：{r}");

        // 收尾等线程真正退出：端口立刻释放，不给后续用例留残留监听线程
        server.stop_and_wait();
    }

    #[test]
    fn end_to_end_reports_blocked_and_skipped_outcomes() {
        let server = start(
            0,
            "t".repeat(32),
            "Umidl".to_string(),
            "9.9.9".to_string(),
            |p| {
                if p.url.contains("banned") {
                    CaptureOutcome::Blocked("扩展名 .exe 在黑名单中".into())
                } else {
                    CaptureOutcome::Skipped("自动入队已关闭".into())
                }
            },
        )
        .expect("启动捕获服务");
        let port = server.port();
        let token = "t".repeat(32);

        let blocked = http_request(
            port,
            &format!(
                "GET /capture?url=http%3A%2F%2Fa.example%2Fbanned.exe&token={token} HTTP/1.1\r\n\
                 Host: 127.0.0.1\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(blocked.starts_with("HTTP/1.1 200"), "响应：{blocked}");
        assert!(blocked.contains("\"status\":\"blocked\""), "响应：{blocked}");
        assert!(blocked.contains("扩展名 .exe 在黑名单中"), "响应：{blocked}");

        let skipped = http_request(
            port,
            &format!(
                "GET /capture?url=http%3A%2F%2Fa.example%2Fok.mp4&token={token} HTTP/1.1\r\n\
                 Host: 127.0.0.1\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(skipped.contains("\"status\":\"skipped\""), "响应：{skipped}");
        assert!(skipped.contains("自动入队已关闭"), "响应：{skipped}");
        assert!(skipped.contains("\"queued\":false"), "响应：{skipped}");

        // 收尾等线程真正退出：端口立刻释放，不给后续用例留残留监听线程
        server.stop_and_wait();
    }

    /// 回归：stop_and_wait 之后端口必须立刻可复用。
    ///
    /// 真机上「端口没变、点保存并重启」曾拿到 os error 10048（旧监听线程还占着端口），
    /// 表现成误导性的「端口可能已被其它程序占用」。这里把「停掉后立刻重绑同一端口」钉死。
    #[test]
    fn stop_and_wait_releases_port_for_immediate_rebind() {
        let a = start(
            0,
            "t".repeat(32),
            "Umidl".to_string(),
            "9.9.9".to_string(),
            |_| CaptureOutcome::Queued,
        )
        .expect("第一次启动");
        let port = a.port();
        assert!(port > 0);

        // 旧写法（只置标志不等待）会在这行炸：监听线程最多还要 60ms 才退出
        a.stop_and_wait();

        let b = start(
            port,
            "t".repeat(32),
            "Umidl".to_string(),
            "9.9.9".to_string(),
            |_| CaptureOutcome::Queued,
        )
        .expect("同一端口必须能立刻重新监听");
        assert_eq!(b.port(), port);
        // 新监听立刻可用
        let r = http_request(
            port,
            "GET /ping HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        );
        assert!(r.starts_with("HTTP/1.1 200"), "响应：{r}");
        b.stop_and_wait();
    }
}
