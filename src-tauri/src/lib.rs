//! Umidl 后端核心
pub mod converter;
pub mod docs;
pub mod plugins;
pub mod ctx;
pub mod db;
pub mod downloader;
pub mod github;
/// 本地 HTTP 测试服务：仅供端到端自检使用
#[cfg(feature = "selftest")]
pub mod http_testserver;
pub mod logs;
pub mod models;
pub mod ratelimit;
/// 端到端自检：仅开发 / CI 构建包含
#[cfg(feature = "selftest")]
pub mod selftest;
pub mod settings;
pub mod subtitle;
pub mod tools;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager, State};

use ctx::{AppDirs, Ctx, ToolPaths};
use db::Db;
use models::*;

/* ==================== 应用状态 ==================== */

/// 用户主动停止任务时的意图：暂停（可续传） vs 取消（丢弃半成品）—— BUG-04
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopIntent {
    Pause,
    Cancel,
}

pub struct AppStateInner {
    pub dirs: AppDirs,
    pub db: Arc<Db>,
    pub settings: Mutex<AppSettings>,
    pub cancelled: Mutex<HashSet<String>>,
    pub pids: Mutex<HashMap<String, u32>>,
    pub report: Mutex<Option<TestReport>>,
    pub running: Mutex<HashSet<String>>,
    /// 停止意图（暂停 / 取消）：任务结束时据此决定最终状态
    pub stop_intent: Mutex<HashMap<String, StopIntent>>,
    /// 已删除任务的「墓碑」：运行中的线程不得再把它们写回数据库（BUG-02）
    pub removed: Mutex<HashSet<String>>,
    /// 队列泵串行化：「判定 + 占槽」必须原子，否则并发入队会把上限冲掉（BUG-05）
    pub queue_lock: Mutex<()>,
}

#[derive(Clone)]
pub struct AppState(pub Arc<AppStateInner>);

impl AppStateInner {
    pub fn settings_snapshot(&self) -> AppSettings {
        self.settings.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn mark_cancel(&self, id: &str) {
        if let Ok(mut c) = self.cancelled.lock() {
            c.insert(id.to_string());
        }
    }
    pub fn clear_cancel(&self, id: &str) {
        if let Ok(mut c) = self.cancelled.lock() {
            c.remove(id);
        }
    }
    pub fn is_cancelled(&self, id: &str) -> bool {
        self.cancelled.lock().map(|c| c.contains(id)).unwrap_or(false)
    }
    pub fn register_pid(&self, id: &str, pid: u32) {
        if let Ok(mut m) = self.pids.lock() {
            m.insert(id.to_string(), pid);
        }
    }
    pub fn take_pid(&self, id: &str) -> Option<u32> {
        self.pids.lock().ok().and_then(|mut m| m.remove(id))
    }
    pub fn mark_running(&self, id: &str) {
        if let Ok(mut r) = self.running.lock() {
            r.insert(id.to_string());
        }
    }
    pub fn unmark_running(&self, id: &str) {
        if let Ok(mut r) = self.running.lock() {
            r.remove(id);
        }
    }
    pub fn is_running(&self, id: &str) -> bool {
        self.running.lock().map(|r| r.contains(id)).unwrap_or(false)
    }
    pub fn running_count(&self) -> usize {
        self.running.lock().map(|r| r.len()).unwrap_or(0)
    }
    /// 记录「用户为什么停下来」：暂停还是取消
    pub fn mark_stop(&self, id: &str, intent: StopIntent) {
        if let Ok(mut m) = self.stop_intent.lock() {
            m.insert(id.to_string(), intent);
        }
    }
    pub fn stop_intent(&self, id: &str) -> Option<StopIntent> {
        self.stop_intent.lock().ok().and_then(|m| m.get(id).copied())
    }
    pub fn clear_stop(&self, id: &str) {
        if let Ok(mut m) = self.stop_intent.lock() {
            m.remove(id);
        }
    }
    /// 立墓碑：任务被删除后，仍在跑的线程不再写库、不再让它复活
    pub fn mark_removed(&self, id: &str) {
        if let Ok(mut r) = self.removed.lock() {
            r.insert(id.to_string());
        }
    }
    pub fn is_removed(&self, id: &str) -> bool {
        self.removed.lock().map(|r| r.contains(id)).unwrap_or(false)
    }
    /// 运行期线程是否还应该把任务写回数据库（删除过的任务不复活）
    pub fn should_persist(&self, id: &str) -> bool {
        !self.is_removed(id)
    }
    /// 队列泵的独占锁（中毒也照常放行，不 panic）
    pub fn lock_queue(&self) -> std::sync::MutexGuard<'_, ()> {
        self.queue_lock.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn build_ctx(state: &AppStateInner, app: Option<&AppHandle>) -> Ctx {
    let settings = state.settings_snapshot();
    let mut ctx = Ctx::new(state.dirs.clone(), ToolPaths::default(), settings);
    let tools = tools::resolve_all(&ctx);
    ctx.tools = tools;
    if let Some(a) = app {
        let handle = a.clone();
        ctx = ctx.with_emit(Arc::new(move |event: &str, payload: serde_json::Value| {
            let _ = handle.emit(event, payload);
        }));
    }
    ctx
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/* ==================== 工具 ==================== */

/// 给历史「已完成但没封面」的任务补封面：用 ffmpeg 从本地视频抽帧，
/// 离线也能用；单次最多处理 40 条，避免首次启动耗时过长。
#[tauri::command]
async fn backfill_covers(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<usize, String> {
    let inner = state.0.clone();
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let ctx = build_ctx(&inner, Some(&app2));
        let list = inner.db.list_downloads().unwrap_or_default();
        let mut n = 0usize;
        for mut t in list {
            // 有本地封面文件就跳过；远程 URL（会被热链保护挡住）视为需要补
            let has_local = t
                .thumbnail
                .as_deref()
                .map(|p| !p.starts_with("http"))
                .unwrap_or(false);
            if has_local || !matches!(t.status, TaskStatus::Done) {
                continue;
            }
            let mut got: Option<String> = None;
            // 1) 远程封面 → 先带 Referer 缓存到本地（绕过热链保护）
            if let Some(url) = t.thumbnail.clone().filter(|u| u.starts_with("http")) {
                got = crate::downloader::cache_thumbnail(&ctx, &url, &t.id, &t.url);
            }
            // 2) 仍拿不到 → 从本地视频抽一帧
            if got.is_none() {
                if let Some(fp) = t.file_path.clone() {
                    got = crate::downloader::cover_from_file(&ctx, fp.as_str(), &t.id, t.duration);
                }
            }
            if let Some(c) = got {
                t.thumbnail = Some(c);
                if inner.db.upsert_download(&t, None).is_ok() {
                    // 走统一出口：补封面事件同样要带正确的 file_exists，
                    // 否则历史任务卡片会被这条事件打成「文件已丢失」
                    emit_task(&app2, &t);
                    n += 1;
                }
            }
            if n >= 40 {
                break;
            }
        }
        n
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn detect_tools(state: State<'_, AppState>) -> Result<Vec<ToolStatus>, String> {
    // 工具检测要启动多个子进程（yt-dlp --version 约 1.4s），必须放到阻塞线程池：
    // 同步命令会占住 Tauri 命令线程，导致同一时刻的 getSettings / 任务列表全部排队。
    let inner = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let ctx = build_ctx(&inner, None);
        tools::detect(&ctx)
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn install_tool(app: AppHandle, state: State<'_, AppState>, name: String) -> Result<ToolStatus, String> {
    let s = state.0.clone();
    let ctx = build_ctx(&s, Some(&app));
    tauri::async_runtime::spawn_blocking(move || tools::install_tool(&ctx, &name).map_err(err))
        .await
        .map_err(err)?
}

#[tauri::command]
async fn install_whisper_model(
    app: AppHandle,
    state: State<'_, AppState>,
    model: String,
) -> Result<ToolStatus, String> {
    let s = state.0.clone();
    let ctx = build_ctx(&s, Some(&app));
    tauri::async_runtime::spawn_blocking(move || tools::install_whisper_model(&ctx, &model).map_err(err))
        .await
        .map_err(err)?
}

/// 设置窗口主题：浅色/深色强制窗口（同时影响 WebView2 的 prefers-color-scheme），
/// 「跟随系统」传 None —— 这才能让系统昼夜切换真正被识别（配置里写死 theme 会屏蔽它）
#[tauri::command]
fn set_window_theme(app: AppHandle, mode: String) -> Result<(), String> {
    let theme = match mode.as_str() {
        "light" => Some(tauri::Theme::Light),
        "dark" => Some(tauri::Theme::Dark),
        _ => None,
    };
    if let Some(w) = app.get_webview_window("main") {
        w.set_theme(theme).map_err(err)?;
    }
    Ok(())
}

/// 列出 Whisper 模型（内置 + 自定义）：是否已下载、体积、当前使用
#[tauri::command]
async fn list_whisper_models(state: State<'_, AppState>) -> Result<Vec<WhisperModelInfo>, String> {
    let inner = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let ctx = build_ctx(&inner, None);
        tools::list_whisper_models(&ctx)
    })
    .await
    .map_err(|e| e.to_string())
}

/// 自定义模型导入：支持 http(s) 链接、本地 .bin 路径、官方仓库文件名
#[tauri::command]
async fn install_whisper_custom(
    app: AppHandle,
    state: State<'_, AppState>,
    spec: String,
) -> Result<ToolStatus, String> {
    let s = state.0.clone();
    let ctx = build_ctx(&s, Some(&app));
    tauri::async_runtime::spawn_blocking(move || {
        tools::install_whisper_custom(&ctx, &spec).map_err(err)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn delete_whisper_model(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let inner = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let ctx = build_ctx(&inner, None);
        tools::delete_whisper_model(&ctx, &name).map_err(err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/* ═════════════════ 1.9：受管工具目录（自定义目录 + 按需勾选下载） ═════════════════ */

/// 当前工具目录状况：生效目录（绝对路径）+ 默认目录 + 设置里的原始值
fn tool_dir_info_of(dirs: &AppDirs, configured: &str) -> ToolDirInfo {
    let default_dir = dirs.bin.clone();
    let configured = configured.trim().to_string();
    let (dir, valid, error) = if configured.is_empty() {
        (default_dir.clone(), true, None)
    } else if tools::is_absolute_path(&configured) {
        (PathBuf::from(&configured), true, None)
    } else {
        (
            default_dir.clone(),
            false,
            Some(format!(
                "[tools.dir.relative] {configured} :: 配置里的工具目录不是绝对路径，已回退默认目录"
            )),
        )
    };
    ToolDirInfo {
        dir: dir.to_string_lossy().to_string(),
        default_dir: default_dir.to_string_lossy().to_string(),
        custom: !configured.is_empty(),
        configured,
        valid,
        error,
    }
}

/// 工具目录现状（设置页回显「生效目录」：始终是绝对路径）
#[tauri::command]
fn tool_dir_info(state: State<'_, AppState>) -> ToolDirInfo {
    tool_dir_info_of(&state.0.dirs, &state.0.settings_snapshot().tool_dir)
}

/// 更换工具安装目录：`migrate = true` 时把已有工具搬过去
/// （先复制 + 体积校验，全部成功才删旧目录、才落盘设置；失败则旧目录原样保留并报准确原因）
#[tauri::command]
async fn set_tool_dir(
    state: State<'_, AppState>,
    dir: String,
    migrate: bool,
) -> Result<ToolDirChange, String> {
    let s = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let cur = s.settings_snapshot();
        let from = tools::effective_tool_dir(&s.dirs.bin, &cur.tool_dir);
        // 空字符串 = 恢复默认目录（数据目录下的 bin/）
        let target = if dir.trim().is_empty() {
            s.dirs.bin.clone()
        } else {
            tools::validate_tool_dir(&dir).map_err(err)?
        };
        let rep = if migrate && target != from {
            let r = tools::migrate_tool_dir(&from, &target);
            if !r.ok {
                // 迁移失败：不落盘、不删旧 —— 报清楚哪一条失败
                return Err(format!(
                    "[tools.dir.migrate_failed] {} :: {}",
                    target.display(),
                    r.errors.join("；")
                ));
            }
            Some(r)
        } else {
            None
        };
        let mut next = cur.clone();
        next.tool_dir = if target == s.dirs.bin {
            String::new()
        } else {
            target.to_string_lossy().to_string()
        };
        settings::save(&s.dirs, &next).map_err(err)?;
        if let Ok(mut g) = s.settings.lock() {
            *g = next.clone();
        }
        tools::invalidate_memo();
        let info = tool_dir_info_of(&s.dirs, &next.tool_dir);
        logs::log_line(
            &s.dirs,
            "tools",
            &format!(
                "工具目录：{} → {}（迁移 {} 个文件 / {} 字节，删除旧条目 {}）",
                from.display(),
                info.dir,
                rep.as_ref().map(|r| r.files_copied).unwrap_or(0),
                rep.as_ref().map(|r| r.bytes_copied).unwrap_or(0),
                rep.as_ref().map(|r| r.removed_old).unwrap_or(0),
            ),
        );
        Ok(ToolDirChange {
            previous_dir: from.to_string_lossy().to_string(),
            migrated: rep.is_some(),
            files_copied: rep.as_ref().map(|r| r.files_copied).unwrap_or(0),
            bytes_copied: rep.as_ref().map(|r| r.bytes_copied).unwrap_or(0),
            already_present: rep.as_ref().map(|r| r.already_present).unwrap_or(0),
            removed_old: rep.as_ref().map(|r| r.removed_old).unwrap_or(0),
            leftovers: rep.as_ref().map(|r| r.leftovers.clone()).unwrap_or_default(),
            skipped: rep.as_ref().map(|r| r.skipped.clone()).unwrap_or_default(),
            info,
        })
    })
    .await
    .map_err(err)?
}

/// 批量安装所选工具（「下载所选」）：逐个装、互不影响，已装的可重装
#[tauri::command]
async fn install_tools(
    app: AppHandle,
    state: State<'_, AppState>,
    names: Vec<String>,
) -> Result<Vec<ToolInstallOutcome>, String> {
    let s = state.0.clone();
    let ctx = build_ctx(&s, Some(&app));
    tauri::async_runtime::spawn_blocking(move || tools::install_tools(&ctx, &names))
        .await
        .map_err(err)
}

/// 校验所选工具：绕过版本缓存真的启动一次程序，返回实际路径 / 版本 / 可用性
#[tauri::command]
async fn verify_tools(
    state: State<'_, AppState>,
    names: Vec<String>,
) -> Result<Vec<ToolInstallOutcome>, String> {
    let inner = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let ctx = build_ctx(&inner, None);
        tools::verify_tools(&ctx, &names)
    })
    .await
    .map_err(err)
}

#[tauri::command]
fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

/* ==================== 设置 ==================== */

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> AppSettings {
    state.0.settings_snapshot()
}

/// 前端性能埋点：把前端各阶段耗时写到 stderr，便于定位启动瓶颈
#[tauri::command]
fn frontend_log(msg: String) {
    eprintln!("[umi][fe] {msg}");
}

#[tauri::command]
fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AppSettings,
) -> Result<AppSettings, String> {
    let mut s = settings;
    if s.download_dir.trim().is_empty() {
        s.download_dir = state.0.dirs.downloads.to_string_lossy().to_string();
    }
    settings::save(&state.0.dirs, &s).map_err(err)?;
    if let Ok(mut g) = state.0.settings.lock() {
        *g = s.clone();
    }
    // 全局限速（令牌桶）：设置变化即时生效
    ratelimit::LIMITER.configure(if s.speed_limit_enabled {
        s.speed_limit_kb
    } else {
        0
    });
    // 并发上限可能被调大 → 立刻唤醒排队任务补位（BUG-05）
    pump_queue(&state.0, &app);
    Ok(s)
}

#[tauri::command]
fn default_download_dir(state: State<'_, AppState>) -> String {
    state.0.dirs.downloads.to_string_lossy().to_string()
}

#[tauri::command]
fn app_data_dir(state: State<'_, AppState>) -> String {
    state.0.dirs.data.to_string_lossy().to_string()
}

/* ==================== 自定义 LOGO ==================== */

/// 允许的图片格式；窗口/任务栏图标另外只支持 png / ico
const LOGO_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "webp", "svg", "ico"];
const LOGO_MAX_BYTES: u64 = 4 * 1024 * 1024;

fn logo_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("branding")
}

fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// 校验并落盘自定义 LOGO（纯路径操作，便于自检复用）；返回保存后的路径
fn apply_logo_file(data_dir: &Path, src: &Path) -> Result<PathBuf, String> {
    if !src.is_file() {
        return Err("选择的图片不存在或不是文件".into());
    }
    let ext = ext_of(src);
    if !LOGO_EXTS.contains(&ext.as_str()) {
        return Err(format!(
            "不支持的图片格式：.{}（支持 PNG / JPG / WEBP / SVG / ICO）",
            if ext.is_empty() { "未知" } else { ext.as_str() }
        ));
    }
    let len = std::fs::metadata(src).map_err(err)?.len();
    if len > LOGO_MAX_BYTES {
        return Err(format!(
            "图片过大（{:.1} MB），请选择 4 MB 以内的图片",
            len as f64 / 1048576.0
        ));
    }
    let dir = logo_dir(data_dir);
    std::fs::create_dir_all(&dir).map_err(err)?;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            let is_logo = p
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("logo."))
                .unwrap_or(false);
            if is_logo {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    let dst = dir.join(format!("logo.{ext}"));
    std::fs::copy(src, &dst).map_err(err)?;
    Ok(dst)
}

/// 写入设置并落盘（保持内存与磁盘一致）
fn persist_logo(state: &State<'_, AppState>, path: Option<String>) -> Result<(), String> {
    let mut g = state.0.settings.lock().map_err(|_| "设置写入失败".to_string())?;
    g.custom_logo = path;
    let snapshot = g.clone();
    drop(g);
    settings::save(&state.0.dirs, &snapshot).map_err(err)
}

#[tauri::command]
fn set_custom_logo(app: AppHandle, state: State<'_, AppState>, path: String) -> Result<String, String> {
    let dst = apply_logo_file(&state.0.dirs.data, Path::new(&path))?;
    let saved = dst.to_string_lossy().to_string();
    persist_logo(&state, Some(saved.clone()))?;
    // 窗口 / 任务栏图标：仅位图格式可解码，SVG/WEBP 保持默认图标
    let ext = ext_of(&dst);
    if let Some(win) = app.get_webview_window("main") {
        if matches!(ext.as_str(), "png" | "ico") {
            if let Ok(bytes) = std::fs::read(&dst) {
                if let Ok(img) = tauri::image::Image::from_bytes(&bytes) {
                    let _ = win.set_icon(img);
                }
            }
        }
    }
    Ok(saved)
}

#[tauri::command]
fn clear_custom_logo(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let dir = logo_dir(&state.0.dirs.data);
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let _ = std::fs::remove_file(e.path());
        }
    }
    persist_logo(&state, None)?;
    if let Some(win) = app.get_webview_window("main") {
        if let Some(icon) = app.default_window_icon().cloned() {
            let _ = win.set_icon(icon);
        }
    }
    Ok(())
}

/// 完成后系统通知（Windows 通知区域气泡，无需额外依赖或注册）
fn notify_finish(title: &str, body: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let esc = |s: &str| s.replace('\'', "''").replace('\n', " ").replace('\r', "");
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; \
             $n = New-Object System.Windows.Forms.NotifyIcon; \
             $n.Icon = [System.Drawing.SystemIcons]::Information; \
             $n.Visible = $true; \
             $n.ShowBalloonTip(6000, '{}', '{}', [System.Windows.Forms.ToolTipIcon]::Info); \
             Start-Sleep -Seconds 6; \
             $n.Dispose()",
            esc(title),
            esc(body)
        );
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
            .creation_flags(0x0800_0000)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = (title, body);
    }
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("路径不存在：{path}"));
    }
    #[cfg(windows)]
    {
        // 系统 Shell 打开（等价于双击）。走 ShellExecuteW 而不是 `cmd /C start`：
        //   * 路径里的 `&` `^` `%VAR%` 不会被 cmd 当元字符吃掉（文件名带 `%PATH%` 会被展开、
        //     带 `&` / `^` 会把命令岔成另一条 —— 结果开了个空 cmd 窗口，文件根本没打开）；
        //   * 返回值如实反映成败（≤ 32 是错误码），不再“调用成功就报成功”。
        shell_open(&p)
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(&path).spawn().map_err(err)?;
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(&path).spawn().map_err(err)?;
        Ok(())
    }
}

/// 以 NUL 结尾的 UTF-16 缓冲（Windows 宽字符 API 的字符串格式）。
/// ShellExecuteW 只认这种形式 —— 路径里的空格、中文引号、`%VAR%`、`&` 都不需要任何转义。
#[cfg(windows)]
fn wide_nul(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// 用系统 Shell 打开文件 / 目录（Windows）。
///
/// 用 ShellExecuteW 而不是起 `cmd /C start`：cmd 会解析 `&` `^` `%VAR%` 等元字符，
/// 文件名里带上这些字符就会「开个空窗口、文件没打开」，而 spawn 还返回成功 —— 用户看到的就是
/// 「点了打开，啥都没发生」。ShellExecuteW 直接收宽字符路径，且返回码能反映出成败。
#[cfg(windows)]
fn shell_open(p: &std::path::Path) -> Result<(), String> {
    // shell32 的 ShellExecuteW：本 crate 不直接依赖 windows-sys，这里按需声明（只用到这一个函数）
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: *mut core::ffi::c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show_cmd: i32,
        ) -> *mut core::ffi::c_void;
    }
    const SW_SHOWNORMAL: i32 = 1;
    let verb = wide_nul("open");
    let file = wide_nul(&p.to_string_lossy());
    // SAFETY: 两个指针都指向以 NUL 结尾、且在本次调用期间存活的 UTF-16 缓冲；其余参数为 NULL。
    let rc = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    let code = rc as isize;
    if code <= 32 {
        Err(format!(
            "系统无法打开（ShellExecute 返回 {code}）：{}",
            p.display()
        ))
    } else {
        Ok(())
    }
}

/// Explorer 的 `/select,"<路径>"` 参数：整串路径包在引号里，空格 / 中文 / 引号原样保留。
/// 用 `raw_arg` 原样传给 explorer（它是直接读命令行的 GUI 程序，不走 cmd 的元字符解析）。
pub(crate) fn reveal_select_arg(path: &std::path::Path) -> String {
    format!("/select,\"{}\"", path.display())
}

#[tauri::command]
fn reveal_path(path: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(&path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        return match reveal_target(&p) {
            // 文件还在 → 在资源管理器里选中它（路径含空格时整串加引号交给 raw_arg，别再经过 cmd）
            Some(t) if t.is_file() => std::process::Command::new("explorer")
                .raw_arg(reveal_select_arg(&t))
                .creation_flags(0x0800_0000)
                .spawn()
                .map(|_| ())
                .map_err(err),
            // 本身就是个目录 → 直接打开它
            Some(t) => std::process::Command::new("explorer")
                .arg(t.to_string_lossy().to_string())
                .creation_flags(0x0800_0000)
                .spawn()
                .map(|_| ())
                .map_err(err),
            // 文件真不在了 → 明确报错。**绝不退回父目录**：那会让按钮“看起来成功”，
            // 用户拿到一个没选中任何文件的窗口，看到的就是「字幕文件不在文件夹里显示」。
            None => Err(format!("文件已不存在（可能被移动或删除）：{}", p.display())),
        };
    }
    #[cfg(not(windows))]
    {
        if !p.exists() {
            return Err(format!("文件已不存在（可能被移动或删除）：{}", p.display()));
        }
        open_path(p.to_string_lossy().to_string())
    }
}

#[tauri::command]
fn read_text_file(path: String, limit: Option<usize>) -> Result<String, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| format!("读取失败：{e}"))?;
    let max = limit.unwrap_or(400_000);
    if text.len() > max {
        Ok(text.chars().take(max).collect())
    } else {
        Ok(text)
    }
}

/* ==================== 下载 ==================== */

#[tauri::command]
async fn probe_url(state: State<'_, AppState>, url: String, playlist: bool) -> Result<MediaInfo, String> {
    let s = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let ctx = build_ctx(&s, None);
        downloader::probe(&ctx, &url, playlist).map_err(err)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
fn list_downloads(state: State<'_, AppState>) -> Result<Vec<DownloadTask>, String> {
    let mut list = state.0.db.list_downloads().map_err(err)?;
    annotate_file_exists(&mut list);
    Ok(list)
}

/// 标注每条任务的文件是否还在磁盘上（界面据此隐藏"打开/在文件夹中显示"按钮）
pub(crate) fn annotate_file_exists(list: &mut [DownloadTask]) {
    for t in list.iter_mut() {
        t.file_exists = match t.file_path.as_ref() {
            Some(p) if !p.trim().is_empty() => std::path::Path::new(p).is_file(),
            _ => false,
        };
    }
}

/// "在文件夹中显示"应该打开的目标 —— **只认盘上真实存在的东西**：
///   文件还在   → 该文件（Explorer 会选中它）
///   是个目录   → 该目录
///   文件没了   → None，调用方必须报错。
///
/// 这里**绝不退回父目录**（曾经的行为）：父目录一开，按钮就“看起来成功”了，可用户拿到的是
/// 一个没选中任何文件的窗口 —— 「生成的字幕文件不在文件夹里显示」正是这么来的。
/// 诚实的做法是把「文件已不存在」摊给用户，而不是开个空窗口假装定位成功。
pub(crate) fn reveal_target(path: &std::path::Path) -> Option<std::path::PathBuf> {
    if path.is_file() || path.is_dir() {
        Some(path.to_path_buf())
    } else {
        None
    }
}

/// 事件载荷：与 `list_downloads` 同口径——`file_exists` 反映磁盘真相。
///
/// 事件里带的是任务对象创建时的初值 `false`，前端拿到后会原样 upsert 进队列，
/// 把「文件其实还在磁盘上」的状态覆盖成「已丢失」——表现就是完成任务卡片
/// 误报「文件已丢失」、且「打开文件 / 在文件夹中显示」按钮整排消失。
/// 所有出口（进度 tick / 完成 / 暂停 / 取消 / 补封面）都经这里出，口径与列表接口一致。
pub(crate) fn event_payload(t: &DownloadTask) -> DownloadTask {
    let mut t = t.clone();
    annotate_file_exists(std::slice::from_mut(&mut t));
    t
}

fn emit_task(app: &AppHandle, t: &DownloadTask) {
    let _ = app.emit("download://update", event_payload(t));
}

/* ==================== 并发排队（BUG-05） ==================== */

/// 已在跑的任务数达到并发上限 → 该任务应该排队（不再起引擎进程）
pub fn should_queue(running: usize, limit: i64) -> bool {
    running >= limit.max(1) as usize
}

/// 本次可以放行的排队任务（按入队顺序，最多把空槽填满）—— 纯函数便于单测
pub fn plan_queue(running: usize, limit: i64, queued: &[String]) -> Vec<String> {
    let free = (limit.max(1) as usize).saturating_sub(running);
    queued.iter().take(free).cloned().collect()
}

/// 引擎停止后的任务状态：
///   取消        → Canceled（并清理半成品）
///   暂停 / 看门狗中止 → Paused（保留断点，可续传）
///   正常结束    → None（交给调用方的 exit_code 分支判断）
pub fn status_after_engine_stop(
    intent: Option<StopIntent>,
    canceled: bool,
    killed: bool,
) -> Option<TaskStatus> {
    if intent == Some(StopIntent::Cancel) {
        return Some(TaskStatus::Canceled);
    }
    if canceled || killed {
        return Some(TaskStatus::Paused);
    }
    None
}

/// 任务的输出目录（优先用请求里存的 output_dir，否则用当前下载目录）
fn task_output_dir(s: &Arc<AppStateInner>, id: &str) -> PathBuf {
    let from_req = s
        .db
        .get_download_request(id)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<DownloadRequest>(&raw).ok())
        .and_then(|r| r.output_dir)
        .filter(|d| !d.trim().is_empty())
        .map(PathBuf::from);
    from_req.unwrap_or_else(|| build_ctx(s, None).download_dir())
}

/// 下载产物的候选路径：`<dir>/<url 文件名>` 及其 `.aria2`（控制文件）/ `.part`（半成品）
pub fn download_artifact_paths(dir: &Path, url: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    if let Some(name) = downloader::url_file_name(url) {
        v.push(dir.join(format!("{name}.aria2")));
        v.push(dir.join(format!("{name}.part")));
        v.push(dir.join(&name));
    }
    v
}

/// 只清理引擎控制文件（`.aria2` / `.part`）：绝不会动已落盘的数据文件
pub fn cleanup_engine_files(dir: &Path, url: &str) {
    if let Some(name) = downloader::url_file_name(url) {
        let _ = std::fs::remove_file(dir.join(format!("{name}.aria2")));
        let _ = std::fs::remove_file(dir.join(format!("{name}.part")));
    }
}

/// 清理半成品：下载中的任务 file_path 还是 NULL，
/// 所以必须按「输出目录 + URL 基名」推断真实落盘位置（BUG-02 / BUG-04）
pub fn cleanup_download_artifacts(dir: &Path, url: &str, file_path: &Option<String>) {
    for p in download_artifact_paths(dir, url) {
        let _ = std::fs::remove_file(&p);
    }
    if let Some(p) = file_path.as_ref().filter(|p| !p.trim().is_empty()) {
        let _ = std::fs::remove_file(p);
        let _ = std::fs::remove_file(format!("{p}.part"));
        let _ = std::fs::remove_file(format!("{p}.aria2"));
    }
}

/// 有空槽就唤醒排队中的任务（任务结束 / 删除 / 续传 / 改并发设置时调用）
pub fn pump_queue(s: &Arc<AppStateInner>, app: &AppHandle) {
    let _g = s.lock_queue();
    let limit = s.settings_snapshot().concurrency;
    let mut guard = 0usize;
    loop {
        guard += 1;
        // 上限兜底：任何异常情况都不允许这里死循环
        if guard > 256 || should_queue(s.running_count(), limit) {
            break;
        }
        let next = match s.db.next_queued_download() {
            Ok(Some(v)) => v,
            _ => break,
        };
        let (id, raw) = next;
        if s.is_removed(&id) {
            // 排队期间被删除：清掉残留行再继续
            let _ = s.db.delete_download(&id);
            continue;
        }
        let req: DownloadRequest = match raw.as_deref().map(serde_json::from_str::<DownloadRequest>) {
            Some(Ok(r)) => r,
            _ => {
                if let Ok(Some(mut t)) = s.db.get_download(&id) {
                    t.status = TaskStatus::Error;
                    t.error = Some("排队任务缺少参数，无法启动（请重新创建任务）".into());
                    persist_download(s, &t);
                    emit_task(app, &t);
                }
                continue;
            }
        };
        let out_dir = req
            .output_dir
            .as_ref()
            .filter(|d| !d.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| build_ctx(s, None).download_dir());
        s.mark_running(&id);
        logs::log_line(&s.dirs, "download", &format!("排队任务开始 · {id} · {}", req.url));
        let s2 = s.clone();
        let app2 = app.clone();
        let id2 = id.clone();
        std::thread::spawn(move || {
            run_download_job(s2, app2, req, id2, out_dir);
        });
    }
}

#[tauri::command]
async fn start_download(
    app: AppHandle,
    state: State<'_, AppState>,
    req: DownloadRequest,
) -> Result<DownloadTask, String> {
    let s = state.0.clone();
    let limit = s.settings_snapshot().concurrency;
    let created = ctx::now_ms();
    let mut task = downloader::new_task(&req, created);
    // 封面本地化：下载任务卡片离线也能显示封面
    if let Some(tu) = req.thumbnail_url.clone().filter(|s| !s.trim().is_empty()) {
        let c = build_ctx(&s, None);
        let key = format!("task-{}", task.id);
        let referer = req.url.clone();
        if let Some(local) = downloader::cache_thumbnail(&c, &tu, &key, &referer) {
            task.thumbnail = Some(local);
        }
    }
    let out_dir = {
        let ctx = build_ctx(&s, None);
        req.output_dir
            .as_ref()
            .filter(|d| !d.trim().is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| ctx.download_dir())
    };
    // BUG-08：目标路径超过 Windows MAX_PATH（259）在入队时就报清楚，
    // 否则 aria2 只会回一句 “Download aborted”（真正原因 errorCode=18 被丢掉）
    downloader::check_output_path(&out_dir, &req)?;
    task.format_note = Some(match req.mode.as_str() {
        "audio" => format!("音频 · {}", req.audio_format.clone().unwrap_or_else(|| "mp3".into())),
        "thumb" => "封面图片".into(),
        "subs" => {
            let langs = if req.subtitle_langs.is_empty() {
                "全部语言".to_string()
            } else {
                req.subtitle_langs.join(" / ")
            };
            format!("字幕 · {langs}")
        }
        _ => req
            .format_id
            .clone()
            .map(|f| format!("视频 · {f}"))
            .unwrap_or_else(|| "视频 · 最佳".into()),
    });
    let request_json = serde_json::to_string(&req).ok();
    // BUG-05：并发已满 → 只落库排队（不起引擎、不占槽位），槽位释放时由队列泵唤醒
    let queued = {
        let _g = s.lock_queue();
        if should_queue(s.running_count(), limit) {
            true
        } else {
            s.mark_running(&task.id); // 预留槽位：并发入队不会把上限冲掉
            false
        }
    };
    if queued {
        task.status = TaskStatus::Pending;
    }
    s.db.upsert_download(&task, request_json.as_deref()).map_err(err)?;
    emit_task(&app, &task);
    if queued {
        logs::log_line(
            &s.dirs,
            "download",
            &format!("排队中（并发上限 {}）· {}", limit.max(1), req.url),
        );
        return Ok(task);
    }

    let s2 = s.clone();
    let app2 = app.clone();
    let req2 = req.clone();
    let task_id = task.id.clone();
    std::thread::spawn(move || {
        run_download_job(s2, app2, req2, task_id, out_dir);
    });
    Ok(task)
}

fn update_and_emit(s: &Arc<AppStateInner>, app: &AppHandle, task: &DownloadTask) {
    persist_download(s, task);
    emit_task(app, task);
}

/// 写库（已删除的任务直接跳过 —— 否则任务会被仍在运行的线程 upsert 回库「复活」，BUG-02）
fn persist_download(s: &Arc<AppStateInner>, task: &DownloadTask) {
    if !s.should_persist(&task.id) {
        return;
    }
    let _ = s.db.upsert_download(task, None);
}

/// 字幕任务写库（同样跳过已删除的任务）
fn persist_subtitle(s: &Arc<AppStateInner>, task: &SubtitleTask) {
    if !s.should_persist(&task.id) {
        return;
    }
    let _ = s.db.upsert_subtitle(task, None);
}

/// 下载结束后把结果派发给启用的插件（`download:done` / `download:error`）。
///
/// 插件用 `umi.retry(taskId)` 请求重试时，这里把任务改回 Pending 交回队列泵 ——
/// 与界面上点「继续」走同一套并发控制；重试次数上限由插件自己把控，
/// 每次请求都会写进「设置 → 插件 → 事件日志」，看得见、可排查。
fn dispatch_plugin_download_event(
    s: &Arc<AppStateInner>,
    app: &AppHandle,
    task: &DownloadTask,
    req: &DownloadRequest,
) {
    let event = match task.status {
        TaskStatus::Done => "download:done",
        TaskStatus::Error => "download:error",
        // 暂停 / 取消 / 交接给别人：不算结束，不派发
        _ => return,
    };
    let ctx = build_ctx(s, Some(app));
    let payload = serde_json::json!({
        "taskId": task.id,
        "task_id": task.id,
        "title": task.title,
        "url": req.url,
        "status": if task.status == TaskStatus::Done { "done" } else { "error" },
        "error": task.error,
        "filePath": task.file_path,
    });
    let result = crate::plugins::dispatch_event(&ctx, event, payload);
    let retry = crate::plugins::retry_task_ids(&result, &task.id);
    if retry.is_empty() {
        return;
    }
    for id in retry {
        if let Ok(Some(mut t)) = s.db.get_download(&id) {
            t.status = TaskStatus::Pending;
            t.error = None;
            t.speed = None;
            t.eta = None;
            persist_download(s, &t);
            emit_task(app, &t);
            logs::log_line(&s.dirs, "plugin", &format!("插件请求重试 · {id}"));
        }
    }
    pump_queue(s, app);
}

pub fn run_download_job(
    s: Arc<AppStateInner>,
    app: AppHandle,
    req: DownloadRequest,
    task_id: String,
    out_dir: std::path::PathBuf,
) {
    // 起线程之前就被删了 → 直接退出，不占槽位、不复活记录（BUG-02）
    if s.is_removed(&task_id) {
        return;
    }
    s.clear_cancel(&task_id);
    s.mark_running(&task_id);

    let ctx = build_ctx(&s, Some(&app));
    let mut task = s
        .db
        .get_download(&task_id)
        .ok()
        .flatten()
        .unwrap_or_else(|| downloader::new_task(&req, ctx::now_ms()));

    // 1) 解析媒体信息（拿到标题/封面/时长）
    task.status = TaskStatus::Parsing;
    task.error = None;
    update_and_emit(&s, &app, &task);

    match downloader::probe(&ctx, &req.url, req.playlist) {
        Ok(info) => {
            task.title = info.title.clone();
            task.thumbnail = info.thumbnail.clone();
            task.duration = info.duration;
            task.uploader = info.uploader.clone();
            if info.filesize_approx.is_some() {
                task.total = info.filesize_approx;
            }
            update_and_emit(&s, &app, &task);
        }
        Err(e) => {
            if task.title == req.url {
                // 解析失败不致命：部分站点可直接下载
                let _ = e;
            }
        }
    }

    task.status = TaskStatus::Downloading;
    update_and_emit(&s, &app, &task);

    // 2) 执行下载
    let s_tick = s.clone();
    let app_tick = app.clone();
    let id_tick = task_id.clone();
    let mut last_db = std::time::Instant::now();
    let mut last_emit = std::time::Instant::now();
    let mut last_emit_progress = -1.0f64;
    let mut last_task = task.clone();

    let s_pid = s.clone();
    let id_pid = task_id.clone();
    let s_cancel = s.clone();
    let id_cancel = task_id.clone();

    let engine_label = if downloader::wants_aria2(&req, &ctx.settings) {
        "aria2（16 连接分段并行）"
    } else {
        "yt-dlp（站点解析）"
    };
    logs::log_line(
        &ctx.dirs,
        "download",
        &format!("开始 · 引擎={engine_label} · {}", req.url),
    );
    let outcome = downloader::run_engine(
        &ctx,
        &req,
        &out_dir,
        move |tick| {
            last_task.progress = tick.percent.unwrap_or(last_task.progress);
            last_task.speed = tick.speed.clone();
            last_task.eta = tick.eta.clone();
            if tick.total.is_some() {
                last_task.total = tick.total;
            }
            if tick.downloaded.is_some() {
                last_task.downloaded = tick.downloaded;
            }
            // 性能：进度事件节流（最多 4 次/秒，或进度跳跃 ≥1.5%），避免高频 IPC + 重渲染
            let now = std::time::Instant::now();
            let jumped = (last_task.progress - last_emit_progress).abs() >= 1.5;
            if jumped || now.duration_since(last_emit).as_millis() >= 250 {
                last_emit = now;
                last_emit_progress = last_task.progress;
                emit_task(&app_tick, &last_task);
            }
            if last_db.elapsed().as_millis() > 700 {
                last_db = std::time::Instant::now();
                persist_download(&s_tick, &last_task);
            }
            let _ = (&id_tick,);
        },
        move |pid| s_pid.register_pid(&id_pid, pid),
        // 被删除也视同取消：进程立即停、不再往库里写
        move || s_cancel.is_cancelled(&id_cancel) || s_cancel.is_removed(&id_cancel),
    );

    s.take_pid(&task_id);
    s.unmark_running(&task_id);

    let canceled = s.is_cancelled(&task_id);
    // BUG-04：区分「暂停」与「取消」，收尾分支据此写最终状态
    let intent = s.stop_intent(&task_id);
    s.clear_cancel(&task_id);
    s.clear_stop(&task_id);
    logs::log_line(
        &ctx.dirs,
        "download",
        &format!(
            "结束 · 退出码={} · {}",
            outcome.as_ref().map(|o| o.exit_code).unwrap_or(-1),
            req.url
        ),
    );
    // 槽位已释放：唤醒排队中的任务（BUG-05）
    pump_queue(&s, &app);

    match outcome {
        Ok(o) => {
            task.speed = None;
            task.eta = None;
            if let Some(st) = status_after_engine_stop(intent, canceled, o.killed) {
                task.status = st;
                task.error = None;
                if st == TaskStatus::Paused {
                    if task.progress > 99.0 {
                        task.progress = 99.0;
                    }
                } else {
                    // 取消：清掉 aria2 控制文件与半成品，别留下续传不了的垃圾
                    let dir = out_dir.clone();
                    cleanup_download_artifacts(&dir, &req.url, &task.file_path);
                }
            } else if o.exit_code == 0 {
                task.status = TaskStatus::Done;
                task.progress = 100.0;
                task.file_path = o.final_path.clone().or_else(|| task.file_path.clone());
                task.error = None;
                if let Some(p) = task.file_path.as_ref() {
                    if let Ok(m) = std::fs::metadata(p) {
                        task.total = Some(m.len() as i64);
                        task.downloaded = Some(m.len() as i64);
                    }
                }
                // 封面：远程 URL 会被图床热链保护挡掉 → 先缓存到本地，
                // 失败再从视频抽一帧，保证队列里一定看得到封面
                let need_cover = task
                    .thumbnail
                    .as_deref()
                    .map(|p| p.starts_with("http"))
                    .unwrap_or(true);
                if need_cover {
                    let mut got = task
                        .thumbnail
                        .clone()
                        .filter(|p| p.starts_with("http"))
                        .and_then(|u| {
                            crate::downloader::cache_thumbnail(&ctx, &u, &task.id, &task.url)
                        });
                    if got.is_none() {
                        if let Some(p) = task.file_path.clone() {
                            got = crate::downloader::cover_from_file(&ctx, &p, &task.id, task.duration);
                        }
                    }
                    if got.is_some() {
                        task.thumbnail = got;
                    }
                }
                // 完成后自动定位到文件（可在设置中开关）
                if s.settings_snapshot().open_folder_when_done {
                    if let Some(p) = task.file_path.clone() {
                        let _ = reveal_path(p);
                    }
                }
                if s.settings_snapshot().notify_on_finish {
                    let body = task
                        .file_path
                        .as_ref()
                        .map(|p| {
                            std::path::Path::new(p)
                                .file_name()
                                .map(|f| f.to_string_lossy().to_string())
                                .unwrap_or_else(|| task.title.clone())
                        })
                        .unwrap_or_else(|| task.title.clone());
                    notify_finish("Umidl · 下载完成", &body);
                }
            } else {
                task.status = TaskStatus::Error;
                // BUG-08：保留 errorCode / Exception 这类真正的原因行，而不是只留最后一行
                let tail = downloader::best_error_line(&o.stderr_tail);
                task.error = Some(if tail.is_empty() { "下载失败".into() } else { tail });
            }
        }
        Err(e) => {
            task.status = TaskStatus::Error;
            task.error = Some(e.to_string());
        }
    }
    update_and_emit(&s, &app, &task);
    // 插件事件：把结束结果派发给启用的插件（失败时插件可请求重试，见上面的助手）
    dispatch_plugin_download_event(&s, &app, &task, &req);
}

#[tauri::command]
fn pause_download(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    // 先记「暂停」意图再杀进程：任务结束分支据此写 Paused（而不是 Canceled，BUG-04）
    state.0.mark_stop(&id, StopIntent::Pause);
    state.0.mark_cancel(&id);
    if let Some(pid) = state.0.pids.lock().ok().and_then(|m| m.get(&id).copied()) {
        ctx::kill_tree(pid);
    }
    std::thread::sleep(std::time::Duration::from_millis(120));
    if let Ok(Some(mut t)) = state.0.db.get_download(&id) {
        // 排队中的任务同样可以暂停（暂停后不再被队列泵取走）
        if t.status == TaskStatus::Downloading
            || t.status == TaskStatus::Parsing
            || t.status == TaskStatus::Pending
        {
            t.status = TaskStatus::Paused;
            t.speed = None;
            t.eta = None;
            let _ = state.0.db.upsert_download(&t, None);
            emit_task(&app, &t);
        }
    }
    Ok(())
}

#[tauri::command]
async fn resume_download(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<DownloadTask, String> {
    let s = state.0.clone();
    let task = s.db.get_download(&id).map_err(err)?.ok_or("任务不存在")?;
    let raw = s
        .db
        .get_download_request(&id)
        .map_err(err)?
        .ok_or("缺少任务参数，无法续传（请重新创建任务）")?;
    let req: DownloadRequest = serde_json::from_str(&raw).map_err(|e| format!("任务参数损坏：{e}"))?;
    let out_dir = req
        .output_dir
        .as_ref()
        .filter(|d| !d.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let ctx = build_ctx(&s, None);
            ctx.download_dir()
        });

    // BUG-05：并发已满时续传也要排队，不能越过上限
    let limit = s.settings_snapshot().concurrency;
    let queued = {
        let _g = s.lock_queue();
        if should_queue(s.running_count(), limit) {
            true
        } else {
            s.mark_running(&id);
            false
        }
    };
    if queued {
        let mut t = task.clone();
        t.status = TaskStatus::Pending;
        t.error = None;
        persist_download(&s, &t);
        emit_task(&app, &t);
        return Ok(t);
    }

    let s2 = s.clone();
    let app2 = app.clone();
    let id2 = id.clone();
    std::thread::spawn(move || {
        run_download_job(s2, app2, req, id2, out_dir);
    });
    Ok(task)
}

#[tauri::command]
fn cancel_download(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    let s = state.0.clone();
    // BUG-04：先记「取消」意图（结束分支据此写 Canceled 而不是 Paused），再杀进程
    s.mark_stop(&id, StopIntent::Cancel);
    s.mark_cancel(&id);
    if let Some(pid) = s.pids.lock().ok().and_then(|m| m.get(&id).copied()) {
        ctx::kill_tree(pid);
    }
    std::thread::sleep(std::time::Duration::from_millis(120));
    if let Ok(Some(mut t)) = s.db.get_download(&id) {
        // 还在下载中（没有最终产物）→ 取消即丢弃：连半成品一起清掉；
        // 已经有产物（如暂停过的任务）→ 只清 .aria2 / .part，别动数据文件
        let never_finished = t.file_path.is_none();
        t.status = TaskStatus::Canceled;
        t.speed = None;
        t.eta = None;
        if never_finished {
            let dir = task_output_dir(&s, &id);
            cleanup_download_artifacts(&dir, &t.url, &None);
        } else {
            let dir = task_output_dir(&s, &id);
            cleanup_engine_files(&dir, &t.url);
        }
        persist_download(&s, &t);
        emit_task(&app, &t);
    }
    // 排队中/已结束的任务没有活动进程：把标记清掉即可
    if !s.is_running(&id) {
        s.clear_cancel(&id);
        s.clear_stop(&id);
    }
    pump_queue(&s, &app);
    Ok(())
}

#[tauri::command]
fn remove_download(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    delete_file: bool,
) -> Result<(), String> {
    let s = state.0.clone();
    // BUG-02：删除「下载中」的任务必须先立墓碑（线程不得再写回）、杀引擎、释放槽位，最后才删库。
    // 否则任务线程会把记录 upsert 回库（任务复活），引擎也照旧继续下载、磁盘继续增长。
    s.mark_removed(&id);
    s.mark_stop(&id, StopIntent::Cancel);
    s.mark_cancel(&id);
    if let Some(pid) = s.pids.lock().ok().and_then(|m| m.get(&id).copied()) {
        ctx::kill_tree(pid);
    }
    s.take_pid(&id);
    s.unmark_running(&id);
    if delete_file {
        if let Some(t) = s.db.get_download(&id).ok().flatten() {
            // 下载中的任务 file_path 为 NULL，必须按输出目录 + URL 基名清理
            // `<name>` / `<name>.aria2` / `<name>.part`
            let dir = task_output_dir(&s, &id);
            cleanup_download_artifacts(&dir, &t.url, &t.file_path);
        }
    }
    s.db.delete_download(&id).map_err(err)?;
    s.clear_cancel(&id);
    s.clear_stop(&id);
    // 槽位释放 → 唤醒排队任务（BUG-05）
    pump_queue(&s, &app);
    // 通知界面把这张卡片移除（否则删了记录列表也不刷新，用户会以为按钮失灵）
    let _ = app.emit("download://removed", serde_json::json!({ "id": id }));
    Ok(())
}

#[tauri::command]
fn clear_downloads(app: AppHandle, state: State<'_, AppState>, which: String) -> Result<(), String> {
    state.0.db.clear_downloads(which == "done").map(|_| ()).map_err(err)?;
    let _ = app.emit("download://removed", serde_json::json!({ "all": which == "all" }));
    Ok(())
}

/* ==================== 转换 ==================== */

#[tauri::command]
async fn probe_media(state: State<'_, AppState>, path: String) -> Result<MediaProbe, String> {
    let s = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let ctx = build_ctx(&s, None);
        converter::probe_media(&ctx, &path).map_err(err)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
fn list_converts(state: State<'_, AppState>) -> Result<Vec<ConvertTask>, String> {
    state.0.db.list_converts().map_err(err)
}

#[tauri::command]
async fn start_convert(
    app: AppHandle,
    state: State<'_, AppState>,
    req: ConvertRequest,
) -> Result<ConvertTask, String> {
    let s = state.0.clone();
    let ctx = build_ctx(&s, None);
    let mut req = req;
    let src = std::path::PathBuf::from(&req.input_file);
    if !src.is_file() {
        return Err(format!("输入文件不存在：{}", req.input_file));
    }
    // 自动检测：在推导输出路径之前，按源文件真实编码决定目标格式与编码
    if req.auto_format.is_some() {
        match converter::probe_media(&ctx, &req.input_file) {
            Ok(p) => {
                let t = converter::auto_target(&p);
                req.format = t.format.clone();
                req.video_codec = Some(t.video.clone());
                req.audio_codec = Some(t.audio.clone());
                req.extract_audio = t.extract_audio;
                req.auto_format = Some(t.reason.clone());
            }
            Err(e) => {
                req.auto_format = Some(format!("自动检测失败，沿用所选参数：{e}"));
            }
        }
    }
    let out_dir = req
        .output_dir
        .as_ref()
        .filter(|d| !d.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| src.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| ctx.download_dir()));
    std::fs::create_dir_all(&out_dir).map_err(err)?;
    let output = converter::output_path_for(&src, &out_dir, &req.format);
    let task = converter::new_convert_task(&req, &output, ctx::now_ms());
    let request_json = serde_json::to_string(&req).ok();
    s.db.upsert_convert(&task, request_json.as_deref()).map_err(err)?;

    let _ = app.emit("convert://update", &task);

    let s2 = s.clone();
    let app2 = app.clone();
    let id = task.id.clone();
    std::thread::spawn(move || {
        run_convert_job(s2, app2, req, id, output);
    });
    Ok(task)
}

pub fn run_convert_job(
    s: Arc<AppStateInner>,
    app: AppHandle,
    req: ConvertRequest,
    task_id: String,
    output: std::path::PathBuf,
) {
    s.clear_cancel(&task_id);
    s.mark_running(&task_id);
    let ctx = build_ctx(&s, Some(&app));
    let mut task = s
        .db
        .get_convert(&task_id)
        .ok()
        .flatten()
        .unwrap_or_else(|| converter::new_convert_task(&req, &output, ctx::now_ms()));

    task.status = TaskStatus::Converting;
    task.output_file = output.to_string_lossy().to_string();
    let _ = s.db.upsert_convert(&task, None);
    let _ = app.emit("convert://update", &task);

    let s_tick = s.clone();
    let app_tick = app.clone();
    let mut last = task.clone();
    let mut last_db = std::time::Instant::now();
    let mut last_emit = std::time::Instant::now();
    let mut last_emit_progress = -1.0f64;

    let s_pid = s.clone();
    let id_pid = task_id.clone();
    let s_cancel = s.clone();
    let id_cancel = task_id.clone();

    let outcome = converter::run_convert(
        &ctx,
        &req,
        &output,
        move |tick| {
            if let Some(p) = tick.percent {
                last.progress = p;
            }
            if let Some(sp) = tick.speed.as_ref() {
                let _ = sp;
            }
            if tick.done {
                last.progress = 100.0;
            }
            // 性能：进度事件节流（最多 4 次/秒；完成时立即发送）
            let now = std::time::Instant::now();
            if tick.done
                || (last.progress - last_emit_progress).abs() >= 1.5
                || now.duration_since(last_emit).as_millis() >= 250
            {
                last_emit = now;
                last_emit_progress = last.progress;
                let _ = app_tick.emit("convert://update", &last);
            }
            if last_db.elapsed().as_millis() > 700 {
                last_db = std::time::Instant::now();
                let _ = s_tick.db.upsert_convert(&last, None);
            }
        },
        move |pid| s_pid.register_pid(&id_pid, pid),
        move || s_cancel.is_cancelled(&id_cancel),
    );

    s.take_pid(&task_id);
    s.unmark_running(&task_id);
    let canceled = s.is_cancelled(&task_id);
    s.clear_cancel(&task_id);

    match outcome {
        Ok(o) if o.exit_code == 0 && output.is_file() => {
            task.status = TaskStatus::Done;
            task.progress = 100.0;
            task.error = None;
            let _ = s.db.upsert_convert(&task, None);
            let _ = app.emit("convert://update", &task);
            if s.settings_snapshot().open_folder_when_done {
                let _ = reveal_path(output.to_string_lossy().to_string());
            }
            if s.settings_snapshot().notify_on_finish {
                let name = output
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| task.input_file.clone());
                notify_finish("Umidl · 转换完成", &name);
            }
        }
        Ok(o) => {
            if canceled || o.killed {
                task.status = TaskStatus::Canceled;
            } else {
                task.status = TaskStatus::Error;
                task.error = Some(
                    o.stderr_tail
                        .lines()
                        .last()
                        .unwrap_or("转换失败")
                        .chars()
                        .take(300)
                        .collect(),
                );
            }
            let _ = s.db.upsert_convert(&task, None);
            let _ = app.emit("convert://update", &task);
        }
        Err(e) => {
            task.status = TaskStatus::Error;
            task.error = Some(e.to_string());
            let _ = s.db.upsert_convert(&task, None);
            let _ = app.emit("convert://update", &task);
        }
    }
}

#[tauri::command]
fn cancel_convert(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.0.mark_cancel(&id);
    if let Some(pid) = state.0.pids.lock().ok().and_then(|m| m.get(&id).copied()) {
        ctx::kill_tree(pid);
    }
    Ok(())
}

#[tauri::command]
fn remove_convert(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.0.db.delete_convert(&id).map_err(err)
}

/* ==================== 字幕 ==================== */

#[tauri::command]
fn list_subtitles(state: State<'_, AppState>) -> Result<Vec<SubtitleTask>, String> {
    state.0.db.list_subtitles().map_err(err)
}

#[tauri::command]
async fn start_subtitle(
    app: AppHandle,
    state: State<'_, AppState>,
    req: SubtitleRequest,
) -> Result<SubtitleTask, String> {
    let s = state.0.clone();
    if !std::path::Path::new(&req.video_path).is_file() {
        return Err(format!("视频文件不存在：{}", req.video_path));
    }
    let model = req
        .model
        .clone()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| s.settings_snapshot().whisper_model);
    // BUG-15：配置的模型还没下载时（例如默认 base，而机器上只有 tiny），自动改用
    // 已安装的模型 —— 默认设置也要能开箱即用；任务里记的是真实使用的模型
    let (model, fell_back) = tools::effective_model_for(&s.dirs.models, &model);
    if fell_back {
        logs::log_line(
            &s.dirs,
            "subtitle",
            &format!("配置的模型未安装，自动改用已安装模型 {} · {}", model, req.video_path),
        );
    }
    let task = subtitle::new_subtitle_task(&req, &model, ctx::now_ms());
    let request_json = serde_json::to_string(&req).ok();
    s.db.upsert_subtitle(&task, request_json.as_deref()).map_err(err)?;
    let _ = app.emit("subtitle://update", &task);

    let s2 = s.clone();
    let app2 = app.clone();
    let id = task.id.clone();
    std::thread::spawn(move || {
        run_subtitle_job(s2, app2, req, id, model);
    });
    Ok(task)
}

pub fn run_subtitle_job(
    s: Arc<AppStateInner>,
    app: AppHandle,
    req: SubtitleRequest,
    task_id: String,
    model: String,
) {
    // 起线程之前就被删了 → 直接退出（与下载路径同一套墓碑机制）
    if s.is_removed(&task_id) {
        return;
    }
    s.clear_cancel(&task_id);
    s.mark_running(&task_id);

    let mut settings = s.settings_snapshot();
    settings.whisper_model = model.clone();
    let mut ctx = Ctx::new(s.dirs.clone(), ToolPaths::default(), settings);
    ctx.tools = tools::resolve_all(&ctx);
    let handle = app.clone();
    ctx = ctx.with_emit(Arc::new(move |event: &str, payload: serde_json::Value| {
        let _ = handle.emit(event, payload);
    }));

    let mut task = s
        .db
        .get_subtitle(&task_id)
        .ok()
        .flatten()
        .unwrap_or_else(|| subtitle::new_subtitle_task(&req, &model, ctx::now_ms()));

    let tmp_dir = s.dirs.cache.join("subtitle-work");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let out_base = subtitle::subtitle_out_base(&ctx, &req);
    let wav = tmp_dir.join(format!("{}.wav", task_id));

    // BUG-13：失败分支也要给产物一个交代 —— whisper 已经写出的有效 srt 保留下来、
    // 把路径写进日志与任务记录（错误文案里也带上），无效的产物直接删掉，别留垃圾在视频目录
    let fail = |task: &mut SubtitleTask, s: &Arc<AppStateInner>, app: &AppHandle, msg: String| {
        let leftovers = subtitle::whisper_products(&out_base);
        let kept: Vec<PathBuf> = leftovers
            .iter()
            .filter(|p| {
                std::fs::read_to_string(p)
                    .map(|t| subtitle::validate_srt(&t).is_ok())
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        for p in &leftovers {
            if kept.iter().any(|k| k == p) {
                continue;
            }
            let _ = std::fs::remove_file(p);
        }
        task.status = TaskStatus::Error;
        task.error = Some(match kept.first() {
            Some(p) => format!("{msg}；已生成的字幕文件保留在：{}", p.to_string_lossy()),
            None => msg,
        });
        if let Some(p) = kept.first() {
            task.subtitle_path = Some(p.to_string_lossy().to_string());
            logs::log_line(
                &s.dirs,
                "subtitle",
                &format!(
                    "失败但保留有效产物 · {} · {}",
                    p.to_string_lossy(),
                    task.video_path
                ),
            );
        } else if !leftovers.is_empty() {
            logs::log_line(
                &s.dirs,
                "subtitle",
                &format!("已清理 {} 个无效产物 · {}", leftovers.len(), task.video_path),
            );
        }
        persist_subtitle(s, task);
        let _ = app.emit("subtitle://update", &*task);
        let _ = std::fs::remove_file(&wav);
    };

    // 1) 提取音频
    task.status = TaskStatus::Extracting;
    task.progress = 2.0;
    persist_subtitle(&s, &task);
    let _ = app.emit("subtitle://update", &task);

    let s_pid = s.clone();
    let id_pid = task_id.clone();
    let s_cancel = s.clone();
    let id_cancel = task_id.clone();
    let extract = subtitle::extract_audio(
        &ctx,
        &req.video_path,
        &wav,
        move |pid| s_pid.register_pid(&id_pid, pid),
        move || s_cancel.is_cancelled(&id_cancel),
    );
    s.take_pid(&task_id);

    match extract {
        Ok(o) if o.exit_code == 0 && wav.is_file() => {}
        Ok(o) => {
            let msg = if o.stderr_tail.trim().is_empty() {
                "音频提取失败".to_string()
            } else {
                format!("音频提取失败：{}", o.stderr_tail.lines().last().unwrap_or(""))
            };
            fail(&mut task, &s, &app, msg);
            s.unmark_running(&task_id);
            return;
        }
        Err(e) => {
            fail(&mut task, &s, &app, e.to_string());
            s.unmark_running(&task_id);
            return;
        }
    }

    // 2) Whisper 识别
    task.status = TaskStatus::Transcribing;
    task.progress = 8.0;
    persist_subtitle(&s, &task);
    let _ = app.emit("subtitle://update", &task);

    let s_tick = s.clone();
    let app_tick = app.clone();
    let mut last = task.clone();
    let mut last_db = std::time::Instant::now();
    let s_pid2 = s.clone();
    let id_pid2 = task_id.clone();
    let s_cancel2 = s.clone();
    let id_cancel2 = task_id.clone();

    let res = subtitle::transcribe_to_format(
        &ctx,
        &req,
        &out_base,
        &wav,
        move |p| {
            last.progress = 8.0 + (p.clamp(0.0, 100.0) * 0.92);
            let _ = app_tick.emit("subtitle://update", &last);
            if last_db.elapsed().as_millis() > 700 {
                last_db = std::time::Instant::now();
                persist_subtitle(&s_tick, &last);
            }
        },
        move |pid| s_pid2.register_pid(&id_pid2, pid),
        move || s_cancel2.is_cancelled(&id_cancel2),
    );

    s.take_pid(&task_id);
    s.unmark_running(&task_id);
    let canceled = s.is_cancelled(&task_id);
    s.clear_cancel(&task_id);
    let _ = std::fs::remove_file(&wav);

    match res {
        Ok(path) => {
            task.status = TaskStatus::Done;
            task.progress = 100.0;
            task.subtitle_path = Some(path.to_string_lossy().to_string());
            task.error = None;
            if s.settings_snapshot().open_folder_when_done {
                let _ = reveal_path(path.to_string_lossy().to_string());
            }
            if s.settings_snapshot().notify_on_finish {
                let name = path
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| task.video_path.clone());
                notify_finish("Umidl · 字幕生成完成", &name);
            }
        }
        Err(e) => {
            if canceled {
                task.status = TaskStatus::Canceled;
            } else {
                task.status = TaskStatus::Error;
                task.error = Some(e.to_string());
            }
        }
    }
    // BUG-01：回写真实产物路径（成功时就是 whisper 实际写出的那个文件）
    persist_subtitle(&s, &task);
    let _ = app.emit("subtitle://update", &task);
}

#[tauri::command]
fn cancel_subtitle(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.0.mark_cancel(&id);
    if let Some(pid) = state.0.pids.lock().ok().and_then(|m| m.get(&id).copied()) {
        ctx::kill_tree(pid);
    }
    Ok(())
}

#[tauri::command]
fn remove_subtitle(state: State<'_, AppState>, id: String) -> Result<(), String> {
    // 墓碑 + 杀进程：删掉记录后，仍在跑的字幕任务不再复活、也不再占着引擎
    state.0.mark_removed(&id);
    state.0.mark_stop(&id, StopIntent::Cancel);
    state.0.mark_cancel(&id);
    if let Some(pid) = state.0.pids.lock().ok().and_then(|m| m.get(&id).copied()) {
        ctx::kill_tree(pid);
    }
    state.0.take_pid(&id);
    state.0.unmark_running(&id);
    state.0.db.delete_subtitle(&id).map_err(err)
}

/* ==================== 自检（仅开发 / CI 构建包含） ==================== */

#[tauri::command]
fn list_test_cases() -> Vec<(String, String, String, bool)> {
    #[cfg(feature = "selftest")]
    {
        selftest::case_catalog()
    }
    #[cfg(not(feature = "selftest"))]
    {
        Vec::new()
    }
}

#[tauri::command]
async fn run_selftest(
    app: AppHandle,
    state: State<'_, AppState>,
    deep: bool,
) -> Result<TestReport, String> {
    #[cfg(feature = "selftest")]
    {
        let s = state.0.clone();
        let report = tauri::async_runtime::spawn_blocking(move || {
            let ctx = build_ctx(&s, None);
            let handle = app.clone();
            selftest::run_all(&ctx, &s.db, deep, Some(Arc::new(move |case: &TestCase| {
                let _ = handle.emit("selftest://update", case);
            })))
        })
        .await
        .map_err(err)?;
        if let Ok(mut r) = state.0.report.lock() {
            *r = Some(report.clone());
        }
        Ok(report)
    }
    #[cfg(not(feature = "selftest"))]
    {
        let _ = (app, state, deep);
        Err("正式版本未包含自检模块".into())
    }
}

/* ==================== 启动 ==================== */

// 移动端（Android / iOS）入口：Tauri 2 要求带 mobile 入口标记，
// cfg(mobile) 只在 android/ios 目标下生效，桌面构建完全不受影响。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            let dirs = AppDirs::new();
            dirs.ensure();
            // 工具探测缓存落盘：热启动直接复用，避免每次启动都拉起 yt-dlp 探测版本
            tools::init_cache(&dirs.data);
            let settings = settings::load(&dirs);
            let custom_logo = settings.custom_logo.clone();
            let db = Db::open(&dirs.db_file).map_err(|e| format!("数据库初始化失败：{e}"))?;
            // 上次异常退出：进行中的下载转为可续传的暂停态；
            // 超出并发上限的进行中任务恢复成「排队」，等槽位释放后自动继续（BUG-05）
            let _ = db.reset_stale_downloads(settings.concurrency.max(1) as usize);
            let _ = db.reset_stale_converts();
            let _ = db.reset_stale_subtitles();
            // BUG-03：上次被强杀/崩溃时留下的受管引擎子进程（父进程已死）→ 启动时清扫回收。
            // 放后台线程做，避免拖慢启动；只回收 bin 目录下的瞬时引擎，不碰用户自己的进程。
            {
                let dirs2 = dirs.clone();
                // 只清扫「生效的工具目录」：自定义目录里的残留引擎同样要回收
                let sweep_bin = tools::effective_tool_dir(&dirs.bin, &settings.tool_dir);
                std::thread::spawn(move || {
                    let n = ctx::sweep_orphan_engines(&sweep_bin);
                    if n > 0 {
                        logs::log_line(&dirs2, "app", &format!("启动清扫：回收了 {n} 个残留引擎进程"));
                    }
                });
            }
            // 全局限速（令牌桶）按设置初始化
            ratelimit::LIMITER.configure(if settings.speed_limit_enabled {
                settings.speed_limit_kb
            } else {
                0
            });
            logs::log_line(&dirs, "app", &format!("Umidl {} 启动", env!("CARGO_PKG_VERSION")));
            app.manage(AppState(Arc::new(AppStateInner {
                dirs,
                db: Arc::new(db),
                settings: Mutex::new(settings),
                cancelled: Mutex::new(HashSet::new()),
                pids: Mutex::new(HashMap::new()),
                report: Mutex::new(None),
                running: Mutex::new(HashSet::new()),
                stop_intent: Mutex::new(HashMap::new()),
                removed: Mutex::new(HashSet::new()),
                queue_lock: Mutex::new(()),
            })));
            // 窗口 / 任务栏图标：自定义 LOGO 优先（PNG/ICO 可解码），否则用打包时嵌入的图标。
            // 若不显式设置，窗口图标为空，Windows 会退回通用默认图标 ——
            // exe 资源里虽然是新图标，但任务栏 / 任务管理器读的是窗口图标。
            // 注意：setup 阶段窗口（HWND）可能尚未真正创建，此刻 set_icon 会静默无效，
            // 因此这里先试一次，再由后台线程在窗口就绪后补一次，保证重启后也生效。
            {
                let handle = app.handle().clone();
                let custom = custom_logo.clone();
                let apply_icon = move || {
                    let Some(win) = handle.get_webview_window("main") else {
                        return;
                    };
                    if let Some(p) = custom.as_ref() {
                        let path = Path::new(p);
                        if matches!(ext_of(path).as_str(), "png" | "ico") {
                            if let Ok(bytes) = std::fs::read(path) {
                                if let Ok(img) = tauri::image::Image::from_bytes(&bytes) {
                                    if win.set_icon(img).is_ok() {
                                        return;
                                    }
                                }
                            }
                        }
                    }
                    match handle.default_window_icon().cloned() {
                        Some(icon) => {
                            if let Err(e) = win.set_icon(icon) {
                                eprintln!("[umi] 设置窗口图标失败：{e}");
                            }
                        }
                        None => eprintln!("[umi] 未找到默认窗口图标（bundle.icon 需包含 icons/icon.ico）"),
                    }
                };
                apply_icon();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(900));
                    apply_icon();
                });
            }
            // 插件事件：启动完成（后台线程派发；插件异常不影响主程序）
            {
                let inner = app.state::<AppState>().0.clone();
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    let ctx = build_ctx(&inner, Some(&handle));
                    let res = crate::plugins::dispatch_event(&ctx, "app:start", serde_json::json!({}));
                    logs::log_line(&inner.dirs, "plugin", &format!("app:start 派发结果：{res}"));
                });
            }
            Ok(())
        })

        .invoke_handler(tauri::generate_handler![
            detect_tools,
            backfill_covers,
            frontend_log,
            set_custom_logo,
            clear_custom_logo,
            install_tool,
            install_tools,
            verify_tools,
            tool_dir_info,
            set_tool_dir,
            install_whisper_model,
            install_whisper_custom,
            list_whisper_models,
            delete_whisper_model,
            set_window_theme,
            app_version,
            app_data_dir,
            get_settings,
            save_settings,
            default_download_dir,
            open_path,
            reveal_path,
            read_text_file,
            probe_url,
            start_download,
            list_downloads,
            pause_download,
            resume_download,
            cancel_download,
            remove_download,
            clear_downloads,
            probe_media,
            list_converts,
            start_convert,
            cancel_convert,
            remove_convert,
            list_subtitles,
            start_subtitle,
            cancel_subtitle,
            remove_subtitle,
            list_test_cases,
            run_selftest,
            set_autostart,
            get_autostart,
            engine_info,
            doc_capabilities,
            probe_document,
            enqueue_links,
            convert_formats,
            file_sizes,
            plugin_sandbox_info,
            plugin_list,
            plugin_market_list,
            plugin_install,
            plugin_install_from_url,
            plugin_uninstall,
            plugin_set_enabled,
            plugin_test,
            plugin_run_resolvers,
            export_logs,
            check_update,
        ])
        .build(tauri::generate_context!())
        .expect("Umidl 启动失败")
        .run(|app_handle, event| match event {
            // BUG-03：退出（正常关窗 / 托盘退出）时回收所有受管引擎子进程，
            // 否则 aria2c / yt-dlp / ffmpeg 会变成孤儿进程继续写盘
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit => {
                kill_all_engines(app_handle);
            }
            _ => {}
        });
}

/// 退出路径：把登记过的引擎子进程整棵杀掉（BUG-03）
fn kill_all_engines(app: &tauri::AppHandle) {
    let s = match app.try_state::<AppState>() {
        Some(st) => st.0.clone(),
        None => return,
    };
    let pids: Vec<u32> = s.pids.lock().map(|m| m.values().copied().collect()).unwrap_or_default();
    if pids.is_empty() {
        return;
    }
    logs::log_line(&s.dirs, "app", &format!("退出：回收 {} 个引擎子进程", pids.len()));
    for pid in pids {
        ctx::kill_tree(pid);
    }
}

/* ═════════════════════ 1.4 新增命令 ═════════════════════ */

/// 开机自启动开关
#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    let m = app.autolaunch();
    if enabled {
        m.enable().map_err(|e| e.to_string())?;
    } else {
        m.disable().map_err(|e| e.to_string())?;
    }
    Ok(m.is_enabled().unwrap_or(enabled))
}

/// 查询开机自启动是否已开启
#[tauri::command]
fn get_autostart(app: tauri::AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

/// 引擎与限速概况（界面顶部展示）
#[tauri::command]
fn engine_info(state: State<'_, AppState>) -> serde_json::Value {
    let s = state.0.settings_snapshot();
    let tools = tools::resolve_all(&build_ctx(&state.0, None));
    serde_json::json!({
        "engine": s.engine,
        "aria2_ready": tools.aria2.is_some(),
        "aria2_path": tools.aria2.map(|p| p.to_string_lossy().to_string()),
        "split": 16,
        "speed_limit_kb": if s.speed_limit_enabled { s.speed_limit_kb } else { 0 },
        "playlist_mode": s.playlist_mode,
        "proxy_mode": s.proxy_mode,
    })
}

/* ═════════════════════ 1.6 新增命令：批量导入 / 转换格式目录 ═════════════════════ */

/// 批量导入链接并入队（多行文本 / txt 导入）。
/// 返回 { added, skipped, tasks, errors }
#[tauri::command]
async fn enqueue_links(
    app: AppHandle,
    state: State<'_, AppState>,
    text: String,
    output_dir: Option<String>,
) -> Result<serde_json::Value, String> {
    let s = state.0.clone();

    let mut added = 0usize;
    let mut skipped = 0usize;
    let mut count = 0usize;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut tasks: Vec<serde_json::Value> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    // 注释按**整行**忽略：只按空白/逗号切词会把注释文字的后半截（逗号之后的词）当成链接入队。
    let tokens = text
        .split('\n')
        .filter(|l| {
            let t = l.trim();
            !(t.is_empty() || t.starts_with('#') || t.starts_with("//"))
        })
        .flat_map(|l| {
            l.split(|c: char| c == '\t' || c == ' ' || c == ',' || c == '，')
                .collect::<Vec<_>>()
        });

    for raw in tokens {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if count >= 200 {
            break;
        }
        if !seen.insert(line.to_string()) {
            skipped += 1;
            continue;
        }
        if s.is_running(line) {
            skipped += 1;
            continue;
        }
        count += 1;
        let req = DownloadRequest {
            url: line.to_string(),
            output_dir: output_dir.clone().filter(|d| !d.trim().is_empty()),
            ..Default::default()
        };
        match start_download(app.clone(), state.clone(), req).await {
            Ok(t) => {
                added += 1;
                tasks.push(serde_json::to_value(&t).unwrap_or(serde_json::Value::Null));
            }
            Err(e) => {
                skipped += 1;
                errors.push(format!("{line} · {e}"));
            }
        }
    }

    Ok(serde_json::json!({
        "added": added,
        "skipped": skipped,
        "tasks": tasks,
        "errors": errors,
    }))
}

/// 转换格式目录：按 视频/音频/图片/文档 分组，带本机引擎真实可用性
#[tauri::command]
fn convert_formats(state: State<'_, AppState>) -> serde_json::Value {
    let s = state.0.clone();
    let ctx = build_ctx(&s, None);
    crate::converter::convert_formats(&ctx)
}

/// 一批文件的真实大小 / 是否存在（转换队列卡片要显示「源大小 → 产物大小」）
///
/// 只读文件系统元数据，不启动任何子进程；路径不存在时返回 size=null / exists=false。
#[tauri::command]
fn file_sizes(paths: Vec<String>) -> Vec<serde_json::Value> {
    paths
        .into_iter()
        .map(|p| {
            let meta = std::fs::metadata(&p).ok();
            let size = meta.as_ref().map(|m| m.len()).filter(|_| {
                meta.as_ref().map(|m| m.is_file()).unwrap_or(false)
            });
            let modified = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64);
            serde_json::json!({
                "path": p,
                "size": size,
                "exists": size.is_some(),
                "modified": modified,
            })
        })
        .collect()
}

/* ═════════════════════ 新增命令：文档转换 / 插件沙箱 ═════════════════════ */

/// 文档转换能力：原生格式 + pandoc / poppler 是否就绪
#[tauri::command]
fn doc_capabilities(state: State<'_, AppState>) -> serde_json::Value {
    let s = state.0.clone();
    let ctx = build_ctx(&s, None);
    crate::docs::capabilities(&ctx)
}

/// 文档信息探测（页数 / 工作表 / 幻灯片 / 字符数）
#[tauri::command]
fn probe_document(path: String) -> serde_json::Value {
    crate::docs::probe_document(&path)
}

/// 插件沙箱自述（引擎 / 内存上限 / 脚本时间预算）
#[tauri::command]
fn plugin_sandbox_info() -> serde_json::Value {
    crate::plugins::sandbox_selftest()
}

/// 已安装插件列表
#[tauri::command]
fn plugin_list(state: State<'_, AppState>) -> serde_json::Value {
    let s = state.0.clone();
    let ctx = build_ctx(&s, None);
    crate::plugins::list_plugins(&ctx)
}

/// 插件市场清单
#[tauri::command]
fn plugin_market_list(app: AppHandle, state: State<'_, AppState>) -> serde_json::Value {
    let s = state.0.clone();
    let ctx = build_ctx(&s, Some(&app));
    crate::plugins::market_list(&ctx)
}

/// 从市场安装插件（内容寻址校验：sha256 不一致直接拒绝）
#[tauri::command]
fn plugin_install(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<serde_json::Value, String> {
    let s = state.0.clone();
    let ctx = build_ctx(&s, Some(&app));
    crate::plugins::install_from_market(&ctx, &id).map_err(|e| e.to_string())
}

/// 从 GitHub 仓库 / https 直链安装插件
/// （仅 https、≤ 5 MB、60 s 超时；下载内容只落盘不执行，只写现有插件目录内）
#[tauri::command]
fn plugin_install_from_url(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
) -> Result<serde_json::Value, String> {
    let s = state.0.clone();
    let ctx = build_ctx(&s, Some(&app));
    crate::plugins::install_from_url(&ctx, &url).map_err(|e| e.to_string())
}

/// 卸载插件
#[tauri::command]
fn plugin_uninstall(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    let s = state.0.clone();
    let ctx = build_ctx(&s, Some(&app));
    crate::plugins::uninstall_plugin(&ctx, &id).map_err(|e| e.to_string())
}

/// 启用 / 禁用插件
#[tauri::command]
fn plugin_set_enabled(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    let s = state.0.clone();
    let ctx = build_ctx(&s, Some(&app));
    crate::plugins::set_plugin_enabled(&ctx, &id, enabled).map_err(|e| e.to_string())
}

/// 在沙箱里试跑插件（含 selfTest）
#[tauri::command]
fn plugin_test(app: AppHandle, state: State<'_, AppState>, id: String) -> serde_json::Value {
    let s = state.0.clone();
    let ctx = build_ctx(&s, Some(&app));
    crate::plugins::test_plugin(&ctx, &id)
}

/// 让所有启用插件的解析器对 URL 表态
#[tauri::command]
fn plugin_run_resolvers(app: AppHandle, state: State<'_, AppState>, url: String) -> serde_json::Value {
    let s = state.0.clone();
    let ctx = build_ctx(&s, Some(&app));
    crate::plugins::run_resolvers(&ctx, &url)
}

/// 诊断摘要（导出日志用）
fn build_diag_summary(s: &AppStateInner) -> String {
    let cfg = s.settings_snapshot();
    let ctx = build_ctx(s, None);
    let tools = tools::resolve_all(&ctx);
    let fmt = |p: &Option<PathBuf>| {
        p.as_ref()
            .map(|x| x.to_string_lossy().to_string())
            .unwrap_or_else(|| "未安装".into())
    };
    format!(
        "版本: {}\n平台: {} {}\n数据目录: {}\n下载目录: {}\n\n\
         yt-dlp: {}\nffmpeg: {}\nffprobe: {}\nwhisper: {}\naria2c: {}\n\n\
         引擎: {} · 分段: 16 · 全局限速: {} KB/s · 代理模式: {} · 分P模式: {}\n\
         自启动: {}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        s.dirs.data.display(),
        cfg.download_dir,
        fmt(&tools.ytdlp),
        fmt(&tools.ffmpeg),
        fmt(&tools.ffprobe),
        fmt(&tools.whisper),
        fmt(&tools.aria2),
        cfg.engine,
        if cfg.speed_limit_enabled { cfg.speed_limit_kb } else { 0 },
        cfg.proxy_mode,
        cfg.playlist_mode,
        cfg.launch_at_login,
    )
}

/// 导出日志包（含环境摘要）到指定目录或 .txt 路径
#[tauri::command]
fn export_logs(state: State<'_, AppState>, dest: String) -> Result<String, String> {
    let summary = build_diag_summary(&state.0);
    let path = logs::export(&state.0.dirs, Path::new(&dest), &summary).map_err(err)?;
    let out = path.to_string_lossy().to_string();
    logs::log_line(&state.0.dirs, "logs", &format!("已导出日志：{out}"));
    Ok(out)
}

/// 本仓库 GitHub Releases 最新版接口（公开 API，无需鉴权；必须带 User-Agent，否则 GitHub 直接 403）
const UPDATE_API_URL: &str = "https://api.github.com/repos/HuanMoovo/umidl/releases/latest";
/// GitHub API 要求的 User-Agent（缺省会被拒）
const UPDATE_USER_AGENT: &str = "Umidl";

/// 纯函数：版本串 → 数字段。去掉 v / V 前缀，按 '.' 分段，每段必须全是数字。
/// 非法（空串 / 空段 / 非数字段，如 "nightly"、"v1.8.x"）返回 None —— 调用方据此报「无法识别版本号」，
/// 绝不把识别不了的 tag 当成「已最新」。
pub fn parse_version_tag(raw: &str) -> Option<Vec<u64>> {
    let t = raw.trim().trim_start_matches(['v', 'V']);
    if t.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for seg in t.split('.') {
        if seg.is_empty() || !seg.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        out.push(seg.parse::<u64>().ok()?);
    }
    Some(out)
}

/// 纯函数：tag 与 current 比较 —— tag 更新 → Some(1)；相同 → Some(0)；更旧 → Some(-1)；tag 非法 → None。
/// 段数不一致时缺段按 0 处理（v1.8 == 1.8.0）。
pub fn compare_versions(tag: &str, current: &str) -> Option<i32> {
    let a = parse_version_tag(tag)?;
    let b = parse_version_tag(current)?;
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return Some(if x > y { 1 } else { -1 });
        }
    }
    Some(0)
}

/// Release notes 截断：界面只有一小块位置，600 字符足够说清更新内容
fn truncate_release_notes(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let mut out: String = t.chars().take(600).collect();
    if t.chars().count() > 600 {
        out.push('…');
    }
    Some(out)
}

/// 检查更新：GET 本仓库的 GitHub Releases latest，取 tag_name / html_url 与当前版本比较。
/// 与工具下载 / 封面缓存同一套代理决策（effective_proxy + client_proxy_plan，端口拒连自动回退直连），
/// 不用裸 reqwest 直连 —— 否则开着代理的用户会连不上 api.github.com。
#[tauri::command]
async fn check_update(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let s = state.0.clone();
    ratelimit::LIMITER.acquire(4096).await;
    tauri::async_runtime::spawn_blocking(move || fetch_latest_release(&s))
        .await
        .map_err(|e| format!("检查更新任务异常：{e}"))
}

/// 实际抓取（阻塞版，跑在 spawn_blocking 里）：任何失败都折叠成 { ok:false, current, error } 的可读结果
fn fetch_latest_release(state: &AppStateInner) -> serde_json::Value {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let url = UPDATE_API_URL;
    let ctx = build_ctx(state, None);

    let plan = downloader::client_proxy_plan(downloader::effective_proxy(&ctx, url).as_deref());
    let mut builder = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .connect_timeout(std::time::Duration::from_secs(10))
        .user_agent(UPDATE_USER_AGENT);
    match &plan.proxy {
        Some(px) => match reqwest::Proxy::all(px) {
            Ok(p) => builder = builder.proxy(p),
            Err(_) => builder = builder.no_proxy(),
        },
        None if plan.no_proxy => builder = builder.no_proxy(),
        None => {}
    }
    let client = match builder.build() {
        Ok(c) => c,
        Err(e) => {
            return serde_json::json!({ "ok": false, "current": current, "error": format!("无法创建网络客户端：{e}") })
        }
    };

    let resp = match client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            return serde_json::json!({ "ok": false, "current": current, "error": format!("无法连接更新服务：{e}") })
        }
    };
    if !resp.status().is_success() {
        return serde_json::json!({
            "ok": false,
            "current": current,
            "error": format!("更新服务返回 HTTP {}（仓库暂无 Release 或接口限流）", resp.status().as_u16()),
        });
    }
    let body = match resp.text() {
        Ok(t) => t,
        Err(e) => {
            return serde_json::json!({ "ok": false, "current": current, "error": format!("读取更新信息失败：{e}") })
        }
    };
    let release: serde_json::Value = match serde_json::from_str(&body) {
        Ok(j) => j,
        Err(e) => {
            return serde_json::json!({ "ok": false, "current": current, "error": format!("更新信息格式错误：{e}") })
        }
    };

    let tag = release
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if tag.is_empty() {
        return serde_json::json!({ "ok": false, "current": current, "error": "最新 Release 缺少 tag_name" });
    }
    let has_update = match compare_versions(&tag, &current) {
        Some(c) => c > 0,
        None => {
            return serde_json::json!({
                "ok": false,
                "current": current,
                "error": format!("无法识别更新版本号：{tag}"),
            })
        }
    };
    let html_url = release.get("html_url").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let notes = release.get("body").and_then(|v| v.as_str()).unwrap_or("");
    serde_json::json!({
        "ok": true,
        "current": current,
        "latest": tag,
        "has_update": has_update,
        "url": if html_url.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(html_url) },
        "notes": truncate_release_notes(notes),
    })
}


/// 无 GUI 的依赖安装入口（供命令行 / CI 使用）
pub fn cli_install_tools(only: Option<&str>, with_model: Option<&str>) -> i32 {
    let dirs = AppDirs::new();
    dirs.ensure();
    let settings = settings::load(&dirs);
    let mut ctx = Ctx::new(dirs.clone(), ToolPaths::default(), settings);
    ctx.tools = tools::resolve_all(&ctx);
    println!("数据目录: {}", dirs.data.display());

    let targets: Vec<String> = match only {
        Some(name) => vec![name.to_string()],
        None => vec!["yt-dlp".into(), "ffmpeg".into(), "whisper".into()],
    };
    let mut failed = 0;
    for name in &targets {
        match tools::install_tool(&ctx, name) {
            Ok(st) => println!(
                "[OK]   {:<10} {} {}",
                st.name,
                st.version.clone().unwrap_or_default(),
                st.path.clone().unwrap_or_default()
            ),
            Err(e) => {
                failed += 1;
                println!("[FAIL] {name}: {e}");
            }
        }
    }
    if let Some(model) = with_model {
        match tools::install_whisper_model(&ctx, model) {
            Ok(st) => println!("[OK]   模型 {} → {}", model, st.path.unwrap_or_default()),
            Err(e) => {
                failed += 1;
                println!("[FAIL] 模型 {model}: {e}");
            }
        }
    }
    if failed > 0 {
        1
    } else {
        0
    }
}

/// 无 GUI 的自检入口（供命令行 / CI 使用，仅 selftest feature 构建可用）
#[cfg(feature = "selftest")]
pub fn cli_selftest(deep: bool) -> i32 {
    println!("=== Umidl 后端自检 ===\n");
    let dirs = AppDirs::new();
    dirs.ensure();
    tools::init_cache(&dirs.data);
    println!("数据目录: {}", dirs.data.display());
    println!("下载目录: {}\n", dirs.downloads.display());
    let db = match Db::open(&dirs.db_file) {
        Ok(d) => Arc::new(d),
        Err(e) => {
            eprintln!("数据库初始化失败: {e}");
            return 2;
        }
    };
    let settings = settings::load(&dirs);
    let mut ctx = Ctx::new(dirs.clone(), ToolPaths::default(), settings);
    ctx.tools = tools::resolve_all(&ctx);
    let report = selftest::run_all(&ctx, &db, deep, None);
    println!();
    for c in &report.cases {
        let mark = match c.status.as_str() {
            "pass" => "PASS",
            "fail" => "FAIL",
            "skip" => "SKIP",
            _ => "----",
        };
        println!(
            "[{mark}] {:>5}ms  {:<16} {} {}",
            c.duration_ms.unwrap_or(0),
            c.id,
            c.name,
            c.detail.as_deref().map(|d| format!("— {d}")).unwrap_or_default()
        );
    }
    println!(
        "\n总计 {} · 通过 {} · 失败 {} · 跳过 {}",
        report.total, report.passed, report.failed, report.skipped
    );
    if report.failed > 0 {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod fix18_tests {
    use super::*;

    fn test_state() -> Arc<AppStateInner> {
        let base = std::env::temp_dir().join(format!("umi-fix18-{}", ctx::short_id()));
        let _ = std::fs::create_dir_all(&base);
        Arc::new(AppStateInner {
            dirs: AppDirs {
                data: base.clone(),
                bin: base.join("bin"),
                models: base.join("models"),
                cache: base.join("cache"),
                downloads: base.join("downloads"),
                db_file: base.join("umi.db"),
                settings_file: base.join("settings.json"),
            },
            db: Arc::new(Db::in_memory().expect("内存库")),
            settings: Mutex::new(AppSettings::default()),
            cancelled: Mutex::new(HashSet::new()),
            pids: Mutex::new(HashMap::new()),
            report: Mutex::new(None),
            running: Mutex::new(HashSet::new()),
            stop_intent: Mutex::new(HashMap::new()),
            removed: Mutex::new(HashSet::new()),
            queue_lock: Mutex::new(()),
        })
    }

    fn dl(id: &str, url: &str, status: TaskStatus, created: i64) -> DownloadTask {
        DownloadTask {
            id: id.into(),
            url: url.into(),
            title: url.into(),
            status,
            created_time: created,
            ..Default::default()
        }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// BUG-02：删除「下载中」的任务后，运行中的线程不能再把它写回库（任务不再复活）
    #[test]
    fn deleted_download_does_not_come_back() {
        let s = test_state();
        let mut t = dl("8b90ef03", "http://127.0.0.1:18330/bulk_d.bin", TaskStatus::Downloading, 1);
        t.progress = 20.0;
        s.db.upsert_download(&t, Some("{}")).unwrap();
        assert!(s.should_persist(&t.id), "没删除时应照常写库");

        // remove_download 的核心两步：立墓碑 + 删库
        s.mark_removed(&t.id);
        s.db.delete_download(&t.id).unwrap();
        assert!(s.db.get_download(&t.id).unwrap().is_none(), "删除后记录应消失");

        // 线程随后的进度回写 / 收尾回写都必须被守卫拦住
        t.progress = 42.0;
        persist_download(&s, &t);
        assert!(s.db.get_download(&t.id).unwrap().is_none(), "已被删除的任务不得复活");
        assert_eq!(s.db.count_downloads_by_status("downloading").unwrap(), 0);
    }

    /// BUG-02：按 URL 基名清理下载产物，且不误伤别的文件
    #[test]
    fn delete_cleans_name_aria2_and_part() {
        let s = test_state();
        let dir = s.dirs.downloads.clone();
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["bulk_d.bin", "bulk_d.bin.aria2", "bulk_d.bin.part"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        std::fs::write(dir.join("keep_me.bin"), b"x").unwrap();
        let url = "http://127.0.0.1:18330/bulk_d.bin";
        cleanup_download_artifacts(&dir, url, &None);
        for f in ["bulk_d.bin", "bulk_d.bin.aria2", "bulk_d.bin.part"] {
            assert!(!dir.join(f).exists(), "{f} 应被清理");
        }
        assert!(dir.join("keep_me.bin").exists(), "不能误删别的文件");
        // 只清控制文件时，数据文件必须保留
        std::fs::write(dir.join("bulk_d.bin.aria2"), b"x").unwrap();
        std::fs::write(dir.join("bulk_d.bin"), b"x").unwrap();
        cleanup_engine_files(&dir, url);
        assert!(!dir.join("bulk_d.bin.aria2").exists());
        assert!(dir.join("bulk_d.bin").exists(), "取消已完成的产物时应保留数据文件");
    }

    /// BUG-19：`download://update` 事件载荷里的 file_exists 必须反映磁盘真相。
    ///
    /// 修复前事件直接发任务对象（file_exists 恒为创建时的 false），前端 upsert 后
    /// 完成任务被误标「文件已丢失」，且「打开文件 / 在文件夹中显示」按钮整排消失。
    #[test]
    fn event_payload_carries_real_file_exists() {
        let s = test_state();
        let dir = s.dirs.downloads.clone();
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("done.bin");
        std::fs::write(&real, b"hello").unwrap();

        // 1) 文件真的在磁盘上 → 事件里必须是 true（修复前的病根：这里是 false）
        let mut t = dl("aaaa1111", "http://127.0.0.1:18330/done.bin", TaskStatus::Done, 1);
        t.file_path = Some(real.to_string_lossy().to_string());
        t.file_exists = false; // 任务对象出厂值
        let ev = event_payload(&t);
        assert!(ev.file_exists, "文件在磁盘上时事件必须带 file_exists=true");
        assert!(!t.file_exists, "原对象不应被就地改写（仍然只发副本）");

        // 2) 文件被删掉后 → 事件必须诚实地报 false（「文件已丢失」提示要留给真丢的场景）
        std::fs::remove_file(&real).unwrap();
        assert!(!event_payload(&t).file_exists, "文件没了就该报 false");

        // 3) 没有 file_path 的任务（下载中/取消）一律 false，不会误亮按钮
        let p = dl("bbbb2222", "http://127.0.0.1:18330/x.bin", TaskStatus::Downloading, 2);
        assert!(!event_payload(&p).file_exists);
        // 4) 目录不算文件（避免把 output_dir 误当产物）
        let mut d = dl("cccc3333", "http://127.0.0.1:18330/y.bin", TaskStatus::Done, 3);
        d.file_path = Some(dir.to_string_lossy().to_string());
        assert!(!event_payload(&d).file_exists, "目录不是文件");
    }

    /// BUG-05：超并发 → 排队；槽位释放后按入队顺序放行，且绝不超过上限
    #[test]
    fn queue_respects_concurrency_limit() {
        assert!(!should_queue(0, 3));
        assert!(!should_queue(2, 3));
        assert!(should_queue(3, 3));
        assert!(should_queue(4, 3));
        // 非法设置值下限按 1 处理
        assert!(should_queue(1, 0));
        assert!(!should_queue(0, 0));
        assert!(should_queue(1, -5));

        let queued = ids(&["a", "b", "c", "d"]);
        assert_eq!(plan_queue(0, 3, &queued), ids(&["a", "b", "c"]));
        assert_eq!(plan_queue(2, 3, &queued), ids(&["a"]));
        assert!(plan_queue(3, 3, &queued).is_empty(), "满了就不放行");
        assert_eq!(plan_queue(0, 1, &queued), ids(&["a"]), "单槽设置一次只跑一个");
        assert!(plan_queue(0, 3, &[]).is_empty());
    }

    /// BUG-05：队列泵总是先唤醒最早入队的任务
    #[test]
    fn queue_picks_oldest_pending_first() {
        let s = test_state();
        for (id, created) in [("c", 3i64), ("a", 1), ("b", 2)] {
            s.db.upsert_download(&dl(id, &format!("http://x/{id}.bin"), TaskStatus::Pending, created), Some("{}"))
                .unwrap();
        }
        s.db.upsert_download(&dl("z", "http://x/z.bin", TaskStatus::Downloading, 0), Some("{}")).unwrap();
        let (id, req) = s.db.next_queued_download().unwrap().unwrap();
        assert_eq!(id, "a", "最早的排队任务应先被放行");
        assert_eq!(req.as_deref(), Some("{}"));
        assert_eq!(s.db.count_downloads_by_status("pending").unwrap(), 3);
        assert_eq!(s.db.count_downloads_by_status("downloading").unwrap(), 1);
    }

    /// BUG-05：启动时把超出的「下载中」恢复成排队态，正常范围内仍转成可续传的暂停态
    #[test]
    fn startup_requeues_over_limit_only() {
        let s = test_state();
        for (id, created) in [("a", 1i64), ("b", 2), ("c", 3), ("d", 4)] {
            s.db.upsert_download(&dl(id, &format!("http://x/{id}.bin"), TaskStatus::Downloading, created), Some("{}"))
                .unwrap();
        }
        s.db.upsert_download(&dl("q", "http://x/q.bin", TaskStatus::Pending, 5), Some("{}")).unwrap();
        s.db.upsert_download(&dl("done", "http://x/done.bin", TaskStatus::Done, 6), Some("{}")).unwrap();

        s.db.reset_stale_downloads(2).unwrap();
        assert_eq!(s.db.count_downloads_by_status("paused").unwrap(), 2, "并发内的进行中任务 → 暂停可续传");
        assert_eq!(s.db.count_downloads_by_status("pending").unwrap(), 3, "超出的 + 原排队的都留在队列里");
        assert_eq!(s.db.count_downloads_by_status("downloading").unwrap(), 0, "不再有假的「下载中」");
        assert_eq!(s.db.count_downloads_by_status("done").unwrap(), 1, "已完成任务不受影响");
        assert!(s.db.get_download("a").unwrap().unwrap().status == TaskStatus::Paused);
        assert!(s.db.get_download("c").unwrap().unwrap().status == TaskStatus::Pending);
    }

    /// BUG-04：取消 → Canceled；暂停 / 看门狗中止 → Paused；正常结束 → 交给 exit_code 分支
    #[test]
    fn cancel_and_pause_are_distinct() {
        assert_eq!(
            status_after_engine_stop(Some(StopIntent::Cancel), false, false),
            Some(TaskStatus::Canceled)
        );
        assert_eq!(
            status_after_engine_stop(Some(StopIntent::Cancel), true, true),
            Some(TaskStatus::Canceled),
            "取消优先级最高，不能被压成 Paused"
        );
        assert_eq!(
            status_after_engine_stop(Some(StopIntent::Pause), true, false),
            Some(TaskStatus::Paused)
        );
        assert_eq!(status_after_engine_stop(None, true, false), Some(TaskStatus::Paused), "旧调用路径（只标 cancel）保持暂停语义");
        assert_eq!(status_after_engine_stop(None, false, true), Some(TaskStatus::Paused), "看门狗中止 → 可续传");
        assert_eq!(status_after_engine_stop(None, false, false), None, "正常结束不看这里");
        // 状态名与前端约定一致
        assert_eq!(TaskStatus::Canceled.as_str(), "canceled");
        assert_eq!(TaskStatus::Paused.as_str(), "paused");
    }

    /// BUG-04：停止意图与墓碑的状态机（设置 → 读取 → 清除）
    #[test]
    fn stop_intent_and_tombstone_lifecycle() {
        let s = test_state();
        assert_eq!(s.stop_intent("t1"), None);
        assert!(!s.is_removed("t1"));
        assert!(s.should_persist("t1"));

        s.mark_stop("t1", StopIntent::Pause);
        assert_eq!(s.stop_intent("t1"), Some(StopIntent::Pause));
        s.mark_stop("t1", StopIntent::Cancel);
        assert_eq!(s.stop_intent("t1"), Some(StopIntent::Cancel), "后写入的意图覆盖前者");
        s.clear_stop("t1");
        assert_eq!(s.stop_intent("t1"), None);

        s.mark_removed("t1");
        assert!(s.is_removed("t1"));
        assert!(!s.should_persist("t1"), "墓碑立起后不得再写库");
    }

    /// 并发槽位：预留 / 释放与 running_count 一致（上限判定依赖它）
    #[test]
    fn slot_reservation_tracks_running_count() {
        let s = test_state();
        assert_eq!(s.running_count(), 0);
        s.mark_running("a");
        s.mark_running("b");
        assert_eq!(s.running_count(), 2);
        assert!(s.is_running("a"));
        s.unmark_running("a");
        assert_eq!(s.running_count(), 1);
        s.unmark_running("a");
        assert_eq!(s.running_count(), 1, "重复释放不改变计数");
    }

    /* ---------- BUG-20：「在文件夹中显示 / 打开文件」必须真的落到产物上 ---------- */

    /** 每个用例一个临时目录，互不干扰 */
    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("umi-fix20-{tag}-{}", ctx::short_id()));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    /// BUG-20：`reveal_target` 只认盘上真实存在的东西 —— 文件没了必须返回 None（绝不退父目录）。
    ///
    /// 退回父目录正是「点了在文件夹中显示、窗口里却没选中任何（字幕）文件」的来源：
    /// 按钮看起来成功，用户还是找不到产物。
    #[test]
    fn reveal_target_never_falls_back_to_parent() {
        let dir = tmpdir("target");
        let file = dir.join("示例 视频.detected-to-en.srt");
        std::fs::write(&file, b"1\n").unwrap();

        assert_eq!(
            reveal_target(&file).as_deref(),
            Some(file.as_path()),
            "文件在盘上 → 定位文件自己（Explorer 会选中它）"
        );
        assert_eq!(
            reveal_target(&dir).as_deref(),
            Some(dir.as_path()),
            "目录 → 直接打开该目录"
        );

        // 文件被移走 / 删掉：即便「父目录还在」，也必须 None（交给调用方报错）
        let gone = dir.join("示例 视频.detected.srt");
        assert!(
            reveal_target(&gone).is_none(),
            "文件没了 → None，不许退到父目录 {}（那会让按钮假装成功）",
            dir.display()
        );
        // 整条路径都不存在（父目录也不在）
        let nowhere = dir.join("nope").join("x.srt");
        assert!(reveal_target(&nowhere).is_none(), "路径完全不存在 → None");
    }

    /// BUG-20：explorer 的 `/select,"<路径>"` 参数原样带上完整路径（空格 / 中文 / 引号都不丢）
    #[test]
    fn reveal_select_arg_keeps_full_path() {
        let cases = [
            (r"D:\Videos\umi\clip12s.detected.srt", "clip12s.detected.srt"),
            (
                r"D:\Media Files\Videos\Umidl\Sample '샘플 영상' [AbCdEfGhIjK].detected-to-en.srt",
                "detected-to-en.srt",
            ),
            (r"D:\Videos\中文 目录\示例 视频.detected.srt", "示例 视频.detected.srt"),
        ];
        for (p, tail) in cases {
            let arg = reveal_select_arg(std::path::Path::new(p));
            assert_eq!(arg, format!("/select,\"{p}\""), "参数必须整串加引号（含空格才不会解析错）");
            assert!(arg.starts_with("/select,\""), "缺少 /select,\" 前缀：{arg}");
            assert!(arg.ends_with('"'), "缺少收尾引号：{arg}");
            assert!(arg.contains(tail), "产物名不能被截断：{arg}");
        }
    }

    /// BUG-20：打开文件时交给系统 Shell 的宽字符参数 —— 以 NUL 结尾的 UTF-16，
    /// 空格 / 中文 / `&` / `%` 全部原样保留（不走 cmd，就不存在元字符解析）
    #[cfg(windows)]
    #[test]
    fn shell_open_arg_is_nul_terminated_utf16() {
        let p = r"D:\Media Files\Videos\Umidl\A&B %PATH% 示例.srt";
        let wide = wide_nul(p);
        assert_eq!(wide.last().copied(), Some(0u16), "必须以 NUL 结尾");
        assert_eq!(wide.len(), p.chars().count() + 1, "BMP 路径一个字符一个 u16 + 结尾 NUL");
        let round = String::from_utf16(&wide[..wide.len() - 1]).unwrap();
        assert_eq!(round, p, "路径必须原样保留，不能被 cmd 展开 / 截断");
    }

    /// BUG-20：`open_path` 对不存在的路径必须直接报错（不再 spawn 一条注定失败的命令还当成功）
    #[test]
    fn open_path_rejects_missing_path() {
        let dir = tmpdir("open");
        let gone = dir.join("nope.srt");
        let e = open_path(gone.to_string_lossy().to_string()).unwrap_err();
        assert!(e.contains("路径不存在"), "错误文案要明说路径不存在：{e}");
    }

    /// 更新检查：tag_name 解析 —— 去 v 前缀 / 按数字段；非法 tag 一律 None
    #[test]
    fn update_tag_parsing_requires_numeric_segments() {
        assert_eq!(parse_version_tag("v1.8.11"), Some(vec![1, 8, 11]));
        assert_eq!(parse_version_tag("1.8.11"), Some(vec![1, 8, 11]));
        assert_eq!(parse_version_tag("  V2.0  "), Some(vec![2, 0]));
        // 非法 tag：非数字段 / 空段 / 空串 → None（界面据此报「无法识别更新版本号」）
        assert_eq!(parse_version_tag("nightly"), None);
        assert_eq!(parse_version_tag("release-2024"), None);
        assert_eq!(parse_version_tag("v1.8.x"), None);
        assert_eq!(parse_version_tag("v1..2"), None);
        assert_eq!(parse_version_tag("v"), None);
        assert_eq!(parse_version_tag(""), None);
        // 当前版本（Cargo.toml）本身必须可解析，否则更新检查永远失败
        assert!(parse_version_tag(env!("CARGO_PKG_VERSION")).is_some());
    }

    /// 更新检查：版本比较纯函数 —— 新版本 / 相同 / 旧版本 / 非法 tag（≥4 例）
    #[test]
    fn update_version_compare_newer_same_older_invalid() {
        // 新版本：tag > 当前 → 1
        assert_eq!(compare_versions("v1.8.12", "1.8.11"), Some(1));
        assert_eq!(compare_versions("1.9.0", "1.8.11"), Some(1));
        assert_eq!(compare_versions("v2.0.0", "v1.99.99"), Some(1));
        // 相同：前缀带不带都一样 → 0
        assert_eq!(compare_versions("v1.8.11", "1.8.11"), Some(0));
        assert_eq!(compare_versions("V1.8.11", "v1.8.11"), Some(0));
        // 旧版本 → -1
        assert_eq!(compare_versions("v1.8.10", "1.8.11"), Some(-1));
        assert_eq!(compare_versions("v1.7.99", "1.8.0"), Some(-1));
        // 非法 tag → None（绝不误报「已最新」或「有新版本」）
        assert_eq!(compare_versions("nightly", "1.8.11"), None);
        assert_eq!(compare_versions("", "1.8.11"), None);
        assert_eq!(compare_versions("v1.8.x", "1.8.11"), None);
        // 段数不一致：缺段按 0（v1.8 == 1.8.0；多一段非零则更大）
        assert_eq!(compare_versions("v1.8", "1.8.0"), Some(0));
        assert_eq!(compare_versions("v1.8.0.1", "1.8"), Some(1));
        // 界面展示的 notes 截断：空 → None；超长截到 600 + 省略号
        assert_eq!(truncate_release_notes("   "), None);
        assert!(truncate_release_notes(&"更".repeat(700)).unwrap().chars().count() <= 601);
    }
}

