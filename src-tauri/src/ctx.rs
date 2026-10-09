use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::models::AppSettings;

pub type Emitter = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

/// 应用目录集合（不依赖 Tauri，便于单元测试）
#[derive(Debug, Clone)]
pub struct AppDirs {
    pub data: PathBuf,
    pub bin: PathBuf,
    pub models: PathBuf,
    pub cache: PathBuf,
    pub downloads: PathBuf,
    pub db_file: PathBuf,
    pub settings_file: PathBuf,
}

impl AppDirs {
    /// 允许用环境变量 UMI_DATA_DIR / UMI_DOWNLOAD_DIR 覆盖（测试用）
    pub fn new() -> Self {
        let data = std::env::var("UMI_DATA_DIR")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::data_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("umi-downloader")
            });
        let downloads = std::env::var("UMI_DOWNLOAD_DIR")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::download_dir()
                    .unwrap_or_else(|| data.clone())
                    .join("Umidl")
            });
        let dirs = Self {
            bin: data.join("bin"),
            models: data.join("models"),
            cache: data.join("cache"),
            db_file: data.join("umi.db"),
            settings_file: data.join("settings.json"),
            data,
            downloads,
        };
        dirs.ensure();
        dirs
    }

    pub fn ensure(&self) {
        for d in [&self.data, &self.bin, &self.models, &self.cache, &self.downloads] {
            let _ = std::fs::create_dir_all(d);
        }
    }
}

impl Default for AppDirs {
    fn default() -> Self {
        Self::new()
    }
}

/// 已解析的外部工具路径
#[derive(Debug, Clone, Default)]
pub struct ToolPaths {
    pub ytdlp: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
    pub whisper: Option<PathBuf>,
    /// aria2c：分段并行引擎（HTTP/FTP/BitTorrent/磁力）
    pub aria2: Option<PathBuf>,
    /// pandoc：文档格式互转引擎
    pub pandoc: Option<PathBuf>,
    /// pdftotext / pdftoppm：PDF 处理引擎（受管目录 bin/poppler 下）
    pub poppler: Option<PathBuf>,
}

impl ToolPaths {
    pub fn ytdlp(&self) -> anyhow::Result<&Path> {
        self.ytdlp
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("未找到 yt-dlp，请在「设置 → 依赖工具」中自动安装"))
    }
    pub fn ffmpeg(&self) -> anyhow::Result<&Path> {
        self.ffmpeg
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("未找到 ffmpeg，请在「设置 → 依赖工具」中自动安装"))
    }
    pub fn ffprobe(&self) -> anyhow::Result<&Path> {
        self.ffprobe
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("未找到 ffprobe，请在「设置 → 依赖工具」中自动安装"))
    }
    pub fn whisper(&self) -> anyhow::Result<&Path> {
        self.whisper
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("未找到 whisper，请在「设置 → 依赖工具」中自动安装"))
    }
    /// aria2c：分段并行引擎（HTTP/FTP/BT/磁力）
    pub fn aria2(&self) -> anyhow::Result<&Path> {
        self.aria2.as_deref().ok_or_else(|| {
            anyhow::anyhow!("未找到 aria2c 分段引擎，请在「设置 → 依赖工具」中一键安装")
        })
    }
    /// ffmpeg 所在目录（传给 yt-dlp --ffmpeg-location）
    pub fn ffmpeg_dir(&self) -> Option<PathBuf> {
        self.ffmpeg.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf()))
    }
}

/// 一次任务执行的上下文
#[derive(Clone)]
pub struct Ctx {
    pub dirs: AppDirs,
    pub tools: ToolPaths,
    pub settings: AppSettings,
    pub emit: Option<Emitter>,
}

impl Ctx {
    pub fn new(mut dirs: AppDirs, tools: ToolPaths, settings: AppSettings) -> Self {
        // 受管工具根目录的唯一落地点：设置里的 tool_dir（空 / 非法 → 数据目录下的 bin/）。
        // 所有工具解析 / 调用 / 安装 / 清扫都读 dirs.bin，因此换目录在这里生效一次即可全局跟随。
        dirs.bin = crate::tools::effective_tool_dir(&dirs.bin, &settings.tool_dir);
        Self { dirs, tools, settings, emit: None }
    }

    pub fn with_emit(mut self, emit: Emitter) -> Self {
        self.emit = Some(emit);
        self
    }

    pub fn emit_json<T: serde::Serialize>(&self, event: &str, payload: &T) {
        if let Some(e) = &self.emit {
            match serde_json::to_value(payload) {
                Ok(v) => e(event, v),
                Err(err) => cwarn(&format!("序列化事件失败 {}: {err}", event)),
            }
        }
    }

    pub fn emit_raw(&self, event: &str, payload: serde_json::Value) {
        if let Some(e) = &self.emit {
            e(event, payload);
        }
    }

    /// 当前模型文件路径。内置模型存短名（base → ggml-base.bin），
    /// 自定义模型直接存文件名（已是 .bin 时原样使用）
    pub fn model_path(&self) -> PathBuf {
        self.dirs.models.join(crate::tools::model_file_name(&self.settings.whisper_model))
    }

    pub fn download_dir(&self) -> PathBuf {
        let d = if self.settings.download_dir.trim().is_empty() {
            self.dirs.downloads.clone()
        } else {
            PathBuf::from(&self.settings.download_dir)
        };
        let _ = std::fs::create_dir_all(&d);
        d
    }
}

/// 简易告警输出（避免依赖 log crate）
pub fn cwarn(msg: &str) {
    eprintln!("[umi][warn] {msg}");
}

/// Windows 下隐藏子进程控制台窗口
pub fn command_for(program: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// 创建目录并返回
pub fn ensure_dir(p: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(p)
}

/// Windows 下按进程树杀进程；其它平台直接 kill
pub fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let _ = cmd.status();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill").args(["-9", &pid.to_string()]).status();
    }
}

/// 宽松解码子进程输出（UTF-8 优先，失败回落 GBK）
pub fn decode_output(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let (cow, _, _) = encoding_rs::GBK.decode(bytes);
            cow.into_owned()
        }
    }
}

/// 把字节流按缓冲块切分成「行」，供进度解析使用
pub struct LineBuffer {
    buf: Vec<u8>,
}

impl LineBuffer {
    pub fn new() -> Self {
        Self { buf: Vec::with_capacity(8192) }
    }

    /// 追加数据，返回完整的行（已解码）
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n' || b == b'\r') {
            let line: Vec<u8> = self.buf.drain(..pos).collect();
            self.buf.drain(..1);
            let text = decode_output(&line).trim().to_string();
            if !text.is_empty() {
                out.push(text);
            }
        }
        out
    }

    /// 冲刷残留（无换行结尾的最后一行）
    pub fn flush(&mut self) -> Option<String> {
        if self.buf.is_empty() {
            return None;
        }
        let rest: Vec<u8> = self.buf.drain(..).collect();
        let text = decode_output(&rest).trim().to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }
}

impl Default for LineBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/* ==================== 受管引擎残留清扫（BUG-03） ==================== */

/// 受管的「瞬时」引擎：应用退出后不应该继续留在系统里。
pub const SWEEP_ENGINES: [&str; 5] =
    ["aria2c.exe", "yt-dlp.exe", "ffmpeg.exe", "ffprobe.exe", "whisper-cli.exe"];

/// PowerShell 单引号字符串转义（内部单引号写两遍）
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// 生成「清扫残留引擎」脚本：把 bin 目录下、父进程已经不存在的受管引擎全部结束掉。
/// 返回结束掉的进程数。只认 bin 目录里的瞬时引擎，绝不会误伤别的程序。
pub fn orphan_sweep_script(bin_dir: &Path) -> String {
    let names: Vec<String> = SWEEP_ENGINES.iter().map(|n| ps_quote(n)).collect();
    format!(
        "$ErrorActionPreference='SilentlyContinue'; \
         $bin={bin}; $names=@({names}); \
         $all=Get-CimInstance Win32_Process; \
         $alive=@{{}}; foreach($p in $all) {{ $alive[[int]$p.ProcessId]=1 }}; \
         $killed=0; \
         foreach($p in $all) {{ \
           if($names -notcontains $p.Name) {{ continue }}; \
           if(-not $p.ExecutablePath) {{ continue }}; \
           if(-not ($p.ExecutablePath.StartsWith($bin,'OrdinalIgnoreCase'))) {{ continue }}; \
           if($alive.ContainsKey([int]$p.ParentProcessId)) {{ continue }}; \
           Stop-Process -Id $p.ProcessId -Force; $killed++ \
         }}; \
         Write-Output $killed",
        bin = ps_quote(&bin_dir.to_string_lossy()),
        names = names.join(",")
    )
}

/// 启动时清扫：回收「父进程已死」的受管引擎（上次异常退出留下的孤儿进程）。
/// 非 Windows 平台直接返回 0。
#[cfg(windows)]
pub fn sweep_orphan_engines(bin_dir: &Path) -> usize {
    use std::os::windows::process::CommandExt;
    if !bin_dir.is_dir() {
        return 0;
    }
    let script = orphan_sweep_script(bin_dir);
    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script]);
    cmd.creation_flags(0x0800_0000);
    let out = match cmd.output() {
        Ok(o) => o,
        Err(_) => return 0,
    };
    String::from_utf8_lossy(&out.stdout).trim().parse::<usize>().unwrap_or(0)
}

#[cfg(not(windows))]
pub fn sweep_orphan_engines(_bin_dir: &Path) -> usize {
    0
}

/// 生成一个短随机 id
pub fn short_id() -> String {
    uuid::Uuid::new_v4().to_string()[..8].to_string()
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod fix18_tests {
    use super::*;

    /// BUG-03 端到端（默认忽略，需要真实 PowerShell）：
    /// 生成的脚本必须能被解析执行，并返回「回收了几个进程」。
    ///   cargo test --lib -- --ignored orphan_sweep --nocapture
    #[test]
    #[ignore]
    fn orphan_sweep_runs_on_real_powershell() {
        let missing = std::env::temp_dir().join("umi-sweep-not-exist-xyz");
        assert_eq!(sweep_orphan_engines(&missing), 0, "目录不存在时直接 0");
        let bin = AppDirs::new().bin;
        let n = sweep_orphan_engines(&bin);
        println!("真实清扫：{n} 个残留引擎进程");
        let script = orphan_sweep_script(&bin);
        println!("脚本长度 {} 字符", script.len());
        assert!(n < 100, "回收数量异常：{n}");
    }

    /// BUG-03：启动清扫只认 bin 目录里的「瞬时」引擎，且按「父进程已死」判定孤儿
    #[test]
    fn orphan_sweep_script_is_narrow_and_safe() {
        let s = orphan_sweep_script(Path::new("C:/Users/user/AppData/Roaming/umi-downloader/bin"));
        for name in SWEEP_ENGINES {
            assert!(s.contains(&format!("'{name}'")), "缺少 {name}：{s}");
        }
        assert!(s.contains("ParentProcessId"), "必须按父进程是否存活判断孤儿");
        assert!(s.contains("Stop-Process -Id"), "要真的结束进程");
        assert!(s.contains("C:/Users/user/AppData/Roaming/umi-downloader/bin"));
        // 路径里的单引号必须转义，否则脚本会被注入/语法错误
        let s2 = orphan_sweep_script(Path::new("C:/it's/bin"));
        assert!(s2.contains("'C:/it''s/bin'"), "{s2}");
        // 大括号要正确落地（PowerShell 的 @{} 不能被 format! 吃掉）
        assert!(s.contains("$alive=@{};"), "{s}");
    }
}
