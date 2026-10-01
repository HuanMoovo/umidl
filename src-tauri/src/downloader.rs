use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;

use crate::ctx::{command_for, ensure_dir, kill_tree, LineBuffer, Ctx};
use crate::models::{DownloadRequest, DownloadTask, MediaInfo, SubtitleTrack, TaskStatus, VideoFormat, AppSettings};

/* ==================== 正则 ==================== */

struct Res {
    percent: Regex,
    total: Regex,
    speed: Regex,
    eta: Regex,
    dest: Regex,
    merge: Regex,
    extract: Regex,
    already: Regex,
    error: Regex,
}

fn res() -> &'static Res {
    static R: OnceLock<Res> = OnceLock::new();
    R.get_or_init(|| Res {
        percent: Regex::new(r"\[download\]\s+([\d.]+)%").unwrap(),
        total: Regex::new(r"of\s+~?\s*([\d.]+)\s*(B|KiB|MiB|GiB|TiB)").unwrap(),
        speed: Regex::new(r"at\s+([\d.]+)\s*(B|KiB|MiB|GiB|TiB)/s").unwrap(),
        eta: Regex::new(r"ETA\s+(\d+:\d+(?::\d+)?)").unwrap(),
        dest: Regex::new(r"\[download\]\s+Destination:\s*(.+)$").unwrap(),
        merge: Regex::new(r#"\[Merger\]\s+Merging formats into\s+"(.+)"\s*$"#).unwrap(),
        extract: Regex::new(r"\[ExtractAudio\]\s+Destination:\s*(.+)$").unwrap(),
        already: Regex::new(r"\[download\]\s+(.+?)\s+has already been downloaded").unwrap(),
        error: Regex::new(r"(?i)^ERROR:\s*(.+)$").unwrap(),
    })
}

fn unit_to_bytes(v: f64, unit: &str) -> i64 {
    let m: f64 = match unit {
        "B" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "TiB" => 1024.0f64.powi(4),
        _ => 1.0,
    };
    (v * m) as i64
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ProgressTick {
    pub percent: Option<f64>,
    pub total: Option<i64>,
    pub downloaded: Option<i64>,
    pub speed: Option<String>,
    pub eta: Option<String>,
}

/// 解析 yt-dlp 的一行下载输出（纯函数，可单测）
pub fn parse_progress(line: &str) -> Option<ProgressTick> {
    if !line.contains("[download]") {
        return None;
    }
    if line.contains("Destination:") || line.contains("has already been downloaded") {
        return None;
    }
    let r = res();
    let mut t = ProgressTick::default();
    if let Some(c) = r.percent.captures(line) {
        t.percent = c[1].parse::<f64>().ok();
    }
    if let Some(c) = r.total.captures(line) {
        let v = c[1].parse::<f64>().unwrap_or(0.0);
        t.total = Some(unit_to_bytes(v, &c[2]));
    }
    if let Some(c) = r.speed.captures(line) {
        let v = c[1].parse::<f64>().unwrap_or(0.0);
        t.speed = Some(format!("{} {}/s", trim_num(v), &c[2]));
    }
    if let Some(c) = r.eta.captures(line) {
        t.eta = Some(c[1].to_string());
    }
    if let (Some(p), Some(total)) = (t.percent, t.total) {
        t.downloaded = Some(((p / 100.0) * total as f64) as i64);
    }
    if t.percent.is_none() && t.speed.is_none() && t.total.is_none() && t.eta.is_none() {
        return None;
    }
    Some(t)
}

fn trim_num(v: f64) -> String {
    if (v - v.round()).abs() < 0.005 {
        format!("{:.0}", v)
    } else {
        format!("{:.2}", v)
    }
}

/* ==================== 多流进度合并（视频流 + 音频流） ==================== */

/// yt-dlp 对 DASH / HLS 站点会把**视频流与音频流当成两个文件**分别下载，
/// 每条流各自从 0% 报到 100% —— 界面上就是「进度条跑了两遍」。
///
/// 实测（本地 HLS 双轨 + yt-dlp 2026.08.19）真实输出形状：
/// ```text
/// [info] master: Downloading 1 format(s): 133+72      ← 打印的 1 不可信，冒号后是两条流
/// [download] Destination: …f133.mp4                   ← 第 1 条流
/// [download]  65.8% … 50.6% … 50.0% … 100%            ← 同一条流内百分比也会来回摆（分片估算）
/// [download] Destination: …f72.mp4                    ← 第 2 条流
/// [download] 100%                                     ← 注意：落盘后还会再打一次 100%
/// [Merger] Merging formats into "…mp4"
/// ```
///
/// 所以这里做三件事：
/// 1. 流数取「打印值」与「`+` 分隔的 id 个数」的较大者；
/// 2. 见到新的 `[download] Destination:` 就认为新的一条流开始；
/// 3. 展示值只增不减（把分片抖动与二次 100% 都抹平），条区间按 `[i/n, (i+1)/n]` 折算。
#[derive(Debug)]
pub struct StreamTracker {
    /// 预计流数（≥1）
    total: u32,
    /// 已经开始下载的流数（Destination 行计数）
    started: u32,
    /// 已完成的流数
    done: u32,
    /// 当前流自己的百分比
    cur: f64,
    /// 已展示过的总进度（只增不减）
    shown: f64,
}

impl Default for StreamTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamTracker {
    pub fn new() -> Self {
        Self {
            total: 1,
            started: 0,
            done: 0,
            cur: 0.0,
            shown: 0.0,
        }
    }

    /// 看一行输出，维护「一共几条流 / 现在第几条」。命中返回 true。
    /// - `Downloading … format(s): …` → 新条目（分 P / 播放列表下一项）重新计数
    /// - `[download] Destination: …`   → 新的一条流开始，之前那条算完成
    pub fn observe(&mut self, line: &str) -> bool {
        if let Some(n) = formats_count(line) {
            self.total = n;
            self.started = 0;
            self.done = 0;
            self.cur = 0.0;
            self.shown = 0.0;
            return true;
        }
        if line.contains("[download] Destination:") {
            self.started += 1;
            if self.started > 1 {
                self.done = self.started - 1;
                self.cur = 0.0;
            }
            self.total = self.total.max(self.started);
            return true;
        }
        false
    }

    /// 把一条进度折算成总进度。只有速度 / ETA（没有百分号）时不动 percent。
    pub fn absorb(&mut self, t: &mut ProgressTick) {
        let Some(p) = t.percent else { return };
        if self.total <= 1 && self.started <= 1 {
            // 单流（绝大多数直链 / 单文件下载）：保持原样，只去抖；100% 视为收尾
            if p >= 99.995 {
                self.done = 1;
                self.cur = 0.0;
            } else {
                self.cur = p;
            }
        } else if p >= 99.995 {
            // 这条流下完了（落盘时还会再来一条 100%，用 min 顶住）
            self.done = (self.done + 1).min(self.total);
            self.cur = 0.0;
        } else {
            self.cur = p;
        }
        let overall = self.overall();
        self.shown = self.shown.max(overall);
        t.percent = Some(self.shown);
    }

    /// 已完成流 + 当前流进度 → 0..100。
    pub fn overall(&self) -> f64 {
        if self.done >= self.total {
            return 100.0;
        }
        ((self.done as f64 + self.cur / 100.0) / self.total.max(1) as f64 * 100.0).min(99.9)
    }

    pub fn total_streams(&self) -> u32 {
        self.total
    }
}

/// 从 `Downloading N format(s): <ids>` 行里读出真实流数（纯函数，可单测）。
/// 打印的 N 不可信：HLS 双轨实测是 `Downloading 1 format(s): 133+72`（两条流），
/// 所以取「打印值」与「id 个数」的较大者。
pub fn formats_count(line: &str) -> Option<u32> {
    let idx = line.find("Downloading ")?;
    let rest = &line[idx + "Downloading ".len()..];
    let (num_s, after) = rest.split_once(" format(s)")?;
    let printed: u32 = num_s.trim().parse().ok()?;
    let ids = after.split_once(':').map(|(_, s)| s).unwrap_or("");
    let listed = ids
        .split([' ', ',', '+', '\t'])
        .filter(|s| !s.trim().is_empty())
        .count() as u32;
    Some(printed.max(listed).max(1))
}

/// 从一行输出中提取「最终产物路径」
pub fn parse_output_path(line: &str) -> Option<String> {
    let r = res();
    for re in [&r.merge, &r.extract, &r.already] {
        if let Some(c) = re.captures(line) {
            let p = c[1].trim().trim_matches('"').trim().to_string();
            if !p.is_empty() {
                return Some(p);
            }
        }
    }
    if let Some(c) = r.dest.captures(line) {
        let p = c[1].trim().trim_matches('"').trim().to_string();
        if !p.is_empty() {
            return Some(p);
        }
    }
    None
}

pub fn parse_error_line(line: &str) -> Option<String> {
    res().error.captures(line).map(|c| c[1].trim().to_string())
}

/// 输出分类
pub enum LineKind {
    Progress(ProgressTick),
    Path(String),
    Error(String),
    Ignore,
}

pub fn classify_line(line: &str) -> LineKind {
    if let Some(e) = parse_error_line(line) {
        return LineKind::Error(e);
    }
    if let Some(t) = parse_progress(line) {
        return LineKind::Progress(t);
    }
    if let Some(p) = parse_output_path(line) {
        return LineKind::Path(p);
    }
    // 裸路径行（yt-dlp --print after_move:filepath 的输出）
    let cand = line.trim().trim_matches('"');
    if (cand.contains(':') || cand.starts_with('/')) && Path::new(cand).is_file() {
        return LineKind::Path(cand.to_string());
    }
    LineKind::Ignore
}

/* ==================== 参数构建 ==================== */

/// 代理端口探活（350ms 超时）
/// 极常见的场景：系统代理开着但客户端（Clash / v2ray）没运行，端口拒连 →
/// 若不探活，所有联网请求都会失败，用户看到的就是「真实站点解析报错」。
pub fn proxy_alive(proxy: &str) -> bool {
    let p = proxy.trim();
    if p.is_empty() {
        return false;
    }
    let after_scheme = p.rsplit("://").next().unwrap_or(p);
    let hostport = after_scheme.split('/').next().unwrap_or(after_scheme);
    let hostport = hostport.rsplit('@').next().unwrap_or(hostport);
    let addr = if hostport.contains(':') {
        hostport.to_string()
    } else {
        format!("{hostport}:80")
    };
    use std::net::ToSocketAddrs;
    match addr.to_socket_addrs() {
        Ok(mut it) => match it.next() {
            Some(a) => std::net::TcpStream::connect_timeout(&a, std::time::Duration::from_millis(350)).is_ok(),
            None => false,
        },
        Err(_) => false,
    }
}

/// 生效的代理地址：显式设置优先，其次环境变量，最后读取 Windows 系统代理（与浏览器一致）。
/// 本机 / 内网地址一律不使用代理；代理端口探活失败时自动回退直连。
pub fn effective_proxy(ctx: &Ctx, url: &str) -> Option<String> {
    if url_is_loopback(url) {
        return None;
    }
    if let Some(px) = ctx.settings.proxy.as_ref().filter(|s| !s.trim().is_empty()) {
        if proxy_alive(px) {
            return Some(px.clone());
        }
        crate::ctx::cwarn(&format!("已配置代理不可用（端口拒连），自动改为直连：{px}"));
        return None;
    }
    // 环境变量覆盖（UMIDL_PROXY / HTTPS_PROXY / HTTP_PROXY / ALL_PROXY）：
    // 启动器、脚本、CI 里最省事的开关，优先级高于系统代理。
    if let Some(px) = env_proxy() {
        if proxy_alive(&px) {
            log_proxy_once("环境变量", &px);
            return Some(px);
        }
        crate::ctx::cwarn(&format!("环境变量里的代理不可用（端口拒连），自动改为直连：{px}"));
        return None;
    }
    if let Some(sys) = system_proxy() {
        if proxy_alive(&sys) {
            log_proxy_once("系统代理", &sys);
            return Some(sys);
        }
        crate::ctx::cwarn(&format!("系统代理已启用但客户端未运行，自动改为直连：{sys}"));
        return None;
    }
    None
}

/// 代理决策日志去重：同一进程内同一个（来源, 地址）只记一次 ——
/// 否则每个下载源 / 每次封面请求都会往日志里刷同样一行
fn log_proxy_once(tag: &str, proxy: &str) {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    let seen = SEEN.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    let first = match seen.lock() {
        Ok(mut g) => g.insert(format!("{tag}|{proxy}")),
        Err(_) => true,
    };
    if first {
        crate::ctx::cwarn(&format!("使用代理：{proxy}（来源：{tag}）"));
    }
}

/// 下载/联网客户端最终采用的代理配置（纯数据，便于单测）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyPlan {
    /// 显式交给客户端的代理地址（None = 直连）
    pub proxy: Option<String>,
    /// 是否必须显式禁止客户端读取代理环境变量
    pub no_proxy: bool,
}

/// 由 `effective_proxy` 的决策结果推导客户端构造参数（纯函数，便于单测）：
///
/// - 决策为直连（None / 空白）→ 必须 `no_proxy = true`。reqwest 默认会自己读
///   `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY`，于是「环境变量里的代理不可用（端口拒连），
///   自动改为直连」只是日志说说，实际请求仍然走那个死代理（真机上就是这么挂的）。
/// - 决策为用某代理 → 显式带地址（调用方用 `Proxy::all(url)`），且不禁用环境变量读取
///   （显式设置优先级更高，留着无害）。
pub fn client_proxy_plan(decided: Option<&str>) -> ProxyPlan {
    match decided.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(px) => ProxyPlan {
            proxy: Some(px.to_string()),
            no_proxy: false,
        },
        None => ProxyPlan {
            proxy: None,
            no_proxy: true,
        },
    }
}

/// 代理地址规范化：`127.0.0.1:7892` → `http://127.0.0.1:7892`（缺协议是最常见的写法错误；
/// `socks*://`、`http(s)://` 原样保留）。空串 / 纯空白 → None。
pub fn normalize_proxy(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("socks5://")
        || lower.starts_with("socks5h://")
        || lower.starts_with("socks4://")
        || lower.starts_with("socks4a://")
        || lower.starts_with("socks://")
    {
        Some(s.to_string())
    } else {
        Some(format!("http://{s}"))
    }
}

/// 代理环境变量：`UMIDL_PROXY` 优先（应用级覆盖），其次 `HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY`；
/// 大小写两种写法都认（与 curl / reqwest 的习惯一致）。某个变量没设或只写了空白 → 视为没设，
/// 继续看下一个；全部都如此 → None。
pub fn env_proxy() -> Option<String> {
    const KEYS: [&str; 8] = [
        "UMIDL_PROXY",
        "umidl_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ];
    // 必须在「第一个有值的变量」上停下，而不是「第一个设过的变量」：`UMIDL_PROXY="   "`
    // 这类空白写法（启动器 set 了个空值）不能把后面的 HTTPS_PROXY / HTTP_PROXY / ALL_PROXY 一起挡掉。
    KEYS.iter().find_map(|k| std::env::var(k).ok().and_then(|v| normalize_proxy(&v)))
}

/// 解析 `reg query … /v ProxyEnable` 输出里的 REG_DWORD 值（纯函数，便于单测）。
///
/// 真机输出形如（注意是 **REG_DWORD**，不是 REG_SZ）：
/// ```text
/// HKEY_CURRENT_USER\...\Internet Settings
///     ProxyEnable    REG_DWORD    0x1
/// ```
/// 旧实现只会找 `REG_SZ` 后面的文本 → 这里永远返回 None，于是「系统代理」分支等于不存在。
/// 支持 `0x1` 十六进制写法与 `1` 十进制写法；查不到 / 值不是数字 → None（当作没启用）。
pub fn parse_reg_dword(text: &str) -> Option<u32> {
    for line in text.lines() {
        let Some(pos) = line.find("REG_DWORD") else { continue };
        let raw = line[pos + "REG_DWORD".len()..].trim().trim_matches('"').trim();
        if raw.is_empty() {
            continue;
        }
        if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
            if let Ok(v) = u32::from_str_radix(hex, 16) {
                return Some(v);
            }
            continue;
        }
        if let Ok(v) = raw.parse::<u32>() {
            return Some(v);
        }
    }
    None
}

/// 解析 `reg query … /v ProxyServer` 输出里的 REG_SZ 值（纯函数）。
/// 找不到该类型 → None；值是空串 → `Some("")`（由调用方决定怎么处理）。
pub fn parse_reg_sz(text: &str) -> Option<String> {
    for line in text.lines() {
        let upper = line.to_ascii_uppercase();
        for t in ["REG_EXPAND_SZ", "REG_SZ"] {
            if let Some(pos) = upper.find(t) {
                return Some(line[pos + t.len()..].trim().to_string());
            }
        }
    }
    None
}

/// 注册表里的 `ProxyServer` 取值 → 规范化代理地址（纯函数）。
///
/// 取值花样很多，行为必须确定：
/// - `127.0.0.1:7892` → `http://127.0.0.1:7892`
/// - `http=127.0.0.1:7892;https=127.0.0.1:7892` → 取 `http=` 那一段
/// - `127.0.0.1:7892;https=127.0.0.1:7892`（第一段没有键名）→ 取第一段（默认代理）
/// - 只有键名没有值 / 空串 / 全是分隔符 → None
pub fn proxy_addr_from_reg_value(raw: &str) -> Option<String> {
    let r = raw.trim();
    if r.is_empty() {
        return None;
    }
    let addr = if r.contains('=') {
        let segs: Vec<&str> = r.split(';').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        let by_http = segs
            .iter()
            .find(|s| s.to_ascii_lowercase().starts_with("http="))
            .and_then(|s| s.split_once('=').map(|(_, v)| v.trim().to_string()))
            .filter(|v| !v.is_empty());
        // 没有 http= 时，退回「没带键名的那一段」（Windows 允许 `host:port;https=host:port` 混写）
        let plain = segs.iter().find(|s| !s.contains('=')).map(|s| (*s).to_string());
        // 再退一步：任意 `key=value` 里的值
        let any = segs
            .iter()
            .find_map(|s| s.split_once('=').map(|(_, v)| v.trim().to_string()))
            .filter(|v| !v.is_empty());
        by_http.or(plain).or(any)?
    } else {
        r.to_string()
    };
    normalize_proxy(&addr)
}

/// 由注册表两段输出推导系统代理（纯函数，便于单测）：
/// `ProxyEnable` 必须是 1（0x1 / 1 都认），否则视为未启用；`ProxyServer` 缺失或空 → None。
pub fn system_proxy_from_reg(enable_out: &str, server_out: &str) -> Option<String> {
    let enabled = parse_reg_dword(enable_out)?;
    if enabled == 0 {
        return None;
    }
    let raw = parse_reg_sz(server_out)?;
    proxy_addr_from_reg_value(&raw)
}

/// 读取 Windows 系统代理（HKCU 注册表 ProxyEnable / ProxyServer）
#[cfg(windows)]
pub fn system_proxy() -> Option<String> {
    use std::os::windows::process::CommandExt;
    const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";
    // 只看原文，解析交给 parse_reg_dword / parse_reg_sz（旧实现只认 REG_SZ，ProxyEnable 永远解析不出来）
    let run = |name: &str| -> String {
        let out = std::process::Command::new("reg")
            .args(["query", KEY, "/v", name])
            .creation_flags(0x0800_0000)
            .output();
        match out {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            _ => String::new(),
        }
    };
    system_proxy_from_reg(&run("ProxyEnable"), &run("ProxyServer"))
}

#[cfg(not(windows))]
pub fn system_proxy() -> Option<String> {
    None
}

/// 判断 URL 是否指向本机（127.0.0.0/8、localhost、::1、0.0.0.0）
/// 本机地址必须绕过系统代理：Windows 代理设置（Clash / v2ray 等）会把 127.0.0.1 也代理走，
/// 导致本地下载返回 502 Bad Gateway。
pub fn url_is_loopback(url: &str) -> bool {
    let after_scheme = match url.split_once("://") {
        Some((_, rest)) => rest,
        None => url,
    };
    let host_part = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or_else(|| after_scheme.split(['/', '?', '#']).next().unwrap_or(""));
    let host = if host_part.starts_with('[') {
        host_part.split(']').next().unwrap_or("").trim_start_matches('[')
    } else {
        host_part.split(':').next().unwrap_or("")
    };
    let h = host.to_lowercase();
    h == "localhost"
        || h == "::1"
        || h == "0.0.0.0"
        || h.ends_with(".localhost")
        || h.starts_with("127.")
}

/// yt-dlp 分片并发参数（纯函数，便于单测）
///
/// 对应设置项 `ytdlp_concurrency`（1..=16，越界自动限幅）：
/// 数值越大，m3u8/DASH 分片下载越快，但请求更密集，易被站点限速。
pub fn ytdlp_fragment_args(concurrency: i64) -> Vec<String> {
    vec![
        "--concurrent-fragments".into(),
        concurrency.clamp(crate::models::YTDLP_CONCURRENCY_RANGE.0, crate::models::YTDLP_CONCURRENCY_RANGE.1).to_string(),
    ]
}

pub fn build_args(req: &DownloadRequest, ctx: &Ctx, out_dir: &Path) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "--newline".into(),
        "--no-color".into(),
        "--progress".into(),
        "--no-warnings".into(),
        "--ignore-config".into(),
        "--retries".into(),
        "5".into(),
        "--fragment-retries".into(),
        "5".into(),
        "--no-mtime".into(),
    ];
    // 分片并发（设置可调，1..=16 限幅）
    a.extend(ytdlp_fragment_args(ctx.settings.ytdlp_concurrency));
    a.push(if req.playlist { "--yes-playlist".into() } else { "--no-playlist".into() });

    let template = req
        .filename_template
        .clone()
        .or_else(|| ctx.settings.filename_template.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "%(title).150B [%(id)s].%(ext)s".into());
    a.push("-P".into());
    a.push(out_dir.to_string_lossy().to_string());
    a.push("-o".into());
    a.push(template);

    match req.mode.as_str() {
        // 仅下载封面
        "thumb" => {
            a.push("--skip-download".into());
            a.push("--write-thumbnail".into());
            a.push("--convert-thumbnails".into());
            a.push("jpg".into());
        }
        // 仅下载字幕（可多语言）
        "subs" => {
            a.push("--skip-download".into());
            a.push("--write-subs".into());
            a.push("--write-auto-subs".into());
            a.push("--convert-subs".into());
            a.push("srt".into());
            let langs = if req.subtitle_langs.is_empty() {
                ctx.settings.subtitle_language.clone()
            } else {
                req.subtitle_langs.join(",")
            };
            a.push("--sub-langs".into());
            a.push(
                if langs.trim().is_empty() || langs.trim() == "auto" {
                    "all".to_string()
                } else {
                    langs
                },
            );
        }
        "audio" => {
            a.push("-f".into());
            a.push("bestaudio/best".into());
            a.push("-x".into());
            a.push("--audio-format".into());
            a.push(req.audio_format.clone().unwrap_or_else(|| "mp3".into()));
            a.push("--audio-quality".into());
            a.push("0".into());
        }
        "best" => {
            a.push("-f".into());
            a.push("bestvideo*+bestaudio/best".into());
        }
        _ => {
            let expr = match req.format_id.as_ref().filter(|s| !s.trim().is_empty()) {
                Some(fid) => format!("{fid}+bestaudio/{fid}/bestvideo*+bestaudio/best"),
                None => "bestvideo*+bestaudio/best".to_string(),
            };
            a.push("-f".into());
            a.push(expr);
        }
    }

    let only_media = matches!(req.mode.as_str(), "audio" | "best" | "video" | "");
    let container = req
        .merge_container
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "mp4".into());
    if only_media {
        a.push("--merge-output-format".into());
        a.push(container);
    }
    if req.embed_thumbnail && only_media {
        a.push("--embed-thumbnail".into());
    }
    if req.embed_subs && only_media {
        a.push("--write-subs".into());
        a.push("--embed-subs".into());
        a.push("--sub-langs".into());
        a.push("zh.*,en.*,en".into());
    }
    let rate = req
        .rate_limit
        .clone()
        .or_else(|| ctx.settings.rate_limit.clone());
    if let Some(rl) = rate.as_ref().filter(|s| !s.trim().is_empty()) {
        a.push("--limit-rate".into());
        a.push(rl.clone());
    }
    if let Some(dir) = ctx.tools.ffmpeg_dir() {
        a.push("--ffmpeg-location".into());
        a.push(dir.to_string_lossy().to_string());
    }
    match effective_proxy(ctx, &req.url) {
        Some(px) => {
            a.push("--proxy".into());
            a.push(px);
        }
        // 本机地址强制直连，避免被系统代理（Clash / v2ray）拦截 → 502
        None if url_is_loopback(&req.url) => {
            a.push("--proxy".into());
            a.push(String::new());
        }
        None => {}
    }
    if let Some(ck) = ctx.settings.cookies_file.as_ref().filter(|s| !s.trim().is_empty()) {
        if Path::new(ck).is_file() {
            a.push("--cookies".into());
            a.push(ck.clone());
        }
    }
    a.push("--".into());
    a.push(req.url.clone());
    a
}

/* ==================== 解析媒体信息 ==================== */

pub fn parse_media_info(v: &serde_json::Value) -> MediaInfo {
    let is_playlist = v.get("_type").and_then(|t| t.as_str()) == Some("playlist");
    let node = if is_playlist {
        v.get("entries")
            .and_then(|e| e.as_array())
            .and_then(|arr| arr.iter().find(|x| x.get("id").is_some()))
            .unwrap_or(v)
    } else {
        v
    };

    let s = |k: &str| node.get(k).and_then(|x| x.as_str()).map(|x| x.to_string());
    let n = |k: &str| node.get(k).and_then(|x| x.as_f64());

    let mut formats: Vec<VideoFormat> = Vec::new();
    if let Some(arr) = node.get("formats").and_then(|x| x.as_array()) {
        for f in arr {
            let vcodec = f.get("vcodec").and_then(|x| x.as_str()).map(|x| x.to_string());
            let acodec = f.get("acodec").and_then(|x| x.as_str()).map(|x| x.to_string());
            let width = f.get("width").and_then(|x| x.as_i64());
            let height = f.get("height").and_then(|x| x.as_i64());
            // 视频判定：优先看 vcodec；不少站点（archive.org、部分 CMS）不给编解码字段，
            // 此时用宽高 / 分辨率兜底，否则会被误判成纯音频。
            let res_hint = f
                .get("resolution")
                .and_then(|x| x.as_str())
                .map(|r| r.contains('x') && r != "audio only")
                .unwrap_or(false);
            let has_video = match vcodec.as_deref() {
                Some(c) => c != "none",
                None => width.unwrap_or(0) > 0 || height.unwrap_or(0) > 0 || res_hint,
            };
            // 音频判定：优先 acodec；否则看音频码率 / 采样率 / 声道
            let has_audio = match acodec.as_deref() {
                Some(c) => c != "none",
                None => {
                    f.get("abr").and_then(|x| x.as_f64()).unwrap_or(0.0) > 0.0
                        || f.get("asr").and_then(|x| x.as_f64()).unwrap_or(0.0) > 0.0
                        || f.get("audio_channels").and_then(|x| x.as_i64()).unwrap_or(0) > 0
                        // 视频容器通常自带音轨（archive.org 的 ogv/mp4 即如此）
                        || (has_video && !f.get("acodec").is_some())
                }
            };
            let resolution = f
                .get("resolution")
                .and_then(|x| x.as_str())
                .filter(|x| !x.is_empty() && *x != "audio only")
                .map(|x| x.to_string())
                .or_else(|| match (width, height) {
                    (Some(w), Some(h)) => Some(format!("{w}x{h}")),
                    _ => None,
                })
                .unwrap_or_else(|| if has_video { "unknown".into() } else { "audio only".into() });
            let filesize = f
                .get("filesize")
                .and_then(|x| x.as_i64())
                .or_else(|| f.get("filesize_approx").and_then(|x| x.as_i64()));
            formats.push(VideoFormat {
                format_id: f.get("format_id").and_then(|x| x.as_str()).unwrap_or("?").to_string(),
                ext: f.get("ext").and_then(|x| x.as_str()).unwrap_or("?").to_string(),
                resolution,
                fps: f.get("fps").and_then(|x| x.as_f64()),
                vcodec,
                acodec,
                filesize,
                filesize_text: filesize.map(|f| crate::tools::human_size(f.max(0) as u64)),
                note: f.get("format_note").and_then(|x| x.as_str()).map(|x| x.to_string()),
                has_video,
                has_audio,
            });
        }
    }

    let subtitles = node
        .get("subtitles")
        .and_then(|x| x.as_object())
        .map(|m| m.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    // 字幕轨道明细：手动字幕优先，自动字幕补充
    let mut subtitle_tracks: Vec<SubtitleTrack> = Vec::new();
    for (auto, key) in [(false, "subtitles"), (true, "automatic_captions")] {
        let Some(map) = node.get(key).and_then(|x| x.as_object()) else {
            continue;
        };
        for (lang, arr) in map {
            if auto && subtitles.iter().any(|l| l == lang) {
                continue;
            }
            let Some(first) = arr.as_array().and_then(|a| a.first()) else {
                continue;
            };
            if subtitle_tracks.iter().any(|t| &t.lang == lang && t.auto == auto) {
                continue;
            }
            subtitle_tracks.push(SubtitleTrack {
                lang: lang.clone(),
                name: first.get("name").and_then(|x| x.as_str()).map(|x| x.to_string()),
                ext: first
                    .get("ext")
                    .and_then(|x| x.as_str())
                    .unwrap_or("vtt")
                    .to_string(),
                auto,
                url: first.get("url").and_then(|x| x.as_str()).map(|x| x.to_string()),
            });
        }
    }
    subtitle_tracks.sort_by(|a, b| a.auto.cmp(&b.auto).then_with(|| a.lang.cmp(&b.lang)));

    // 封面：从 thumbnails 中挑一张清晰又不大的
    let mut thumb_url = s("thumbnail");
    let mut tw = None;
    let mut th = None;
    if let Some(arr) = node.get("thumbnails").and_then(|x| x.as_array()) {
        let mut best: Option<(i64, &serde_json::Value)> = None;
        for t in arr {
            let w = t.get("width").and_then(|x| x.as_i64()).unwrap_or(0);
            let h = t.get("height").and_then(|x| x.as_i64()).unwrap_or(0);
            if t.get("url").and_then(|x| x.as_str()).is_none() {
                continue;
            }
            if w > 1280 {
                continue;
            }
            if best.map(|(bw, _)| w > bw).unwrap_or(true) {
                best = Some((w, t));
            }
            let _ = h;
        }
        if let Some((_, t)) = best {
            thumb_url = t.get("url").and_then(|x| x.as_str()).map(|x| x.to_string());
            tw = t.get("width").and_then(|x| x.as_i64());
            th = t.get("height").and_then(|x| x.as_i64());
        }
    }

    let playlist_count = v
        .get("playlist_count")
        .and_then(|x| x.as_i64())
        .or_else(|| v.get("entries").and_then(|e| e.as_array()).map(|a| a.len() as i64));

    MediaInfo {
        id: s("id").unwrap_or_default(),
        title: s("title").unwrap_or_else(|| "未命名".into()),
        thumbnail: thumb_url,
        thumbnail_local: None,
        thumbnail_width: tw,
        thumbnail_height: th,
        uploader: s("uploader").or_else(|| s("channel")).or_else(|| s("creator")),
        duration: n("duration"),
        webpage_url: s("webpage_url").unwrap_or_default(),
        extractor: s("extractor_key").or_else(|| s("extractor")),
        description: s("description").map(|d| d.chars().take(600).collect()),
        filesize_approx: n("filesize_approx").map(|f| f as i64),
        formats,
        subtitles,
        subtitle_tracks,
        is_playlist,
        playlist_count,
        view_count: node.get("view_count").and_then(|x| x.as_i64()),
        like_count: node.get("like_count").and_then(|x| x.as_i64()),
        upload_date: s("upload_date"),
        probe_ms: None,
    }
}

/// 已下载文件的封面：
/// 1) 本地 cover 缓存命中 → 直接返回
/// 2) 否则用 ffmpeg 从视频抽一帧（历史任务 / 未解析直接下载 / 站点不提供封面图）
/// 纯音频文件不抽帧（返回 None，界面显示占位图标）
pub fn cover_from_file(ctx: &Ctx, file: &str, key: &str, duration: Option<f64>) -> Option<String> {
    let path = Path::new(file);
    if !path.is_file() {
        return None;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    const VIDEO_EXT: [&str; 11] = [
        "mp4", "mkv", "webm", "mov", "avi", "flv", "ts", "m4v", "ogv", "mpg", "wmv",
    ];
    if !VIDEO_EXT.contains(&ext.as_str()) {
        return None;
    }
    let safe: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect();
    let safe = if safe.is_empty() { "cover".to_string() } else { safe };
    let dir = ctx.dirs.cache.join("covers");
    let _ = std::fs::create_dir_all(&dir);
    let dest = dir.join(format!("{safe}.jpg"));
    if let Ok(m) = std::fs::metadata(&dest) {
        if m.len() > 512 {
            return Some(dest.to_string_lossy().to_string());
        }
    }
    // 抽取位置：短视频取整片 15%（最多 30s），再退回第 0 帧
    let at = duration
        .filter(|d| *d > 6.0)
        .map(|d| (d * 0.15).min(30.0))
        .unwrap_or(1.0);
    if crate::converter::extract_thumbnail(ctx, file, at, &dest).is_err()
        && crate::converter::extract_thumbnail(ctx, file, 0.0, &dest).is_err()
    {
        return None;
    }
    std::fs::metadata(&dest)
        .ok()
        .filter(|m| m.len() > 512)
        .map(|_| dest.to_string_lossy().to_string())
}

/// 把封面缓存到本地（失败静默忽略，不影响解析结果）
pub fn cache_thumbnail(ctx: &Ctx, url: &str, key: &str, referer: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() || !url.starts_with("http") {
        return None;
    }
    let mut builder = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(6))
        .user_agent(USER_AGENT);
    // 代理同样按「已裁决结果」构造：决策为直连时必须禁用环境变量代理，否则死代理会连累封面
    let plan = client_proxy_plan(effective_proxy(ctx, url).as_deref());
    match &plan.proxy {
        Some(px) => match reqwest::Proxy::all(px) {
            Ok(p) => builder = builder.proxy(p),
            Err(_) => builder = builder.no_proxy(),
        },
        None if plan.no_proxy => builder = builder.no_proxy(),
        None => {}
    }
    let client = builder.build().ok()?;

    let ext = url
        .split('?')
        .next()
        .unwrap_or("")
        .rsplit('.')
        .next()
        .filter(|e| e.len() <= 4 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("jpg");
    let safe: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect();
    let safe = if safe.is_empty() { "thumb".to_string() } else { safe };
    let dir = ctx.dirs.cache.join("thumbs");
    let _ = std::fs::create_dir_all(&dir);
    let dest = dir.join(format!("{safe}.{ext}"));

    if let Ok(m) = std::fs::metadata(&dest) {
        if m.len() > 512 {
            return Some(dest.to_string_lossy().to_string());
        }
    }

    let mut rb = client.get(url);
    if !referer.trim().is_empty() {
        rb = rb.header(reqwest::header::REFERER, referer.trim());
    }
    let bytes = rb.send().ok()?.bytes().ok()?;
    if bytes.len() < 512 {
        return None;
    }
    std::fs::write(&dest, &bytes).ok()?;
    Some(dest.to_string_lossy().to_string())
}

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";

/// 探测媒体信息（联网）。带「代理失败 → 直连重试」兜底：
/// 很多用户系统代理开着但代理客户端没运行（端口拒连），此时必须能自动回退直连。
pub fn probe(ctx: &Ctx, url: &str, playlist: bool) -> anyhow::Result<MediaInfo> {
    let started = std::time::Instant::now();
    let attempts: Vec<Option<String>> = match effective_proxy(ctx, url) {
        Some(px) => vec![Some(px), Some(String::new())],
        None => vec![None],
    };
    let mut errors: Vec<String> = Vec::new();
    let n = attempts.len();
    for (i, px) in attempts.into_iter().enumerate() {
        match probe_once(ctx, url, playlist, px.as_deref()) {
            Ok(mut info) => {
                if i > 0 && n > 1 {
                    crate::ctx::cwarn(&format!("代理链路失败，已直连重试成功：{url}"));
                }
                info.probe_ms = Some(started.elapsed().as_millis() as i64);
                return Ok(info);
            }
            Err(e) => errors.push(e.to_string()),
        }
    }
    anyhow::bail!("{}", errors.join(" ｜ "))
}

/// 单次探测：严格校验退出码与输出，杜绝把 yt-dlp 的 "null" 当成有效结果
fn probe_once(
    ctx: &Ctx,
    url: &str,
    playlist: bool,
    proxy: Option<&str>,
) -> anyhow::Result<MediaInfo> {
    let exe = ctx.tools.ytdlp()?.to_path_buf();
    let mut cmd = command_for(&exe);
    cmd.arg("--dump-single-json")
        .arg("--no-warnings")
        .arg("--ignore-config")
        .arg("--skip-download");
    if playlist {
        cmd.arg("--yes-playlist").arg("--flat-playlist");
    } else {
        cmd.arg("--no-playlist");
    }
    if let Some(px) = proxy {
        cmd.arg("--proxy").arg(px);
    }
    if let Some(ck) = ctx.settings.cookies_file.as_ref().filter(|s| !s.trim().is_empty()) {
        if Path::new(ck).is_file() {
            cmd.arg("--cookies").arg(ck);
        }
    }
    cmd.arg("--").arg(url);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());

    let out = cmd.output()?;
    let stdout = crate::ctx::decode_output(&out.stdout);
    let stderr = crate::ctx::decode_output(&out.stderr);
    let err_line = || -> String {
        stderr
            .lines()
            .rev()
            .find(|l| l.to_uppercase().contains("ERROR"))
            .or_else(|| stderr.lines().last())
            .unwrap_or("解析失败")
            .trim()
            .chars()
            .take(400)
            .collect()
    };

    // 关键：yt-dlp 失败时仍可能向 stdout 输出 "null"，不能只看 stdout 是否为空
    if !out.status.success() {
        anyhow::bail!("{}", err_line());
    }
    let trimmed = stdout.trim();
    if trimmed.is_empty() || trimmed == "null" {
        anyhow::bail!("{}", err_line());
    }
    let v: serde_json::Value = serde_json::from_str(trimmed)
        .map_err(|e| anyhow::anyhow!("解析 yt-dlp 输出失败：{e}"))?;
    if !v.is_object() {
        anyhow::bail!("yt-dlp 未返回有效媒体信息：{}", err_line());
    }
    let mut info = parse_media_info(&v);
    // 解析到空壳时给出可执行的错误，而不是把空信息交给界面
    if info.formats.is_empty() && info.title.trim().is_empty() {
        anyhow::bail!(
            "未能解析该链接的媒体信息：视频可能不可用、需要登录（可在设置中指定 Cookies），或站点接口已变更 ｜ {}",
            err_line()
        );
    }
    if !playlist && info.formats.is_empty() {
        anyhow::bail!(
            "未获取到任何可下载格式（可能需要登录 / Cookies）：{}",
            info.title
        );
    }
    // 封面缓存到本地：离线也能显示，且可直接另存为封面文件
    if let Some(t) = info.thumbnail.clone() {
        let key = if info.id.is_empty() {
            info.title.clone()
        } else {
            info.id.clone()
        };
        let referer = if info.webpage_url.is_empty() {
            url.to_string()
        } else {
            info.webpage_url.clone()
        };
        if let Some(local) = cache_thumbnail(ctx, &t, &key, &referer) {
            info.thumbnail_local = Some(local);
        }
    }
    Ok(info)
}

/* ==================== 执行下载 ==================== */

pub struct RunOutcome {
    pub exit_code: i32,
    pub final_path: Option<String>,
    pub stderr_tail: String,
    pub killed: bool,
}

/* ==================== 错误原因摘取（BUG-08） ==================== */

/// 从引擎 stderr 尾部里挑出「真正说明原因」的那一行。
///
/// aria2 的真实原因写在 `[ERROR] ... Download aborted` 之后的 `Exception:` /
/// `errorCode=` 行；以前只取 `lines().last()`，UI 上只能看到无关的 “Download aborted”。
pub fn best_error_line(tail: &str) -> String {
    let lines: Vec<&str> = tail.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    if lines.is_empty() {
        return String::new();
    }
    const KEYS: [&str; 6] = ["errorCode=", "Exception:", "Failed to", "errNum=", "cause:", "No such file"];
    if let Some(l) = lines.iter().rev().find(|l| KEYS.iter().any(|k| l.contains(k))) {
        return (*l).to_string();
    }
    if let Some(l) = lines.iter().rev().find(|l| l.contains("[ERROR]")) {
        return (*l).to_string();
    }
    lines[lines.len() - 1].to_string()
}

/* ==================== 目标路径长度预检（BUG-08） ==================== */

/// Windows MAX_PATH 上限（含结尾 NUL 共 260）
pub const WIN_MAX_PATH: usize = 260;

/// 路径长度（按 UTF-16 码元计，与 Windows 的判定口径一致）
pub fn path_unit_len(p: &Path) -> usize {
    p.as_os_str().to_string_lossy().encode_utf16().count()
}

/// 目标路径过长直接返回可读错误：aria2 只会回一句 “Download aborted”（errorCode=18），
/// 与其等引擎失败，不如入队时就告诉用户原因。
pub fn check_target_path_len(full: &Path) -> Result<(), String> {
    let n = path_unit_len(full);
    if n > 259 {
        return Err(format!(
            "目标路径过长（{} 个字符，Windows 上限 259）：{}。请缩短下载目录或文件名后重试",
            n,
            full.display()
        ));
    }
    Ok(())
}

/// aria2 用的文件名：URL path 的最后一段（去掉 scheme/host、query、fragment，并做百分号解码）
pub fn url_file_name(url: &str) -> Option<String> {
    let no_frag = url.split('#').next().unwrap_or(url);
    let no_query = no_frag.split('?').next().unwrap_or(no_frag);
    // 跳过 scheme://host：只有 path 里的最后一段才是文件名
    // （否则 https://a.com/ 会把主机名当成文件名）
    let path = match no_query.split_once("://") {
        Some((_, rest)) => rest.split_once('/').map(|(_, p)| p).unwrap_or(""),
        None => no_query,
    };
    let seg = path.trim_end_matches('/').rsplit('/').next()?;
    if seg.trim().is_empty() {
        return None;
    }
    let decoded = percent_decode(seg);
    // 解码后可能带分隔符（%2F）：只保留最后一段，避免预检/清理作用到别的目录
    let name = decoded.rsplit(['/', '\\']).next().unwrap_or(&decoded).to_string();
    if !name_is_plausible(&name) {
        return None;
    }
    Some(name)
}

/// 看起来像「能落盘的文件名」吗（不是 `magnet:` 这类协议串、也不是路径片段）
pub fn name_is_plausible(n: &str) -> bool {
    let t = n.trim();
    if t.is_empty() || t == "." || t == ".." {
        return false;
    }
    // Windows 不允许的字符：: * ? " < > | 以及分隔符
    !t.chars()
        .any(|c| matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\\' | '/'))
        && !t.chars().any(|c| (c as u32) < 0x20)
}

/// 最小百分号解码（只处理 UTF-8 序列，失败则原样返回）
pub fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0usize;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hex {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// 入队前的目标路径预检：目录本身与（能推断出的）文件名都要在 MAX_PATH 以内
pub fn check_output_path(out_dir: &Path, req: &DownloadRequest) -> Result<(), String> {
    check_target_path_len(out_dir).map_err(|e| format!("{e}（下载目录）"))?;
    if let Some(name) = url_file_name(&req.url) {
        check_target_path_len(&out_dir.join(name))?;
    }
    Ok(())
}

/// 执行一次 yt-dlp 下载
pub fn run_download<F>(
    ctx: &Ctx,
    req: &DownloadRequest,
    out_dir: &Path,
    mut on_tick: F,
    register_pid: impl Fn(u32),
    is_canceled: impl Fn() -> bool,
) -> anyhow::Result<RunOutcome>
where
    F: FnMut(ProgressTick),
{
    let exe = ctx.tools.ytdlp()?.to_path_buf();
    ensure_dir(out_dir)?;
    let args = build_args(req, ctx, out_dir);

    let mut cmd = command_for(&exe);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    let started_ms = crate::ctx::now_ms();
    let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("启动 yt-dlp 失败：{e}"))?;
    let pid = child.id();
    register_pid(pid);

    let (tx, rx) = mpsc::channel::<(bool, String)>();
    let mut readers = Vec::new();
    if let Some(so) = child.stdout.take() {
        let tx2 = tx.clone();
        readers.push(std::thread::spawn(move || {
            read_lines(so, tx2, false);
        }));
    }
    if let Some(se) = child.stderr.take() {
        let tx2 = tx.clone();
        readers.push(std::thread::spawn(move || {
            read_lines(se, tx2, true);
        }));
    }
    drop(tx);

    let mut final_path: Option<String> = None;
    let mut errs: Vec<String> = Vec::new();
    let mut killed = false;
    let started = Instant::now();

    // 多流（视频流 + 音频流）折算成一条总进度，避免进度条"跑两遍"（见 StreamTracker 注释）
    let mut tracker = StreamTracker::new();
    let mut handle = |is_err: bool, line: String,
                  final_path: &mut Option<String>,
                  errs: &mut Vec<String>,
                  on_tick: &mut F| {
        tracker.observe(&line);
        match classify_line(&line) {
            LineKind::Error(m) => push_tail(errs, m),
            LineKind::Progress(mut t) => {
                tracker.absorb(&mut t);
                on_tick(t);
            }
            LineKind::Path(p) => {
                if Path::new(&p).is_file() {
                    *final_path = Some(p);
                }
            }
            LineKind::Ignore => {
                if is_err {
                    push_tail(errs, line);
                }
            }
        }
    };

    loop {
        match rx.recv_timeout(Duration::from_millis(150)) {
            Ok((is_err, line)) => handle(is_err, line, &mut final_path, &mut errs, &mut on_tick),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if is_canceled() && !killed {
            kill_tree(pid);
            killed = true;
        }
        if started.elapsed() > Duration::from_secs(6 * 3600) && !killed {
            kill_tree(pid);
            killed = true;
            push_tail(&mut errs, "下载超过 6 小时，已自动中止".into());
        }
        if let Ok(Some(_)) = child.try_wait() {
            let deadline = Instant::now() + Duration::from_millis(600);
            while Instant::now() < deadline {
                match rx.recv_timeout(Duration::from_millis(80)) {
                    Ok((is_err, line)) => {
                        handle(is_err, line, &mut final_path, &mut errs, &mut on_tick)
                    }
                    Err(_) => break,
                }
            }
            break;
        }
    }

    let status = child.wait()?;
    for h in readers {
        let _ = h.join();
    }
    let code = status.code().unwrap_or(-1);

    if final_path.is_none() {
        if let Some(p) = newest_media_file(out_dir, started_ms) {
            final_path = Some(p.to_string_lossy().to_string());
        }
    }

    Ok(RunOutcome {
        exit_code: code,
        final_path,
        stderr_tail: errs.join("\n"),
        killed,
    })
}

fn push_tail(v: &mut Vec<String>, s: String) {
    if !s.trim().is_empty() {
        v.push(s);
        if v.len() > 40 {
            v.remove(0);
        }
    }
}

fn read_lines<R: Read>(mut rd: R, tx: mpsc::Sender<(bool, String)>, is_err: bool) {
    let mut buf = [0u8; 8192];
    let mut lb = LineBuffer::new();
    loop {
        match rd.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                for l in lb.push(&buf[..n]) {
                    if tx.send((is_err, l)).is_err() {
                        return;
                    }
                }
            }
        }
    }
    if let Some(l) = lb.flush() {
        let _ = tx.send((is_err, l));
    }
}

/// 探测输出目录里最新的媒体文件（路径兜底）
pub fn newest_media_file(dir: &Path, since_ms: i64) -> Option<PathBuf> {
    // 含封面 / 字幕等附带产物扩展名，方便“单独下载封面 / 字幕”任务定位成果
    const EXTS: [&str; 28] = [
        "mp4", "mkv", "webm", "mov", "avi", "flv", "ts", "m4v", "3gp", "mpg", "mp3", "m4a", "aac",
        "flac", "wav", "opus", "ogg", "wma", "jpg", "jpeg", "png", "webp", "srt", "vtt", "ass",
        "lrc", "json3", "srv3",
    ];
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(i64, PathBuf)> = None;
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
        if !EXTS.contains(&ext.as_str()) {
            continue;
        }
        let meta = match e.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mt = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        if mt + 8000 < since_ms {
            continue;
        }
        if best.as_ref().map(|(bt, _)| mt > *bt).unwrap_or(true) {
            best = Some((mt, p));
        }
    }
    best.map(|(_, p)| p)
}

/// 创建新任务对象
pub fn new_task(req: &DownloadRequest, created: i64) -> DownloadTask {
    DownloadTask {
        id: crate::ctx::short_id(),
        title: req
            .title_hint
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| req.url.clone()),
        url: req.url.clone(),
        thumbnail: req.thumbnail_url.clone(),
        duration: None,
        uploader: None,
        status: TaskStatus::Pending,
        progress: 0.0,
        speed: None,
        eta: None,
        downloaded: None,
        total: None,
        file_path: None,
        error: None,
        format_note: None,
        created_time: created,
        file_exists: false,
    }
}


/// 引擎路由入口：直链/协议链接走 aria2 分段并行，站点链接走 yt-dlp
pub fn run_engine<F>(
    ctx: &Ctx,
    req: &DownloadRequest,
    out_dir: &Path,
    on_tick: F,
    register_pid: impl Fn(u32),
    is_canceled: impl Fn() -> bool,
) -> anyhow::Result<RunOutcome>
where
    F: FnMut(ProgressTick),
{
    if wants_aria2(req, &ctx.settings) {
        run_aria2(ctx, req, out_dir, on_tick, register_pid, is_canceled)
    } else {
        run_download(ctx, req, out_dir, on_tick, register_pid, is_canceled)
    }
}

/* ==================== aria2c 分段并行引擎（HTTP / FTP / BitTorrent / 磁力） ==================== */

/// 常见"文件直链"扩展名：命中就走分段并行引擎
const ARIA2_FILE_EXTS: &[&str] = &[
    "mp4", "mkv", "webm", "avi", "mov", "flv", "m4v", "ts", "mpg", "mpeg", "wmv", "3gp", "rmvb",
    "mp3", "m4a", "flac", "wav", "aac", "ogg", "opus", "wma", "ape",
    "zip", "rar", "7z", "tar", "gz", "xz", "bz2", "iso", "exe", "msi", "apk", "dmg", "pkg", "deb",
    "rpm", "torrent", "bin", "img", "vhd",
    "pdf", "epub", "mobi", "azw3", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "csv",
    "json", "xml", "srt", "ass", "vtt", "lrc",
    "jpg", "jpeg", "png", "webp", "gif", "bmp", "ico", "svg", "tif", "tiff", "psd", "heic",
];

/// 看起来像文件直链？（HLS/DASH 的 m3u8/mpd 不算——那些交给 yt-dlp 合并）
pub fn is_probably_file_url(url: &str) -> bool {
    match crate::filters::Filters::ext_of(url) {
        Some(ext) => ARIA2_FILE_EXTS.contains(&ext.as_str()),
        None => false,
    }
}

/// 直链或协议链接（HTTP/FTP/BT/磁力）——这两类由 aria2 负责
pub fn is_direct_or_protocol(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    if u.starts_with("magnet:") || u.starts_with("ed2k:") || u.ends_with(".torrent") {
        return true;
    }
    if u.starts_with("ftp://") || u.starts_with("ftps://") {
        return true;
    }
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return false;
    }
    is_probably_file_url(&u)
}

/// 引擎路由：true = aria2 分段并行；false = yt-dlp 站点解析
pub fn wants_aria2(req: &DownloadRequest, settings: &AppSettings) -> bool {
    match settings.engine.as_str() {
        "aria2" => true,
        "ytdlp" => false,
        _ => is_direct_or_protocol(&req.url),
    }
}

fn parse_size_token(s: &str) -> Option<i64> {
    let s = s.trim();
    let idx = s.find(|c: char| c.is_ascii_alphabetic())?;
    let (num, unit) = s.split_at(idx);
    let v: f64 = num.trim().parse().ok()?;
    Some(unit_to_bytes(v, unit.trim()))
}

/// 解析 aria2 的摘要行：[#7f6e 1.2MiB/10.0MiB(12%) CN:16 DL:1.1MiB ETA:8s]
pub fn parse_aria2_progress(line: &str) -> Option<ProgressTick> {
    let open = line.find('(')?;
    let close = line[open..].find(')')? + open;
    let percent: f64 = line[open + 1..close].trim().strip_suffix('%')?.trim().parse().ok()?;
    let mut tick = ProgressTick {
        percent: Some(percent),
        ..Default::default()
    };
    // 大小：位于 "(" 之前，形如 1.2MiB/10.0MiB
    let before = &line[..open];
    if let Some(tok) = before.split_whitespace().rev().find(|t| t.contains('/')) {
        let mut it = tok.split('/');
        if let (Some(a), Some(b)) = (it.next(), it.next()) {
            tick.downloaded = parse_size_token(a);
            tick.total = parse_size_token(b);
        }
    }
    for (key, slot) in [("DL:", 0usize), ("ETA:", 1usize)] {
        if let Some(i) = line.find(key) {
            let val: String = line[i + key.len()..]
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != ']')
                .collect();
            if !val.is_empty() {
                if slot == 0 {
                    tick.speed = Some(val);
                } else {
                    tick.eta = Some(val);
                }
            }
        }
    }
    Some(tick)
}

/// aria2 分段并行参数（纯函数，便于单测）
///
/// - `connections` → `--max-connection-per-server`（1..=16 限幅）
/// - `split` → `--split`（1..=16 限幅）
/// - `min_split_mb` → `--min-split-size`（1..=64 MiB 限幅后换算成字节）
///
/// 注意：aria2 的 `--min-split-size` 必须 ≥ 1M（1048576），传更小的值 aria2
/// 会直接拒绝启动（errorCode=28），所以这里绝不能把 MB 值直接当字节透传。
pub fn aria2_segment_args(connections: i64, split: i64, min_split_mb: i64) -> Vec<String> {
    let c = connections.clamp(
        crate::models::ARIA2_CONNECTIONS_RANGE.0,
        crate::models::ARIA2_CONNECTIONS_RANGE.1,
    );
    let s = split.clamp(crate::models::ARIA2_SPLIT_RANGE.0, crate::models::ARIA2_SPLIT_RANGE.1);
    let m = min_split_mb.clamp(
        crate::models::ARIA2_MIN_SPLIT_MB_RANGE.0,
        crate::models::ARIA2_MIN_SPLIT_MB_RANGE.1,
    );
    vec![
        format!("--max-connection-per-server={c}"),
        format!("--split={s}"),
        format!("--min-split-size={}", m * 1024 * 1024),
    ]
}

/// 构造 aria2c 参数（分段并行 + 断点续传 + 全局限速 + 代理）
pub fn build_aria2_args(ctx: &Ctx, req: &DownloadRequest, out_dir: &Path) -> Vec<String> {
    let lower = req.url.trim().to_ascii_lowercase();
    let is_bt = lower.starts_with("magnet:") || lower.ends_with(".torrent");
    let mut a: Vec<String> = Vec::new();
    a.push("--no-conf".into());
    a.push("--continue=true".into()); // 断点续传（含断电后重新拉起）
    a.push("--dir".into());
    a.push(out_dir.to_string_lossy().to_string());
    a.push("--summary-interval=1".into()); // 每秒一行进度摘要，便于解析
    a.push("--console-log-level=notice".into()); // 输出完成路径行
    a.push("--show-console-readout=true".into()); // 实时进度行（原地刷新，解析时按回车符拆）
    a.push("--download-result=default".into());
    a.push("--enable-color=false".into());
    a.push("--file-allocation=none".into()); // 秒开，不做预分配
    a.push("--auto-file-renaming=false".into());
    a.push("--max-concurrent-downloads=1".into());
    // IDM 式动态分段：连接数 / 分段数 / 最小分片全部来自设置（1..=16 / 1..=64MB 限幅）
    a.extend(aria2_segment_args(
        ctx.settings.aria2_connections,
        ctx.settings.aria2_split,
        ctx.settings.aria2_min_split_mb,
    ));
    if is_bt {
        a.push("--seed-time=0".into());
        a.push("--enable-dht=true".into());
        a.push("--bt-enable-lpd=true".into());
        a.push("--bt-max-peers=128".into());
        a.push("--follow-torrent=mem".into());
        a.push("--bt-save-metadata=false".into());
        a.push("--dht-listen-port=6881-6999".into());
        a.push("--listen-port=6881-6999".into());
        a.push("--bt-tracker-connect-timeout=10".into());
    } else {
        a.push("--remote-time=true".into());
        a.push("--content-disposition=true".into());
        a.push("--check-certificate=false".into());
        a.push("--timeout=30".into());
        a.push("--max-tries=5".into());
        a.push("--retry-wait=2".into());
    }
    // 全局限速（设置里的令牌桶参数 → 引擎原生限速）
    if ctx.settings.speed_limit_enabled && ctx.settings.speed_limit_kb > 0 {
        a.push(format!(
            "--max-overall-download-limit={}K",
            ctx.settings.speed_limit_kb
        ));
    } else if let Some(rl) = req.rate_limit.clone().filter(|s| !s.trim().is_empty()) {
        a.push(format!("--max-download-limit={}", rl.trim()));
    }
    if let Some(px) = effective_proxy(ctx, &req.url) {
        a.push(format!("--all-proxy={px}"));
    }
    a.push(req.url.clone());
    a
}

/// 从 aria2 输出行里捞本地文件路径。
/// 真机格式（两处都要认）：
///   1) `09/27 17:48:24 [NOTICE] Download complete: D:/x/f.zip`（带时间戳前缀，strip_prefix 匹配不到）
///   2) `0b5f8f|OK  |   3.0MiB/s|D:/x/f.zip`（结果表格行，路径在最后一个 | 之后）
fn aria2_path_from_line(line: &str) -> Option<String> {
    const MARK: &str = "Download complete:";
    if let Some(idx) = line.find(MARK) {
        let p = line[idx + MARK.len()..].trim();
        if !p.is_empty() && Path::new(p).is_file() {
            return Some(p.to_string());
        }
    }
    if line.contains("|OK") {
        if let Some(last) = line.rsplit('|').next() {
            let p = last.trim();
            if p.len() > 3 && Path::new(p).is_file() {
                return Some(p.to_string());
            }
        }
    }
    None
}

/// 运行 aria2c（与 run_download 同构：进度回调 / 取消 / 进程登记）
pub fn run_aria2<F>(
    ctx: &Ctx,
    req: &DownloadRequest,
    out_dir: &Path,
    mut on_tick: F,
    register_pid: impl Fn(u32),
    is_canceled: impl Fn() -> bool,
) -> anyhow::Result<RunOutcome>
where
    F: FnMut(ProgressTick),
{
    if req.url.trim().to_ascii_lowercase().starts_with("ed2k:") {
        anyhow::bail!("ED2K 电驴链接目前没有可用的开源引擎，暂不支持（HTTP/FTP/BT/磁力均可用）");
    }
    let exe = ctx.tools.aria2()?.to_path_buf();
    ensure_dir(out_dir)?;
    let args = build_aria2_args(ctx, req, out_dir);

    let mut cmd = command_for(&exe);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    let started_ms = crate::ctx::now_ms();
    let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("启动 aria2c 失败：{e}"))?;
    let pid = child.id();
    register_pid(pid);

    let (tx, rx) = mpsc::channel::<(bool, String)>();
    let mut readers = Vec::new();
    if let Some(so) = child.stdout.take() {
        let tx2 = tx.clone();
        readers.push(std::thread::spawn(move || read_lines(so, tx2, false)));
    }
    if let Some(se) = child.stderr.take() {
        let tx2 = tx.clone();
        readers.push(std::thread::spawn(move || read_lines(se, tx2, true)));
    }
    drop(tx);

    let mut final_path: Option<String> = None;
    let mut errs: Vec<String> = Vec::new();
    let mut killed = false;
    let started = Instant::now();
    let mut last_pct = -1.0f64;

    loop {
        match rx.recv_timeout(Duration::from_millis(150)) {
            Ok((is_err, line)) => {
                // aria2 的进度行用 \r 原地刷新，这里按 \r / \n 一起拆
                for part in line.split(['\r', '\n']) {
                    let trimmed = part.trim().to_string();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Some(t) = parse_aria2_progress(&trimmed) {
                        if let Some(p) = t.percent {
                            if (p - last_pct).abs() >= 0.05 {
                                last_pct = p;
                                on_tick(t);
                            }
                        }
                    } else if let Some(p) = aria2_path_from_line(&trimmed) {
                        final_path = Some(p);
                    } else if trimmed.contains("[ERROR]") {
                        push_tail(&mut errs, trimmed.clone());
                    } else if is_err && !trimmed.is_empty() {
                        push_tail(&mut errs, trimmed.clone());
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if is_canceled() && !killed {
            kill_tree(pid);
            killed = true;
        }
        if started.elapsed() > Duration::from_secs(12 * 3600) && !killed {
            kill_tree(pid);
            killed = true;
        }
        if let Some(status) = child.try_wait().ok().flatten() {
            if status.code().is_some() {
                // 进程已退出：把剩余输出读完再收尾
                std::thread::sleep(Duration::from_millis(120));
                while let Ok((is_err, line)) = rx.try_recv() {
                    let trimmed = line.trim().to_string();
                    if let Some(t) = parse_aria2_progress(&trimmed) {
                        on_tick(t);
                    } else if let Some(p) = aria2_path_from_line(&trimmed) {
                        final_path = Some(p);
                    } else if is_err || trimmed.contains("[ERROR]") {
                        push_tail(&mut errs, trimmed);
                    }
                }
                break;
            }
        }
    }

    for r in readers {
        let _ = r.join();
    }
    let status = child.wait()?;
    let code = status.code().unwrap_or(-1);
    if code == 0 {
        if let Some(p) = final_path
            .clone()
            .or_else(|| newest_media_file(out_dir, started_ms).map(|p| p.to_string_lossy().to_string()))
        {
            final_path = Some(p);
        }
    }
    Ok(RunOutcome {
        exit_code: code,
        final_path,
        stderr_tail: errs.join("\n"),
        killed,
    })
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    fn tick(p: Option<f64>) -> ProgressTick {
        ProgressTick {
            percent: p,
            ..Default::default()
        }
    }

    /// 逐行喂进 tracker，收集展示出来的百分比（模拟 run_download 的 handle 闭包）
    fn run(transcript: &[&str]) -> Vec<f64> {
        let mut tr = StreamTracker::new();
        let mut seen = Vec::new();
        for line in transcript {
            tr.observe(line);
            if let LineKind::Progress(mut t) = classify_line(line) {
                tr.absorb(&mut t);
                if let Some(p) = t.percent {
                    seen.push(p);
                }
            }
        }
        seen
    }

    /// 实测行：打印的是 1，冒号后面其实是两条流（HLS 双轨）
    #[test]
    fn formats_line_counts_real_streams_not_printed_number() {
        assert_eq!(
            formats_count("[info] master: Downloading 1 format(s): 133+72"),
            Some(2)
        );
        assert_eq!(
            formats_count("[info] abc: Downloading 1 format(s): hls-2176"),
            Some(1)
        );
        assert_eq!(
            formats_count("[info] abc: Downloading 2 format(s): 137, 140"),
            Some(2)
        );
        assert_eq!(formats_count("没有任何标记的一行"), None);
    }

    /// 用户实测反馈的主场景：视频流 + 音频流各跑一遍 0→100%
    #[test]
    fn two_streams_become_one_continuous_bar() {
        let mut tr = StreamTracker::new();
        assert!(tr.observe("[info] x: Downloading 2 format(s): 137, 140"));
        assert_eq!(tr.total_streams(), 2);

        let mut t = tick(Some(25.0));
        tr.absorb(&mut t);
        assert_eq!(t.percent, Some(12.5));

        let mut t = tick(Some(100.0)); // 第 1 条流结束 → 总进度一半
        tr.absorb(&mut t);
        assert_eq!(t.percent, Some(50.0));

        let mut t = tick(Some(40.0)); // 第 2 条流 40% → 总 70%
        tr.absorb(&mut t);
        assert_eq!(t.percent, Some(70.0));

        let mut t = tick(Some(100.0));
        tr.absorb(&mut t);
        assert_eq!(t.percent, Some(100.0));
    }

    /// 新的 Destination 行 = 前一条流结束（即使 formats 行只说 1 条）
    #[test]
    fn second_destination_marks_first_stream_done() {
        let mut tr = StreamTracker::new();
        tr.observe("[info] m: Downloading 1 format(s): 133+72");
        assert_eq!(tr.total_streams(), 2);
        tr.observe("[download] Destination: m.f133.mp4");
        let mut t = tick(Some(80.0));
        tr.absorb(&mut t);
        assert_eq!(t.percent, Some(40.0));
        // 第二条流开始
        assert!(tr.observe("[download] Destination: m.f72.mp4"));
        let mut t = tick(Some(20.0));
        tr.absorb(&mut t);
        assert_eq!(t.percent, Some(60.0));
    }

    #[test]
    fn single_stream_keeps_original_percent() {
        let mut tr = StreamTracker::new();
        assert!(tr.observe("[info] x: Downloading 1 format(s): 22"));
        let mut t = tick(Some(37.5));
        tr.absorb(&mut t);
        assert_eq!(t.percent, Some(37.5));
    }

    /// HLS 分片估算会让同一条流的百分比来回摆：展示值必须只增不减
    #[test]
    fn fragment_jitter_never_moves_the_bar_backwards() {
        let mut tr = StreamTracker::new();
        tr.observe("[info] m: Downloading 1 format(s): 133");
        let seen: Vec<f64> = [10.0, 39.1, 65.8, 50.6, 50.0, 100.0, 98.7, 100.0]
            .into_iter()
            .map(|p| {
                let mut t = tick(Some(p));
                tr.absorb(&mut t);
                t.percent.unwrap()
            })
            .collect();
        assert!(
            seen.windows(2).all(|w| w[1] >= w[0]),
            "不能回退：{seen:?}"
        );
        assert_eq!(seen.last().copied(), Some(100.0));
    }

    /// 只有速度 / ETA 的行不能改进度（否则会把总进度冲掉）
    #[test]
    fn speed_only_tick_does_not_touch_percent() {
        let mut tr = StreamTracker::new();
        tr.observe("[info] x: Downloading 2 format(s): 137, 140");
        let mut t = ProgressTick {
            percent: None,
            speed: Some("1.20 MiB/s".into()),
            eta: Some("00:12".into()),
            ..Default::default()
        };
        tr.absorb(&mut t);
        assert_eq!(t.percent, None);
    }

    /// 播放列表 / 分 P：下一个条目重新计数
    #[test]
    fn next_item_resets_the_stream_counter() {
        let seen = run(&[
            "[info] x: Downloading 2 format(s): 137, 140",
            "[download] 100% of 10.00MiB in 00:10",
            "[download] 100% of 2.00MiB in 00:02",
            "[info] y: Downloading 2 format(s): 137, 140",
            "[download]  10.0% of 10.00MiB at 1.00MiB/s ETA 00:09",
        ]);
        assert_eq!(seen.last().copied(), Some(5.0));
    }

    /// 端到端：把本地 HLS 双轨实测（yt-dlp 2026.08.19）的真实输出整段喂进去，
    /// 断言 —— 只增不减、不超过 100、经历「第一条流完成 = 50%」、最后到 100%。
    #[test]
    fn real_hls_transcript_is_one_monotonic_pass() {
        let lines = [
            "[generic] Extracting URL: http://127.0.0.1:8931/master.m3u8",
            "[generic] master: Downloading webpage",
            "[generic] master: Downloading m3u8 information",
            "[generic] master: Checking m3u8 live status",
            "[info] master: Downloading 1 format(s): 133+72",
            "[hlsnative] Downloading m3u8 manifest",
            "[hlsnative] Total fragments: 1",
            "[download] Destination: dl1\\master [master].f133.mp4",
            "[download]   0.8% of ~ 129.98KiB at      0.00B/s ETA Unknown (frag 0/1)",
            "[download]   2.3% of ~ 130.98KiB at      0.00B/s ETA Unknown (frag 0/1)",
            "[download]   5.3% of ~ 132.98KiB at      0.00B/s ETA Unknown (frag 0/1)",
            "[download]  11.0% of ~ 136.98KiB at      0.00B/s ETA Unknown (frag 0/1)",
            "[download]  21.4% of ~ 144.98KiB at      0.00B/s ETA Unknown (frag 0/1)",
            "[download]  39.1% of ~ 160.98KiB at      0.00B/s ETA Unknown (frag 0/1)",
            "[download]  65.8% of ~ 192.98KiB at      0.00B/s ETA Unknown (frag 0/1)",
            "[download]  50.6% of ~ 256.98KiB at      0.00B/s ETA Unknown (frag 0/1)",
            "[download]  50.0% of ~ 259.97KiB at      0.00B/s ETA Unknown (frag 1/1)",
            "[download] 100% of  129.98KiB in 00:00:00 at 9.55MiB/s",
            "[hlsnative] Downloading m3u8 manifest",
            "[hlsnative] Total fragments: 5",
            "[download] Destination: dl1\\master [master].f72.mp4",
            "[download]   1.5% of ~  67.01KiB at      0.00B/s ETA Unknown (frag 0/5)",
            "[download]   4.2% of ~  72.01KiB at      0.00B/s ETA Unknown (frag 0/5)",
            "[download]   8.5% of ~  82.01KiB at      0.00B/s ETA Unknown (frag 0/5)",
            "[download]  13.1% of ~ 102.01KiB at      0.00B/s ETA Unknown (frag 0/5)",
            "[download]  10.0% of ~ 134.02KiB at      0.00B/s ETA Unknown (frag 1/5)",
            "[download]  28.9% of ~  98.28KiB at      0.00B/s ETA Unknown (frag 1/5)",
            "[download]  27.3% of ~ 118.28KiB at      0.00B/s ETA Unknown (frag 1/5)",
            "[download]  25.2% of ~ 128.06KiB at      0.00B/s ETA Unknown (frag 2/5)",
            "[download]  39.0% of ~  85.37KiB at      0.00B/s ETA Unknown (frag 2/5)",
            "[download]  48.8% of ~  97.04KiB at      0.00B/s ETA Unknown (frag 2/5)",
            "[download]  46.4% of ~ 110.37KiB at      0.00B/s ETA Unknown (frag 2/5)",
            "[download]  59.4% of ~  87.90KiB at  243.92KiB/s ETA Unknown (frag 3/5)",
            "[download]  68.5% of ~  96.65KiB at  243.92KiB/s ETA Unknown (frag 3/5)",
            "[download]  65.9% of ~ 106.65KiB at  243.92KiB/s ETA Unknown (frag 3/5)",
            "[download]  62.9% of ~ 111.76KiB at  243.92KiB/s ETA Unknown (frag 4/5)",
            "[download] 100.0% of ~  71.23KiB at  243.92KiB/s ETA Unknown (frag 4/5)",
            "[download]  98.7% of ~  72.15KiB at  243.92KiB/s ETA Unknown (frag 5/5)",
            "[download] 100% of   71.23KiB in 00:00:00 at 845.01KiB/s",
            "[Merger] Merging formats into \"dl1\\master [master].mp4\"",
            "Deleting original file dl1\\master [master].f133.mp4 (pass -k to keep)",
            "Deleting original file dl1\\master [master].f72.mp4 (pass -k to keep)",
        ];
        let seen = run(&lines);
        assert!(!seen.is_empty());
        assert!(
            seen.windows(2).all(|w| w[1] >= w[0]),
            "总进度必须只增不减（修复前这里是 100 → 0）：{seen:?}"
        );
        assert!(seen.iter().all(|p| *p >= 0.0 && *p <= 100.0), "{seen:?}");
        assert_eq!(seen.last().copied(), Some(100.0));
        assert!(
            seen.contains(&50.0),
            "第一条流下完时总进度应恰好是一半：{seen:?}"
        );
        // 修复前的问题：中途出现「跌回低位再重新爬」
        assert!(
            !seen.windows(2).any(|w| w[1] + 1.0 < w[0]),
            "不能再出现进度倒退：{seen:?}"
        );
    }
}

#[cfg(test)]
mod fix18_tests {
    use super::*;

    /// BUG-08：真正的原因行（errorCode / Exception）不能被「最后一行」丢掉
    #[test]
    fn keeps_the_real_cause_line() {
        let tail = "09/27 20:27:09 [ERROR] CUID#7 - Download aborted. URI=http://127.0.0.1:18330/longpath.bin\n\
                    Exception: [AbstractCommand.cc:403] errorCode=18 URI=http://127.0.0.1:18330/longpath.bin\n\
                    [util.cc:1948] errNum=2 errorCode=18 Failed to make the directory C:/x, cause: No such file or directory";
        let line = best_error_line(tail);
        assert!(line.contains("errorCode=18"), "应挑出 errorCode 行：{line}");
        assert!(line.contains("Failed to make the directory"), "{line}");
        assert_ne!(line, "09/27 20:27:09 [ERROR] CUID#7 - Download aborted. URI=http://127.0.0.1:18330/longpath.bin");
        // 只有一行 ERROR 时退回最后一行
        assert_eq!(best_error_line("plain failure line"), "plain failure line");
        assert_eq!(best_error_line("   "), "");
    }

    /// BUG-08：>259 字符的目标路径在入队时就要报清楚
    #[test]
    fn long_target_path_is_rejected_before_enqueue() {
        let long_dir = format!("C:/{}", "a".repeat(300));
        let err = check_target_path_len(Path::new(&long_dir)).unwrap_err();
        assert!(err.contains("目标路径过长"), "{err}");
        assert!(err.contains("259"), "{err}");
        // 正常长度放行
        assert!(check_target_path_len(Path::new("C:/videos/clip.mp4")).is_ok());
        // 259 是上限（>259 才拒绝）
        let edge = format!("C:/{}", "b".repeat(256));
        assert_eq!(path_unit_len(Path::new(&edge)), 259);
        assert!(check_target_path_len(Path::new(&edge)).is_ok());
        let over = format!("C:/{}", "c".repeat(257));
        assert!(check_target_path_len(Path::new(&over)).is_err());
    }

    /// aria2 的文件名 = URL 最后一段（含百分号解码 / 去 query）
    #[test]
    fn url_file_name_matches_engine_behaviour() {
        assert_eq!(url_file_name("https://a.com/x").as_deref(), Some("x"));
        assert_eq!(url_file_name("https://a.com/a/b/c.part1.rar").as_deref(), Some("c.part1.rar"));
        assert_eq!(url_file_name("http://127.0.0.1:18330/longpath.bin").as_deref(), Some("longpath.bin"));
        assert_eq!(url_file_name("https://a.com/dir/f.zip?token=1#x").as_deref(), Some("f.zip"));
        assert_eq!(url_file_name("https://a.com/%E6%B5%8B%E8%AF%95.mp4").as_deref(), Some("测试.mp4"));
        assert_eq!(url_file_name("https://a.com/a%2Fb.bin").as_deref(), Some("b.bin"), "解码后的分隔符只取最后一段");
        assert_eq!(url_file_name("https://a.com/").as_deref(), None);
        assert_eq!(url_file_name("magnet:?xt=urn:btih:abc").as_deref(), None);
    }

    /// 预检用「目录 + URL 文件名」拼出来的完整路径判断长度
    #[test]
    fn output_path_precheck_uses_url_name() {
        let dir = format!("C:/{}", "d".repeat(120));
        let req = DownloadRequest {
            url: format!("http://127.0.0.1:18330/{}.bin", "e".repeat(200)),
            ..Default::default()
        };
        let err = check_output_path(Path::new(&dir), &req).unwrap_err();
        assert!(err.contains("目标路径过长"), "{err}");
        let ok = DownloadRequest { url: "http://127.0.0.1:18330/small.bin".into(), ..Default::default() };
        assert!(check_output_path(Path::new(&dir), &ok).is_ok());
        // 目录本身超长也要拦
        let deep = format!("C:/{}", "f".repeat(300));
        assert!(check_output_path(Path::new(&deep), &ok).unwrap_err().contains("下载目录"));
    }
}

#[cfg(test)]
mod aria2_tests {
    use super::*;
    use crate::ctx::{AppDirs, Ctx, ToolPaths};
    use crate::models::AppSettings;

    fn tctx(settings: AppSettings) -> Ctx {
        Ctx::new(
            AppDirs::new(),
            ToolPaths { ytdlp: None, ffmpeg: None, ffprobe: None, whisper: None, aria2: None, pandoc: None, poppler: None, emule: None },
            settings,
        )
    }

    #[test]
    fn detects_direct_links() {
        assert!(is_probably_file_url("https://a.com/b/video.mp4"));
        assert!(is_probably_file_url("https://a.com/f.zip?token=1"));
        assert!(!is_probably_file_url("https://www.youtube.com/watch?v=abc"));
        assert!(!is_probably_file_url("https://a.com/stream.m3u8"), "HLS 交给 yt-dlp");
        assert!(is_direct_or_protocol("magnet:?xt=urn:btih:xyz"));
        assert!(is_direct_or_protocol("ftp://ftp.a.com/pub/x.iso"));
        assert!(is_direct_or_protocol("https://a.com/x.torrent"));
        assert!(!is_direct_or_protocol("https://www.bilibili.com/video/BV1xx"));
    }

    #[test]
    fn parses_aria2_summary_line() {
        let t = parse_aria2_progress("[#7f6e5d 1.2MiB/10.0MiB(12%) CN:16 DL:1.1MiB ETA:8s]").unwrap();
        assert_eq!(t.percent, Some(12.0));
        assert_eq!(t.total, Some((10.0 * 1024.0 * 1024.0) as i64));
        assert!(t.downloaded.unwrap() > 1_200_000);
        assert_eq!(t.speed.as_deref(), Some("1.1MiB"));
        assert_eq!(t.eta.as_deref(), Some("8s"));
        assert!(parse_aria2_progress("nothing here").is_none());
        // 真机 aria2 1.37.0 实测输出（无 ETA 段）
        let real = parse_aria2_progress("[#ca2d7d 1.2MiB/2.3MiB(50%) CN:3 DL:1.8MiB]").unwrap();
        assert_eq!(real.percent, Some(50.0));
        assert_eq!(real.speed.as_deref(), Some("1.8MiB"));
        assert_eq!(real.total, Some((2.3 * 1024.0 * 1024.0) as i64));
        assert!(real.eta.is_none());
    }

    #[test]
    fn args_include_segmentation_and_resume() {
        let req = DownloadRequest { url: "https://a.com/big.iso".into(), ..Default::default() };
        let ctx = tctx(AppSettings { speed_limit_enabled: true, speed_limit_kb: 512, ..Default::default() });
        let args = build_aria2_args(&ctx, &req, Path::new("D:/x"));
        let joined = args.join(" ");
        assert!(joined.contains("--continue=true"), "断点续传");
        assert!(joined.contains("--split=16"), "16 分段并行");
        assert!(joined.contains("--max-overall-download-limit=512K"), "全局限速");
        assert!(joined.contains("--max-connection-per-server=16"));
        assert_eq!(args.last().unwrap(), "https://a.com/big.iso");
    }

    #[test]
    fn bt_args_branch() {
        let req = DownloadRequest { url: "magnet:?xt=urn:btih:abc".into(), ..Default::default() };
        let ctx = tctx(AppSettings::default());
        let joined = build_aria2_args(&ctx, &req, Path::new("D:/x")).join(" ");
        assert!(joined.contains("--seed-time=0"));
        assert!(joined.contains("--enable-dht=true"));
        assert!(joined.contains("--dht-listen-port=6881-6999"));
        assert!(!joined.contains("--max-tries"), "BT 分支不带 HTTP 重试参数");
    }

    #[test]
    fn extracts_local_path_from_real_aria2_lines() {
        // 真机 1.37.0 输出：完成行带时间戳前缀，旧写法 strip_prefix 永远匹配不到
        let tmp = std::env::temp_dir().join("umidl_aria2_path_test.bin");
        std::fs::write(&tmp, b"x").unwrap();
        let p = tmp.to_string_lossy().to_string();
        let line1 = format!("09/27 17:48:24 [NOTICE] Download complete: {p}");
        assert_eq!(aria2_path_from_line(&line1).as_deref(), Some(p.as_str()));
        let line2 = format!("0b5f8f|OK  |   3.0MiB/s|{p}");
        assert_eq!(aria2_path_from_line(&line2).as_deref(), Some(p.as_str()));
        // 不存在的路径不该被当成结果
        assert!(aria2_path_from_line("09/27 17:48:24 [NOTICE] Download complete: D:/nope/x.zip").is_none());
        assert!(aria2_path_from_line("[#ca2d7d 1.2MiB/2.3MiB(50%) CN:3 DL:1.8MiB]").is_none());
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn engine_routing_respects_setting() {
        let req = DownloadRequest { url: "https://a.com/x.mp4".into(), ..Default::default() };
        assert!(wants_aria2(&req, &AppSettings { engine: "auto".into(), ..Default::default() }));
        assert!(!wants_aria2(&req, &AppSettings { engine: "ytdlp".into(), ..Default::default() }));
        let site = DownloadRequest { url: "https://www.youtube.com/watch?v=1".into(), ..Default::default() };
        assert!(!wants_aria2(&site, &AppSettings { engine: "auto".into(), ..Default::default() }));
        assert!(wants_aria2(&site, &AppSettings { engine: "aria2".into(), ..Default::default() }));
    }

    /* ---------- 多线程 / 连接数可配置 ---------- */

    #[test]
    fn aria2_segment_args_clamp_and_never_go_below_one_mib() {
        // 0 / 负数 → 下限 1（aria2 对 0 会直接报错）
        let lo = aria2_segment_args(0, -4, 0);
        assert!(lo.contains(&"--max-connection-per-server=1".to_string()), "{lo:?}");
        assert!(lo.contains(&"--split=1".to_string()), "{lo:?}");
        // 999 → 上限 16
        let hi = aria2_segment_args(999, 999, 999);
        assert!(hi.contains(&"--max-connection-per-server=16".to_string()), "{hi:?}");
        assert!(hi.contains(&"--split=16".to_string()), "{hi:?}");
        // --min-split-size 必须 ≥ 1048576（1M），否则 aria2 直接拒启（errorCode=28）
        let parse_min = |args: &[String]| -> i64 {
            args.iter()
                .find_map(|s| s.strip_prefix("--min-split-size="))
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(-1)
        };
        assert_eq!(parse_min(&aria2_segment_args(16, 16, 1)), 1048576, "1MB → 1048576 字节");
        assert_eq!(parse_min(&hi), 64 * 1024 * 1024, "上限 64MB");
        assert!(parse_min(&lo) >= 1024 * 1024, "任何输入下都不得小于 1M：{lo:?}");
    }

    #[test]
    fn aria2_args_use_configured_threads() {
        let req = DownloadRequest { url: "https://a.com/big.iso".into(), ..Default::default() };
        let ctx = tctx(AppSettings {
            aria2_connections: 8,
            aria2_split: 4,
            aria2_min_split_mb: 4,
            ..Default::default()
        });
        let args = build_aria2_args(&ctx, &req, Path::new("D:/x"));
        let joined = args.join(" ");
        assert!(joined.contains("--max-connection-per-server=8"), "{joined}");
        assert!(joined.contains("--split=4"), "{joined}");
        assert!(joined.contains("--min-split-size=4194304"), "4MB → 4194304 字节：{joined}");
        assert!(!joined.contains("--max-connection-per-server=16"), "不应再写死 16：{joined}");

        // 越界设置也必须落在 aria2 能接受的范围内
        let ctx = tctx(AppSettings {
            aria2_connections: 999,
            aria2_split: 0,
            aria2_min_split_mb: 0,
            ..Default::default()
        });
        let joined = build_aria2_args(&ctx, &req, Path::new("D:/x")).join(" ");
        assert!(joined.contains("--max-connection-per-server=16"), "{joined}");
        assert!(joined.contains("--split=1"), "{joined}");
        assert!(joined.contains("--min-split-size=1048576"), "{joined}");
    }

    #[test]
    fn ytdlp_concurrent_fragments_follow_settings() {
        assert_eq!(ytdlp_fragment_args(0), vec!["--concurrent-fragments".to_string(), "1".to_string()]);
        assert_eq!(ytdlp_fragment_args(-5), vec!["--concurrent-fragments".to_string(), "1".to_string()]);
        assert_eq!(ytdlp_fragment_args(999), vec!["--concurrent-fragments".to_string(), "16".to_string()]);
        assert_eq!(ytdlp_fragment_args(7), vec!["--concurrent-fragments".to_string(), "7".to_string()]);

        let req = DownloadRequest { url: "https://youtu.be/xyz".into(), ..Default::default() };
        let ctx = tctx(AppSettings { ytdlp_concurrency: 6, ..Default::default() });
        let joined = build_args(&req, &ctx, Path::new("D:/x")).join(" ");
        assert!(joined.contains("--concurrent-fragments 6"), "{joined}");
        assert!(!joined.contains("--concurrent-fragments 4"), "不应再写死 4：{joined}");
    }
}

#[cfg(test)]
mod dlproxy_tests {
    use super::*;
    use crate::ctx::{AppDirs, ToolPaths};
    use std::sync::Mutex;

    /// 与 fix18_tests 同款的最小 Ctx（只带设置，工具路径全空）
    fn pctx(settings: AppSettings) -> Ctx {
        Ctx::new(
            AppDirs::new(),
            ToolPaths { ytdlp: None, ffmpeg: None, ffprobe: None, whisper: None, aria2: None, pandoc: None, poppler: None, emule: None },
            settings,
        )
    }

    /// 环境变量是进程级全局状态，而 `cargo test` 默认多线程并行：本模块里凡是要改这 8 个
    /// 代理变量的用例，都必须先拿这把锁串起来，否则用例之间会互相覆盖读到的值
    /// （那是测试写法问题，不是产品逻辑问题）。锁中毒（某个用例 panic）时取回内层数据继续用，
    /// 一个用例失败不该把同模块其它用例连锁拖死。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// 与 `env_proxy` 的 KEYS 完全对齐的变量名清单：快照 / 清空 / 恢复共用一份，避免漏清某个变量
    const ENV_PROXY_KEYS: [&str; 8] = [
        "UMIDL_PROXY",
        "umidl_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ];

    /// 持锁 + 快照 + 清空全部代理环境变量；Drop 时先清空再原样恢复。
    /// 即应用例中途失败（断言 panic）也不会把这套污染过的环境留给后面的用例。
    struct EnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        /// 返回值必须绑定到局部变量（如 `let (_lock, _env) = EnvGuard::new();`）：
        /// 变量按声明逆序析构，所以 `_env` 先恢复环境、`_lock` 后释放锁。
        #[must_use]
        fn new() -> (std::sync::MutexGuard<'static, ()>, Self) {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let saved = ENV_PROXY_KEYS.iter().map(|k| (*k, std::env::var(k).ok())).collect();
            Self::clear();
            (lock, Self { saved })
        }

        fn clear() {
            for k in ENV_PROXY_KEYS {
                std::env::remove_var(k);
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            Self::clear();
            for (k, v) in &self.saved {
                if let Some(v) = v {
                    std::env::set_var(k, v);
                }
            }
        }
    }

    /// 代理地址规范化：缺协议（最常见的写法）补 http://，已知协议原样保留，空值 → None
    #[test]
    fn normalize_proxy_fills_missing_scheme() {
        assert_eq!(normalize_proxy("127.0.0.1:7892").as_deref(), Some("http://127.0.0.1:7892"));
        assert_eq!(
            normalize_proxy("  127.0.0.1:7892  ").as_deref(),
            Some("http://127.0.0.1:7892"),
            "首尾空白要忽略"
        );
        assert_eq!(
            normalize_proxy("http://127.0.0.1:7892").as_deref(),
            Some("http://127.0.0.1:7892"),
            "已带协议不能重复拼"
        );
        assert_eq!(
            normalize_proxy("HTTPS://Proxy.Local:8080").as_deref(),
            Some("HTTPS://Proxy.Local:8080"),
            "大小写不敏感"
        );
        assert_eq!(normalize_proxy("socks5://127.0.0.1:1080").as_deref(), Some("socks5://127.0.0.1:1080"));
        assert_eq!(normalize_proxy("socks5h://127.0.0.1:1080").as_deref(), Some("socks5h://127.0.0.1:1080"));
        assert_eq!(normalize_proxy("").as_deref(), None);
        assert_eq!(normalize_proxy("   ").as_deref(), None);
    }

    /// 环境变量代理：UMIDL_PROXY 优先，其次 HTTPS_PROXY / ALL_PROXY；空白视为未设置
    #[test]
    fn env_proxy_prefers_app_specific_var() {
        let (_lock, _env) = EnvGuard::new();

        assert_eq!(env_proxy(), None, "一个都没设置时不该凭空造代理");

        std::env::set_var("ALL_PROXY", "127.0.0.1:7899");
        assert_eq!(env_proxy().as_deref(), Some("http://127.0.0.1:7899"));
        std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:7898");
        assert_eq!(env_proxy().as_deref(), Some("http://127.0.0.1:7898"), "HTTPS_PROXY 优先于 ALL_PROXY");
        std::env::set_var("UMIDL_PROXY", "127.0.0.1:7890");
        assert_eq!(env_proxy().as_deref(), Some("http://127.0.0.1:7890"), "UMIDL_PROXY 最优先");

        std::env::set_var("UMIDL_PROXY", "   ");
        assert_eq!(env_proxy().as_deref(), Some("http://127.0.0.1:7898"), "本应用变量为空白时退回通用变量");
    }

    /// 空白值要逐级回退（回归用例）：取值必须停在「第一个有值的变量」，而不是「第一个设过的变量」——
    /// 否则一个空白变量会把后面所有变量一起挡掉（曾于 env_proxy 中真实发生过）
    #[test]
    fn env_proxy_blank_values_fall_through_to_next_var() {
        let (_lock, _env) = EnvGuard::new();

        std::env::set_var("ALL_PROXY", "socks5://127.0.0.1:1080");
        std::env::set_var("HTTP_PROXY", "127.0.0.1:7898");
        std::env::set_var("UMIDL_PROXY", "   ");
        assert_eq!(env_proxy().as_deref(), Some("http://127.0.0.1:7898"), "空白的 UMIDL_PROXY 不该挡住 HTTP_PROXY");

        std::env::set_var("HTTPS_PROXY", "\t ");
        assert_eq!(env_proxy().as_deref(), Some("http://127.0.0.1:7898"), "空白的 HTTPS_PROXY 不该挡住 HTTP_PROXY");

        std::env::set_var("HTTP_PROXY", "");
        assert_eq!(
            env_proxy().as_deref(),
            Some("socks5://127.0.0.1:1080"),
            "空串 HTTP_PROXY 应继续回退到 ALL_PROXY，且 socks5 协议原样保留"
        );

        for k in ENV_PROXY_KEYS {
            std::env::set_var(k, "  ");
        }
        assert_eq!(env_proxy(), None, "八个变量全是空白时等同于都没设置");
    }

    /// 死代理不参与：显式设置/环境变量指到拒连端口时，一律回退直连（不把用户卡死在坏代理上）
    #[test]
    fn dead_proxy_falls_back_to_direct() {
        let mut s = AppSettings::default();
        s.proxy = Some("http://127.0.0.1:59117".into());
        let ctx = pctx(s);
        assert!(!proxy_alive("http://127.0.0.1:59117"));
        assert_eq!(effective_proxy(&ctx, "https://huggingface.co/x"), None);
        // 本机地址永远直连（系统代理会把 127.0.0.1 也代理走，导致 502）
        let mut s2 = AppSettings::default();
        s2.proxy = Some("http://127.0.0.1:59116".into());
        let ctx2 = pctx(s2);
        assert_eq!(effective_proxy(&ctx2, "http://127.0.0.1:6970/api"), None);
    }

    /* ==================== A3：直连决策必须真的直连 ==================== */

    /// 客户端代理方案：决策为直连 → 必须 no_proxy；决策为用代理 → 显式带地址且不禁用环境变量
    #[test]
    fn client_proxy_plan_follows_decision() {
        let direct = client_proxy_plan(None);
        assert_eq!(direct.proxy, None);
        assert!(direct.no_proxy, "决策为直连时必须显式禁用环境变量代理，否则 reqwest 自己会读 HTTP_PROXY");

        for blank in ["", "   ", "\t"] {
            let p = client_proxy_plan(Some(blank));
            assert!(p.proxy.is_none() && p.no_proxy, "空白代理等同于直连：{blank:?}");
        }

        let via = client_proxy_plan(Some("  http://127.0.0.1:7892  "));
        assert_eq!(via.proxy.as_deref(), Some("http://127.0.0.1:7892"), "地址要 trim 后原样使用");
        assert!(!via.no_proxy, "用代理时不禁用环境变量读取（显式设置优先级更高）");
    }

    /// 临时摘掉 NO_PROXY（reqwest 会读它）：本组用例要验证「设了代理就必须走代理」
    struct NoProxyCleared(Vec<(&'static str, Option<String>)>);

    impl NoProxyCleared {
        fn new() -> Self {
            let keys = ["NO_PROXY", "no_proxy"];
            let saved: Vec<(&'static str, Option<String>)> =
                keys.iter().map(|k| (*k, std::env::var(k).ok())).collect();
            for k in keys {
                std::env::remove_var(k);
            }
            Self(saved)
        }
    }

    impl Drop for NoProxyCleared {
        fn drop(&mut self) {
            for (k, v) in &self.0 {
                std::env::remove_var(k);
                if let Some(v) = v {
                    std::env::set_var(k, v);
                }
            }
        }
    }

    /// 极简一次性 HTTP 服务（不依赖 selftest feature 的 http_testserver）：
    /// 监听 127.0.0.1 随机端口，回一份固定内容的 200；10 秒后自行退出，不留线程
    fn one_shot_http_server(body: &'static str) -> String {
        fn serve(s: &mut std::net::TcpStream, body: &str) {
            use std::io::{Read as _, Write as _};
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = s.write_all(resp.as_bytes());
            let _ = s.flush();
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut s, _)) => serve(&mut s, body),
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    Err(_) => break,
                }
            }
        });
        format!("http://{addr}/probe.bin")
    }

    /// A3 回归（真机复现的那个）：环境变量指着死代理时，决策为直连就必须真的直连 ——
    /// 先证明「不 no_proxy 时环境变量代理确实生效」（对照组，否则本用例没有证明力），
    /// 再证明 dl_client_with 的直连决策能正常拿到内容，最后证明显式代理确实被使用。
    #[test]
    fn direct_decision_ignores_env_proxy() {
        let (_lock, _env) = EnvGuard::new();
        let _np = NoProxyCleared::new();
        for k in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
            std::env::set_var(k, "http://127.0.0.1:9");
        }

        let url = one_shot_http_server("umi-a3-direct-ok");

        // 对照组：旧写法（不 no_proxy）→ reqwest 读环境变量，请求被塞进死代理，必然失败
        let naive = reqwest::blocking::Client::builder().build().unwrap();
        assert!(
            naive.get(&url).send().is_err(),
            "对照组必须失败：不 no_proxy 时环境变量代理会生效（否则本用例证明不了任何事）"
        );

        // 直连决策：请求真的直连
        let client = crate::tools::dl_client_with(&client_proxy_plan(None)).unwrap();
        let body = client.get(&url).send().unwrap().text().unwrap();
        assert_eq!(body, "umi-a3-direct-ok", "决策为直连时必须真的绕开环境变量代理");

        // 用代理决策：显式 Proxy::all 真的被使用（这里是死端口 → 必失败）
        let plan = client_proxy_plan(Some("http://127.0.0.1:9"));
        let via = crate::tools::dl_client_with(&plan).unwrap();
        assert!(
            via.get(&url).send().is_err(),
            "显式指定的代理必须真的被使用（否则「用代理」这条分支是假的）"
        );
    }

    /* ==================== A4：系统代理注册表解析 ==================== */

    /// 真机 `reg query` 原文样本（ProxyEnable 是 REG_DWORD，旧实现只认 REG_SZ → 永远解析不出来）
    const REG_ENABLE_ON: &str = "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\r\n    ProxyEnable    REG_DWORD    0x1\r\n\r\n";
    const REG_ENABLE_OFF: &str = "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\r\n    ProxyEnable    REG_DWORD    0x0\r\n\r\n";
    const REG_SERVER_PLAIN: &str = "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\r\n    ProxyServer    REG_SZ    127.0.0.1:7892\r\n\r\n";
    const REG_NOT_FOUND: &str = "\r\n错误: 系统找不到指定的注册表项或值。\r\n\r\n";

    /// REG_DWORD：十六进制 / 十进制 / 大小写都认；不是数字 / 缺失 → None
    #[test]
    fn parse_reg_dword_reads_hex_and_decimal() {
        assert_eq!(parse_reg_dword(REG_ENABLE_ON), Some(1));
        assert_eq!(parse_reg_dword(REG_ENABLE_OFF), Some(0));
        assert_eq!(parse_reg_dword("    ProxyEnable    REG_DWORD    0x00000001\r\n"), Some(1));
        assert_eq!(parse_reg_dword("    ProxyEnable    REG_DWORD    1\r\n"), Some(1), "十进制写法也要认");
        assert_eq!(parse_reg_dword("    ProxyEnable    REG_DWORD    0X0\r\n"), Some(0), "0X 大写前缀");
        assert_eq!(parse_reg_dword(REG_NOT_FOUND), None, "查不到 → 当没启用");
        assert_eq!(parse_reg_dword(""), None);
        assert_eq!(parse_reg_dword("    ProxyEnable    REG_DWORD    \r\n"), None, "空值不能当中了奖");
    }

    /// 系统代理决策：ProxyEnable=0 → None；=0x1 且有 ProxyServer → http://<ProxyServer>
    #[test]
    fn system_proxy_from_reg_decides_from_both_values() {
        assert_eq!(
            system_proxy_from_reg(REG_ENABLE_ON, REG_SERVER_PLAIN).as_deref(),
            Some("http://127.0.0.1:7892"),
            "ProxyEnable=0x1 + ProxyServer 有值 → 直接用（旧实现这里恒为 None）"
        );
        assert_eq!(
            system_proxy_from_reg(REG_ENABLE_OFF, REG_SERVER_PLAIN),
            None,
            "ProxyEnable=0x0 → 未启用，即使 ProxyServer 还留着值也不能用"
        );
        assert_eq!(system_proxy_from_reg(REG_ENABLE_ON, REG_NOT_FOUND), None, "ProxyServer 缺失 → None");
        assert_eq!(
            system_proxy_from_reg(REG_ENABLE_ON, "    ProxyServer    REG_SZ    \r\n"),
            None,
            "ProxyServer 空值 → None"
        );
        assert_eq!(system_proxy_from_reg(REG_NOT_FOUND, REG_SERVER_PLAIN), None, "读不到 ProxyEnable → None");
    }

    /// ProxyServer 复合值：http= 优先；只有 https= 时退回「没带键名的那一段」；
    /// 全是键名没有值 → None（行为必须确定，不能靠猜）
    #[test]
    fn proxy_addr_from_reg_value_handles_composite() {
        assert_eq!(
            proxy_addr_from_reg_value("http=127.0.0.1:7892;https=127.0.0.1:7892").as_deref(),
            Some("http://127.0.0.1:7892"),
            "http= 优先"
        );
        assert_eq!(
            proxy_addr_from_reg_value("127.0.0.1:7892;https=127.0.0.1:7892").as_deref(),
            Some("http://127.0.0.1:7892"),
            "第一段没键名 → 当作默认代理（旧实现会解析成空串然后整个放弃）"
        );
        assert_eq!(
            proxy_addr_from_reg_value("HTTPS=127.0.0.1:7893").as_deref(),
            Some("http://127.0.0.1:7893"),
            "只有 https= → 用它给的值"
        );
        assert_eq!(proxy_addr_from_reg_value("socks=127.0.0.1:1080").as_deref(), Some("http://127.0.0.1:1080"));
        assert_eq!(proxy_addr_from_reg_value("http=;https=;"), None, "只有分隔符/空值 → None");
        assert_eq!(proxy_addr_from_reg_value(""), None);
        assert_eq!(proxy_addr_from_reg_value("   "), None);
        assert_eq!(
            proxy_addr_from_reg_value("http://127.0.0.1:7892").as_deref(),
            Some("http://127.0.0.1:7892"),
            "已带协议不重复拼"
        );
    }

    /// 真机注册表联动（Windows 才跑）：本机 HKCU 里 ProxyEnable 是 REG_DWORD，
    /// 解析结果必须是「要么 None，要么 http(s)://host:port」，且与注册表原文一致
    #[cfg(windows)]
    #[test]
    fn system_proxy_on_this_machine_is_parseable() {
        use std::os::windows::process::CommandExt;
        const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";
        let raw = |name: &str| -> String {
            std::process::Command::new("reg")
                .args(["query", KEY, "/v", name])
                .creation_flags(0x0800_0000)
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default()
        };
        let enable = parse_reg_dword(&raw("ProxyEnable"));
        assert!(enable.is_some(), "真机上 ProxyEnable 必须能解析出来（REG_DWORD 0x1）");
        match system_proxy() {
            Some(px) => assert!(
                px.starts_with("http://") || px.starts_with("https://") || px.starts_with("socks"),
                "解析出的系统代理必须是完整地址：{px}"
            ),
            None => assert_eq!(enable, Some(0), "只有 ProxyEnable=0 时才允许解析为 None"),
        }
    }
}
