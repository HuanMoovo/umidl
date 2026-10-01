use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use regex::Regex;

use crate::ctx::{command_for, ensure_dir, kill_tree, Ctx};
use crate::models::{SubtitleRequest, SubtitleTask, TaskStatus};

/* ==================== 字幕数据结构 ==================== */

#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub index: usize,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

fn ts_to_ms(h: u64, m: u64, s: u64, ms: u64) -> u64 {
    ((h * 3600 + m * 60 + s) * 1000) + ms
}

pub fn ms_to_srt(ms: u64) -> String {
    let h = ms / 3_600_000;
    let m = (ms / 60_000) % 60;
    let s = (ms / 1000) % 60;
    let f = ms % 1000;
    format!("{h:02}:{m:02}:{s:02},{f:03}")
}

pub fn ms_to_vtt(ms: u64) -> String {
    ms_to_srt(ms).replace(',', ".")
}

pub fn ms_to_ass(ms: u64) -> String {
    let h = ms / 3_600_000;
    let m = (ms / 60_000) % 60;
    let s = (ms / 1000) % 60;
    let cs = (ms % 1000) / 10;
    format!("{h}:{m:02}:{s:02}.{cs:02}")
}

/// 解析 SRT 文本为 cue 列表（纯函数）
pub fn parse_srt(text: &str) -> Vec<Cue> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let ts_re = Regex::new(
        r"(\d{1,2}):(\d{2}):(\d{2})[,.](\d{1,3})\s*-->\s*(\d{1,2}):(\d{2}):(\d{2})[,.](\d{1,3})",
    )
    .unwrap();
    let mut cues: Vec<Cue> = Vec::new();
    for raw_block in normalized.split("\n\n") {
        let block = raw_block.trim_matches('\u{feff}').trim();
        if block.is_empty() {
            continue;
        }
        let lines: Vec<&str> = block.lines().filter(|l| !l.trim().is_empty()).collect();
        if lines.is_empty() {
            continue;
        }
        let (idx, ts_idx) = if lines[0].trim().parse::<usize>().is_ok() {
            (lines[0].trim().parse::<usize>().unwrap_or(cues.len() + 1), 1usize)
        } else {
            (cues.len() + 1, 0usize)
        };
        if ts_idx >= lines.len() {
            continue;
        }
        let ts_line = lines[ts_idx].trim();
        if let Some(c) = ts_re.captures(ts_line) {
            let get = |i: usize| c[i].parse::<u64>().unwrap_or(0);
            let ms_fix = |v: u64| if v < 10 { v * 100 } else if v < 100 { v * 10 } else { v };
            let start = ts_to_ms(get(1), get(2), get(3), ms_fix(get(4)));
            let end = ts_to_ms(get(5), get(6), get(7), ms_fix(get(8)));
            let body = lines[(ts_idx + 1)..].join("\n").trim().to_string();
            cues.push(Cue { index: idx, start_ms: start, end_ms: end, text: body });
        }
    }
    cues
}

pub fn to_srt(cues: &[Cue]) -> String {
    let mut out = String::new();
    for (i, c) in cues.iter().enumerate() {
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            ms_to_srt(c.start_ms),
            ms_to_srt(c.end_ms),
            c.text.trim()
        ));
    }
    out
}

pub fn to_vtt(cues: &[Cue]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for c in cues {
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            c.index,
            ms_to_vtt(c.start_ms),
            ms_to_vtt(c.end_ms),
            c.text.trim()
        ));
    }
    out
}

pub fn to_txt(cues: &[Cue]) -> String {
    cues.iter()
        .map(|c| c.text.replace('\n', " ").trim().to_string())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn to_json(cues: &[Cue]) -> String {
    let arr: Vec<serde_json::Value> = cues
        .iter()
        .map(|c| {
            serde_json::json!({
                "index": c.index,
                "start": ms_to_srt(c.start_ms),
                "end": ms_to_srt(c.end_ms),
                "start_ms": c.start_ms,
                "end_ms": c.end_ms,
                "text": c.text,
            })
        })
        .collect();
    serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".into())
}

pub fn to_ass(cues: &[Cue], title: &str) -> String {
    let mut out = String::new();
    out.push_str("[Script Info]\n");
    out.push_str(&format!("Title: {title}\n"));
    out.push_str("ScriptType: v4.00+\nWrapStyle: 0\nScaledBorderAndShadow: yes\n");
    out.push_str("PlayResX: 1920\nPlayResY: 1080\n\n");
    out.push_str("[V4+ Styles]\n");
    out.push_str("Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n");
    out.push_str("Style: Default,Microsoft YaHei,64,&H00FFFFFF,&H000000FF,&H00101010,&H80000000,0,0,0,0,100,100,0,0,1,3,1,2,60,60,60,1\n\n");
    out.push_str("[Events]\n");
    out.push_str("Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");
    for c in cues {
        let text = c.text.replace('\n', "\\N");
        out.push_str(&format!(
            "Dialogue: 0,{},{},Default,,0,0,0,,{}\n",
            ms_to_ass(c.start_ms),
            ms_to_ass(c.end_ms),
            text
        ));
    }
    out
}

/// 校验 SRT 结构：段数、时间轴单调、格式合法
pub fn validate_srt(text: &str) -> Result<usize, String> {
    if text.trim().is_empty() {
        return Err("字幕内容为空".into());
    }
    let cues = parse_srt(text);
    if cues.is_empty() {
        return Err("未解析到任何字幕条目（时间轴格式不合法）".into());
    }
    let mut last = 0u64;
    for (i, c) in cues.iter().enumerate() {
        if c.end_ms < c.start_ms {
            return Err(format!("第 {} 条字幕结束时间早于开始时间", i + 1));
        }
        if c.start_ms + 1 < last {
            return Err(format!("第 {} 条字幕时间轴未单调递增", i + 1));
        }
        if c.text.trim().is_empty() {
            return Err(format!("第 {} 条字幕文本为空", i + 1));
        }
        last = c.start_ms;
    }
    Ok(cues.len())
}

pub fn format_for_ext(ext: &str) -> String {
    match ext.to_lowercase().as_str() {
        "vtt" | "webvtt" => "vtt".into(),
        "ass" | "ssa" => "ass".into(),
        "json" => "json".into(),
        "txt" | "text" => "txt".into(),
        _ => "srt".into(),
    }
}

pub fn render(text: &str, target: &str, title: &str) -> String {
    let cues = parse_srt(text);
    match format_for_ext(target).as_str() {
        "vtt" => to_vtt(&cues),
        "ass" => to_ass(&cues, title),
        "txt" => to_txt(&cues),
        "json" => to_json(&cues),
        _ => to_srt(&cues),
    }
}

/* ==================== FFmpeg 抽音轨 ==================== */

pub struct StepOutcome {
    pub exit_code: i32,
    pub stderr_tail: String,
    pub killed: bool,
}

/// 「进度停滞」看门狗默认阈值：输出 5 分钟没有任何增长即判定卡死
/// （子进程 stderr 管道写满 64 KiB 后 ffmpeg 会永久阻塞在 write 上）
pub const EXTRACT_STALL_SECS: u64 = 300;
/// 绝对上限：即使输出一直在缓慢增长，也不允许超过 6 小时
pub const EXTRACT_MAX_SECS: u64 = 6 * 3600;
/// stderr 错误行保留条数
const EXTRACT_TAIL_LINES: usize = 30;

/// 停滞判定（纯函数，便于单测）
pub fn is_stalled(last_progress: Instant, now: Instant, limit: Duration) -> bool {
    now.saturating_duration_since(last_progress) > limit
}

/// 停滞阈值：可用环境变量 UMI_EXTRACT_STALL_SECS 覆盖（端到端验证用），下限 5 秒
pub fn stall_limit() -> Duration {
    let secs = std::env::var("UMI_EXTRACT_STALL_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(EXTRACT_STALL_SECS)
        .max(5);
    Duration::from_secs(secs)
}

fn file_size(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn push_err(errs: &mut Vec<String>, line: String) {
    if line.trim().is_empty() {
        return;
    }
    errs.push(line);
    if errs.len() > EXTRACT_TAIL_LINES {
        errs.remove(0);
    }
}

/// 提取 16kHz 单声道 PCM（Whisper 最佳输入格式）
///
/// 关键点（BUG-09）：stdout / stderr **必须各起一个 reader 线程实时排空**。
/// Windows 匿名管道只有 64 KiB，只 pipe 不读会让 ffmpeg 在末段写满缓冲后永久阻塞；
/// 实测一次 720 s 的提取累计 stderr 约 126 KB（≈2 倍管道容量），必然踩坑。
/// 同时把原来的 3600 s 硬看门狗换成「输出停滞 5 分钟」判定，便于快速失败并报出真实原因。
pub fn extract_audio(
    ctx: &Ctx,
    video: &str,
    wav_out: &Path,
    register_pid: impl Fn(u32),
    is_canceled: impl Fn() -> bool,
) -> anyhow::Result<StepOutcome> {
    let ffmpeg = ctx.tools.ffmpeg()?.to_path_buf();
    if let Some(p) = wav_out.parent() {
        ensure_dir(p)?;
    }
    let mut cmd = command_for(&ffmpeg);
    cmd.args([
        "-hide_banner",
        // -nostats：把 stderr 写入量降到约 1/11，从源头降低管道写满的概率
        "-nostats",
        "-nostdin",
        "-y",
        "-i",
        video,
        "-vn",
        "-ac",
        "1",
        "-ar",
        "16000",
        "-c:a",
        "pcm_s16le",
        "-f",
        "wav",
        &wav_out.to_string_lossy().to_string(),
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .stdin(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("启动 ffmpeg 失败：{e}"))?;
    let pid = child.id();
    register_pid(pid);

    // 实时排空两条管道（与 downloader.rs 的 run_download 同构）
    let (tx, rx) = std::sync::mpsc::channel::<(bool, String)>();
    let bytes = Arc::new(AtomicU64::new(0));
    let mut readers = Vec::new();
    if let Some(so) = child.stdout.take() {
        let tx2 = tx.clone();
        let b2 = bytes.clone();
        readers.push(std::thread::spawn(move || read_lines_counted(so, tx2, false, b2)));
    }
    if let Some(se) = child.stderr.take() {
        let tx2 = tx.clone();
        let b2 = bytes.clone();
        readers.push(std::thread::spawn(move || read_lines_counted(se, tx2, true, b2)));
    }
    drop(tx);

    let mut errs: Vec<String> = Vec::new();
    let mut killed = false;
    let started = Instant::now();
    let limit = stall_limit();
    let mut last_progress = Instant::now();
    let mut seen_bytes = 0u64;
    let mut seen_size = file_size(wav_out);

    loop {
        match rx.recv_timeout(Duration::from_millis(120)) {
            Ok((is_err, line)) => {
                if is_err {
                    push_err(&mut errs, line);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        // 进度信号：读到的字节数（含 stderr）或落盘 wav 体积任一增长 → 刷新停滞计时
        let b = bytes.load(Ordering::Relaxed);
        let size = file_size(wav_out);
        if b > seen_bytes || size > seen_size {
            seen_bytes = seen_bytes.max(b);
            seen_size = seen_size.max(size);
            last_progress = Instant::now();
        }
        if is_canceled() && !killed {
            kill_tree(pid);
            killed = true;
        }
        if !killed && is_stalled(last_progress, Instant::now(), limit) {
            kill_tree(pid);
            killed = true;
            push_err(
                &mut errs,
                format!(
                    "音频提取停滞：{} 秒内输出没有任何增长（子进程疑似被 stderr 管道写满阻塞），已中止",
                    limit.as_secs()
                ),
            );
        }
        if !killed && started.elapsed() > Duration::from_secs(EXTRACT_MAX_SECS) {
            kill_tree(pid);
            killed = true;
            push_err(&mut errs, "音频提取超过 6 小时，已自动中止".into());
        }
        if let Ok(Some(_)) = child.try_wait() {
            let deadline = Instant::now() + Duration::from_millis(400);
            while Instant::now() < deadline {
                match rx.recv_timeout(Duration::from_millis(80)) {
                    Ok((is_err, line)) => {
                        if is_err {
                            push_err(&mut errs, line);
                        }
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
    Ok(StepOutcome {
        exit_code: status.code().unwrap_or(-1),
        stderr_tail: errs.join("\n"),
        killed,
    })
}

/// 读取一条管道并把行推给主循环，同时累计已读字节数（用于停滞判定）
fn read_lines_counted<R: std::io::Read>(
    mut rd: R,
    tx: std::sync::mpsc::Sender<(bool, String)>,
    is_err: bool,
    bytes: Arc<AtomicU64>,
) {
    let mut buf = [0u8; 8192];
    let mut lb = crate::ctx::LineBuffer::new();
    loop {
        match rd.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                bytes.fetch_add(n as u64, Ordering::Relaxed);
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

/* ==================== Whisper 转写 ==================== */

fn whisper_res() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"progress\s*=\s*(\d+)\s*%").unwrap())
}

pub fn parse_whisper_progress(line: &str) -> Option<f64> {
    whisper_res().captures(line).and_then(|c| c[1].parse::<f64>().ok())
}

pub fn build_whisper_args(
    model: &Path,
    wav: &Path,
    out_base: &Path,
    language: &str,
    translate: bool,
    threads: usize,
) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-m".into(),
        model.to_string_lossy().to_string(),
        "-f".into(),
        wav.to_string_lossy().to_string(),
        "-of".into(),
        out_base.to_string_lossy().to_string(),
        "-l".into(),
        if language.trim().is_empty() { "auto".into() } else { language.to_string() },
        "-osrt".into(),
        "-pp".into(),
        "-t".into(),
        threads.to_string(),
    ];
    if translate {
        a.push("-tr".into());
    }
    a
}

pub fn run_whisper<F>(
    ctx: &Ctx,
    wav: &Path,
    out_base: &Path,
    language: &str,
    translate: bool,
    mut on_progress: F,
    register_pid: impl Fn(u32),
    is_canceled: impl Fn() -> bool,
) -> anyhow::Result<(StepOutcome, PathBuf)>
where
    F: FnMut(f64),
{
    let exe = ctx.tools.whisper()?.to_path_buf();
    let model = ctx.model_path();
    if !crate::tools::model_ready(&model) {
        // BUG-15：报错要说清「缺哪个模型 / 文件该在哪 / 已装的是哪些 / 去哪儿装」，
        // 而不是让用户对着一句「识别失败」猜。
        anyhow::bail!(
            "{}",
            crate::tools::missing_model_message(&ctx.dirs.models, &ctx.settings.whisper_model, &model)
        );
    }
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);

    // BUG-15 真凶：whisper-cli 用 ANSI（非 Unicode）API 打开输入 / 输出文件。路径里出现
    // 系统代码页表示不了的字符（韩文、部分 emoji…）时，它写不出 srt **却仍然返回退出码 0**，
    // 上游只能看到一句「未生成字幕文件」。所以含非 ASCII 字符的那一侧先换成纯 ASCII
    // 暂存路径，跑完再把产物搬回用户可见的位置（Rust 的 fs 走 Unicode API，没这问题）。
    let (wav_run, base_run) = whisper_safe_paths(wav, out_base);
    let staged_wav = wav_run != wav;
    let staged_out = base_run != out_base;
    if staged_wav {
        if let Some(p) = wav_run.parent() {
            ensure_dir(p)?;
        }
        std::fs::copy(wav, &wav_run).map_err(|e| anyhow::anyhow!("准备识别输入失败：{e}"))?;
    }
    if staged_out {
        // 清掉上一次可能残留的暂存产物，避免把旧文件当成这次的结果
        let _ = std::fs::remove_file(srt_path_for_base(&base_run));
    }

    // 兼容新旧两代 CLI：优先 --help 里出现 -of 的新版
    let args = build_whisper_args(&model, &wav_run, &base_run, language, translate, threads);

    let mut cmd = command_for(&exe);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("启动 whisper 失败：{e}"))?;
    let pid = child.id();
    register_pid(pid);

    let (tx, rx) = std::sync::mpsc::channel::<(bool, String)>();
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

    let mut errs: Vec<String> = Vec::new();
    let mut killed = false;
    let started = Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_millis(150)) {
            Ok((is_err, line)) => {
                if let Some(p) = parse_whisper_progress(&line) {
                    on_progress(p);
                } else if is_err && !line.trim().is_empty() {
                    errs.push(line);
                    if errs.len() > 40 {
                        errs.remove(0);
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if is_canceled() && !killed {
            kill_tree(pid);
            killed = true;
        }
        if started.elapsed() > Duration::from_secs(12 * 3600) && !killed {
            kill_tree(pid);
            killed = true;
            errs.push("识别超过 12 小时，已自动中止".into());
        }
        if let Ok(Some(_)) = child.try_wait() {
            let deadline = Instant::now() + Duration::from_millis(400);
            while Instant::now() < deadline {
                match rx.recv_timeout(Duration::from_millis(80)) {
                    Ok((is_err, line)) => {
                        if let Some(p) = parse_whisper_progress(&line) {
                            on_progress(p);
                        } else if is_err && !line.trim().is_empty() {
                            errs.push(line);
                        }
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
    // BUG-01：whisper 的 `-of <base>` 是「追加」后缀写出 `<base>.srt`（不是替换扩展名），
    // 所以这里按「基名 + .srt」定位，并在找不到时回退扫描 `<基名>*.srt`
    // BUG-15：暂存路径跑出来的产物要搬回用户可见的位置，任务里记的才是真产物
    let srt = match resolve_whisper_srt(&base_run) {
        Some(p) if staged_out => {
            let dest = srt_path_for_base(out_base);
            match promote_file(&p, &dest) {
                Ok(()) => dest,
                Err(e) => anyhow::bail!(
                    "字幕已识别，但写入目标位置失败（{}）：{e}；暂存产物保留在 {}",
                    dest.display(),
                    p.display()
                ),
            }
        }
        Some(p) => p,
        None => srt_path_for_base(out_base),
    };
    if staged_wav {
        // 暂存的输入音频用完就删；产物已在目标位置（或上面已经报错保留了）
        let _ = std::fs::remove_file(&wav_run);
    }
    Ok((
        StepOutcome {
            exit_code: status.code().unwrap_or(-1),
            stderr_tail: errs.join("\n"),
            killed,
        },
        srt,
    ))
}

/* ==================== 产物路径（BUG-01 / BUG-13） ==================== */

/// 按「基名 + 后缀」拼产物路径：`clip.zh` → `clip.zh.srt`。
/// 注意不能再用 `Path::with_extension`（它会把 `.zh` 语言后缀替换掉）。
pub fn product_path(base: &Path, ext: &str) -> PathBuf {
    let mut s = base.as_os_str().to_os_string();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

/* ---------- 产物命名（BUG-19：原文与译文绝不能同名） ---------- */

/// 文件名安全标签：只保留 ASCII 字母 / 数字 / `-` / `_` / `.`，其余换成 `_`，
/// 并去掉首尾的分隔符 —— 语言值本身很干净，这里只是不让脏值拼出非法或越级路径。
pub fn sanitize_tag(raw: &str) -> String {
    let s: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect();
    let s = s.trim_matches(|c| c == '.' || c == '_').to_string();
    if s.is_empty() {
        "detected".into()
    } else {
        s
    }
}

/// 产物名里的「源语言标签」：`auto`（或空值）→ `detected`，这是既有惯例，别改。
pub fn language_tag(language: &str) -> String {
    let raw = language.trim();
    let raw = if raw.is_empty() { "auto" } else { raw };
    if raw.eq_ignore_ascii_case("auto") {
        "detected".into()
    } else {
        sanitize_tag(raw)
    }
}

/// 请求里写的译文目标语言（去掉空白）；没开译文时是 None。
pub fn translate_requested(req: &SubtitleRequest) -> Option<&str> {
    req.translate_to.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// 译文目标语言的产物标签（归一化）：`en` / `english` → `en`，其余值降级为文件名安全标签。
pub fn translate_tag(req: &SubtitleRequest) -> Option<String> {
    let t = translate_requested(req)?;
    let low = t.to_ascii_lowercase();
    Some(if low == "en" || low == "english" { "en".into() } else { sanitize_tag(&low) })
}

/// Whisper 的 `-tr` 只能「译成英文」：只有目标语言是 en / english 才算真的要翻译。
/// 别的目标语言不能悄悄退化成原文（那会写出一份「名字写着 ja、内容却是原文」的假译文），
/// 由调用方明确报错。
pub fn translate_enabled(req: &SubtitleRequest) -> bool {
    matches!(
        translate_requested(req).map(|t| t.to_ascii_lowercase()).as_deref(),
        Some("en") | Some("english")
    )
}

/// 产物基名（确定性、可预期）：
///   * 原文：`<视频基名>.<源语言标签>`             —— `auto` → `detected`
///   * 译文：`<视频基名>.<源语言标签>-to-<目标语言>`
/// 译文用 `-to-`（而不是再插一个 `.`）是有意的：译文产物因此永远落在原文产物的
/// `<基名>.` 前缀之外 —— `whisper_products()` 这类「同基名前缀扫描/清理」不会把
/// 兄弟任务的译文认成自己的产物。
/// 源语言标签必定不同 → 同一批任务里任意「原文 / 译文 / 多语言」组合都不会同名：
///   `song.detected.srt` + `song.detected-to-en.srt`
///   `song.zh.srt`       + `song.zh-to-en.srt`
///   `song.en.srt`       + `song.en-to-en.srt`（原文语言就是 en 也不撞）
/// 同一个任务重跑 → 只覆盖自己那一份（幂等），别人的产物一律不碰。
pub fn subtitle_product_tag(req: &SubtitleRequest) -> String {
    let lang = language_tag(&req.language);
    match translate_tag(req) {
        Some(target) => format!("{lang}-to-{target}"),
        None => lang,
    }
}

/// whisper 的 srt 产物路径：`<基名>.srt`
pub fn srt_path_for_base(base: &Path) -> PathBuf {
    product_path(base, "srt")
}

/// 定位 whisper 实际写出的 srt：
///   1) 精确匹配 `<基名>.srt`
///   2) 回退扫描同目录下 `<基名>.*.srt`（不同 whisper 版本的命名差异）
pub fn resolve_whisper_srt(base: &Path) -> Option<PathBuf> {
    let exact = srt_path_for_base(base);
    if exact.is_file() {
        return Some(exact);
    }
    let _ = base.parent()?;
    let mut candidates = whisper_products(base);
    candidates.sort_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH)
    });
    candidates.pop()
}

/// 基名对应的全部 srt 产物（按文件名排序），用于失败时清理 / 指引
pub fn whisper_products(base: &Path) -> Vec<PathBuf> {
    let dir = match base.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let stem = match base.file_name() {
        Some(s) => s.to_string_lossy().to_string(),
        None => return Vec::new(),
    };
    let prefix = format!("{stem}.");
    let mut out: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let name = match p.file_name() {
                Some(n) => n.to_string_lossy().to_string(),
                None => continue,
            };
            if name.starts_with(&prefix) && name.to_lowercase().ends_with(".srt") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/* ==================== whisper 路径兼容（BUG-15） ==================== */

/// 路径里有没有非 ASCII 字符 —— 有就可能踩 whisper-cli 的 ANSI 打不开坑
pub fn needs_ascii_staging(p: &Path) -> bool {
    !p.to_string_lossy().is_ascii()
}

/// 纯 ASCII 文件名（暂存用）：非 ASCII 字符一律换成下划线
pub fn ascii_name(hint: &str) -> String {
    let s: String = hint
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let s = s.trim_matches('_').to_string();
    if s.is_empty() {
        "whisper".into()
    } else {
        s
    }
}

/// 暂存目录：优先用 wav 所在目录（本来就是 cache/subtitle-work，名字干净时不额外搬运），
/// 该目录也不干净时退回系统临时目录，最后兜底用相对路径（进程工作目录）。
pub fn ascii_stage_dir(wav: &Path) -> PathBuf {
    if let Some(d) = wav.parent() {
        if !d.as_os_str().is_empty() && !needs_ascii_staging(d) {
            return d.to_path_buf();
        }
    }
    let tmp = std::env::temp_dir();
    if !needs_ascii_staging(&tmp) {
        let d = tmp.join("umidl-whisper");
        let _ = std::fs::create_dir_all(&d);
        return d;
    }
    PathBuf::from(".")
}

/// whisper 实际使用的「输入音频 + 输出基名」。
/// 哪一侧的路径含非 ASCII 字符，就只给那一侧换成纯 ASCII 的暂存路径
/// （`<ASCII 名>.wav` / `<ASCII 名>.out`，名字取自 wav 的 stem，天然按任务隔离）；
/// 已经干净的一侧原样使用，避免把文件搬来搬去。
pub fn whisper_safe_paths(wav: &Path, out_base: &Path) -> (PathBuf, PathBuf) {
    let wav_ok = !needs_ascii_staging(wav);
    let base_ok = !needs_ascii_staging(out_base);
    if wav_ok && base_ok {
        return (wav.to_path_buf(), out_base.to_path_buf());
    }
    let wav_run = if wav_ok { wav.to_path_buf() } else { ascii_stage_path(wav, "wav") };
    let base_run = if base_ok { out_base.to_path_buf() } else { ascii_stage_path(wav, "out") };
    (wav_run, base_run)
}

/// 暂存路径：ASCII 目录下、由 wav 名字派生的纯 ASCII 路径（扩展名可指定）
pub fn ascii_stage_path(wav: &Path, ext: &str) -> PathBuf {
    let dir = ascii_stage_dir(wav);
    let stem = wav
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    dir.join(format!("{}.{}", ascii_name(&stem), ext))
}

/// 把暂存产物搬回真正的目标路径。Rust 的 fs 走 Unicode API，中文 / 韩文路径都没问题；
/// 跨盘 rename 失败时退化为「复制 + 删除源」。
pub fn promote_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(p) = to.parent() {
        if !p.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(p);
        }
    }
    if to.exists() {
        let _ = std::fs::remove_file(to);
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

/// whisper 跑完却没有产物时的报错文案（BUG-15）：带上退出码与 whisper 的最后一行输出，
/// 并按失败特征给出可行动的下一步 —— 而不是笼统一句「未生成字幕文件」。
pub fn whisper_no_output_error(exit_code: i32, stderr_tail: &str, want: &Path) -> String {
    let last = stderr_tail
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .last()
        .unwrap_or("");
    let low = stderr_tail.to_lowercase();
    let mut hints: Vec<&str> = Vec::new();
    if low.contains("failed to open") {
        hints.push("输出路径无法写入：请确认目标目录存在、可写，且路径中没有非法字符");
    }
    if low.contains("failed to load model") || low.contains("failed to open model") || low.contains("load model") {
        hints.push("模型文件可能损坏或不完整，请在「设置 → 依赖工具 → Whisper 模型」重新下载");
    } else if hints.is_empty() {
        hints.push("可先确认「设置 → 依赖工具 → Whisper 模型」已下载，再重试");
    }
    let mut msg = format!("Whisper 未生成字幕文件（{}）· 退出码 {exit_code}", want.display());
    if !last.is_empty() {
        msg.push_str(&format!(" · whisper 输出：{last}"));
    }
    msg.push_str(&format!(" · {}", hints.join("；")));
    msg
}

fn read_lines<R: std::io::Read>(mut rd: R, tx: std::sync::mpsc::Sender<(bool, String)>, is_err: bool) {
    let mut buf = [0u8; 8192];
    let mut lb = crate::ctx::LineBuffer::new();
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

/// 转写并导出目标格式，返回最终字幕文件路径
pub fn transcribe_to_format<F>(
    ctx: &Ctx,
    req: &SubtitleRequest,
    out_base: &Path,
    wav: &Path,
    on_progress: F,
    register_pid: impl Fn(u32),
    is_canceled: impl Fn() -> bool,
) -> anyhow::Result<PathBuf>
where
    F: FnMut(f64),
{
    let translate = translate_enabled(req);
    // BUG-19：请求了译文但目标不是英文 —— Whisper 的 `-tr` 只支持译成英文。
    // 与其悄悄产出「名叫 ja、内容却是原文」的假译文，不如明确报错（前端只发 en）。
    if !translate {
        if let Some(asked) = translate_requested(req) {
            anyhow::bail!(
                "暂不支持「{asked}」字幕：Whisper 的翻译模式只能译成英文（en），请改选英文或关闭该开关"
            );
        }
    }
    let (outcome, srt_path) = run_whisper(
        ctx,
        wav,
        out_base,
        &req.language,
        translate,
        on_progress,
        register_pid,
        is_canceled,
    )?;
    // 双保险：run_whisper 已经回退扫描过，这里再确认一次（进程退出与落盘偶有先后差）
    let srt_path = if srt_path.is_file() {
        srt_path
    } else {
        resolve_whisper_srt(out_base).unwrap_or(srt_path)
    };
    if outcome.exit_code != 0 && !srt_path.is_file() {
        anyhow::bail!(
            "Whisper 识别失败（退出码 {}）：{}",
            outcome.exit_code,
            outcome.stderr_tail.lines().last().unwrap_or("未知错误")
        );
    }
    if !srt_path.is_file() {
        // BUG-15：把退出码与 whisper 的真实输出一起摊给用户，别再只说「未生成字幕文件」
        anyhow::bail!(
            "{}",
            whisper_no_output_error(
                outcome.exit_code,
                &outcome.stderr_tail,
                &srt_path_for_base(out_base)
            )
        );
    }
    let raw = std::fs::read_to_string(&srt_path).unwrap_or_default();
    if raw.trim().is_empty() {
        anyhow::bail!("识别结果为空：音频中未检测到可识别语音");
    }
    let target = format_for_ext(&req.output_format);
    if target == "srt" {
        return Ok(srt_path);
    }
    let title = out_base.file_name().and_then(|s| s.to_str()).unwrap_or("umi");
    let rendered = render(&raw, &target, title);
    // 目标格式沿用同一套命名：`<基名>.<格式>`（如 clip.zh.vtt），不再替换语言后缀
    let dest = product_path(out_base, &target);
    std::fs::write(&dest, rendered)?;
    Ok(dest)
}

pub fn new_subtitle_task(req: &SubtitleRequest, model: &str, created: i64) -> SubtitleTask {
    SubtitleTask {
        id: crate::ctx::short_id(),
        video_path: req.video_path.clone(),
        language: req.language.clone(),
        model: model.to_string(),
        subtitle_path: None,
        // BUG-19：把「这份是译文」写进任务本身，队列行不用再靠前端内存里的一张临时表
        translate_to: translate_requested(req).map(|s| s.to_string()),
        status: TaskStatus::Pending,
        progress: 0.0,
        error: None,
        created_time: created,
    }
}

pub fn subtitle_out_base(ctx: &Ctx, req: &SubtitleRequest) -> PathBuf {
    let src = Path::new(&req.video_path);
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("subtitle");
    let dir = req
        .output_dir
        .as_ref()
        .filter(|d| !d.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            src.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| ctx.download_dir())
        });
    let _ = std::fs::create_dir_all(&dir);
    // BUG-19：原文 `<基名>.<源语言>`、译文 `<基名>.<源语言>-to-<目标语言>`，
    // 两条产物从此各有各的名字（此前译文沿用原文基名 → 互相覆盖 → 用户看不到英文字幕）
    dir.join(format!("{stem}.{}", subtitle_product_tag(req)))
}

#[cfg(test)]
mod fix18_tests {
    use super::*;
    use crate::ctx::{AppDirs, ToolPaths};
    use crate::models::AppSettings;

    fn workdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("umi-sub-{tag}-{}", crate::ctx::short_id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn ctx_in(base: &Path) -> Ctx {
        let dirs = AppDirs {
            data: base.to_path_buf(),
            bin: base.join("bin"),
            models: base.join("models"),
            cache: base.join("cache"),
            downloads: base.join("downloads"),
            db_file: base.join("umi.db"),
            settings_file: base.join("settings.json"),
        };
        Ctx::new(dirs, ToolPaths::default(), AppSettings::default())
    }

    fn srt_req(video: &Path, dir: &Path, lang: &str) -> SubtitleRequest {
        SubtitleRequest {
            video_path: video.to_string_lossy().to_string(),
            language: lang.to_string(),
            model: Some("tiny".into()),
            translate_to: None,
            output_format: "srt".into(),
            output_dir: Some(dir.to_string_lossy().to_string()),
        }
    }

    /// BUG-01：产物路径必须是「基名 + .srt」，绝不能再把语言后缀替换掉
    #[test]
    fn srt_path_appends_instead_of_replacing_language_suffix() {
        let base = Path::new("C:/videos/clip12s.zh");
        assert_eq!(srt_path_for_base(base), PathBuf::from("C:/videos/clip12s.zh.srt"));
        assert_ne!(
            srt_path_for_base(base),
            base.with_extension("srt"),
            "with_extension 会把 .zh 替掉 —— 那正是字幕必然失败的原因"
        );
        assert_eq!(product_path(base, "vtt"), PathBuf::from("C:/videos/clip12s.zh.vtt"));
        assert_eq!(product_path(base, "srt"), PathBuf::from("C:/videos/clip12s.zh.srt"));
    }

    /// BUG-01：真实命名链路（subtitle_out_base → 产物）端到端可定位
    #[test]
    fn out_base_keeps_language_and_product_is_resolvable() {
        let dir = workdir("outbase");
        let video = dir.join("clip12s.mp4");
        std::fs::write(&video, b"x").unwrap();
        let ctx = ctx_in(&dir);
        let req = srt_req(&video, &dir, "zh");
        let base = subtitle_out_base(&ctx, &req);
        assert_eq!(base, dir.join("clip12s.zh"));
        let srt = srt_path_for_base(&base);
        assert_eq!(srt, dir.join("clip12s.zh.srt"));
        // whisper 写出的就是这个文件 → 必须找得到
        std::fs::write(&srt, "1\n00:00:00,000 --> 00:00:01,000\n你好\n").unwrap();
        assert_eq!(resolve_whisper_srt(&base), Some(srt));
        // auto 语言经过归一化
        let auto = srt_req(&video, &dir, "auto");
        assert_eq!(subtitle_out_base(&ctx, &auto), dir.join("clip12s.detected"));
    }

    /// BUG-01：找不到精确产物时回退扫描 `<基名>*.srt`，且不会串到别的视频
    #[test]
    fn resolve_falls_back_to_same_stem_products_only() {
        let dir = workdir("resolve");
        let base = dir.join("clip12s.zh");
        assert!(resolve_whisper_srt(&base).is_none(), "什么都没有时返回 None");
        // 不同 whisper 版本可能写成 <基名>.<别的>.srt
        std::fs::write(dir.join("clip12s.zh.2026.srt"), "1").unwrap();
        let got = resolve_whisper_srt(&base).expect("回退扫描应命中");
        assert_eq!(got, dir.join("clip12s.zh.2026.srt"));
        // 精确命名优先
        std::fs::write(dir.join("clip12s.zh.srt"), "1").unwrap();
        assert_eq!(resolve_whisper_srt(&base), Some(dir.join("clip12s.zh.srt")));
        // 别的视频/语言的产物不能被误认
        assert!(resolve_whisper_srt(&dir.join("other.zh")).is_none());
        // 非 srt 产物不算
        assert!(resolve_whisper_srt(&dir.join("clip12s.en")).is_none());
    }

    /// BUG-13：失败时要能列出该基名下的全部 srt 产物（用于保留有效 / 清理无效）
    #[test]
    fn products_list_only_same_stem() {
        let dir = workdir("products");
        let base = dir.join("a.zh");
        std::fs::write(dir.join("a.zh.srt"), "1").unwrap();
        std::fs::write(dir.join("a.zh.2.srt"), "2").unwrap();
        std::fs::write(dir.join("a.zh.vtt"), "x").unwrap();
        std::fs::write(dir.join("a2.zh.srt"), "x").unwrap();
        std::fs::write(dir.join("b.zh.srt"), "x").unwrap();
        let names: Vec<String> = whisper_products(&base)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["a.zh.2.srt".to_string(), "a.zh.srt".to_string()]);
    }

    /// BUG-15：路径全 ASCII 时原样使用，不做多余搬运
    #[test]
    fn ascii_paths_are_used_as_is() {
        let wav = Path::new("C:/umi/cache/subtitle-work/9f81da97.wav");
        let base = Path::new("C:/videos/clip12s.zh");
        assert!(!needs_ascii_staging(wav));
        assert!(!needs_ascii_staging(base));
        let (w, b) = whisper_safe_paths(wav, base);
        assert_eq!(w, wav.to_path_buf());
        assert_eq!(b, base.to_path_buf());
    }

    /// BUG-15 真凶回归：whisper-cli 用 ANSI API 开关文件，韩文等非系统代码页字符会
    /// 让它「写不出 srt 还返回 0」—— 所以这类路径必须先换成一串纯 ASCII 暂存路径。
    #[test]
    fn non_ascii_paths_are_staged_ascii() {
        let wav = Path::new("C:/cache/subtitle-work/9f81da97.wav");
        let base = Path::new("D:/Media Files/Sample '샘플 영상' Official Special Clip.detected");
        assert!(needs_ascii_staging(base), "韩文路径必须被判为需要暂存");
        let (w, b) = whisper_safe_paths(wav, base);
        assert!(w.to_string_lossy().is_ascii(), "{w:?}");
        assert!(b.to_string_lossy().is_ascii(), "{b:?}");
        assert_eq!(w, PathBuf::from("C:/cache/subtitle-work/9f81da97.wav"));
        assert_eq!(b, PathBuf::from("C:/cache/subtitle-work/9f81da97.out"));
        // 输入路径本身不干净时也要暂存（否则 whisper 连输入都读不到，退出码 2）
        let dirty_wav = Path::new("C:/用户/缓存/9f81da97.wav");
        let (w2, b2) = whisper_safe_paths(dirty_wav, Path::new("D:/x/a.zh"));
        assert!(w2.to_string_lossy().is_ascii() && b2.to_string_lossy().is_ascii());
        assert!(w2.file_name().unwrap().to_string_lossy().starts_with("9f81da97"));
        assert!(!b2.to_string_lossy().contains("用户"));
    }

    /// BUG-15：暂存文件名只留 ASCII，实在取不出名字也要有个兜底名
    #[test]
    fn ascii_name_sanitizes_and_falls_back() {
        assert_eq!(ascii_name("9f81da97"), "9f81da97");
        assert_eq!(ascii_name("clip-12_x"), "clip-12_x");
        assert_eq!(ascii_name("영상 'clip'"), "clip");
        assert_eq!(ascii_name("''"), "whisper");
        assert_eq!(ascii_name(""), "whisper");
    }

    /// BUG-15：产物要从 ASCII 暂存位置搬回含非 ASCII 字符的目标路径（并能覆盖旧文件）
    #[test]
    fn promote_file_moves_to_unicode_target() {
        let work = workdir("promote");
        let staged = work.join("9f81da97.out.srt");
        std::fs::write(&staged, "1\n00:00:00,000 --> 00:00:01,000\n你好\n").unwrap();
        let dest = work.join("출력 폴더 'тест'").join("영상 'clip'.srt");
        promote_file(&staged, &dest).unwrap();
        assert!(!staged.exists(), "搬完不能留两份");
        assert!(std::fs::read_to_string(&dest).unwrap().contains("你好"));
        // 目标已存在 → 覆盖而不是报错
        std::fs::write(&staged, "1\n00:00:00,000 --> 00:00:01,000\n覆盖后\n").unwrap();
        promote_file(&staged, &dest).unwrap();
        assert!(std::fs::read_to_string(&dest).unwrap().contains("覆盖后"));
        assert!(validate_srt(&std::fs::read_to_string(&dest).unwrap()).is_ok(), "产物得是合法 srt");
    }

    /// BUG-15：没有产物时的报错要带上退出码 + whisper 的真实输出，并按特征给行动建议
    #[test]
    fn no_output_error_carries_exit_code_and_tail() {
        let want = Path::new("D:/Media Files/Sample '샘플 영상'.detected.srt");
        let tail = "load_backend: loaded CPU backend\nopen: failed to open 'D:/x/????.detected.srt' for writing\n";
        let msg = whisper_no_output_error(0, tail, want);
        assert!(msg.contains("退出码 0"), "退出码 0（写不出文件却报成功）是本案特征：{msg}");
        assert!(msg.contains("failed to open"), "要摊出 whisper 的真实输出：{msg}");
        assert!(msg.contains("无法写入"), "要给出可行动的下一步：{msg}");
        assert!(msg.contains("Sample '샘플 영상'.detected.srt"), "要点明目标文件：{msg}");
        // 模型加载失败 → 提示重新下载模型
        let m2 = whisper_no_output_error(2, "error: failed to load model 'ggml-base.bin'", want);
        assert!(m2.contains("模型文件可能损坏"), "{m2}");
        // 什么都没有时也不能是空话
        let m3 = whisper_no_output_error(0, "", want);
        assert!(m3.contains("退出码 0") && m3.contains("依赖工具"), "{m3}");
    }

    /// BUG-15 端到端（默认忽略，需要 whisper-cli + 一个可用模型）：
    /// 真实调一次 whisper，目标路径故意含非 ASCII（韩文 / 空格 / 单引号）——
    /// 修复前 whisper 会「打不开输出文件但退出码 0」，产物根本不存在。
    ///   UMI_WHISPER_TEST_WAV=<带语音的 wav> cargo test --lib -- --ignored unicode_output --nocapture
    #[test]
    #[ignore]
    fn unicode_output_path_still_writes_srt() {
        let src = match std::env::var("UMI_WHISPER_TEST_WAV") {
            Ok(p) if Path::new(&p).is_file() => PathBuf::from(p),
            _ => {
                println!("跳过：未设置 UMI_WHISPER_TEST_WAV（指向一段有语音的 wav）");
                return;
            }
        };
        let mut ctx = ctx_in(&workdir("e2e-dirs"));
        ctx.dirs = AppDirs::new();
        ctx.tools = crate::tools::resolve_all(&ctx);
        if ctx.tools.whisper.is_none() {
            println!("跳过：未找到 whisper-cli");
            return;
        }
        let (model, fell) = crate::tools::effective_model_for(&ctx.dirs.models, &ctx.settings.whisper_model);
        if fell {
            println!("配置的模型不可用，自动改用 {model}");
            ctx.settings.whisper_model = model;
        }
        if !crate::tools::model_ready(&ctx.model_path()) {
            println!("跳过：模型目录里没有可用模型");
            return;
        }
        let work = workdir("unicode-e2e");
        let wav = work.join("clip.wav");
        std::fs::copy(&src, &wav).unwrap();
        let out_dir = work.join("출력 폴더 'test'");
        std::fs::create_dir_all(&out_dir).unwrap();
        let base = out_dir.join("영상 'clip'");
        let (outcome, srt) = run_whisper(&ctx, &wav, &base, "auto", false, |_| {}, |_| {}, || false)
            .expect("run_whisper 不应返回 Err");
        let body = std::fs::read_to_string(&srt).unwrap_or_default();
        println!(
            "exit={} · 产物={} · 字节={} · 首行={}",
            outcome.exit_code,
            srt.display(),
            body.len(),
            body.lines().next().unwrap_or("")
        );
        println!("whisper 尾部输出：{}", outcome.stderr_tail.lines().last().unwrap_or(""));
        assert_eq!(outcome.exit_code, 0, "whisper 应正常退出");
        assert!(srt.is_file(), "非 ASCII 目标路径必须真的写出字幕：{}", srt.display());
        assert_eq!(srt, srt_path_for_base(&base), "产物要落在用户可见的基名 + .srt");
        assert!(validate_srt(&body).is_ok(), "产物得是合法 srt");
        assert!(!work.join("clip.out.srt").exists(), "暂存产物不能留在 cache 里");
    }

    /// BUG-09 端到端（默认忽略，需要 ffmpeg）：
    /// 用「先吐一段真实数据、然后彻底卡住」的本地 HTTP 源喂 extract_audio，
    /// 期望在 UMI_EXTRACT_STALL_SECS 秒内判定停滞并中止，而不是傻等 1 小时。
    ///   cargo test --lib -- --ignored stall_watchdog --nocapture
    #[test]
    #[ignore]
    fn stall_watchdog_kills_frozen_ffmpeg() {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        let base = workdir("stall");
        let mut ctx = ctx_in(&base);
        // 用真实探测结果填 tools：extract_audio 读的是 ctx.tools，不是现场探测
        ctx.tools = crate::tools::resolve_all(&ctx);
        if ctx.tools.ffmpeg.is_none() {
            println!("跳过：未找到 ffmpeg");
            return;
        }
        // 造一段真实 TS 输入（8 秒 testsrc + 正弦音）
        let src = base.join("src.ts");
        let ffmpeg = ctx.tools.ffmpeg.clone().unwrap();
        let gen = std::process::Command::new(&ffmpeg)
            .args([
                "-hide_banner", "-nostdin", "-y",
                "-f", "lavfi", "-i", "testsrc=size=160x120:rate=10:duration=20",
                "-f", "lavfi", "-i", "sine=frequency=440:duration=20",
                "-c:v", "mpeg2video", "-c:a", "mp2", "-f", "mpegts",
            ])
            .arg(&src)
            .output()
            .expect("生成测试流失败");
        assert!(gen.status.success() && src.is_file(), "ffmpeg 生成测试流失败");
        let body = std::fs::read(&src).unwrap();
        let head_len = (body.len() / 2).max(64 * 1024).min(body.len());

        // 「先吐一半，然后彻底卡住」的 HTTP 源
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let head = body[..head_len].to_vec();
        let srv = std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf);
                let _ = sock.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                );
                let _ = sock.write_all(&head);
                let _ = sock.flush();
                // 卡住：不再写任何数据（模拟「输入健康但输出冻结」）
                std::thread::sleep(Duration::from_secs(60));
            }
        });

        std::env::set_var("UMI_EXTRACT_STALL_SECS", "6");
        let wav = base.join("stalled.wav");
        let t0 = Instant::now();
        let out = extract_audio(
            &ctx,
            &format!("http://127.0.0.1:{port}/slow.ts"),
            &wav,
            |_| {},
            || false,
        )
        .expect("extract_audio 不应返回 Err（停滞要作为 StepOutcome 报出来）");
        let dt = t0.elapsed();
        println!(
            "停滞用例：耗时 {:.1}s · killed={} · exit={} · tail={}",
            dt.as_secs_f64(),
            out.killed,
            out.exit_code,
            out.stderr_tail.lines().last().unwrap_or("")
        );
        std::env::remove_var("UMI_EXTRACT_STALL_SECS");
        assert!(out.killed, "停滞必须被判死并杀掉子进程");
        assert!(
            out.stderr_tail.contains("停滞"),
            "错误信息要说明是停滞：{}",
            out.stderr_tail
        );
        assert!(dt < Duration::from_secs(45), "停滞判定太慢：{:.1}s", dt.as_secs_f64());
        assert!(wav.is_file(), "停滞前已写入的 wav 片段应保留在磁盘上");
        let _ = wav;
        let _ = srv.join();
    }

    /// BUG-09：停滞看门狗——输出在一段时间内没有任何增长即判定卡死
    #[test]
    fn stall_watchdog_detects_frozen_output() {
        let t0 = Instant::now();
        let limit = Duration::from_secs(300);
        assert!(!is_stalled(t0, t0 + Duration::from_secs(120), limit), "120s 无增长还不算卡死");
        assert!(!is_stalled(t0, t0 + limit, limit), "刚好到阈值不算超时");
        assert!(is_stalled(t0, t0 + Duration::from_secs(301), limit), "超过阈值必须判卡死");
        // 默认阈值 5 分钟，且环境变量可覆盖（端到端验证用）
        assert_eq!(EXTRACT_STALL_SECS, 300);
        assert!(stall_limit() >= Duration::from_secs(5), "下限 5 秒，防止误杀");
    }
}

/// BUG-19 回归：开了「额外生成一份英文字幕」后两份产物要真实落盘、且**绝不互相覆盖**。
/// 这里只测纯函数（命名推导 / 回搬定位 / 暂存隔离），不把 whisper 塞进单测。
#[cfg(test)]
mod fix19_tests {
    use super::*;
    use crate::ctx::{AppDirs, ToolPaths};
    use crate::models::AppSettings;

    fn workdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("umi-sub19-{tag}-{}", crate::ctx::short_id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn ctx_in(base: &Path) -> Ctx {
        let dirs = AppDirs {
            data: base.to_path_buf(),
            bin: base.join("bin"),
            models: base.join("models"),
            cache: base.join("cache"),
            downloads: base.join("downloads"),
            db_file: base.join("umi.db"),
            settings_file: base.join("settings.json"),
        };
        Ctx::new(dirs, ToolPaths::default(), AppSettings::default())
    }

    fn req(video: &Path, dir: &Path, lang: &str, translate_to: Option<&str>) -> SubtitleRequest {
        SubtitleRequest {
            video_path: video.to_string_lossy().to_string(),
            language: lang.to_string(),
            model: Some("tiny".into()),
            translate_to: translate_to.map(|s| s.to_string()),
            output_format: "srt".into(),
            output_dir: Some(dir.to_string_lossy().to_string()),
        }
    }

    /// 本案命名规则：原文 `<基名>.<源语言>`，译文 `<基名>.<源语言>-to-<目标语言>`
    #[test]
    fn product_tag_appends_target_language_for_translation() {
        let dir = PathBuf::from("C:/v");
        let v = dir.join("clip.mp4");
        assert_eq!(subtitle_product_tag(&req(&v, &dir, "auto", None)), "detected");
        assert_eq!(subtitle_product_tag(&req(&v, &dir, "zh", None)), "zh");
        assert_eq!(subtitle_product_tag(&req(&v, &dir, "auto", Some("en"))), "detected-to-en");
        assert_eq!(subtitle_product_tag(&req(&v, &dir, "zh", Some("en"))), "zh-to-en");
        // 原文语言就是 en 也不能撞：`.en` vs `.en-to-en`
        assert_eq!(subtitle_product_tag(&req(&v, &dir, "en", Some("en"))), "en-to-en");
        // 空白 / 大小写 / 别名一律归一化（前端发 "en"，手工 IPC 也不能写出怪名）
        assert_eq!(subtitle_product_tag(&req(&v, &dir, "", None)), "detected");
        assert_eq!(subtitle_product_tag(&req(&v, &dir, "AUTO", Some("English"))), "detected-to-en");
        assert_eq!(subtitle_product_tag(&req(&v, &dir, "zh", Some("  en  "))), "zh-to-en");
        // 脏值不能拼出越级路径
        assert!(!subtitle_product_tag(&req(&v, &dir, "../../etc", Some("en"))).contains('/'));
    }

    #[test]
    fn translate_helpers_normalize_and_never_silently_drop_target() {
        let dir = PathBuf::from("C:/v");
        let v = dir.join("clip.mp4");
        assert!(!translate_enabled(&req(&v, &dir, "auto", None)));
        assert!(!translate_enabled(&req(&v, &dir, "auto", Some(""))));
        assert!(!translate_enabled(&req(&v, &dir, "auto", Some("   "))));
        assert!(translate_enabled(&req(&v, &dir, "auto", Some("en"))));
        assert!(translate_enabled(&req(&v, &dir, "auto", Some("EN"))));
        assert!(translate_enabled(&req(&v, &dir, "auto", Some("English"))));
        // 非英文目标：不算「要翻译」（调用方会明确报错，而不是产出一份名不副实的文件）
        assert!(!translate_enabled(&req(&v, &dir, "auto", Some("ja"))));
        assert_eq!(translate_tag(&req(&v, &dir, "auto", Some("ja"))).as_deref(), Some("ja"));
        assert_eq!(translate_tag(&req(&v, &dir, "auto", None)), None);
    }

    /// 核心回归：同一批任务（每个语言一份原文 + 一份译文）里，产物路径两两不同
    #[test]
    fn original_and_translated_products_never_collide() {
        let dir = workdir("collide");
        let video = dir.join("clip.mp4");
        std::fs::write(&video, b"x").unwrap();
        let ctx = ctx_in(&dir);
        // 前端 jobs 的形状：每个语言一个原文任务，开了翻译再补一个译文任务
        let langs = ["auto", "zh", "en", "ja", "ko", "fr", "de", "ru"];
        let mut paths: Vec<PathBuf> = Vec::new();
        for lang in langs {
            for t in [None, Some("en")] {
                let r = req(&video, &dir, lang, t);
                let base = subtitle_out_base(&ctx, &r);
                // 原文与译文必须都落在同一个目录、名字可区分
                assert_eq!(base.parent().unwrap(), dir);
                paths.push(srt_path_for_base(&base));
            }
        }
        let total = paths.len();
        let mut uniq = paths.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), total, "同批产物出现同名，会互相覆盖：{paths:?}");
        // 译文名永远不是原文名的前缀（否则兄弟产物的扫描/清理会互相认错）
        for lang in langs {
            let orig = subtitle_out_base(&ctx, &req(&video, &dir, lang, None));
            let tr = subtitle_out_base(&ctx, &req(&video, &dir, lang, Some("en")));
            assert_ne!(orig, tr, "{lang}: 原文与译文基名相同");
            let prefix = format!("{}.", orig.file_name().unwrap().to_string_lossy());
            assert!(
                !tr.file_name().unwrap().to_string_lossy().starts_with(&prefix),
                "{lang}: 译文产物落进了原文的 <基名>. 前缀里"
            );
        }
        // 命名是确定性的：同请求两次得到同一路径（重跑覆盖自己、不会越写越多）
        let a = subtitle_out_base(&ctx, &req(&video, &dir, "auto", Some("en")));
        let b = subtitle_out_base(&ctx, &req(&video, &dir, "auto", Some("en")));
        assert_eq!(a, b);
        assert!(a.to_string_lossy().ends_with("clip.detected-to-en"), "{a:?}");
    }

    /// 两份产物都要真实落盘、内容各异、且都是合法 SRT（用真实命名链路写盘）
    #[test]
    fn both_products_are_written_non_empty_and_different() {
        let dir = workdir("both");
        let video = dir.join("clip.mp4");
        std::fs::write(&video, b"x").unwrap();
        let ctx = ctx_in(&dir);

        let orig_base = subtitle_out_base(&ctx, &req(&video, &dir, "auto", None));
        let tr_base = subtitle_out_base(&ctx, &req(&video, &dir, "auto", Some("en")));
        let orig = srt_path_for_base(&orig_base);
        let tr = srt_path_for_base(&tr_base);

        // 原文任务写盘
        let zh_srt = "1\n00:00:00,000 --> 00:00:02,000\n你好，世界\n\n2\n00:00:02,000 --> 00:00:04,000\n第二句\n\n";
        std::fs::write(&orig, zh_srt).unwrap();
        // 译文任务写盘（whisper -tr 的英文产物）
        let en_srt = "1\n00:00:00,000 --> 00:00:02,000\nHello, world\n\n2\n00:00:02,000 --> 00:00:04,000\nThe second line\n\n";
        std::fs::write(&tr, en_srt).unwrap();

        for p in [&orig, &tr] {
            assert!(p.is_file(), "产物没落盘：{}", p.display());
            let body = std::fs::read_to_string(p).unwrap();
            assert!(!body.trim().is_empty(), "产物为空：{}", p.display());
            assert!(validate_srt(&body).is_ok(), "不是合法 srt：{}", p.display());
        }
        assert_ne!(
            std::fs::read_to_string(&orig).unwrap(),
            std::fs::read_to_string(&tr).unwrap(),
            "两份产物内容必须不同（译文不能被原文覆盖）"
        );
        assert!(dir.join("clip.detected.srt").is_file());
        assert!(dir.join("clip.detected-to-en.srt").is_file());
        // 队列里两份都能定位到自己的文件
        assert_eq!(resolve_whisper_srt(&orig_base), Some(orig.clone()));
        assert_eq!(resolve_whisper_srt(&tr_base), Some(tr.clone()));
    }

    /// 同一任务重跑：只覆盖自己那一份，另一份原样保留（幂等且互不干扰）
    #[test]
    fn rerun_overwrites_only_its_own_product() {
        let dir = workdir("rerun");
        let video = dir.join("clip.mp4");
        std::fs::write(&video, b"x").unwrap();
        let ctx = ctx_in(&dir);
        let orig = srt_path_for_base(&subtitle_out_base(&ctx, &req(&video, &dir, "zh", None)));
        let tr = srt_path_for_base(&subtitle_out_base(&ctx, &req(&video, &dir, "zh", Some("en"))));

        std::fs::write(&orig, "1\n00:00:00,000 --> 00:00:01,000\n第一次原文\n\n").unwrap();
        std::fs::write(&tr, "1\n00:00:00,000 --> 00:00:01,000\nFirst translation\n\n").unwrap();

        // 第二次跑同一个译文任务（whisper 的 -of 基名一样 → 覆盖同一个文件）
        std::fs::write(&tr, "1\n00:00:00,000 --> 00:00:01,000\nSecond translation\n\n").unwrap();
        assert!(std::fs::read_to_string(&tr).unwrap().contains("Second translation"));
        assert!(tr.is_file() && orig.is_file(), "重跑不能把另一份产物弄没");
        assert!(
            std::fs::read_to_string(&orig).unwrap().contains("第一次原文"),
            "重跑译文不允许动到原文产物"
        );

        // 第二次跑同一个原文任务 → 同理只覆盖自己
        std::fs::write(&orig, "1\n00:00:00,000 --> 00:00:01,000\n第二次原文\n\n").unwrap();
        assert!(std::fs::read_to_string(&tr).unwrap().contains("Second translation"));
        assert!(std::fs::read_to_string(&orig).unwrap().contains("第二次原文"));
    }

    /// 原文的产物扫描不能把兄弟任务的译文认成自己的（`-to-` 命名的意义所在）
    #[test]
    fn sibling_translation_is_not_scanned_as_original_product() {
        let dir = workdir("scan");
        let base = dir.join("clip.detected");
        let tr_base = dir.join("clip.detected-to-en");
        std::fs::write(dir.join("clip.detected.srt"), "1").unwrap();
        std::fs::write(dir.join("clip.detected-to-en.srt"), "1").unwrap();
        assert_eq!(whisper_products(&base), vec![dir.join("clip.detected.srt")]);
        assert_eq!(whisper_products(&tr_base), vec![dir.join("clip.detected-to-en.srt")]);
    }

    /// 并发跑原文 + 译文时，两条转写的 ASCII 暂存基名必须不同（否则会互相踩掉中间产物）
    #[test]
    fn staged_ascii_bases_are_task_scoped_and_distinct() {
        let tmp = workdir("stage");
        let work = tmp.join("cache").join("subtitle-work");
        std::fs::create_dir_all(&work).unwrap();
        let out_dir = tmp.join("출력 폴더 'test'");
        std::fs::create_dir_all(&out_dir).unwrap();
        // run_subtitle_job 的形状：wav 名 = 任务 id（两个任务 id 不同）
        let wav_a = work.join("9f81da97.wav");
        let wav_b = work.join("27c1ab30.wav");
        let base_a = out_dir.join("영상 'clip'.detected");
        let base_b = out_dir.join("영상 'clip'.detected-to-en");
        let (wa, ba) = whisper_safe_paths(&wav_a, &base_a);
        let (wb, bb) = whisper_safe_paths(&wav_b, &base_b);
        assert!(ba.to_string_lossy().is_ascii() && bb.to_string_lossy().is_ascii());
        assert_ne!(ba, bb, "两次转写的暂存产物基名相同 → 会互相覆盖");
        assert_ne!(srt_path_for_base(&ba), srt_path_for_base(&bb));
        assert_ne!(wa, wb, "输入音频也不能共用同一个暂存名");
    }
}
