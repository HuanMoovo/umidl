use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::ctx::{command_for, ensure_dir, Ctx};
use crate::models::{ToolInstallOutcome, ToolStatus, WhisperModelInfo};

const ARIA2_VERSION: &str = "1.37.0";
const WHISPER_BUILD: &str = "b5130";
/* 模型下载源：官方源 + 国内镜像源（可用环境变量 HF_ENDPOINT 整体覆盖） */
const HF_OFFICIAL: &str = "https://huggingface.co";
const HF_MIRROR: &str = "https://hf-mirror.com";

pub fn hint_for(name: &str) -> (&'static str, Option<&'static str>) {
    match name {
        "yt-dlp" => ("视频解析与下载核心（1000+ 站点支持）", Some("约 17 MB")),
        "ffmpeg" => ("音视频转码、合成、字幕烧录核心", Some("约 80 MB")),
        "ffprobe" => ("媒体信息探测（随 ffmpeg 一起提供）", Some("随 ffmpeg")),
        "whisper" => ("Whisper.cpp AI 语音识别引擎", Some("约 22 MB")),
        "aria2" => ("分段并行下载引擎（HTTP/FTP/BT/磁力，16 连接）", Some("约 5 MB")),
        "pandoc" => ("文档格式互转引擎（docx/odt/rtf/epub/html/md）", Some("约 35 MB")),
        "poppler" => ("PDF 处理引擎（pdftotext / pdftoppm）", Some("约 42 MB")),
        "emule" => ("ED2K 电驴下载引擎（eMule 社区版，可被本程序接管）", Some("约 4 MB")),
        "imagemagick" => (
            "图片格式引擎（PSD / DDS 写出、HEIC 读取等 ffmpeg 写不出的格式）",
            Some("约 12 MB（解压后约 240 MB）"),
        ),
        "whisper-model" => ("Whisper 语音模型文件", Some("75 MB ~ 1.6 GB")),
        _ => ("", None),
    }
}

fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// 在 PATH 中查找可执行文件
fn which(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let candidates: Vec<String> = vec![name.to_string(), exe_name(name)];
    for dir in std::env::split_paths(&path_var) {
        for c in &candidates {
            let full = dir.join(c);
            if full.is_file() {
                return Some(full);
            }
        }
    }
    None
}

/// 工具探活方式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeKind {
    /// 启动一次子进程，同时拿到「能不能跑 + 版本号」
    Exec,
    /// 只核对文件存在，**绝不启动**程序：GUI 程序（eMule）的 `--version` / `--help`
    /// 都不会退出，启动式探活会把整个刷新永久卡住（真机上锁死过设置页）
    ExistsOnly,
}

/// 该工具的探活方式（纯函数，便于单测）
pub fn probe_kind(name: &str) -> ProbeKind {
    match name {
        "emule" => ProbeKind::ExistsOnly,
        _ => ProbeKind::Exec,
    }
}

/// 探活方式的对外文本（写进 ToolStatus，界面据此标注「不启动探测」）
pub fn probe_kind_str(name: &str) -> &'static str {
    match probe_kind(name) {
        ProbeKind::Exec => "exec",
        ProbeKind::ExistsOnly => "exists",
    }
}

/// 单次探活命令的超时：到点强制结束（含子孙进程），刷新不会永久卡住
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// 版本号候选是否像版本：至少含一个数字（否则会把
/// `I/O Error: Couldn't open file '--version': No error.` 里的 "No" 当成版本号）、
/// 不含空白与引号
pub fn looks_like_version(s: &str) -> bool {
    !s.is_empty()
        && !s.chars().any(|c| c.is_whitespace() || c == '\'' || c == '"')
        && s.chars().any(|c| c.is_ascii_digit())
}

/// 严格版：形如 `3.11` / `26.09.0` / `v1.9.4` / `2025.09.26`（可选 v 前缀、可选 -/+ 后缀），
/// 至少两段纯数字 —— 用来在「整行扫第一个像版本号的 token」时挡掉 `10061)` 这类噪声
pub fn is_dotted_version(s: &str) -> bool {
    let t = s.trim_start_matches(['v', 'V']);
    let core = t.split(['-', '+']).next().unwrap_or("");
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() >= 2
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// 从一行程序输出里挑出版本号（纯函数，便于单测；真机输出行直接当输入）
pub fn version_candidate(name: &str, line: &str) -> Option<String> {
    let l = line.trim();
    if l.is_empty() {
        return None;
    }
    let toks: Vec<&str> = l.split_whitespace().collect();
    match name {
        // yt-dlp --version 只输出一行版本号，没有 "version" 字样
        "yt-dlp" => Some(l.to_string()).filter(|c| looks_like_version(c)),
        // magick -version 首行形如 "Version: ImageMagick 7.1.2-31 Q16-HDRI x64 …"，
        // 版本号在 "ImageMagick" 之后；沿用「version 后面的 token」会取到 "ImageMagick"
        "imagemagick" => toks
            .iter()
            .position(|t| t.eq_ignore_ascii_case("ImageMagick"))
            .and_then(|i| toks.get(i + 1))
            .map(|v| (*v).to_string()),
        // pandoc 首行是 "pandoc 3.11"（没有 version 字样）→ 取第一个像版本号的 token；
        // poppler 同一条规则也稳（"pdftotext version 26.09.0" → 26.09.0），
        // 且错误行（`I/O Error: … '--version': No error.`）不会产出 "No"
        "pandoc" | "poppler" => toks.iter().find(|t| is_dotted_version(t)).map(|t| (*t).to_string()),
        // 形如 "ffmpeg version 8.0.1..." / "aria2 version 1.37.0" / "whisper.cpp version v1.9.4"
        _ => toks
            .iter()
            .position(|t| t.to_lowercase().contains("version"))
            .and_then(|i| toks.get(i + 1))
            .map(|v| (*v).to_string())
            .filter(|c| looks_like_version(c)),
    }
}

/// 子进程捕获结果
pub struct CmdCapture {
    /// 是否因超时被强制结束
    pub timed_out: bool,
    /// 是否正常退出且退出码为 0
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// 结束探活进程（连子孙一起）：Windows 用 `taskkill /T /F`，
/// 只 kill 父进程的话子进程还握着管道写端，读输出的线程会一直等下去
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .creation_flags(0x0800_0000)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
    }
    #[cfg(unix)]
    {
        // 子进程在 run_capture 里以 process_group(0) 起，自成一个进程组（组 id = 子进程 pid）。
        // 只杀直接子进程不够：`sh -c "sleep 5"` 让孙进程继续抱着 stdout/stderr 管道的写端，
        // 读线程要等孙进程自然退出才拿到 EOF —— Linux CI 上表现为「超时后整整 5 秒才返回」。
        // 负号 = 整个进程组，与 Windows 的 taskkill /T 对齐。
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill();
}

/// 运行一个子进程并捕获输出，`timeout` 到点强制结束整棵进程树。
/// 只用标准库：spawn 后轮询 try_wait；stdout / stderr 各起一个读线程收干，
/// 避免管道缓冲区写满导致子进程假死（这也是「探活卡死」的另一条常见路径）。
pub fn run_capture(cmd: &mut std::process::Command, timeout: Duration) -> CmdCapture {
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        // 让子进程自成一个进程组（组 id = 子进程 pid）。超时按组杀进程树（kill_tree）才连
        // 孙进程一起带走；否则 `sh -c "sleep 5"` 的孙进程会继续抱着管道，把「准时返回」拖成
        // 「等它自然结束」。Windows 侧不用进程组，taskkill /T 直接按父子关系杀。
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => {
            return CmdCapture {
                timed_out: false,
                ok: false,
                stdout: String::new(),
                stderr: String::new(),
            }
        }
    };
    let drain = |mut p: std::process::ChildStdout| {
        std::thread::spawn(move || {
            let mut v = Vec::new();
            let _ = p.read_to_end(&mut v);
            v
        })
    };
    let so = child.stdout.take().map(drain);
    let se = child.stderr.take().map(|mut p| {
        std::thread::spawn(move || {
            let mut v = Vec::new();
            let _ = p.read_to_end(&mut v);
            v
        })
    });
    let deadline = std::time::Instant::now() + timeout;
    let mut status = None;
    let mut timed_out = false;
    loop {
        match child.try_wait() {
            Ok(Some(s)) => {
                status = Some(s);
                break;
            }
            Ok(None) => {}
            Err(_) => break,
        }
        if std::time::Instant::now() >= deadline {
            timed_out = true;
            kill_tree(&mut child);
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    let join = |h: Option<std::thread::JoinHandle<Vec<u8>>>| {
        h.map(|h| h.join().unwrap_or_default()).unwrap_or_default()
    };
    CmdCapture {
        timed_out,
        ok: status.map(|s| s.success()).unwrap_or(false),
        stdout: crate::ctx::decode_output(&join(so)),
        stderr: crate::ctx::decode_output(&join(se)),
    }
}

/// 一次子进程同时拿到「可用性 + 版本号」，避免同一工具被启动两次（超时可注入，便于单测）
fn run_probe_with(p: &Path, name: &str, timeout: Duration) -> (bool, Option<String>) {
    // 不适用启动式探活的工具（GUI 程序）：只核对文件在不在
    if probe_kind(name) == ProbeKind::ExistsOnly {
        return (p.is_file(), None);
    }
    // poppler 的 pdftotext 不认 `--version`（会当成文件名，输出
    // "I/O Error: Couldn't open file '--version': No error." → 旧解析器把 "No" 当版本号）；`-v` 才对
    let args: Vec<&str> = match name {
        "poppler" => vec!["-v"],
        "ffmpeg" | "ffprobe" => vec!["-version"],
        _ => vec!["--version"],
    };
    let mut ok = false;
    let mut version = None;
    for a in [args, vec!["--help"]] {
        let mut cmd = command_for(p);
        cmd.args(&a);
        let cap = run_capture(&mut cmd, timeout);
        if cap.timed_out {
            // GUI 程序 / 卡住的程序：已经强制结束，记「无法确认版本」而不是继续等第二个参数
            crate::ctx::cwarn(&format!(
                "{name} 探活超时（{}s，已强制结束进程），记为无法确认版本",
                timeout.as_secs()
            ));
            return (p.is_file(), None);
        }
        let stdout = cap.stdout;
        let stderr = cap.stderr;
        let text = if stdout.trim().is_empty() { stderr } else { stdout };
        ok = cap.ok || !text.trim().is_empty();
        for line in text.lines() {
            if let Some(c) = version_candidate(name, line) {
                version = Some(c);
                break;
            }
        }
        if ok && version.is_some() {
            break;
        }
        if ok {
            break;
        }
    }
    (ok, version)
}

/// 一次子进程同时拿到「可用性 + 版本号」，避免同一工具被启动两次
fn run_probe(p: &Path, name: &str) -> (bool, Option<String>) {
    run_probe_with(p, name, PROBE_TIMEOUT)
}

/// 工具探测缓存（路径 + 修改时间）：落盘持久化，热启动时完全不再启动子进程
#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
struct ProbeCacheEntry {
    mtime: u64,
    ok: bool,
    version: Option<String>,
}

static CACHE_PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
static PROBE_CACHE: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, ProbeCacheEntry>>,
> = std::sync::OnceLock::new();

/// 探活缓存文件名。
///
/// **改名即作废**：缓存键是「路径 + 文件 mtime」，而工具目录迁移是保留 mtime 的复制，
/// 所以旧版本写下的错误结论会被新版本继续命中 —— 曾经真实发生过：poppler 的条目里
/// 存着 `version: "No"`（旧解析器把 `I/O Error: … '--version': No error.` 里的 "No"
/// 当成了版本号），修好解析器后界面**仍然**显示「已装 · No」。
/// 探活规则 / 版本解析规则一变，这里就换一个文件名，让老用户的脏缓存自然作废。
const PROBE_CACHE_FILE: &str = "tool_cache_v3.json";

/// 启动时调用一次：指定缓存文件位置并载入
pub fn init_cache(dir: &Path) {
    let _ = CACHE_PATH.set(dir.join(PROBE_CACHE_FILE));
    let map: std::collections::HashMap<String, ProbeCacheEntry> = CACHE_PATH
        .get()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let _ = PROBE_CACHE.set(std::sync::Mutex::new(map));
}

fn mtime_key(p: &Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_lookup(p: &Path) -> Option<ProbeCacheEntry> {
    let key = p.to_string_lossy().to_string();
    let c = PROBE_CACHE.get()?.lock().ok()?;
    let e = c.get(&key)?.clone();
    (e.mtime == mtime_key(p)).then_some(e)
}

fn cache_store(p: &Path, ok: bool, version: Option<String>) {
    let key = p.to_string_lossy().to_string();
    let Some(cache) = PROBE_CACHE.get() else { return };
    let snapshot = {
        let Ok(mut c) = cache.lock() else { return };
        c.insert(
            key,
            ProbeCacheEntry {
                mtime: mtime_key(p),
                ok,
                version,
            },
        );
        let snap: std::collections::HashMap<String, ProbeCacheEntry> = c.clone();
        snap
    };
    if let Some(path) = CACHE_PATH.get() {
        if let Ok(txt) = serde_json::to_string(&snapshot) {
            let _ = std::fs::write(path, txt);
        }
    }
}

/// 快速可用性校验：托管/内置二进制必须真的能跑起来（结果走缓存）
fn runs_ok(p: &Path, name: &str) -> bool {
    if let Some(e) = cache_lookup(p) {
        return e.ok;
    }
    let (ok, version) = run_probe(p, name);
    cache_store(p, ok, version);
    ok
}

/// 解析某个工具的最终路径：显式设置 > 受管目录 > 可执行文件同级 > PATH
pub fn resolve_tool(ctx: &Ctx, name: &str, explicit: Option<&String>) -> (Option<PathBuf>, String) {
    let stem = match name {
        "whisper" => "whisper-cli",
        "aria2" => "aria2c",
        "emule" => "emule",
        other => other,
    };
    let file = exe_name(stem);
    let alt = exe_name("main");

    // 需要独立子目录/独立可执行名的工具（自带 DLL 或含多程序）
    let sub: Option<(&str, &str)> = match name {
        "poppler" => Some(("poppler", "pdftotext")),
        "emule" => Some(("emule", "emule")),
        "imagemagick" => Some(("imagemagick", "magick")),
        _ => None,
    };
    if let Some((dir, exe)) = sub {
        let cand = ctx.dirs.bin.join(dir).join(exe_name(exe));
        if cand.is_file() {
            return (Some(cand), "受管目录".into());
        }
    }

    if let Some(p) = explicit {
        let t = p.trim();
        if !t.is_empty() {
            let pb = PathBuf::from(t);
            if pb.is_file() {
                return (Some(pb), "managed".into());
            }
        }
    }

    // 受管目录（自动下载安装到这里）——必须通过可用性校验，损坏则回退
    let managed = ctx.dirs.bin.join(&file);
    if managed.is_file() && runs_ok(&managed, name) {
        return (Some(managed), "managed".into());
    }
    let managed_whisper = ctx.dirs.bin.join("whisper").join(&file);
    if managed_whisper.is_file() && runs_ok(&managed_whisper, name) {
        return (Some(managed_whisper), "managed".into());
    }
    // 兼容旧版 whisper.cpp 的可执行文件名 main.exe
    let managed_whisper_legacy = ctx.dirs.bin.join("whisper").join(&alt);
    if name == "whisper" && managed_whisper_legacy.is_file() && runs_ok(&managed_whisper_legacy, name) {
        return (Some(managed_whisper_legacy), "managed".into());
    }

    // 与主程序同级（便携绿色版 / 安装包内置）
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let beside = dir.join(&file);
            if beside.is_file() && runs_ok(&beside, name) {
                return (Some(beside), "bundled".into());
            }
            let beside_sub = dir.join("bin").join(&file);
            if beside_sub.is_file() && runs_ok(&beside_sub, name) {
                return (Some(beside_sub), "bundled".into());
            }
            if name == "whisper" {
                let bw = dir.join("whisper").join(&file);
                if bw.is_file() && runs_ok(&bw, name) {
                    return (Some(bw), "bundled".into());
                }
            }
        }
    }

    if let Some(p) = which(stem) {
        return (Some(p), "system".into());
    }

    (None, "missing".into())
}

/// 定位受管 ImageMagick：`bin/imagemagick/magick.exe` → `bin/magick.exe` → 系统 PATH
///
/// 官方 Windows 便携包只有 .7z（内置解压器只认 zip），所以不做在线安装，
/// 只检测并使用已经放进受管目录的 magick.exe（缺失时见 install_tool 的手动放置提示）。
pub fn resolve_imagemagick(ctx: &Ctx) -> Option<PathBuf> {
    let managed = ctx.dirs.bin.join("imagemagick").join(exe_name("magick"));
    if managed.is_file() {
        return Some(managed);
    }
    let flat = ctx.dirs.bin.join(exe_name("magick"));
    if flat.is_file() {
        return Some(flat);
    }
    which("magick")
}

pub fn resolve_all(ctx: &Ctx) -> crate::ctx::ToolPaths {
    let (ytdlp, _) = resolve_tool(ctx, "yt-dlp", ctx.settings.ytdlp_path.as_ref());
    let (ffmpeg, _) = resolve_tool(ctx, "ffmpeg", ctx.settings.ffmpeg_path.as_ref());
    let (ffprobe, _) = resolve_tool(ctx, "ffprobe", None);
    let (whisper, _) = resolve_tool(ctx, "whisper", ctx.settings.whisper_path.as_ref());
    let (aria2, _) = resolve_tool(ctx, "aria2", None);
    let (pandoc, _) = resolve_tool(ctx, "pandoc", None);
    let (poppler, _) = resolve_tool(ctx, "poppler", None);
    let (emule, _) = resolve_tool(ctx, "emule", None);
    // ffprobe 通常与 ffmpeg 同目录
    let ffprobe = ffprobe.or_else(|| {
        ffmpeg
            .as_ref()
            .and_then(|f| f.parent().map(|d| d.join(exe_name("ffprobe"))))
            .filter(|p| p.is_file())
    });
    crate::ctx::ToolPaths { ytdlp, ffmpeg, ffprobe, whisper, aria2, pandoc, poppler, emule }
}

/// 版本号（走「路径 + 修改时间」缓存，热启动不再启动子进程）
pub fn tool_version(path: &Path, name: &str) -> Option<String> {
    if let Some(e) = cache_lookup(path) {
        return e.version;
    }
    let (ok, version) = run_probe(path, name);
    cache_store(path, ok, version.clone());
    version
}

/// detect 的短时记忆（改工具目录 / 刚装完工具后可用 invalidate_memo 立刻作废）
static DETECT_MEMO: std::sync::OnceLock<std::sync::Mutex<Option<(std::time::Instant, Vec<ToolStatus>)>>> =
    std::sync::OnceLock::new();

/// 让下一次 detect 重新探测（换工具目录、重装/校验工具之后调用）
pub fn invalidate_memo() {
    let memo = DETECT_MEMO.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(mut g) = memo.lock() {
        *g = None;
    }
}

/// 一次「工具刷新」的总时间预算：超预算后剩下的工具不再启动新进程（只用缓存 / 文件存在判定），
/// 保证「重新检测 / 换目录后刷新」一定在有限时间内返回（GUI 程序、卡住的可执行文件都拖不死它）
const DETECT_BUDGET: Duration = Duration::from_secs(20);

/// 时间预算是否还允许启动新的探活进程（纯函数，便于单测）
pub fn within_probe_budget(started: std::time::Instant, budget: Duration) -> bool {
    started.elapsed() < budget
}

/// 版本号：`fresh = true` 绕过缓存、真的启动一次程序（「校验」走这条路），否则走「路径 + 修改时间」缓存。
/// `allow_exec = false`（时间预算耗尽）时只读缓存，绝不启动新进程。
fn probe_path(p: &Path, name: &str, fresh: bool, allow_exec: bool) -> Option<String> {
    if !allow_exec {
        return cache_lookup(p).and_then(|e| e.version);
    }
    if fresh {
        let (ok, version) = run_probe(p, name);
        cache_store(p, ok, version.clone());
        version
    } else {
        tool_version(p, name)
    }
}

/// 生成指定工具的状态（顺序与 names 一致）。
/// `fresh = true` 时绕过版本缓存真的启动一次程序 —— 校验按钮需要「现在到底还能不能跑」。
fn statuses_for(ctx: &Ctx, names: &[&str], fresh: bool) -> Vec<ToolStatus> {
    let tools = resolve_all(ctx);
    let started = std::time::Instant::now();
    let mut out = Vec::new();

    for name in names.iter().copied() {
        let (hint, size_hint) = hint_for(name);
        // 预算耗尽 → 后面的工具只做文件核对（不启动新进程），刷新一定有限时间内返回
        let allow_exec = within_probe_budget(started, DETECT_BUDGET);

        // Whisper 模型不在工具目录里（在模型目录），单独成条；不随工具目录迁移
        if name == "whisper-model" {
            let model_path = ctx.model_path();
            let found = model_path.is_file();
            out.push(ToolStatus {
                name: name.into(),
                found,
                path: Some(model_path.to_string_lossy().to_string()),
                version: None,
                source: if found { "managed".into() } else { "missing".into() },
                managed_path: Some(model_path.to_string_lossy().to_string()),
                hint: format!("{hint}（当前模型：{}）", ctx.settings.whisper_model),
                size_hint: Some(model_size_hint(&ctx.settings.whisper_model)),
                origin: None,
                installable: true,
                probe: None,
            });
            continue;
        }

        // ImageMagick 不在 ToolPaths 里（它不是下载/转码主链的一部分），单独探测：
        // PSD / DDS 等 ffmpeg 写不出的图片格式由它写出，格式目录的 available 依赖它是否存在
        if name == "imagemagick" {
            let im_path = resolve_imagemagick(ctx);
            let found = im_path.is_some();
            let version = im_path.as_deref().and_then(|p| probe_path(p, name, fresh, allow_exec));
            out.push(ToolStatus {
                name: name.into(),
                found,
                path: im_path.map(|p| p.to_string_lossy().to_string()),
                version,
                source: if found { "managed".into() } else { "missing".into() },
                managed_path: Some(
                    ctx.dirs
                        .bin
                        .join("imagemagick")
                        .join(exe_name("magick"))
                        .to_string_lossy()
                        .to_string(),
                ),
                hint: hint.to_string(),
                size_hint: size_hint.map(|s| s.to_string()),
                origin: None,
                installable: installable(name),
                probe: Some(probe_kind_str(name).into()),
            });
            continue;
        }

        let path = match name {
            "yt-dlp" => tools.ytdlp.clone(),
            "ffmpeg" => tools.ffmpeg.clone(),
            "ffprobe" => tools.ffprobe.clone(),
            "whisper" => tools.whisper.clone(),
            "aria2" => tools.aria2.clone(),
            "pandoc" => tools.pandoc.clone(),
            "poppler" => tools.poppler.clone(),
            "emule" => tools.emule.clone(),
            _ => None,
        };
        let explicit = match name {
            "yt-dlp" => ctx.settings.ytdlp_path.as_ref(),
            "ffmpeg" => ctx.settings.ffmpeg_path.as_ref(),
            "whisper" => ctx.settings.whisper_path.as_ref(),
            _ => None,
        };
        let source = if path.is_some() {
            resolve_tool(ctx, name, explicit).1
        } else {
            "missing".to_string()
        };
        let version = path.as_deref().and_then(|p| probe_path(p, name, fresh, allow_exec));
        out.push(ToolStatus {
            name: name.to_string(),
            found: path.is_some(),
            path: path.map(|p| p.to_string_lossy().to_string()),
            version,
            source,
            managed_path: Some(
                match name {
                    "whisper" => ctx.dirs.bin.join("whisper").join(exe_name("whisper-cli")),
                    _ => ctx.dirs.bin.join(exe_name(name)),
                }
                .to_string_lossy()
                .to_string(),
            ),
            hint: hint.to_string(),
            size_hint: size_hint.map(|s| s.to_string()),
            origin: None,
            installable: installable(name),
            probe: Some(probe_kind_str(name).into()),
        });
    }
    out
}

/// 检测全部依赖（顺序 = TOOL_ORDER）
pub fn detect(ctx: &Ctx) -> Vec<ToolStatus> {
    // 短时记忆：启动流程中设置页与任务仓库可能同时请求（实测会跑两次），
    // 2 秒内直接复用结果，避免重复启动子进程；手动刷新（超过 2 秒）不受影响。
    let memo = DETECT_MEMO.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(g) = memo.lock() {
        if let Some((t, v)) = g.as_ref() {
            if t.elapsed() < std::time::Duration::from_secs(2) {
                return v.clone();
            }
        }
    }

    let t_start = std::time::Instant::now();
    let out = statuses_for(ctx, &TOOL_ORDER, false);
    let elapsed = t_start.elapsed();

    crate::ctx::cwarn(&format!(
        "工具检测耗时 {} ms（首次需探测 yt-dlp 版本，之后走缓存）",
        elapsed.as_millis()
    ));
    if elapsed > DETECT_BUDGET {
        crate::ctx::cwarn(&format!(
            "工具检测超过预算 {}s（实际 {}s）：本轮对尾部的工具只做了文件核对，未启动进程",
            DETECT_BUDGET.as_secs(),
            elapsed.as_secs()
        ));
    }
    if let Ok(mut g) = memo.lock() {
        *g = Some((std::time::Instant::now(), out.clone()));
    }
    out
}

/* ==================== Whisper 模型 ==================== */

/// 内置模型清单：(设置里的名字, 预计体积, 质量星级)
pub const WHISPER_BUILTIN: [(&str, &str, &str); 6] = [
    ("tiny", "75 MB", "★☆☆"),
    ("base", "142 MB", "★★☆"),
    ("small", "466 MB", "★★★"),
    ("medium", "1.5 GB", "★★★★"),
    ("large-v3-turbo", "1.6 GB", "★★★★★"),
    ("large-v3", "3.1 GB", "★★★★★"),
];

/// 设置里的模型标识 → 磁盘文件名
pub fn model_file_name(name: &str) -> String {
    let n = name.trim();
    if n.is_empty() {
        return "ggml-base.bin".into();
    }
    if n.to_lowercase().ends_with(".bin") {
        n.to_string()
    } else {
        format!("ggml-{n}.bin")
    }
}

/* ==================== 模型可用性 / 自动回落（BUG-15） ==================== */

/// 模型文件可用判定：存在且体积合理（下载中断留下的残片不能拿去识别）
pub fn model_ready(p: &Path) -> bool {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0) > 1_000_000
}

/// 模型目录里不属于内置清单的 ggml-*.bin（自定义模型），按文件名排序
pub fn custom_model_files(models_dir: &Path) -> Vec<String> {
    let builtin: Vec<String> = WHISPER_BUILTIN
        .iter()
        .map(|(n, _, _)| format!("ggml-{n}.bin"))
        .collect();
    let mut out: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(models_dir) {
        for e in rd.flatten() {
            let f = e.file_name().to_string_lossy().to_string();
            if !f.to_lowercase().ends_with(".bin") || builtin.iter().any(|b| b == &f) {
                continue;
            }
            if model_ready(&e.path()) {
                out.push(f);
            }
        }
    }
    out.sort();
    out
}

/// 已安装的模型标识（内置短名 + 自定义文件名），顺序与设置页一致
pub fn installed_models_in(models_dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (name, _, _) in WHISPER_BUILTIN {
        if model_ready(&models_dir.join(format!("ggml-{name}.bin"))) {
            out.push(name.to_string());
        }
    }
    out.extend(custom_model_files(models_dir));
    out
}

/// 实际要用的模型（BUG-15）：配置的模型没下载时自动回落到已安装的模型，
/// 内置按体积升序回退（tiny 最优先），保证「默认设置也能开箱即用」。
/// 返回 (实际使用的模型标识, 是否发生了回落)。
pub fn effective_model_for(models_dir: &Path, wanted: &str) -> (String, bool) {
    let want = wanted.trim();
    let want = if want.is_empty() { "base" } else { want };
    if model_ready(&models_dir.join(model_file_name(want))) {
        return (want.to_string(), false);
    }
    for (name, _, _) in WHISPER_BUILTIN {
        if model_ready(&models_dir.join(format!("ggml-{name}.bin"))) {
            return ((*name).to_string(), true);
        }
    }
    if let Some(f) = custom_model_files(models_dir).into_iter().next() {
        return (f, true);
    }
    (want.to_string(), false)
}

/// 「模型没装」时的可行动报错（BUG-15）：缺哪个模型、期望文件在哪、已装了哪些、去哪儿装
pub fn missing_model_message(models_dir: &Path, wanted: &str, expected: &Path) -> String {
    let want = wanted.trim();
    let want = if want.is_empty() { "base" } else { want };
    let installed = installed_models_in(models_dir);
    let list = if installed.is_empty() {
        "无".to_string()
    } else {
        installed.join("、")
    };
    let alternative = if installed.is_empty() {
        String::new()
    } else {
        format!("，或在字幕页把「Whisper 模型」切换成已安装的模型（{}）", installed.join("、"))
    };
    format!(
        "缺少 Whisper 模型「{want}」（期望文件：{}）；当前已安装：{list}；\
         请打开「设置 → 依赖工具 → Whisper 模型」下载 {want}{alternative}",
        expected.display()
    )
}

/// URL / 路径 → 安全的模型文件名（统一 ggml-*.bin 形式，便于后续识别）
fn safe_model_file(spec: &str) -> String {
    let raw = spec
        .split(['?', '#'])
        .next()
        .unwrap_or(spec)
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(spec)
        .trim();
    let mut s: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '-' })
        .collect();
    if s.is_empty() {
        s = "ggml-custom.bin".into();
    }
    if !s.to_lowercase().ends_with(".bin") {
        s.push_str(".bin");
    }
    if !s.to_lowercase().starts_with("ggml-") {
        s = format!("ggml-{s}");
    }
    s
}

/// 列出模型目录里的模型（内置 + 自定义），标注是否已下载与是否为当前使用
pub fn list_whisper_models(ctx: &Ctx) -> Vec<WhisperModelInfo> {
    let current = ctx.settings.whisper_model.trim().to_string();
    let current_file = model_file_name(&current);
    let mut out: Vec<WhisperModelInfo> = Vec::new();

    for (name, hint, quality) in WHISPER_BUILTIN {
        let file = format!("ggml-{name}.bin");
        let p = ctx.dirs.models.join(&file);
        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        out.push(WhisperModelInfo {
            name: name.to_string(),
            file: file.clone(),
            builtin: true,
            downloaded: size > 1_000_000,
            size_bytes: size,
            size_hint: hint.to_string(),
            quality: quality.to_string(),
            current: current_file == file,
        });
    }

    // 自定义模型：模型目录里所有其它 ggml-*.bin
    if let Ok(rd) = std::fs::read_dir(&ctx.dirs.models) {
        for e in rd.flatten() {
            let file = e.file_name().to_string_lossy().to_string();
            if !file.to_lowercase().ends_with(".bin") || out.iter().any(|m| m.file == file) {
                continue;
            }
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(WhisperModelInfo {
                name: file.clone(),
                file: file.clone(),
                builtin: false,
                downloaded: size > 1_000_000,
                size_bytes: size,
                size_hint: human_size(size),
                quality: String::new(),
                current: current_file == file,
            });
        }
    }
    out
}

pub fn model_size_hint(model: &str) -> String {
    match model {
        "tiny" => "75 MB".into(),
        "base" => "142 MB".into(),
        "small" => "466 MB".into(),
        "medium" => "1.5 GB".into(),
        "large-v3-turbo" => "1.6 GB".into(),
        "large-v3" => "3.1 GB".into(),
        _ => "未知".into(),
    }
}

/* ==================== 受管工具目录（1.9：唯一来源 + 自定义 + 迁移） ====================
 *
 * 1.8.6 及以前：工具一律装在「数据目录/bin」（AppDirs::bin，即 %APPDATA%/umi-downloader/bin），
 * 用户既不能改目录，也不能挑要装哪几个工具（只有整包「一键安装」）。
 * 现在：
 *   - 工具根目录由设置 `tool_dir` 决定，**所有**解析 / 调用 / 安装 / 清扫只认它
 *     （落地点在 Ctx::new，见 ctx.rs —— 换目录后一处生效、全局跟随）；
 *   - 目录可迁移：先复制 + 体积校验，全部成功才删旧目录；任何失败都保留旧目录原样。
 */

/// 工具清单（检测 / 安装 / 迁移 / 批量操作的唯一顺序来源，与设置页展示顺序一致）
pub const TOOL_ORDER: [&str; 10] = [
    "yt-dlp", "ffmpeg", "ffprobe", "whisper", "aria2", "pandoc", "poppler", "emule",
    "imagemagick", "whisper-model",
];

/// 本平台是否能自动下载安装（无官方静态包的工具为 false，界面据此不显示「下载」按钮）
pub fn installable(name: &str) -> bool {
    name == "whisper-model" || download_url(name).is_some()
}

/// 目录错误报文：`[<code>] <对象> :: <原因>`（与模型下载同一套契约，前端按 code 出四语言文案）
fn dir_err(code: &str, subject: &str, detail: &str) -> String {
    format!("[{code}] {subject} :: {detail}")
}

/// 绝对路径判定：`Path::is_absolute`（含 UNC）之外，额外接受 Windows 盘符写法 `X:\` `X:/`
/// —— 单测要能在任何平台上验证同一条规则。
pub fn is_absolute_path(s: &str) -> bool {
    if Path::new(s).is_absolute() {
        return true;
    }
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// 生效的工具根目录（纯函数，便于单测）：
/// 设置里的 `tool_dir` 是唯一来源 —— 非空且是绝对路径就用它；空 / 相对路径 / 手改坏的配置
/// 一律回落默认目录，保证「一处配置写错」不会让所有工具瞬间解析不到。
pub fn effective_tool_dir(default_bin: &Path, configured: &str) -> PathBuf {
    let t = configured.trim();
    if t.is_empty() {
        return default_bin.to_path_buf();
    }
    if !is_absolute_path(t) {
        crate::ctx::cwarn(&format!(
            "设置里的工具目录不是绝对路径（{t}），已回落到默认目录 {}",
            default_bin.display()
        ));
        return default_bin.to_path_buf();
    }
    PathBuf::from(t)
}

/// 校验用户输入的工具目录（严格；错误报文带稳定错误码，前端本地化）。
/// 空 / 非法字符 / 相对路径 / 指向文件 / 建不出来 / 建得出来但写不进去，都给出准确原因。
pub fn validate_tool_dir(input: &str) -> Result<PathBuf, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(dir_err("tools.dir.empty", "", "工具目录不能为空"));
    }
    if raw.chars().any(|c| c == '\0' || "<>\"|?*".contains(c)) {
        return Err(dir_err(
            "tools.dir.invalid",
            raw,
            "目录名包含非法字符（< > \" | ? *）",
        ));
    }
    if !is_absolute_path(raw) {
        return Err(dir_err(
            "tools.dir.relative",
            raw,
            r"请填写绝对路径，例如 D:\Umidl\tools",
        ));
    }
    let p = PathBuf::from(raw);
    if p.is_file() {
        return Err(dir_err("tools.dir.not_dir", raw, "这个路径是一个文件，不是目录"));
    }
    std::fs::create_dir_all(&p).map_err(|e| dir_err("tools.dir.create_failed", raw, &e.to_string()))?;
    // 目录建得出来不代表写得进去（只读盘 / 受控文件夹 / 权限不足）——真写一个探测文件
    let probe = p.join(format!(".umi-write-probe-{}", crate::ctx::short_id()));
    std::fs::write(&probe, b"ok").map_err(|e| dir_err("tools.dir.not_writable", raw, &e.to_string()))?;
    let _ = std::fs::remove_file(&probe);
    Ok(p)
}

/// 受管条目类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
}

impl EntryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EntryKind::File => "file",
            EntryKind::Dir => "dir",
        }
    }
}

/// 某个工具在受管目录里的相对条目（文件 / 整目录）——安装位置、迁移、清理共用这一张表。
/// 返回空表示该工具**不在**工具目录里（whisper-model 存在模型目录）。
pub fn managed_entries(name: &str) -> Vec<(String, EntryKind)> {
    match name {
        "yt-dlp" => vec![(exe_name("yt-dlp"), EntryKind::File)],
        "ffmpeg" => vec![(exe_name("ffmpeg"), EntryKind::File)],
        "ffprobe" => vec![(exe_name("ffprobe"), EntryKind::File)],
        // whisper 需要同目录的 DLL，且旧版可执行名是 main.exe → 整个子目录一起搬
        "whisper" => vec![
            ("whisper".to_string(), EntryKind::Dir),
            (exe_name("whisper-cli"), EntryKind::File),
        ],
        "aria2" => vec![(exe_name("aria2c"), EntryKind::File)],
        "pandoc" => vec![(exe_name("pandoc"), EntryKind::File)],
        "poppler" => vec![
            ("poppler".to_string(), EntryKind::Dir),
            (exe_name("pdftotext"), EntryKind::File),
        ],
        "emule" => vec![
            ("emule".to_string(), EntryKind::Dir),
            (exe_name("emule"), EntryKind::File),
        ],
        "imagemagick" => vec![
            ("imagemagick".to_string(), EntryKind::Dir),
            (exe_name("magick"), EntryKind::File),
        ],
        _ => Vec::new(),
    }
}

/// 是否随工具目录迁移（whisper-model 在模型目录，不参与）
pub fn migratable(name: &str) -> bool {
    !managed_entries(name).is_empty()
}

/// 某个工具在受管目录里的**可执行候选位置**（顺序 = 解析优先级；含各自子目录 / 可执行名差异）。
///
/// 这张表是「安装后判定」与 `resolve_tool` 的共同口径：poppler 在 `bin/poppler/pdftotext.exe`
/// （exe 依赖同目录 DLL）、emule 在 `bin/emule/emule.exe`、whisper 在 `bin/whisper/whisper-cli.exe`
/// （旧版叫 main.exe）、其余平铺在 `bin/<exe>`。
pub fn managed_candidates(ctx: &Ctx, name: &str) -> Vec<PathBuf> {
    let bin = &ctx.dirs.bin;
    match name {
        "whisper" => vec![
            bin.join("whisper").join(exe_name("whisper-cli")),
            bin.join("whisper").join(exe_name("main")),
            bin.join(exe_name("whisper-cli")),
        ],
        "aria2" => vec![bin.join(exe_name("aria2c")), bin.join("aria2").join(exe_name("aria2c"))],
        "poppler" => vec![
            bin.join("poppler").join(exe_name("pdftotext")),
            bin.join(exe_name("pdftotext")),
        ],
        "emule" => vec![bin.join("emule").join(exe_name("emule")), bin.join(exe_name("emule"))],
        "imagemagick" => vec![
            bin.join("imagemagick").join(exe_name("magick")),
            bin.join(exe_name("magick")),
        ],
        "ffprobe" => vec![bin.join(exe_name("ffprobe")), bin.join(exe_name("ffmpeg"))],
        other => vec![bin.join(exe_name(other))],
    }
}

/// 从候选位置里挑出「确实装好了」的那一个（`probe` 可注入，便于单测）。
/// 全部候选都不成立 → None（调用方据此报「安装后仍无法运行」）。
pub fn pick_installed(candidates: &[PathBuf], probe: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|p| p.is_file() && probe(p))
        .cloned()
}

/// 目录体积（递归；不存在算 0）——迁移前后用它校验「复制完整」
pub fn dir_size(p: &Path) -> u64 {
    if p.is_file() {
        return std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    }
    let mut total = 0u64;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                total += dir_size(&path);
            } else {
                total += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    total
}

/// 迁移计划里的一条
#[derive(Debug, Clone, serde::Serialize)]
pub struct MigrationEntry {
    /// 相对工具目录的路径（写进报告给用户看）
    pub rel: String,
    /// file | dir
    pub kind: String,
    pub bytes: u64,
    /// 是否属于已知工具（false = 目录里的额外文件 / 目录，一并搬走以免丢失）
    pub known: bool,
    /// 新目录里已有同体积的副本（重复执行迁移时不会重复复制）
    pub existing: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MigrationPlan {
    pub entries: Vec<MigrationEntry>,
    pub bytes: u64,
    /// 不搬的残片（`*.downloading`）——半截文件搬过去也没用
    pub skipped: Vec<String>,
}

/// 迁移计划（只读文件系统，不写任何东西；可单测）：
/// 已知工具的受管条目 + 目录里其它额外条目（绝不静默丢弃），残片除外。
pub fn migration_plan(from: &Path, to: &Path) -> MigrationPlan {
    let mut plan = MigrationPlan::default();
    if !from.is_dir() {
        return plan;
    }
    let mut covered: Vec<String> = Vec::new();
    for name in TOOL_ORDER {
        for (rel, kind) in managed_entries(name) {
            if covered.iter().any(|c| c.eq_ignore_ascii_case(&rel)) {
                continue;
            }
            let src = from.join(&rel);
            let exists = match kind {
                EntryKind::File => src.is_file(),
                EntryKind::Dir => src.is_dir(),
            };
            if !exists {
                continue;
            }
            covered.push(rel.clone());
            plan.entries.push(MigrationEntry {
                rel: rel.clone(),
                kind: kind.as_str().into(),
                bytes: dir_size(&src),
                known: true,
                existing: dir_size(&to.join(&rel)) == dir_size(&src) && dir_size(&src) > 0,
            });
        }
    }
    // 额外条目（用户手动放进来的 exe / dll / 说明文件……）：一起搬，别让人以为工具没了
    if let Ok(rd) = std::fs::read_dir(from) {
        let mut extra: Vec<String> = Vec::new();
        for e in rd.flatten() {
            let f = e.file_name().to_string_lossy().to_string();
            if covered.iter().any(|c| c.eq_ignore_ascii_case(&f)) {
                continue;
            }
            if f.to_lowercase().ends_with(".downloading") {
                plan.skipped.push(f);
                continue;
            }
            extra.push(f);
        }
        extra.sort();
        for f in extra {
            let src = from.join(&f);
            let bytes = dir_size(&src);
            plan.entries.push(MigrationEntry {
                rel: f.clone(),
                kind: if src.is_dir() { "dir".into() } else { "file".into() },
                bytes,
                known: false,
                existing: bytes > 0 && dir_size(&to.join(&f)) == bytes,
            });
        }
    }
    plan.bytes = plan.entries.iter().map(|e| e.bytes).sum();
    plan
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MigrationReport {
    pub from: String,
    pub to: String,
    /// 只有「全部复制并校验通过」才为 true（false 时旧目录一个文件都没删）
    pub ok: bool,
    pub files_copied: usize,
    pub bytes_copied: u64,
    pub already_present: usize,
    pub removed_old: usize,
    /// 旧目录里删不掉 / 仍存在的条目（不影响使用，但要让用户知道）
    pub leftovers: Vec<String>,
    pub skipped: Vec<String>,
    /// 失败项：有内容就整体失败（不删旧、不改设置）
    pub errors: Vec<String>,
}

/// 递归复制文件或目录，返回（文件数, 字节数）
fn copy_entry(src: &Path, dst: &Path) -> anyhow::Result<(usize, u64)> {
    if src.is_dir() {
        ensure_dir(dst)?;
        let mut n = 0usize;
        let mut bytes = 0u64;
        for e in std::fs::read_dir(src)?.flatten() {
            let (cn, cb) = copy_entry(&e.path(), &dst.join(e.file_name()))?;
            n += cn;
            bytes += cb;
        }
        Ok((n, bytes))
    } else {
        if let Some(p) = dst.parent() {
            ensure_dir(p)?;
        }
        std::fs::copy(src, dst)?;
        Ok((1, std::fs::metadata(dst).map(|m| m.len()).unwrap_or(0)))
    }
}

/// 迁移受管工具目录：**先复制 + 体积校验，全部成功才删旧目录**。
/// 任何一步失败 → 旧目录原样保留（不丢文件），并逐条报告失败原因。
pub fn migrate_tool_dir(from: &Path, to: &Path) -> MigrationReport {
    let mut rep = MigrationReport {
        from: from.display().to_string(),
        to: to.display().to_string(),
        ..Default::default()
    };
    if from == to {
        rep.ok = true;
        return rep;
    }
    // 互相嵌套会让「复制」与「删除」互相踩：直接拒绝，别冒丢文件的风险
    if to.starts_with(from) || from.starts_with(to) {
        rep.errors.push(dir_err(
            "tools.dir.nested",
            &to.display().to_string(),
            "新目录不能位于旧目录内部（也不能把旧目录放进新目录里）",
        ));
        return rep;
    }
    if let Err(e) = validate_tool_dir(&to.to_string_lossy()) {
        rep.errors.push(e);
        return rep;
    }
    if !from.is_dir() {
        // 旧目录还不存在 = 没有工具要搬
        rep.ok = true;
        return rep;
    }

    let plan = migration_plan(from, to);
    rep.skipped = plan.skipped.clone();
    for e in &plan.entries {
        let src = from.join(&e.rel);
        let dst = to.join(&e.rel);
        if e.existing {
            rep.already_present += 1;
            continue;
        }
        match copy_entry(&src, &dst) {
            Ok((n, bytes)) => {
                rep.files_copied += n;
                rep.bytes_copied += bytes;
                // 复制后立刻校验体积：半截文件不算成功，绝不能拿它换掉旧目录
                if dir_size(&src) != dir_size(&dst) {
                    rep.errors.push(dir_err(
                        "tools.dir.copy_mismatch",
                        &e.rel,
                        "复制后体积不一致（可能是磁盘空间不足），旧目录保持原样",
                    ));
                }
            }
            Err(err) => rep.errors.push(dir_err("tools.dir.copy_failed", &e.rel, &err.to_string())),
        }
    }
    if !rep.errors.is_empty() {
        return rep;
    }

    // 全部校验通过 → 才删旧目录里的条目（删不掉只记 leftovers，不影响使用）
    for e in &plan.entries {
        let src = from.join(&e.rel);
        let done = if src.is_dir() {
            std::fs::remove_dir_all(&src).is_ok()
        } else {
            std::fs::remove_file(&src).is_ok()
        };
        if done {
            rep.removed_old += 1;
        } else if src.exists() {
            rep.leftovers.push(e.rel.clone());
        }
    }
    rep.ok = true;
    rep
}

/// 勾选集规范化：去空白 / 去重 / 保序；空名字丢弃，未知名字保留（由调用方逐条报错）
pub fn normalize_selection(names: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for raw in names {
        let n = raw.trim().to_string();
        if n.is_empty() || !seen.insert(n.clone()) {
            continue;
        }
        out.push(n);
    }
    out
}

/// 批量安装（「下载所选」）：逐个装、互不影响；已装的可重装（同名文件会被覆盖）。
/// 单个工具的失败只体现在它自己的 outcome 里，不会中断整批。
pub fn install_tools(ctx: &Ctx, names: &[String]) -> Vec<ToolInstallOutcome> {
    let mut out = Vec::new();
    for name in normalize_selection(names) {
        if !TOOL_ORDER.contains(&name.as_str()) {
            out.push(ToolInstallOutcome::failed(
                &name,
                dir_err("tools.unknown", &name, "不是已知的依赖工具"),
            ));
            continue;
        }
        let r = if name == "whisper-model" {
            let m = ctx.settings.whisper_model.trim();
            let m = if m.is_empty() { "base" } else { m };
            install_whisper_model(ctx, m)
        } else {
            install_tool(ctx, &name)
        };
        match r {
            Ok(st) => out.push(ToolInstallOutcome::done(&name, st)),
            Err(e) => out.push(ToolInstallOutcome::failed(
                &name,
                dir_err("tools.install_failed", &name, &e.to_string()),
            )),
        }
    }
    invalidate_memo();
    out
}

/// 校验所选工具：绕过版本缓存真的启动一次程序，报告「现在还能不能跑」+ 实际路径/版本。
pub fn verify_tools(ctx: &Ctx, names: &[String]) -> Vec<ToolInstallOutcome> {
    let mut out = Vec::new();
    for name in normalize_selection(names) {
        if !TOOL_ORDER.contains(&name.as_str()) {
            out.push(ToolInstallOutcome::failed(
                &name,
                dir_err("tools.unknown", &name, "不是已知的依赖工具"),
            ));
            continue;
        }
        match statuses_for(ctx, &[name.as_str()], true).into_iter().next() {
            Some(st) => {
                let ok = st.found;
                let error = if ok {
                    None
                } else {
                    Some(dir_err(
                        "tools.verify_missing",
                        &name,
                        "没有找到可用的可执行文件，请先下载",
                    ))
                };
                out.push(ToolInstallOutcome { name, ok, status: Some(st), error });
            }
            None => out.push(ToolInstallOutcome::failed(
                &name,
                dir_err("tools.unknown", &name, "不是已知的依赖工具"),
            )),
        }
    }
    invalidate_memo();
    out
}

/* ==================== 下载安装 ==================== */

fn download_url(name: &str) -> Option<String> {
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    match name {
        "yt-dlp" => Some(match os {
            "windows" => "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe".into(),
            "macos" => "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos".into(),
            _ => "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp".into(),
        }),
        "ffmpeg" => Some(match os {
            "windows" => "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip".into(),
            "macos" => "https://evermeet.cx/ffmpeg/getrelease/zip".into(),
            _ => "https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-amd64-static.tar.xz".into(),
        }),
        "pandoc" => Some(match os {
            "windows" => "https://github.com/jgm/pandoc/releases/download/3.11/pandoc-3.11-windows-x86_64.zip".into(),
            "macos" => "https://github.com/jgm/pandoc/releases/download/3.11/pandoc-3.11-arm64-macOS.zip".into(),
            _ => "https://github.com/jgm/pandoc/releases/download/3.11/pandoc-3.11-linux-amd64.tar.gz".into(),
        }),
        "poppler" => Some(match os {
            "windows" => "https://github.com/oschwartz10612/poppler-windows/releases/download/v26.09.0-0/Release-26.09.0-0.zip".into(),
            _ => String::new(),
        })
        .filter(|s| !s.is_empty()),
        "emule" => Some(match os {
            "windows" => "https://github.com/irwir/eMule/releases/download/eMule_v0.72a-community/eMule0.72a.zip".into(),
            _ => String::new(),
        })
        .filter(|s| !s.is_empty()),
        "aria2" => Some(match os {
            "windows" => format!("https://github.com/aria2/aria2/releases/download/release-{ARIA2_VERSION}/aria2-{ARIA2_VERSION}-win-64bit-build1.zip"),
            _ => String::new(),
        })
        .filter(|s| !s.is_empty()),
        "whisper" => Some(match (os, arch) {
            ("windows", "x86_64") => format!("https://github.com/ggml-org/whisper.cpp/releases/download/{WHISPER_BUILD}/whisper-bin-x64.zip"),
            ("windows", "aarch64") => format!("https://github.com/ggml-org/whisper.cpp/releases/download/{WHISPER_BUILD}/whisper-bin-win-cpu-arm64.zip"),
            ("macos", _) => String::new(),
            _ => format!("https://github.com/ggml-org/whisper.cpp/releases/download/{WHISPER_BUILD}/whisper-bin-ubuntu-x64.tar.gz"),
        })
        .filter(|s| !s.is_empty()),
        _ => None,
    }
}

fn emit_progress(ctx: &Ctx, name: &str, percent: f64, message: &str) {
    ctx.emit_raw(
        "tool://progress",
        serde_json::json!({ "name": name, "percent": percent, "message": message }),
    );
}

/* ==================== 下载：代理 / 多源回退 / 断点续传 ====================
 *
 * 1.8.6 的真实故障（用户报「Whisper 语音模型下载不了」）：
 *   - 应用自带的 HTTP 客户端（reqwest）只认环境变量里的代理，**不读 Windows 系统代理**
 *     （reqwest 的 system-proxy 特性没开，见 Cargo.toml）；而用户机器上的代理写在注册表里，
 *     于是下载请求直连 huggingface.co → DNS 被污染成 31.13.x.x → SYN 黑洞 → 十几秒后
 *     只回一句 `error sending request for url (...)`，既没说是哪儿错、也没给替代出路；
 *   - 只有 huggingface.co 一个源、只试一次、无断点续传：国内直连基本必然失败。
 * 这里的修法：走 downloader::effective_proxy（设置 → 环境变量 → 系统代理）+ 镜像源回退
 *   + 断点续传 + 可分类/可本地化的错误文案。
 */

/// 模型下载源顺序（纯函数，便于单测）：
/// - `via_proxy = true` → 官方源优先（镜像站走代理常常更慢），镜像兜底；
/// - `via_proxy = false`（国内直连）→ 镜像源优先：官方源 DNS 被污染，先试它只会白等十几秒。
/// `endpoint`（HF_ENDPOINT）非空时整体替换（自建镜像 / 企业内网）。
pub fn model_source_urls_with(file: &str, via_proxy: bool, endpoint: Option<&str>) -> Vec<String> {
    let path = format!("ggerganov/whisper.cpp/resolve/main/{file}");
    if let Some(ep) = endpoint.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        return vec![format!("{}/{}", ep.trim_end_matches('/'), path)];
    }
    let (first, second) = if via_proxy {
        (HF_OFFICIAL, HF_MIRROR)
    } else {
        (HF_MIRROR, HF_OFFICIAL)
    };
    vec![format!("{first}/{path}"), format!("{second}/{path}")]
}

/// 按当前网络环境排序的模型下载源（有可用代理时官方源优先，否则镜像源优先）
pub fn model_source_urls(ctx: &Ctx, file: &str) -> Vec<String> {
    let via_proxy = crate::downloader::effective_proxy(ctx, HF_OFFICIAL).is_some();
    model_source_urls_with(file, via_proxy, hf_endpoint().as_deref())
}

/// 环境变量 HF_ENDPOINT（huggingface 生态通用约定）可整体替换下载入口
pub fn hf_endpoint() -> Option<String> {
    std::env::var("HF_ENDPOINT")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// URL → 主机名（进度文案与错误分类都要显示「是哪个源」）
pub fn host_of(url: &str) -> String {
    let after = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    after.split(['/', '?', '#']).next().unwrap_or("").to_string()
}

/// 残片路径：`ggml-base.bin` → `ggml-base.downloading`（与 delete_whisper_model 的清理一致）
pub fn partial_path(dest: &Path) -> PathBuf {
    dest.with_extension("downloading")
}

/// 断点续传起点：残片已有多少字节（没有残片就是 0）
pub fn partial_bytes(tmp: &Path) -> u64 {
    std::fs::metadata(tmp).map(|m| m.len()).unwrap_or(0)
}

/// 残片最多信多久：跨会话续传要能用，但太旧的残片可能对应「上游已经变了的内容」
/// （yt-dlp / ffmpeg 的 latest 会滚动，模型也可能被重新上传），拼起来就是坏文件 ——
/// 超过窗口的残片直接丢弃重下，宁可多下几分钟也不要一个坏文件。
pub const RESUME_MAX_AGE_SECS: u64 = 6 * 3600;

/// 续传起点决策（纯函数，便于单测）：残片非空且在保活窗口内才续
pub fn resume_start(size: u64, age_secs: u64) -> u64 {
    if size > 0 && age_secs <= RESUME_MAX_AGE_SECS {
        size
    } else {
        0
    }
}

/// 实际续传起点：读残片大小与年龄；过期残片顺手删掉（免得下次还把它的字节数显示给用户）
pub fn resume_from(tmp: &Path) -> u64 {
    let Ok(meta) = std::fs::metadata(tmp) else {
        return 0;
    };
    let size = meta.len();
    let age = meta
        .modified()
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if resume_start(size, age) > 0 {
        return size;
    }
    if size > 0 {
        crate::ctx::cwarn(&format!(
            "残片 {size} 字节已过期（{age} 秒前），丢弃后从头下载"
        ));
        let _ = std::fs::remove_file(tmp);
    }
    0
}

/// 下载失败的类型。code 稳定（前端据此本地化），detail 保留原始信息便于排查。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DlKind {
    /// 域名解析失败（国内最常见的：DNS 污染 / 断网）
    Dns,
    /// 连接超时
    Timeout,
    /// 连不上：拒绝连接 / 网络不可达 / 连接被重置
    Connect,
    /// 代理链路问题（代理进程没开、代理拒绝）
    Proxy,
    /// TLS 握手失败
    Tls,
    /// 服务端明确返回的 HTTP 状态码
    Http(u16),
    /// 断点续传位置失效（HTTP 416）
    Range,
    /// 传着传着断了（服务端声明了长度却没给够）—— 残片已保留，重试会续传
    Truncated,
    /// 本地读写失败（明确的文件写入/定位错误）
    Io,
    /// 认不出来的失败（不瞎猜原因，原文透传给用户）
    Unknown,
    /// 下载内容不是有效模型
    Invalid,
    /// 所有源都失败（聚合错误）
    Sources,
}

impl DlKind {
    /// 稳定错误码：`[model.dns] …` 形式传给前端做四语言本地化（见 src/services/backendError.ts）
    pub fn code(&self) -> String {
        match self {
            DlKind::Dns => "model.dns".into(),
            DlKind::Timeout => "model.timeout".into(),
            DlKind::Connect => "model.connect".into(),
            DlKind::Proxy => "model.proxy".into(),
            DlKind::Tls => "model.tls".into(),
            DlKind::Http(c) => format!("model.http{c}"),
            DlKind::Range => "model.range".into(),
            DlKind::Truncated => "model.truncated".into(),
            DlKind::Io => "model.io".into(),
            DlKind::Unknown => "model.unknown".into(),
            DlKind::Invalid => "model.invalid".into(),
            DlKind::Sources => "model.sources".into(),
        }
    }

    /// 同一个源上要不要再试一次：只有「可能自愈」的失败才值得重试。
    /// DNS 污染 / 连不上 / 403 这类重试也是白等，直接换下一个源更快。
    pub fn worth_retry(&self) -> bool {
        matches!(self, DlKind::Io | DlKind::Range | DlKind::Truncated)
            || matches!(self, DlKind::Http(c) if *c >= 500)
    }
}

/// 下载错误：报文格式 `[<code>] <host> :: <detail>`
#[derive(Debug, Clone)]
pub struct DlErr {
    pub kind: DlKind,
    pub host: String,
    pub detail: String,
}

impl DlErr {
    pub fn new(kind: DlKind, host: &str, detail: impl Into<String>) -> Self {
        Self {
            kind,
            host: host.to_string(),
            detail: detail.into(),
        }
    }

    /// 传给前端的文案：前缀是稳定错误码，前端据此换成四种语言的完整说明
    pub fn message(&self) -> String {
        format!("[{}] {} :: {}", self.kind.code(), self.host, self.detail)
    }

    pub fn into_anyhow(self) -> anyhow::Error {
        anyhow::anyhow!("{}", self.message())
    }
}

impl std::fmt::Display for DlErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// 错误原因链：只取顶层 Display 会丢掉「为什么」——旧版文案不可行动正是这个原因
pub fn err_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut out = vec![e.to_string()];
    let mut cur = e.source();
    while let Some(s) = cur {
        out.push(s.to_string());
        if out.len() >= 8 {
            break;
        }
        cur = s.source();
    }
    out.join(" ← ")
}

/// 关键词 → 错误类型（集中一处，便于单测：真实网络错误不好在单测里构造）。
/// 中英文都认：Windows 的中文系统上 io 错误是中文文案（「远程主机强迫关闭了一个现有的连接」）。
pub fn classify_text(text: &str) -> DlKind {
    let t = text.to_ascii_lowercase();
    let has = |k: &str| t.contains(k);
    if has("dns error")
        || has("failed to lookup address")
        || has("no such host")
        || has("nodename nor servname")
        || has("name or service not known")
        || has("getaddrinfo")
        || has("os error 11001")
        || has("不知道这样的主机")
        || has("无法解析")
        || has("名称解析")
    {
        return DlKind::Dns;
    }
    if has("proxy") || has("tunnel") || has("代理") {
        return DlKind::Proxy;
    }
    if has("certificate") || has("tls") || has("handshake") || has("invalid peer") || has("证书") {
        return DlKind::Tls;
    }
    if has("timed out") || has("timeout") || has("timedout") || has("os error 10060") || has("超时") {
        return DlKind::Timeout;
    }
    if has("connection refused")
        || has("refused")
        || has("unreachable")
        || has("network is down")
        || has("host is down")
        || has("reset by peer")
        || has("forcibly closed")
        || has("connection closed")
        || has("body from connection")
        || has("broken pipe")
        || has("unexpected eof")
        || has("os error 10061")
        || has("强迫关闭")
        || has("积极拒绝")
        || has("连接被重置")
        || has("无法连接")
    {
        return DlKind::Connect;
    }
    DlKind::Unknown
}

pub fn classify_reqwest(e: &reqwest::Error) -> DlKind {
    classify_text(&err_chain(e))
}

/// HTTP 状态码 → 错误类型（416 表示续传位置失效，要清残片重来）
pub fn classify_status(code: u16) -> DlKind {
    if code == 416 {
        DlKind::Range
    } else {
        DlKind::Http(code)
    }
}

/// 下载结果
#[derive(Debug, Clone)]
pub struct FetchOutcome {
    /// 最终文件体积（含续传的部分）
    pub bytes: u64,
    /// 实际使用的下载源主机名
    pub source: String,
    /// 续传起点（0 = 全新下载）
    pub resumed_from: u64,
}

/// 进度事件：带上结构化字段，前端可本地化渲染（老前端读 message 也不至于空着）
fn emit_progress_ex(
    ctx: &Ctx,
    name: &str,
    downloaded: u64,
    total: u64,
    source: &str,
    resumed: bool,
) {
    let percent = if total > 0 {
        (downloaded as f64 / total as f64 * 100.0).min(100.0)
    } else {
        0.0
    };
    let message = if total > 0 {
        format!("{source} · {} / {}", human_size(downloaded), human_size(total))
    } else {
        format!("{source} · {}", human_size(downloaded))
    };
    ctx.emit_raw(
        "tool://progress",
        serde_json::json!({
            "name": name,
            "percent": percent,
            "message": message,
            "source": source,
            "downloaded": downloaded,
            "total": total,
            "resumed": resumed,
        }),
    );
}

/// 下载事件写进应用日志（界面里能看、能导出）—— 错误文案里说的「详细原因见日志」就是它
fn dlog(ctx: &Ctx, name: &str, msg: &str) {
    crate::ctx::cwarn(msg);
    crate::logs::log_line(&ctx.dirs, name, msg);
}

/// 连接超时：国内到被墙站点的 SYN 黑洞靠它及时收场（旧版 30s 太久了）
const DL_CONNECT_TIMEOUT: Duration = Duration::from_secs(12);

/// 下载客户端：代理由 downloader::effective_proxy 统一裁决（显式代理会让 reqwest
/// 自己关掉环境变量探测，所以这里必须把代理完整传进来）
fn dl_client(proxy: Option<&str>) -> anyhow::Result<reqwest::blocking::Client> {
    dl_client_with(&crate::downloader::client_proxy_plan(proxy))
}

/// 按「已裁决好的代理方案」构造下载客户端（单测直接喂 ProxyPlan，不用真起下载）。
///
/// 关键点：决策为**直连**时必须 `.no_proxy()` —— reqwest 默认会自己读
/// `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY`，不显式关掉的话，`effective_proxy`
/// 那句「自动改为直连」只是日志说说，请求实际仍走那个（可能已死的）环境变量代理，
/// 于是两个源一起报 `tunnel error … os error 10061`。
pub(crate) fn dl_client_with(plan: &crate::downloader::ProxyPlan) -> anyhow::Result<reqwest::blocking::Client> {
    let mut b = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(1800))
        .connect_timeout(DL_CONNECT_TIMEOUT)
        .user_agent("umi-downloader/1.0");
    match &plan.proxy {
        // 决策为用代理：显式 Proxy::all(url)
        Some(px) => match reqwest::Proxy::all(px) {
            Ok(p) => b = b.proxy(p),
            Err(e) => {
                // 地址都解析不出来 → 本次必须真直连（否则又会退回环境变量里的那个死代理）
                crate::ctx::cwarn(&format!("代理地址不可用，本次改为直连：{px}（{e}）"));
                b = b.no_proxy();
            }
        },
        None if plan.no_proxy => b = b.no_proxy(),
        None => {}
    }
    Ok(b.build()?)
}

/// 单次下载尝试（支持断点续传）：`dest` 由 `.downloading` 残片改名而来，只有完整下完才落名
fn fetch_once(
    ctx: &Ctx,
    name: &str,
    url: &str,
    dest: &Path,
    proxy: Option<&str>,
) -> Result<FetchOutcome, DlErr> {
    let host = host_of(url);
    let tmp = partial_path(dest);
    if let Some(dir) = tmp.parent() {
        ensure_dir(dir).map_err(|e| DlErr::new(DlKind::Io, &host, e.to_string()))?;
    }
    let mut start_at = resume_from(&tmp);
    let client = dl_client(proxy).map_err(|e| DlErr::new(DlKind::Proxy, &host, e.to_string()))?;

    let mut rb = client.get(url);
    if start_at > 0 {
        rb = rb.header(reqwest::header::RANGE, format!("bytes={start_at}-"));
    }
    let mut resp = rb.send().map_err(|e| {
        let chain = err_chain(&e);
        DlErr::new(classify_reqwest(&e), &host, chain)
    })?;

    let status = resp.status();
    if status.as_u16() == 416 {
        // 残片比目标还大 / 服务端不认这个区间 → 清掉重来（外层会再试一次）
        let _ = std::fs::remove_file(&tmp);
        return Err(DlErr::new(
            DlKind::Range,
            &host,
            format!("HTTP 416：残片 {start_at} 字节已失效，已清理"),
        ));
    }
    if !status.is_success() {
        return Err(DlErr::new(
            classify_status(status.as_u16()),
            &host,
            format!("HTTP {status}"),
        ));
    }
    // 服务端忽略 Range（200 + 完整内容）→ 必须从头写，不能追加
    if status.as_u16() == 200 && start_at > 0 {
        crate::ctx::cwarn(&format!("{host} 不支持断点续传（HTTP 200），从头下载"));
        let _ = std::fs::remove_file(&tmp);
        start_at = 0;
    }
    let total = resp.content_length().unwrap_or(0) + start_at;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&tmp)
        .map_err(|e| DlErr::new(DlKind::Io, &host, e.to_string()))?;
    if start_at > 0 {
        file.seek(std::io::SeekFrom::Start(start_at))
            .map_err(|e| DlErr::new(DlKind::Io, &host, e.to_string()))?;
    } else {
        file.set_len(0).map_err(|e| DlErr::new(DlKind::Io, &host, e.to_string()))?;
    }

    let mut buf = vec![0u8; 128 * 1024];
    let mut written: u64 = 0;
    let throttle = crate::ratelimit::LIMITER.enabled();
    let mut last_emit = std::time::Instant::now();
    emit_progress_ex(ctx, name, start_at, total, &host, start_at > 0);
    loop {
        let n = resp.read(&mut buf).map_err(|e| {
            let chain = e.to_string();
            DlErr::new(classify_text(&chain), &host, chain)
        })?;
        if n == 0 {
            break;
        }
        if throttle {
            // 令牌桶：工具/模型下载同样受全局限速约束
            let wait = crate::ratelimit::LIMITER.wait_time(n as u64);
            if !wait.is_zero() {
                std::thread::sleep(wait);
            }
        }
        file.write_all(&buf[..n])
            .map_err(|e| DlErr::new(DlKind::Io, &host, e.to_string()))?;
        written += n as u64;
        if last_emit.elapsed() > Duration::from_millis(200) {
            last_emit = std::time::Instant::now();
            emit_progress_ex(ctx, name, start_at + written, total, &host, start_at > 0);
        }
    }
    let got = start_at + written;
    let _ = file.flush();
    drop(file);

    // 服务端声明了长度却没给够：留下残片（下次续传），不要冒充成功
    if total > 0 && got < total {
        return Err(DlErr::new(
            DlKind::Truncated,
            &host,
            format!("连接中断：{got}/{total} 字节（残片已保留，可续传）"),
        ));
    }
    std::fs::rename(&tmp, dest).map_err(|e| DlErr::new(DlKind::Io, &host, e.to_string()))?;
    Ok(FetchOutcome {
        bytes: got,
        source: host,
        resumed_from: start_at,
    })
}

/// 下载源日志行里「走没走代理」的说明（纯函数，便于单测）：
/// 用户导出的日志里要能一眼看出这次下载是直连还是经代理 ——
/// 「自动改为直连」的原因在 stderr 的告警里，这里给的是结果
pub fn proxy_note(proxy: Option<&str>) -> String {
    match proxy.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(px) => format!("经代理 {px} 下载"),
        None => "直连下载".into(),
    }
}

/// 多源下载：逐个源尝试；同一源只在「值得重试」的失败上重来一次（残片续传，不清零）
pub fn download_with_fallback(
    ctx: &Ctx,
    name: &str,
    urls: &[String],
    dest: &Path,
) -> Result<FetchOutcome, DlErr> {
    let mut tried: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for url in urls {
        let host = host_of(url);
        // 代理逐个源现算（源不同、可达性不同），规则统一走 effective_proxy
        let proxy = crate::downloader::effective_proxy(ctx, url);
        dlog(ctx, name, &format!("{host} {}", proxy_note(proxy.as_deref())));
        tried.push(host.clone());
        for attempt in 0..2 {
            if attempt > 0 {
                dlog(
                    ctx,
                    name,
                    &format!(
                        "{host} 下载中断，{} 字节残片已保留，正在重试…",
                        partial_bytes(&partial_path(dest))
                    ),
                );
            }
            match fetch_once(ctx, name, url, dest, proxy.as_deref()) {
                Ok(out) => {
                    if out.resumed_from > 0 {
                        dlog(
                            ctx,
                            name,
                            &format!(
                                "续传完成：{} → {} 字节（来源 {host}）",
                                out.resumed_from, out.bytes
                            ),
                        );
                    }
                    return Ok(out);
                }
                Err(e) => {
                    errors.push(format!("{host} ← {}", e.detail));
                    dlog(ctx, name, &format!("下载源 {host} 失败：{}", e.message()));
                    if attempt == 0 && e.kind.worth_retry() {
                        continue;
                    }
                    break;
                }
            }
        }
        if tried.len() < urls.len() {
            let keep = partial_bytes(&partial_path(dest));
            dlog(
                ctx,
                name,
                &format!(
                    "切换到下一个下载源 {}（已保留 {keep} 字节残片）",
                    host_of(&urls[tried.len()])
                ),
            );
            emit_progress_ex(ctx, name, keep, 0, &host_of(&urls[tried.len()]), keep > 0);
        }
    }
    Err(DlErr::new(
        DlKind::Sources,
        &tried.join("、"),
        errors.join(" ｜ "),
    ))
}

/// ggml 模型文件魔数："ggml"（0x67676D6C 的小端落盘形式）——防止把 HTML 错误页当成模型
pub const GGML_MAGIC: [u8; 4] = [0x6c, 0x6d, 0x67, 0x67];

/// 校验下载/导入的模型文件：体积下限 + 魔数。返回体积。
/// 校验不过说明拿到的是错误页 / 半截文件，必须报错而不是留在模型目录里冒充可用模型。
pub fn verify_model_file(p: &Path) -> Result<u64, String> {
    let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    if size <= 1_000_000 {
        return Err(format!("文件只有 {} 字节（完整模型至少几十 MB）", size));
    }
    let mut f = std::fs::File::open(p).map_err(|e| e.to_string())?;
    let mut head = [0u8; 4];
    if f.read_exact(&mut head).is_err() || head != GGML_MAGIC {
        return Err(format!(
            "文件头不是 ggml 模型魔数（{} 字节，读到 {:02x?}）",
            size, head
        ));
    }
    Ok(size)
}

/// 带进度回调的文件下载（工具包 / 归档 / 模型）：走代理链路 + 断点续传 + 多源（单源时也支持续传）
pub fn fetch_to_file(ctx: &Ctx, name: &str, url: &str, dest: &Path) -> anyhow::Result<u64> {
    let urls = vec![url.to_string()];
    let out = download_with_fallback(ctx, name, &urls, dest).map_err(|e| e.into_anyhow())?;
    emit_progress(ctx, name, 100.0, "下载完成");
    Ok(out.bytes)
}

pub fn human_size(bytes: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{v:.0} {}", U[i])
    } else {
        format!("{v:.1} {}", U[i])
    }
}

fn unzip_pick(zip_path: &Path, wanted: &[&str], dest_dir: &Path, flatten: bool) -> anyhow::Result<Vec<PathBuf>> {
    let f = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(f)?;
    ensure_dir(dest_dir)?;
    let mut extracted = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let raw_name = entry.name().to_string();
        let file_name = raw_name.rsplit(['/', '\\']).next().unwrap_or("").to_string();
        if file_name.is_empty() {
            continue;
        }
        let matches = wanted.iter().any(|w| file_name.eq_ignore_ascii_case(w));
        // whisper 需要同目录的动态库
        let is_sidecar = flatten && file_name.to_lowercase().ends_with(".dll");
        if !matches && !is_sidecar {
            continue;
        }
        let out_path = dest_dir.join(&file_name);
        if entry.is_dir() {
            continue;
        }
        let mut out = std::fs::File::create(&out_path)?;
        std::io::copy(&mut entry, &mut out)?;
        drop(out);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(0o755));
        }
        extracted.push(out_path);
    }
    Ok(extracted)
}

/// 整包解压到目录（poppler / eMule 这类 exe 依赖同目录 DLL 的场景）。
/// 会剥掉压缩包最外层的那一层目录（`Release-xx/Library/bin/` 之类），保持相对结构。
/// 返回解压的文件数。
fn unzip_all_into(zip_path: &Path, dest_dir: &Path) -> anyhow::Result<usize> {
    let f = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(f)?;
    ensure_dir(dest_dir)?;

    // 先收集所有条目名，找出需要剥离的公共前缀
    let mut names: Vec<String> = Vec::new();
    for i in 0..archive.len() {
        let e = archive.by_index(i)?;
        names.push(e.name().to_string());
    }
    let strip = if names.iter().all(|n| n.contains('/')) {
        let first = |s: &str| s.split('/').next().unwrap_or("").to_string();
        let head = names.iter().map(|n| first(n)).collect::<Vec<_>>();
        let uniq: std::collections::BTreeSet<&String> = head.iter().collect();
        if uniq.len() == 1 && !head.first().map(|h| h.is_empty()).unwrap_or(true) {
            format!("{}/", first(&names[0]))
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // 若包内存在 xxx/bin/ 子树（poppler 的 <pkg>/Library/bin/ 就是这种），
    // 只解压该子树并拍平：这些 exe 依赖同目录 DLL，多留一层目录会让解析器找不到。
    let bin_prefix = names
        .iter()
        .filter(|n| !n.ends_with('/'))
        .find_map(|n| {
            let low = n.to_ascii_lowercase();
            low.find("/bin/").map(|pos| n[..pos + 5].to_string())
        });
    if let Some(bp) = bin_prefix {
        let mut n = 0usize;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let raw = entry.name().to_string();
            if !raw.starts_with(&bp) || entry.is_dir() {
                continue;
            }
            let rel = raw[bp.len()..].replace('\\', "/");
            if rel.is_empty() {
                continue;
            }
            let out_path = dest_dir.join(&rel);
            if let Some(p) = out_path.parent() {
                ensure_dir(p)?;
            }
            let mut out = std::fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out)?;
            drop(out);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(0o755));
            }
            n += 1;
        }
        return Ok(n);
    }

    let mut n = 0usize;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let raw = entry.name().to_string();
        let rel = raw.strip_prefix(&strip).unwrap_or(&raw).to_string();
        if rel.is_empty() || rel.ends_with('/') || entry.is_dir() {
            continue;
        }
        let out_path = dest_dir.join(rel.replace('\\', "/"));
        if let Some(p) = out_path.parent() {
            ensure_dir(p)?;
        }
        let mut out = std::fs::File::create(&out_path)?;
        std::io::copy(&mut entry, &mut out)?;
        drop(out);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(0o755));
        }
        n += 1;
    }
    Ok(n)
}

fn xtract_tar_gz(_archive: &Path, _dest: &Path) -> anyhow::Result<()> {
    anyhow::bail!("当前平台暂不支持自动安装该工具，请手动指定路径")
}

/// 依文件头判断归档类型（不依赖扩展名，避免临时文件命名影响判断）
pub fn detect_archive(path: &Path) -> &'static str {
    if let Ok(mut f) = std::fs::File::open(path) {
        let mut head = [0u8; 6];
        let n = std::io::Read::read(&mut f, &mut head).unwrap_or(0);
        if n >= 4 {
            match &head[..4] {
                b"PK\x03\x04" => return "zip",
                _ => {}
            }
        }
        if n >= 2 {
            match &head[..2] {
                b"\x1f\x8b" => return "gzip",
                b"MZ" => return "exe",
                _ => {}
            }
        }
        if n >= 6 && &head[..6] == b"\xfd7zXZ\x00" {
            return "xz";
        }
    }
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()) {
        Some(e) if e == "zip" => "zip",
        Some(e) if e == "gz" || e == "tgz" => "gzip",
        Some(e) if e == "xz" => "xz",
        _ => "bin",
    }
}

/// 安装工具（自动下载）
pub fn install_tool(ctx: &Ctx, name: &str) -> anyhow::Result<ToolStatus> {
    if name == "whisper-model" {
        anyhow::bail!("请使用 install_whisper_model 安装模型");
    }
    let url = download_url(name).ok_or_else(|| {
        // 这些工具在各平台没有官方静态包：提示用户如何手动放进受管工具目录
        let hint = match name {
            "aria2" => format!(
                "：macOS 可 brew install aria2、Linux 可 apt/dnf install aria2，随后把 aria2c 复制到 {} 即可（Umidl 不依赖系统 PATH，只从工具目录读取）",
                ctx.dirs.bin.display()
            ),
            "whisper" => format!(
                "：macOS 可 brew install whisper-cpp，随后把 whisper-cli 复制到 {}",
                ctx.dirs.bin.display()
            ),
            "imagemagick" => format!(
                "：官方 Windows 便携包只有 .7z（内置解压器只认 zip），请从 https://download.imagemagick.org/archive/binaries/ \
                 下载 ImageMagick-*-portable-Q16-HDRI-x64.7z，解压后把整个目录放到 {}（即 {}）",
                ctx.dirs.bin.join("imagemagick").display(),
                ctx.dirs.bin.join("imagemagick").join(exe_name("magick")).display()
            ),
            _ => String::new(),
        };
        anyhow::anyhow!(
            "当前平台（{} {}）没有提供 {name} 的自动安装包{hint}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;

    ctx.dirs.ensure();
    // 先清理可能损坏的旧文件
    match name {
        "whisper" => {
            let _ = std::fs::remove_file(ctx.dirs.bin.join("whisper").join(exe_name("whisper-cli")));
        }
        _ => {
            let _ = std::fs::remove_file(ctx.dirs.bin.join(exe_name(name)));
        }
    }
    // 临时文件必须保留真实扩展名（部分归档工具依赖它）
    let url_ext = std::path::Path::new(&url)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin");
    let tmp = ctx.dirs.cache.join(format!("{name}-setup.{url_ext}"));
    emit_progress(ctx, name, 1.0, "开始下载…");
    fetch_to_file(ctx, name, &url, &tmp)?;

    match detect_archive(&tmp) {
        "zip" => match name {
            "ffmpeg" => {
                let exe_f = exe_name("ffmpeg");
                let exe_p = exe_name("ffprobe");
                let got = unzip_pick(&tmp, &[&exe_f, &exe_p], &ctx.dirs.bin, false)?;
                if got.is_empty() {
                    anyhow::bail!("压缩包中未找到 ffmpeg 可执行文件");
                }
            }
            "pandoc" => {
                let got = unzip_pick(&tmp, &[&exe_name("pandoc")], &ctx.dirs.bin, false)?;
                if got.is_empty() {
                    anyhow::bail!("压缩包中未找到 pandoc 可执行文件");
                }
            }
            "poppler" => {
                // poppler 的 exe 依赖同目录 DLL，必须整目录解压
                let dir = ctx.dirs.bin.join("poppler");
                std::fs::create_dir_all(&dir)?;
                let got = unzip_all_into(&tmp, &dir)?;
                if !dir.join(exe_name("pdftotext")).is_file() {
                    anyhow::bail!("压缩包中未找到 pdftotext（已解压 {got} 个文件）");
                }
            }
            "emule" => {
                let dir = ctx.dirs.bin.join("emule");
                std::fs::create_dir_all(&dir)?;
                let got = unzip_all_into(&tmp, &dir)?;
                if !dir.join(exe_name("emule")).is_file() {
                    anyhow::bail!("压缩包中未找到 emule.exe（已解压 {got} 个文件）");
                }
            }
            "whisper" => {
                let dir = ctx.dirs.bin.join("whisper");
                let got = unzip_pick(&tmp, &[&exe_name("whisper-cli"), &exe_name("main")], &dir, true)?;
                if got.is_empty() {
                    anyhow::bail!("压缩包中未找到 whisper-cli 可执行文件");
                }
            }
            "aria2" => {
                let got = unzip_pick(&tmp, &[&exe_name("aria2c")], &ctx.dirs.bin, false)?;
                if got.is_empty() {
                    anyhow::bail!("压缩包中未找到 aria2c 可执行文件");
                }
            }
            _ => {
                let exe = exe_name(name);
                let got = unzip_pick(&tmp, &[&exe], &ctx.dirs.bin, false)?;
                if got.is_empty() {
                    anyhow::bail!("压缩包中未找到 {exe}");
                }
            }
        },
        "gzip" | "xz" => xtract_tar_gz(&tmp, &ctx.dirs.bin)?,
        _ => {
            let dest = match name {
                "whisper" => ctx.dirs.bin.join("whisper").join(exe_name("whisper-cli")),
                _ => ctx.dirs.bin.join(exe_name(name)),
            };
            if dest != tmp {
                if let Some(p) = dest.parent() {
                    ensure_dir(p)?;
                }
                if dest.exists() {
                    let _ = std::fs::remove_file(&dest);
                }
                std::fs::rename(&tmp, &dest)?;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
            }
        }
    }

    let _ = std::fs::remove_file(&tmp);
    emit_progress(ctx, name, 100.0, "安装完成");

    // 安装完成 → 用统一的受管候选表判定「是否真的装好了」。
    // 以前这里只把 yt-dlp / ffmpeg / whisper 三个映射成路径，其余工具（aria2 / pandoc /
    // poppler / emule / imagemagick）装完一律 path=None → 必报「安装后仍无法运行」
    // （真机上 aria2 文件已正确重装、能跑，界面却报失败）。
    let explicit = match name {
        "yt-dlp" => ctx.settings.ytdlp_path.as_ref(),
        "ffmpeg" => ctx.settings.ffmpeg_path.as_ref(),
        "whisper" => ctx.settings.whisper_path.as_ref(),
        _ => None,
    };
    let installed = pick_installed(&managed_candidates(ctx, name), |p| runs_ok(p, name));
    let (path, source) = match installed {
        Some(p) => (Some(p), "managed".to_string()),
        // 受管位置没就绪 → 走统一解析兜底（用户可能把同名程序放在别处 / PATH 里）
        None => {
            let (p, src) = resolve_tool(ctx, name, explicit);
            (p, src)
        }
    };
    if path.is_none() {
        anyhow::bail!("{name} 安装后仍无法运行，请尝试手动指定路径");
    }
    let version = path.as_deref().and_then(|p| tool_version(p, name));
    let (hint, size_hint) = hint_for(name);
    Ok(ToolStatus {
        name: name.into(),
        found: path.is_some(),
        path: path.map(|p| p.to_string_lossy().to_string()),
        version,
        source,
        managed_path: None,
        hint: hint.into(),
        size_hint: size_hint.map(|s| s.into()),
        origin: Some(host_of(&url)),
        installable: installable(name),
        probe: Some(probe_kind_str(name).into()),
    })
}

/// 安装 Whisper 模型
/// 下载内置模型；已存在（>1 MB）时直接复用，避免切换模型时重复下载上 GB 的文件。
/// 下载走「代理 → 镜像源 → 官方源」链路，支持断点续传；失败报错带稳定错误码（前端本地化）。
pub fn install_whisper_model(ctx: &Ctx, model: &str) -> anyhow::Result<ToolStatus> {
    let valid = ["tiny", "base", "small", "medium", "large-v3", "large-v3-turbo"];
    if !valid.contains(&model) {
        anyhow::bail!("不支持的模型：{model}（自定义模型请用「自定义导入」）");
    }
    let file = format!("ggml-{model}.bin");
    let dest = ctx.dirs.models.join(&file);
    ctx.dirs.ensure();
    let hint = model_size_hint(model);
    if model_ready(&dest) {
        return Ok(model_status(&dest, &hint, "managed", None));
    }
    let urls = model_source_urls(ctx, &file);
    emit_progress(
        ctx,
        "whisper-model",
        0.0,
        &format!("{} · {}", host_of(&urls[0]), file),
    );
    let out = match download_with_fallback(ctx, "whisper-model", &urls, &dest) {
        Ok(o) => o,
        Err(e) => {
            dlog(ctx, "whisper-model", &format!("模型 {file} 下载失败：{}", e.message()));
            return Err(e.into_anyhow());
        }
    };
    dlog(
        ctx,
        "whisper-model",
        &format!("模型 {file} 下载完成：{} 字节 ← {}", out.bytes, out.source),
    );
    verify_model_file(&dest)
        .map_err(|msg| anyhow::anyhow!("[{}] {} :: {msg}", DlKind::Invalid.code(), out.source))?;
    emit_progress(ctx, "whisper-model", 100.0, "模型就绪");
    Ok(model_status(&dest, &hint, "managed", Some(&out.source)))
}

fn model_status(dest: &Path, size_hint: &str, source: &str, origin: Option<&str>) -> ToolStatus {
    ToolStatus {
        name: "whisper-model".into(),
        found: dest.is_file(),
        path: Some(dest.to_string_lossy().to_string()),
        version: None,
        source: source.into(),
        managed_path: Some(dest.to_string_lossy().to_string()),
        hint: format!(
            "Whisper 语音模型（{}）",
            dest.file_name().unwrap_or_default().to_string_lossy()
        ),
        size_hint: Some(size_hint.to_string()),
        origin: origin.map(|s| s.to_string()),
        // 模型可下载（走 HuggingFace 多源链路），但不属于工具目录
        installable: true,
        // 模型是文件，谈不上「启动探测」
        probe: None,
    }
}

/// 自定义模型导入，四种写法都支持：
/// 1) GitHub 直链（Release 资产 / `blob/…`→raw / `raw.githubusercontent.com` / 带 `?raw=true`）
///    → 走 GitHub 自己的链路下载（**不套 HF 镜像**），下完校验 ggml 魔数
/// 2) 其它 http(s) 链接 → 直接下载（文件名取自链接末段）
/// 3) 本地 .bin 文件路径 → 复制进模型目录
/// 4) 官方仓库文件名或短名（`ggml-small.bin` / `small`）→ 走同一套多源链路下载
pub fn install_whisper_custom(ctx: &Ctx, spec: &str) -> anyhow::Result<ToolStatus> {
    let raw = spec.trim();
    if raw.is_empty() {
        anyhow::bail!("请输入模型链接或本地文件路径");
    }
    ctx.dirs.ensure();

    // 1) GitHub 直链：单独一条链路（GitHub 与 HF 的可达性互不相干，镜像硬套过去两头不讨好）
    if crate::github::is_github_url(raw) {
        return install_custom_from_github(ctx, raw);
    }
    let lower = raw.to_lowercase();

    // 2) 本地文件
    let as_path = Path::new(raw);
    if !lower.starts_with("http") && as_path.is_file() {
        let file = safe_model_file(raw);
        let dest = ctx.dirs.models.join(&file);
        if dest != as_path {
            std::fs::copy(as_path, &dest)?;
        }
        let size = verify_model_file(&dest).map_err(|msg| anyhow::anyhow!("[{}] {} :: {msg}", DlKind::Invalid.code(), file))?;
        emit_progress(ctx, "whisper-model", 100.0, "模型已导入");
        return Ok(model_status(&dest, &human_size(size), "custom", None));
    }

    // 1) / 3) 远程下载：显式链接只用它自己，短名 / 文件名走多源回退
    let file = safe_model_file(raw);
    let urls = if lower.starts_with("http") {
        vec![raw.to_string()]
    } else {
        model_source_urls(ctx, &file)
    };
    let dest = ctx.dirs.models.join(&file);
    if model_ready(&dest) {
        let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        return Ok(model_status(&dest, &human_size(size), "custom", None));
    }
    emit_progress(
        ctx,
        "whisper-model",
        0.0,
        &format!("{} · {}", host_of(&urls[0]), file),
    );
    let out = match download_with_fallback(ctx, "whisper-model", &urls, &dest) {
        Ok(o) => o,
        Err(e) => {
            dlog(ctx, "whisper-model", &format!("自定义模型 {file} 下载失败：{}", e.message()));
            return Err(e.into_anyhow());
        }
    };
    dlog(
        ctx,
        "whisper-model",
        &format!("自定义模型 {file} 下载完成：{} 字节 ← {}", out.bytes, out.source),
    );
    let size = verify_model_file(&dest)
        .map_err(|msg| anyhow::anyhow!("[{}] {} :: {msg}", DlKind::Invalid.code(), out.source))?;
    emit_progress(ctx, "whisper-model", 100.0, "模型就绪");
    Ok(model_status(&dest, &human_size(size), "custom", Some(&out.source)))
}

/// GitHub 直链安装（自定义模型入口）：
///   下载到暂存 → 校验 ggml 魔数 → 挪进模型目录；校验不过就删掉暂存、如实报错。
///   覆盖四种写法：`releases/download/<tag>/<file>`、`blob/<ref>/<path>`（自动转 raw）、
///   `raw.githubusercontent.com/...`、以及带 `?raw=true` 的下载按钮地址。
///   主地址不可达时按 `download_urls()` 的顺序回退（jsDelivr / gh-proxy），但下载的始终是同一个文件。
pub fn install_custom_from_github(ctx: &Ctx, raw: &str) -> anyhow::Result<ToolStatus> {
    let gh = crate::github::parse_github_url(raw).map_err(|e| {
        anyhow::anyhow!(
            "[{}] {} :: GitHub 链接解析失败：{e}",
            DlKind::Invalid.code(),
            raw
        )
    })?;
    let file = safe_model_file(&gh.file);
    let dest = ctx.dirs.models.join(&file);
    dlog(
        ctx,
        "whisper-model",
        &format!(
            "GitHub 直链安装（{}）：{} → {file}",
            gh.kind.as_str(),
            gh.url
        ),
    );
    if model_ready(&dest) {
        let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        return Ok(model_status(&dest, &human_size(size), "custom", None));
    }
    let stage = ctx.dirs.cache.join(format!("gh-{file}"));
    let _ = std::fs::remove_file(&stage);
    emit_progress(ctx, "whisper-model", 0.0, &format!("github · {file}"));
    let urls = gh.download_urls();
    let out = match download_with_fallback(ctx, "whisper-model", &urls, &stage) {
        Ok(o) => o,
        Err(e) => {
            let _ = std::fs::remove_file(&stage);
            dlog(
                ctx,
                "whisper-model",
                &format!("GitHub 下载失败：{}", e.message()),
            );
            return Err(e.into_anyhow());
        }
    };
    let size = verify_model_file(&stage).map_err(|msg| {
        let _ = std::fs::remove_file(&stage);
        anyhow::anyhow!(
            "[{}] {} :: {msg}。GitHub 的 /blob/ 页面链接给的是网页而不是文件，请用 Release 资产地址、\
             raw.githubusercontent.com 直链，或页面上「Download」按钮给的 ?raw=true 地址",
            DlKind::Invalid.code(),
            out.source
        )
    })?;
    if dest.is_file() {
        let _ = std::fs::remove_file(&dest);
    }
    if std::fs::rename(&stage, &dest).is_err() {
        // 跨卷或占位时退回复制
        std::fs::copy(&stage, &dest)?;
        let _ = std::fs::remove_file(&stage);
    }
    dlog(
        ctx,
        "whisper-model",
        &format!("GitHub 直链安装完成：{file} {size} 字节 ← {}", out.source),
    );
    emit_progress(ctx, "whisper-model", 100.0, "模型就绪");
    Ok(model_status(&dest, &human_size(size), "custom", Some(&out.source)))
}

/// 删除模型文件（含下载残留）
pub fn delete_whisper_model(ctx: &Ctx, name: &str) -> anyhow::Result<()> {
    let file = model_file_name(name);
    let p = ctx.dirs.models.join(&file);
    if p.is_file() {
        std::fs::remove_file(&p)?;
    }
    // 残片路径与 fetch_once 完全一致（含多点文件名，如 ggml-tiny.en.bin）
    let _ = std::fs::remove_file(partial_path(&p));
    Ok(())
}

/// 受管工具目录（1.9）：路径解析 / 校验 / 迁移计划 / 迁移执行 / 勾选集
#[cfg(test)]
mod tooldir_tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("umi-tooldir-{tag}-{}", crate::ctx::short_id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, bytes: usize) {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(p, vec![7u8; bytes]).unwrap();
    }

    /// 工具根目录推导：唯一来源是设置里的 tool_dir，空 / 相对路径一律回落默认目录
    #[test]
    fn effective_dir_only_trusts_absolute_configured_value() {
        let def = PathBuf::from("C:/data/umi-downloader/bin");
        assert_eq!(effective_tool_dir(&def, ""), def, "空配置 = 默认目录");
        assert_eq!(effective_tool_dir(&def, "   "), def, "只有空白 = 默认目录");
        assert_eq!(effective_tool_dir(&def, "tools/bin"), def, "相对路径不可信，回落默认");
        assert_eq!(
            effective_tool_dir(&def, "  C:/Umidl/tools  "),
            PathBuf::from("C:/Umidl/tools"),
            "绝对路径按原值使用（首尾空白剔除）"
        );
        assert_eq!(
            effective_tool_dir(&def, r"D:\Umidl\tools"),
            PathBuf::from(r"D:\Umidl\tools"),
            "Windows 盘符写法也是绝对路径"
        );
        assert_eq!(
            is_absolute_path("/opt/umidl/tools"),
            cfg!(unix),
            "POSIX 风格路径只在类 Unix 上算绝对路径（Windows 上必须带盘符/UNC）"
        );
        assert!(is_absolute_path("C:/x"));
        assert!(!is_absolute_path("bin"));
        assert!(!is_absolute_path(""));
    }

    /// 目录校验：空 / 非法字符 / 相对路径 / 指向文件 / 建不出来，都要给出准确错误码
    #[test]
    fn validate_dir_reports_precise_errors() {
        let base = tmp_dir("validate");

        let e = validate_tool_dir("   ").unwrap_err();
        assert!(e.starts_with("[tools.dir.empty]"), "{e}");

        let e = validate_tool_dir("relative/tools").unwrap_err();
        assert!(e.starts_with("[tools.dir.relative]"), "{e}");

        let e = validate_tool_dir("C:/bad|name").unwrap_err();
        assert!(e.starts_with("[tools.dir.invalid]"), "{e}");

        // 指向一个已存在的文件
        let file = base.join("a-file");
        std::fs::write(&file, b"x").unwrap();
        let e = validate_tool_dir(&file.to_string_lossy()).unwrap_err();
        assert!(e.starts_with("[tools.dir.not_dir]"), "{e}");

        // 目录建不出来（父路径是文件）
        let blocked = file.join("sub");
        let e = validate_tool_dir(&blocked.to_string_lossy()).unwrap_err();
        assert!(e.starts_with("[tools.dir.create_failed]"), "{e}");

        // 正常目录：建得出来也写得进去
        let ok = base.join("tools");
        assert_eq!(validate_tool_dir(&ok.to_string_lossy()).unwrap(), ok);
        assert!(ok.is_dir());
        // 探测文件不能留下垃圾
        assert_eq!(std::fs::read_dir(&ok).unwrap().count(), 0);

        std::fs::remove_dir_all(&base).ok();
    }

    /// 受管条目标签：whisper 整目录（含 DLL / 旧版 main.exe），模型不参与迁移
    #[test]
    fn managed_entries_cover_subdirs_and_skip_model() {
        let w = managed_entries("whisper");
        assert!(w.iter().any(|(r, k)| r == "whisper" && *k == EntryKind::Dir), "{w:?}");
        assert!(migratable("yt-dlp") && migratable("poppler") && migratable("imagemagick"));
        assert!(!migratable("whisper-model"), "模型在模型目录，不随工具目录迁移");
        assert!(managed_entries("whisper-model").is_empty());
        assert_eq!(managed_entries("yt-dlp")[0].0, exe_name("yt-dlp"));
    }

    /// 迁移计划：已知条目 + 额外条目都要在计划里，残片不搬
    #[test]
    fn migration_plan_lists_known_and_extra_entries() {
        let root = tmp_dir("plan");
        let from = root.join("old");
        let to = root.join("new");
        write(&from.join(exe_name("yt-dlp")), 100);
        write(&from.join("whisper").join(exe_name("whisper-cli")), 200);
        write(&from.join("whisper").join("ggml.dll"), 50);
        write(&from.join("notes.txt"), 10);
        write(&from.join("yt-dlp.downloading"), 30);

        let plan = migration_plan(&from, &to);
        let rels: Vec<String> = plan.entries.iter().map(|e| e.rel.clone()).collect();
        assert!(rels.contains(&exe_name("yt-dlp")), "{rels:?}");
        assert!(rels.contains(&"whisper".to_string()), "整目录一起搬：{rels:?}");
        assert!(rels.contains(&"notes.txt".to_string()), "额外文件不能丢：{rels:?}");
        assert_eq!(plan.skipped, vec!["yt-dlp.downloading".to_string()], "残片不搬");
        assert!(!rels.contains(&"yt-dlp.downloading".to_string()));
        // whisper 目录 = 200 + 50
        let w = plan.entries.iter().find(|e| e.rel == "whisper").unwrap();
        assert_eq!(w.bytes, 250);
        assert_eq!(w.kind, "dir");
        assert_eq!(plan.bytes, 100 + 250 + 10);
        std::fs::remove_dir_all(&root).ok();
    }

    /// 迁移执行：先复制 + 校验，成功才删旧；旧目录里的工具不会丢
    #[test]
    fn migrate_moves_tools_and_removes_old_only_after_verify() {
        let root = tmp_dir("migrate");
        let from = root.join("old");
        let to = root.join("new");
        write(&from.join(exe_name("yt-dlp")), 100);
        write(&from.join("whisper").join(exe_name("whisper-cli")), 200);
        write(&from.join("extra.txt"), 20);

        let rep = migrate_tool_dir(&from, &to);
        assert!(rep.ok, "{:?}", rep.errors);
        assert_eq!(rep.files_copied, 3);
        assert!(rep.bytes_copied >= 320);
        // 新目录里文件齐全且体积一致
        assert_eq!(dir_size(&to.join(exe_name("yt-dlp"))), 100);
        assert_eq!(dir_size(&to.join("whisper")), 200);
        assert_eq!(dir_size(&to.join("extra.txt")), 20);
        // 旧目录里的受管条目已经删掉（额外文件也一起搬走）
        assert!(!from.join(exe_name("yt-dlp")).exists());
        assert!(!from.join("whisper").exists());
        assert!(!from.join("extra.txt").exists());

        // 再搬一次（幂等）：已存在的不重复复制，也不会报错
        write(&from.join(exe_name("yt-dlp")), 100);
        let rep2 = migrate_tool_dir(&from, &to);
        assert!(rep2.ok, "{:?}", rep2.errors);
        assert_eq!(rep2.already_present, 1);
        assert_eq!(rep2.files_copied, 0);
        std::fs::remove_dir_all(&root).ok();
    }

    /// 迁移失败（目标是文件 / 目录互相嵌套）：不删旧目录、不报成功
    #[test]
    fn migrate_failure_keeps_old_dir_intact() {
        let root = tmp_dir("fail");
        let from = root.join("old");
        write(&from.join(exe_name("yt-dlp")), 100);

        let file = root.join("not-a-dir");
        std::fs::write(&file, b"x").unwrap();
        let rep = migrate_tool_dir(&from, &file);
        assert!(!rep.ok);
        assert_eq!(rep.files_copied, 0);
        assert!(rep.errors[0].starts_with("[tools.dir.not_dir]"), "{:?}", rep.errors);
        assert!(from.join(exe_name("yt-dlp")).is_file(), "旧目录必须原样保留");

        // 新目录在旧目录内部：直接拒绝（否则复制与删除会互相踩）
        let nested = from.join("inside");
        let rep2 = migrate_tool_dir(&from, &nested);
        assert!(!rep2.ok);
        assert!(rep2.errors[0].starts_with("[tools.dir.nested]"), "{:?}", rep2.errors);
        assert!(from.join(exe_name("yt-dlp")).is_file());
        std::fs::remove_dir_all(&root).ok();
    }

    /// 勾选集规范化：去空白 / 去重 / 保序 / 丢空名（未知名字保留，由装的时候逐条报错）
    #[test]
    fn selection_is_normalized() {
        let got = normalize_selection(&[
            " ffmpeg ".into(),
            "yt-dlp".into(),
            "ffmpeg".into(),
            "".into(),
            "  ".into(),
            "who-knows".into(),
        ]);
        assert_eq!(got, vec!["ffmpeg".to_string(), "yt-dlp".into(), "who-knows".into()]);
    }
}

#[cfg(test)]
mod fix18_tests {
    use super::*;

    fn models_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("umi-tools-{tag}-{}", crate::ctx::short_id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 造一个「已安装」的模型文件（体积判定阈值 1 MB）
    fn install(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), vec![0u8; 1_100_000]).unwrap();
    }

    /// BUG-15：配置的模型没下载时要自动回落，本机只有 tiny 也能开箱即用
    #[test]
    fn effective_model_falls_back_to_installed_one() {
        let dir = models_dir("eff");
        // 什么都没装：保持用户配置，让上层报「缺模型」的精确错误
        assert_eq!(effective_model_for(&dir, "base"), ("base".to_string(), false));
        // 只装了 tiny → 配置 base 也要能跑，回落到 tiny
        install(&dir, "ggml-tiny.bin");
        assert_eq!(effective_model_for(&dir, "base"), ("tiny".to_string(), true));
        assert_eq!(
            effective_model_for(&dir, ""),
            ("tiny".to_string(), true),
            "空配置（默认 base）同样要回落"
        );
        // 配置的模型就在 → 不再回落
        assert_eq!(effective_model_for(&dir, "tiny"), ("tiny".to_string(), false));
        assert_eq!(
            effective_model_for(&dir, "ggml-tiny.bin"),
            ("ggml-tiny.bin".to_string(), false),
            "自定义文件名写法也要认"
        );
        // 装了多个内置模型：按体积升序回落（tiny 优先）
        install(&dir, "ggml-small.bin");
        assert_eq!(effective_model_for(&dir, "medium"), ("tiny".to_string(), true));
        // 只有自定义模型时也能回落
        let dir2 = models_dir("eff-custom");
        install(&dir2, "ggml-mine.bin");
        assert_eq!(effective_model_for(&dir2, "base"), ("ggml-mine.bin".to_string(), true));
    }

    /// BUG-15：半截的下载残片（体积过小）不算「已安装」
    #[test]
    fn tiny_partial_download_is_not_ready() {
        let dir = models_dir("partial");
        std::fs::write(dir.join("ggml-tiny.bin"), vec![0u8; 4096]).unwrap();
        assert!(!model_ready(&dir.join("ggml-tiny.bin")));
        assert!(installed_models_in(&dir).is_empty());
        assert_eq!(effective_model_for(&dir, "tiny"), ("tiny".to_string(), false));
    }

    /// BUG-15：已安装清单 = 内置短名 + 自定义文件名，且不含残片
    #[test]
    fn installed_list_covers_builtin_and_custom() {
        let dir = models_dir("installed");
        install(&dir, "ggml-tiny.bin");
        install(&dir, "ggml-base.bin");
        install(&dir, "ggml-mine.bin");
        std::fs::write(dir.join("ggml-broken.bin"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        assert_eq!(
            installed_models_in(&dir),
            vec!["tiny".to_string(), "base".to_string(), "ggml-mine.bin".to_string()]
        );
    }

    /// BUG-15：模型缺失的报错要能行动 —— 说清缺哪个、期望文件在哪、已装哪些、去哪儿装
    #[test]
    fn missing_model_message_is_actionable() {
        let dir = models_dir("msg");
        let expected = dir.join("ggml-base.bin");
        let msg = missing_model_message(&dir, "base", &expected);
        assert!(msg.contains("缺少 Whisper 模型「base」"), "{msg}");
        assert!(msg.contains(&expected.to_string_lossy().to_string()), "要点明期望文件：{msg}");
        assert!(msg.contains("当前已安装：无"), "{msg}");
        assert!(msg.contains("设置 → 依赖工具"), "要指路去哪儿装：{msg}");

        install(&dir, "ggml-tiny.bin");
        let msg2 = missing_model_message(&dir, "base", &expected);
        assert!(msg2.contains("当前已安装：tiny"), "{msg2}");
        assert!(msg2.contains("切换成已安装的模型（tiny）"), "要给出替代方案：{msg2}");
    }

    /// BUG-15：空配置按默认模型 base 处理（与设置里的默认值一致）
    #[test]
    fn empty_model_name_defaults_to_base() {
        let dir = models_dir("empty");
        assert_eq!(model_file_name("   "), "ggml-base.bin");
        assert!(missing_model_message(&dir, "", &dir.join("ggml-base.bin")).contains("「base」"));
    }
}

/// 模型下载修复：源回退顺序 / 错误分类 / 断点续传命名 / 模型校验
#[cfg(test)]
mod dlmirror_tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("umi-dl-{tag}-{}", crate::ctx::short_id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 下载源顺序：直连（国内默认）镜像源优先，走代理时官方源优先；两者都含完整仓库路径
    #[test]
    fn source_order_follows_network_path() {
        let direct = model_source_urls_with("ggml-base.bin", false, None);
        assert_eq!(direct.len(), 2, "必须有两个源可选：{direct:?}");
        assert!(direct[0].starts_with(HF_MIRROR), "直连先试镜像源：{direct:?}");
        assert!(direct[1].starts_with(HF_OFFICIAL), "官方源兜底：{direct:?}");
        for u in &direct {
            assert!(u.contains("ggerganov/whisper.cpp/resolve/main/ggml-base.bin"), "{u}");
        }

        let proxied = model_source_urls_with("ggml-base.bin", true, None);
        assert!(proxied[0].starts_with(HF_OFFICIAL), "有代理时官方源优先：{proxied:?}");
        assert!(proxied[1].starts_with(HF_MIRROR), "{proxied:?}");
    }

    /// HF_ENDPOINT 覆盖：自建镜像只走它一个源（带不带尾斜杠都要拼对）
    #[test]
    fn hf_endpoint_override_replaces_sources() {
        let urls = model_source_urls_with("ggml-small.bin", false, Some("https://my.mirror/hf"));
        assert_eq!(urls, vec!["https://my.mirror/hf/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"]);
        let trailing = model_source_urls_with("ggml-small.bin", true, Some("https://my.mirror/hf/"));
        assert_eq!(
            trailing,
            vec!["https://my.mirror/hf/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"],
            "尾斜杠不能拼出双斜杠：{trailing:?}"
        );
        let blank = model_source_urls_with("ggml-small.bin", false, Some("   "));
        assert_eq!(blank.len(), 2, "空白端点视为未设置：{blank:?}");
    }

    /// 主机名提取：进度文案与错误分类都要显示「是哪个源」
    #[test]
    fn host_extraction() {
        assert_eq!(host_of("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin"), "huggingface.co");
        assert_eq!(host_of("https://hf-mirror.com/a.bin?x=1"), "hf-mirror.com");
        assert_eq!(host_of("http://127.0.0.1:8080/a.bin"), "127.0.0.1:8080");
        assert_eq!(host_of(""), "");
    }

    /// 残片命名：与删除逻辑一致（多点文件名也不能错位）
    #[test]
    fn partial_file_name_matches_delete() {
        assert_eq!(partial_path(Path::new("C:/m/ggml-base.bin")), PathBuf::from("C:/m/ggml-base.downloading"));
        assert_eq!(
            partial_path(Path::new("C:/m/ggml-tiny.en.bin")),
            PathBuf::from("C:/m/ggml-tiny.en.downloading"),
            "老版 delete 用 <file>.downloading 清不掉这种残片"
        );
        assert_eq!(partial_bytes(Path::new("C:/m/不存在.downloading")), 0, "没有残片就是 0 起点");
    }

    /// 错误分类：DNS 污染 / 超时 / 拒连 / 代理 / TLS 各自归位（前端的文案按这个分类走）
    #[test]
    fn error_classification() {
        assert_eq!(
            classify_text("error sending request: dns error: failed to lookup address information"),
            DlKind::Dns
        );
        assert_eq!(classify_text("tcp connect error: operation timed out"), DlKind::Timeout);
        assert_eq!(classify_text("tcp connect error: Connection refused (os error 10061)"), DlKind::Connect);
        assert_eq!(classify_text("proxy connect failed: tunnel error"), DlKind::Proxy);
        assert_eq!(classify_text("invalid peer certificate: UnknownIssuer"), DlKind::Tls);
        // 中文系统的 io 错误文案也要认（Windows 中文版是「远程主机强迫关闭了一个现有的连接」）
        assert_eq!(classify_text("远程主机强迫关闭了一个现有的连接"), DlKind::Connect);
        assert_eq!(classify_text("由于目标计算机积极拒绝，无法连接"), DlKind::Connect);
        assert_eq!(classify_text("连接超时（操作超时）"), DlKind::Timeout);
        assert_eq!(classify_text("不知道这样的主机"), DlKind::Dns);
        // 认不出来就归到 Unknown（原文透传），不能瞎猜成磁盘问题
        assert_eq!(classify_text("something nobody has seen before"), DlKind::Unknown);
        assert_eq!(DlKind::Unknown.code(), "model.unknown");
        // DNS 优先级要高于 timeout：DNS 失败的信息里常同时出现 timeout 字眼
        assert_eq!(
            classify_text("dns error: failed to lookup address after timeout"),
            DlKind::Dns
        );
    }

    /// HTTP 状态码分类：416 单独一类（续传位置失效），其余按码原样带出给前端
    #[test]
    fn status_classification() {
        assert_eq!(classify_status(416), DlKind::Range);
        assert_eq!(classify_status(403), DlKind::Http(403));
        assert_eq!(classify_status(404), DlKind::Http(404));
        assert_eq!(classify_status(503), DlKind::Http(503));
        assert_eq!(DlKind::Http(403).code(), "model.http403");
        assert_eq!(DlKind::Http(503).code(), "model.http503");
        assert_eq!(DlKind::Range.code(), "model.range");
        assert_eq!(DlKind::Truncated.code(), "model.truncated");
        assert_eq!(DlKind::Sources.code(), "model.sources");
    }

    /// 重试策略：只对「可能自愈」的失败重试，DNS/连不上/4xx 立刻换下一个源（不白等）
    #[test]
    fn retry_policy() {
        assert!(DlKind::Io.worth_retry(), "连接被掐断值得续传重试");
        assert!(DlKind::Truncated.worth_retry(), "传到一半断了要能续传重试");
        assert!(DlKind::Range.worth_retry(), "416 清了残片可以再来一次");
        assert!(DlKind::Http(500).worth_retry());
        assert!(DlKind::Http(502).worth_retry());
        assert!(!DlKind::Http(403).worth_retry());
        assert!(!DlKind::Http(404).worth_retry());
        assert!(!DlKind::Dns.worth_retry(), "DNS 污染重试一百次也一样");
        assert!(!DlKind::Timeout.worth_retry());
        assert!(!DlKind::Connect.worth_retry());
        assert!(!DlKind::Proxy.worth_retry());
        assert!(!DlKind::Tls.worth_retry());
        assert!(!DlKind::Unknown.worth_retry(), "认不出来就别硬重试");
    }

    /// 错误报文契约：`[<code>] <host> :: <detail>`，前端按这个格式取 code 做四语言本地化
    #[test]
    fn error_message_contract() {
        let e = DlErr::new(DlKind::Dns, "hf-mirror.com、huggingface.co", "dns error ← failed to lookup");
        let msg = e.message();
        assert!(msg.starts_with("[model.dns] "), "{msg}");
        assert!(msg.contains(" :: "), "{msg}");
        // 与 src/services/backendError.ts 的解析规则保持一致
        let rest = msg.trim_start_matches('[');
        let (code, tail) = rest.split_once(']').expect("要有 ] 结束错误码");
        assert_eq!(code, "model.dns");
        let (host, detail) = tail.split_once(" :: ").expect("要有 :: 分隔主机与详情");
        assert_eq!(host.trim(), "hf-mirror.com、huggingface.co");
        assert!(detail.contains("dns error"));
        assert_eq!(e.clone().into_anyhow().to_string(), msg, "转 anyhow 后文案不变");
        assert_eq!(format!("{e}"), msg);
    }

    /// 模型校验：魔数不对（HTML 错误页）或体积太小都要拦下来，不能留在模型目录里冒充可用模型
    #[test]
    fn model_file_verification() {
        let dir = tmp_dir("verify");

        // 正常模型：ggml 魔数 + 足够体积
        let ok = dir.join("ggml-good.bin");
        let mut body = GGML_MAGIC.to_vec();
        body.resize(1_100_000, 0);
        std::fs::write(&ok, &body).unwrap();
        assert_eq!(verify_model_file(&ok).unwrap(), 1_100_000);

        // HTML 错误页（体积不够）
        let html = dir.join("ggml-html.bin");
        std::fs::write(&html, b"<!DOCTYPE html><html>403</html>").unwrap();
        let err = verify_model_file(&html).unwrap_err();
        assert!(err.contains("字节"), "{err}");

        // 体积够但头不对（比如被换成压缩包）
        let bad = dir.join("ggml-bad.bin");
        let mut body2 = b"PK\x03\x04".to_vec();
        body2.resize(1_100_000, 0);
        std::fs::write(&bad, &body2).unwrap();
        let err2 = verify_model_file(&bad).unwrap_err();
        assert!(err2.contains("魔数"), "{err2}");

        // 文件不存在
        assert!(verify_model_file(&dir.join("nope.bin")).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 换源时进度条不能被清零（残片要留着继续用）
    #[test]
    fn partial_bytes_survives_source_switch() {
        let dir = tmp_dir("partial");
        let dest = dir.join("ggml-base.bin");
        let tmp = partial_path(&dest);
        std::fs::write(&tmp, vec![0u8; 4_000_000]).unwrap();
        assert_eq!(partial_bytes(&tmp), 4_000_000);
        // 没有 .part 明文落盘、也不该把残片当成品
        assert!(!dest.exists());
        assert!(!model_ready(&dest));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 残片保活窗口：太旧的残片不敢拿来拼接（上游内容可能已经变了）
    #[test]
    fn resume_window_expires_stale_partials() {
        assert_eq!(resume_start(0, 10), 0, "空残片没有续传起点");
        assert_eq!(resume_start(4_000_000, 0), 4_000_000);
        assert_eq!(resume_start(4_000_000, RESUME_MAX_AGE_SECS), 4_000_000, "窗口内可续");
        assert_eq!(resume_start(4_000_000, RESUME_MAX_AGE_SECS + 1), 0, "过期残片丢弃重下");
    }

    /// resume_from 走真实文件：新残片可续，过期残片归零且被清理
    #[test]
    fn resume_from_uses_only_fresh_partials() {
        let dir = tmp_dir("resume");
        let p = dir.join("ggml-base.downloading");
        std::fs::write(&p, vec![0u8; 1024]).unwrap();
        assert_eq!(resume_from(&p), 1024, "刚写的残片要能续");
        assert_eq!(resume_from(&dir.join("缺失.downloading")), 0, "没残片就从 0 开始");

        let old = std::time::SystemTime::now()
            - std::time::Duration::from_secs(RESUME_MAX_AGE_SECS + 600);
        std::fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(resume_from(&p), 0, "过期残片不能拿来续");
        assert!(!p.exists(), "过期残片要顺手清掉，别让用户以为还能接着下");
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// 探活 / 版本 / 安装就绪判定（C2 + C4 + poppler 版本显示）
#[cfg(test)]
mod probe_tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("umi-probe-{tag}-{}", crate::ctx::short_id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn ctx_with_bin(bin: PathBuf) -> Ctx {
        let data = bin.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| bin.clone());
        let dirs = crate::ctx::AppDirs {
            data: data.clone(),
            bin,
            models: data.join("models"),
            cache: data.join("cache"),
            downloads: data.join("downloads"),
            db_file: data.join("umi.db"),
            settings_file: data.join("settings.json"),
        };
        Ctx::new(
            dirs,
            crate::ctx::ToolPaths {
                ytdlp: None,
                ffmpeg: None,
                ffprobe: None,
                whisper: None,
                aria2: None,
                pandoc: None,
                poppler: None,
                emule: None,
            },
            crate::models::AppSettings::default(),
        )
    }

    fn touch(p: &Path) {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(p, b"stub").unwrap();
    }

    /* ---------- C4：探活方式 ---------- */

    /// 只有 GUI 程序（eMule）不启动探测；其余工具仍要启动一次拿版本号
    #[test]
    fn only_gui_tools_are_exists_only() {
        assert_eq!(probe_kind("emule"), ProbeKind::ExistsOnly, "emule 是 GUI 程序，启动探测必然卡死");
        assert_eq!(probe_kind_str("emule"), "exists");
        for name in TOOL_ORDER {
            if name == "emule" {
                continue;
            }
            assert_eq!(probe_kind(name), ProbeKind::Exec, "{name} 仍应启动式探活");
            assert_eq!(probe_kind_str(name), "exec", "{name}");
        }
    }

    /// GUI 工具的探活：只核对文件存在，**不启动**进程 ——
    /// 用「非可执行文件」当输入就能证明这一点（真去 exec 会失败，返回 false）
    #[test]
    fn exists_only_probe_never_executes() {
        let dir = tmp_dir("exists-only");
        let fake = dir.join("emule-not-a-program.txt");
        std::fs::write(&fake, b"not an executable").unwrap();

        let start = std::time::Instant::now();
        assert_eq!(run_probe_with(&fake, "emule", Duration::from_millis(300)), (true, None));
        assert!(start.elapsed() < Duration::from_secs(2), "只做文件核对，应当立刻返回");

        // 对照组：同一个文件按「启动式探活」处理 → spawn 失败 → 不认为可用
        assert_eq!(run_probe_with(&fake, "pandoc", Duration::from_millis(300)), (false, None));
        // 文件不存在 → 明确不可用
        assert_eq!(run_probe_with(&dir.join("nope.exe"), "emule", Duration::from_millis(300)), (false, None));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 会卡住的程序必须被超时强制结束（C4 的根因：GUI / 卡死程序让刷新永不返回）
    #[test]
    fn run_capture_kills_hung_process_on_timeout() {
        #[cfg(windows)]
        let mut cmd = {
            let mut c = std::process::Command::new("ping");
            c.args(["-n", "6", "127.0.0.1"]); // 约 5 秒
            c
        };
        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", "sleep 5"]);
            c
        };
        let start = std::time::Instant::now();
        let cap = run_capture(&mut cmd, Duration::from_millis(400));
        let elapsed = start.elapsed();
        assert!(cap.timed_out, "5 秒的进程在 400ms 超时下必须被判为超时");
        assert!(!cap.ok, "超时的进程不算「正常退出」");
        assert!(elapsed < Duration::from_secs(3), "超时后要立刻返回，实际 {elapsed:?}");
    }

    /// 正常命令：拿到退出码与输出，不误判超时
    #[test]
    fn run_capture_collects_output_for_quick_command() {
        #[cfg(windows)]
        let mut cmd = {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "echo probe-ok"]);
            c
        };
        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", "echo probe-ok"]);
            c
        };
        let cap = run_capture(&mut cmd, Duration::from_secs(10));
        assert!(!cap.timed_out, "正常命令不该被判超时");
        assert!(cap.ok, "退出码 0");
        assert!(cap.stdout.contains("probe-ok"), "要拿到输出：{:?}", cap.stdout);
    }

    /// 时间预算：跑久了就不再启动新的探活进程（保证刷新有限时间内返回）
    #[test]
    fn probe_budget_expires() {
        let now = std::time::Instant::now();
        assert!(within_probe_budget(now, Duration::from_secs(20)));
        assert!(!within_probe_budget(now - Duration::from_secs(30), Duration::from_secs(20)));
    }

    /// 探活缓存必须换过名字：旧文件里存着错误结论（poppler 的 "No"、emule 的 ok=false），
    /// 而缓存键含 mtime、工具目录迁移又保留 mtime → 不换名字就会一直命中脏数据
    #[test]
    fn probe_cache_file_bumped_past_legacy() {
        assert_eq!(PROBE_CACHE_FILE, "tool_cache_v3.json", "探活/解析规则变更要换缓存文件名");
        assert_ne!(PROBE_CACHE_FILE, "tool_cache_v2.json");
        assert_ne!(PROBE_CACHE_FILE, "tool_cache.json");
    }

    /// 下载源日志行：直连 / 经代理 必须写清楚（用户导出的日志要自洽）
    #[test]
    fn proxy_note_says_direct_or_proxied() {
        assert_eq!(proxy_note(None), "直连下载");
        assert_eq!(proxy_note(Some("   ")), "直连下载", "空白代理 = 直连");
        assert_eq!(proxy_note(Some(" http://127.0.0.1:7892 ")), "经代理 http://127.0.0.1:7892 下载");
        assert_eq!(proxy_note(Some("socks5://127.0.0.1:1080")), "经代理 socks5://127.0.0.1:1080 下载");
    }

    /* ---------- 版本号解析（poppler 显示 "No" 的根因） ---------- */

    /// 真机输出行当输入：poppler 必须解析出真版本号，错误行绝不能变成 "No"
    #[test]
    fn version_candidate_reads_real_lines() {
        // pdftotext -v（真机输出，版本行在 stderr）
        assert_eq!(
            version_candidate("poppler", "pdftotext version 26.09.0").as_deref(),
            Some("26.09.0")
        );
        // pdftotext --version（poppler 不认这个参数，把它当文件名）——
        // 旧解析器在这一行取到 "version" 后面的 "No"，界面上就显示成「已装 · No」
        assert_eq!(
            version_candidate("poppler", "I/O Error: Couldn't open file '--version': No error."),
            None,
            "错误行不能变出版本号"
        );
        assert!(!looks_like_version("No"));
        assert!(looks_like_version("26.09.0"));

        assert_eq!(version_candidate("pandoc", "pandoc 3.11").as_deref(), Some("3.11"));
        assert_eq!(
            version_candidate("ffmpeg", "ffmpeg version 8.0.1 Copyright (c) 2000-2025 the FFmpeg developers")
                .as_deref(),
            Some("8.0.1")
        );
        assert_eq!(version_candidate("aria2", "aria2 version 1.37.0").as_deref(), Some("1.37.0"));
        assert_eq!(
            version_candidate("imagemagick", "Version: ImageMagick 7.1.2-31 Q16-HDRI x64 26111").as_deref(),
            Some("7.1.2-31")
        );
        assert_eq!(version_candidate("yt-dlp", "2025.09.26").as_deref(), Some("2025.09.26"));
        // 乱码行 / 空行不产出假版本号
        assert_eq!(version_candidate("poppler", ""), None);
        assert_eq!(version_candidate("poppler", "无法连接。 (os error 10061)"), None);
    }

    /// 严格版本号判定：两段以上纯数字，挡住 "10061)" / "No" 这类噪声
    #[test]
    fn dotted_version_is_strict() {
        for ok in ["3.11", "26.09.0", "v1.9.4", "2025.09.26", "1.37.0-beta1"] {
            assert!(is_dotted_version(ok), "{ok} 应当是版本号");
        }
        for bad in ["No", "10061)", "version", "'--version':", "8.0.1,", ""] {
            assert!(!is_dotted_version(bad), "{bad} 不该被当成版本号");
        }
    }

    /* ---------- C2：安装后按受管候选判定就绪 ---------- */

    /// 受管候选表覆盖全部工具（含各自子目录 / 可执行名差异）：
    /// aria2 平铺、poppler 在 poppler/ 子目录里 —— 这正是不再「装完必报失败」的关键
    #[test]
    fn managed_candidates_cover_every_tool() {
        let dir = tmp_dir("candidates");
        let ctx = ctx_with_bin(dir.join("bin"));

        assert_eq!(
            managed_candidates(&ctx, "aria2"),
            vec![
                ctx.dirs.bin.join(exe_name("aria2c")),
                ctx.dirs.bin.join("aria2").join(exe_name("aria2c")),
            ],
            "aria2 平铺在 bin/ 下（安装的实际落点），另留一个子目录候选"
        );
        assert_eq!(
            managed_candidates(&ctx, "poppler"),
            vec![
                ctx.dirs.bin.join("poppler").join(exe_name("pdftotext")),
                ctx.dirs.bin.join(exe_name("pdftotext")),
            ]
        );
        assert!(managed_candidates(&ctx, "emule")[0]
            .to_string_lossy()
            .ends_with(&exe_name("emule")));
        assert_eq!(
            managed_candidates(&ctx, "emule")[0],
            ctx.dirs.bin.join("emule").join(exe_name("emule"))
        );
        assert_eq!(
            managed_candidates(&ctx, "whisper"),
            vec![
                ctx.dirs.bin.join("whisper").join(exe_name("whisper-cli")),
                ctx.dirs.bin.join("whisper").join(exe_name("main")),
                ctx.dirs.bin.join(exe_name("whisper-cli")),
            ],
            "whisper 在子目录里，且旧版可执行名是 main.exe"
        );

        // 每个可迁移工具都必须有候选位置（旧实现只映射 3 个工具 → 其余重装后必报「仍无法运行」）
        for name in TOOL_ORDER {
            if name == "whisper-model" {
                continue;
            }
            assert!(!managed_candidates(&ctx, name).is_empty(), "{name} 缺少受管候选位置");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 安装就绪判定：命中受管子目录布局即为成功；确实找不到才返回 None；probe 可注入
    #[test]
    fn pick_installed_uses_candidates_and_injected_probe() {
        let dir = tmp_dir("pick");
        let ctx = ctx_with_bin(dir.join("bin"));

        // 什么都没有 → None（才是真的「安装后仍无法运行」）
        let cands = managed_candidates(&ctx, "aria2");
        assert_eq!(pick_installed(&cands, |_| true), None, "文件不存在就不算装好");

        // aria2 平铺布局：bin/aria2c.exe
        touch(&ctx.dirs.bin.join(exe_name("aria2c")));
        assert_eq!(
            pick_installed(&managed_candidates(&ctx, "aria2"), |_| true),
            Some(ctx.dirs.bin.join(exe_name("aria2c")))
        );
        // 文件在但跑不起来 → 不算装好
        assert_eq!(pick_installed(&managed_candidates(&ctx, "aria2"), |_| false), None);

        // poppler 子目录布局：bin/poppler/pdftotext.exe
        let pdf = ctx.dirs.bin.join("poppler").join(exe_name("pdftotext"));
        touch(&pdf);
        assert_eq!(pick_installed(&managed_candidates(&ctx, "poppler"), |_| true), Some(pdf));

        // 第一个候选跑不起来时，回退到后面的候选
        let second = ctx.dirs.bin.join(exe_name("pdftotext"));
        touch(&second);
        let picked = pick_installed(&managed_candidates(&ctx, "poppler"), |p| p == second);
        assert_eq!(picked, Some(second), "前面的候选不可用时要继续往后找");

        std::fs::remove_dir_all(&dir).ok();
    }
}
