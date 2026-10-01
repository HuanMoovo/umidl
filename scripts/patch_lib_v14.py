"""把 1.4 的新模块 / 命令 / 路由接进 lib.rs（按文件行尾安全改写）"""
import io, os, sys

P = 'src/lib.rs'
raw = open(P, encoding='utf-8', newline='').read()
crlf = '\r\n' in raw
s = raw.replace('\r\n', '\n')
fails = []


def rep(old, new, n=1, label=''):
    global s
    c = s.count(old)
    if c != n:
        fails.append(f'{label or old[:40]}: 命中 {c} 次（期望 {n}）')
        return
    s = s.replace(old, new, n)


# ── 1) 模块声明
rep('''pub mod converter;
pub mod ctx;
pub mod db;
pub mod downloader;''',
    '''pub mod capture;
pub mod converter;
pub mod ctx;
pub mod db;
pub mod downloader;
pub mod filters;''', 1, 'mod 声明')
rep('''pub mod models;''', '''pub mod logs;
pub mod models;
pub mod ratelimit;''', 1, 'mod logs/ratelimit')

# ── 2) 下载走引擎路由 + 日志
rep('''    let outcome = downloader::run_download(''',
    '''    let engine_label = if downloader::wants_aria2(&req, &ctx.settings) {
        "aria2（16 连接分段并行）"
    } else {
        "yt-dlp（站点解析）"
    };
    logs::log_line(
        &ctx.dirs,
        "download",
        &format!("开始 · 引擎={engine_label} · {}", req.url),
    );
    let outcome = downloader::run_engine(''', 1, 'run_engine 路由')

# ── 3) 任务收尾后判断是否关机
rep('''    s.take_pid(&task_id);
    s.unmark_running(&task_id);''',
    '''    s.take_pid(&task_id);
    s.unmark_running(&task_id);
    logs::log_line(
        &ctx.dirs,
        "download",
        &format!("结束 · 退出码={} · {}", outcome.exit_code, req.url),
    );
    maybe_shutdown_when_done(&s, &app);''', 1, '关机钩子')

# ── 4) save_settings 里同步全局限速 / 捕获服务
rep('''    settings::save(&state.0.dirs, &s).map_err(err)?;
    if let Ok(mut g) = state.0.settings.lock() {
        *g = s.clone();
    }
    Ok(s)''',
    '''    settings::save(&state.0.dirs, &s).map_err(err)?;
    if let Ok(mut g) = state.0.settings.lock() {
        *g = s.clone();
    }
    // 全局限速（令牌桶）：设置变化即时生效
    ratelimit::LIMITER.configure(if s.speed_limit_enabled {
        s.speed_limit_kb
    } else {
        0
    });
    Ok(s)''', 1, 'save_settings 限速')

# ── 5) setup：初始化限速 + 启动捕获服务
rep('''            // 窗口 / 任务栏图标：自定义 LOGO 优先（PNG/ICO 可解码），否则用打包时嵌入的图标。''',
    '''            // 全局限速（令牌桶）按设置初始化
            ratelimit::LIMITER.configure(if settings.speed_limit_enabled {
                settings.speed_limit_kb
            } else {
                0
            });
            logs::log_line(&dirs, "app", &format!("Umidl {} 启动", env!("CARGO_PKG_VERSION")));
            // 浏览器捕获接收端（端口 0 = 关闭）
            if settings.capture_port > 0 {
                match capture_start(&app.handle().clone(), &settings) {
                    Ok(port) => logs::log_line(&dirs, "capture", &format!("捕获服务已监听 127.0.0.1:{port}")),
                    Err(e) => logs::log_line(&dirs, "capture", &format!("捕获服务启动失败：{e}")),
                }
            }
            // 窗口 / 任务栏图标：自定义 LOGO 优先（PNG/ICO 可解码），否则用打包时嵌入的图标。''',
    1, 'setup 钩子')

# ── 6) 新增命令块（插在 generate_handler 之前）
NEW = r'''
/* ═════════════════════ 1.4 新增命令 ═════════════════════ */

/// 捕获服务句柄（全局唯一）
static CAPTURE: Mutex<Option<capture::CaptureServer>> = Mutex::new(None);

/// 启动捕获服务（setup 与命令共用）
fn capture_start(app: &tauri::AppHandle, settings: &AppSettings) -> Result<u16, String> {
    let mut g = CAPTURE.lock().map_err(|_| "捕获服务锁失败".to_string())?;
    if let Some(old) = g.take() {
        old.stop();
    }
    if settings.capture_port == 0 {
        return Ok(0);
    }
    let app2 = app.clone();
    let dirs = AppDirs::new();
    let server = capture::start(
        settings.capture_port,
        settings.capture_token.clone(),
        "Umidl".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
        move |payload| {
            logs::log_line(
                &dirs,
                "capture",
                &format!("捕获链接：{}（来源 {}）", payload.url, payload.source),
            );
            let _ = app2.emit("capture://url", payload);
        },
    )
    .map_err(|e| format!("监听 127.0.0.1:{} 失败：{e}", settings.capture_port))?;
    let port = server.port();
    *g = Some(server);
    Ok(port)
}

/// 重启捕获服务（设置改动后调用）
#[tauri::command]
fn capture_restart(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let s = state.0.settings_snapshot();
    let port = capture_start(&app, &s)?;
    Ok(serde_json::json!({ "running": port > 0, "port": port }))
}

/// 捕获服务状态
#[tauri::command]
fn capture_status() -> serde_json::Value {
    let running = CAPTURE.lock().ok().and_then(|g| g.as_ref().map(|s| s.port()));
    match running {
        Some(port) => serde_json::json!({ "running": true, "port": port }),
        None => serde_json::json!({ "running": false, "port": 0 }),
    }
}

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

/// 解释一条链接会走哪个引擎、是否会被过滤规则拦下
#[tauri::command]
fn explain_route(state: State<'_, AppState>, url: String) -> serde_json::Value {
    let s = state.0.settings_snapshot();
    let filters = crate::filters::Filters::from_settings(&s);
    let verdict = filters.check_url(&url);
    let req = DownloadRequest {
        url: url.clone(),
        ..Default::default()
    };
    let aria2 = downloader::wants_aria2(&req, &s);
    serde_json::json!({
        "engine": if aria2 { "aria2" } else { "ytdlp" },
        "engine_label": if aria2 { "aria2 · 16 连接分段并行（支持断点续传 / BT / 磁力 / FTP）" } else { "yt-dlp · 站点解析（1000+ 站点 / HLS / DASH）" },
        "blocked": verdict.is_block(),
        "reason": verdict.reason(),
    })
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
         过滤: 扩展名[{}] 域名黑[{}] 域名白[{}] 最小体积[{} MB]\n\
         捕获端口: {} · 自启动: {}\n",
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
        cfg.filter_ext_block,
        cfg.filter_domain_block,
        cfg.filter_domain_allow,
        cfg.filter_min_size_mb,
        cfg.capture_port,
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

/// 语义化版本比较：a > b → 1；相等 → 0；a < b → -1
fn cmp_version(a: &str, b: &str) -> i32 {
    let parse = |v: &str| -> Vec<i64> {
        v.trim()
            .trim_start_matches('v')
            .split(['.', '-', '+'])
            .map(|x| x.parse::<i64>().unwrap_or(0))
            .collect()
    };
    let (va, vb) = (parse(a), parse(b));
    for i in 0..va.len().max(vb.len()) {
        let x = va.get(i).copied().unwrap_or(0);
        let y = vb.get(i).copied().unwrap_or(0);
        if x != y {
            return if x > y { 1 } else { -1 };
        }
    }
    0
}

/// 检查更新（读取设置里的更新清单 JSON）
#[tauri::command]
async fn check_update(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let s = state.0.settings_snapshot();
    let url = s.update_manifest_url.trim().to_string();
    if url.is_empty() {
        return Ok(serde_json::json!({
            "ok": false,
            "current": env!("CARGO_PKG_VERSION"),
            "error": "尚未配置更新清单地址（设置 → 系统与更新）"
        }));
    }
    ratelimit::LIMITER.acquire(4096).await;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    match client.get(&url).send().await {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(j) => {
                let latest = j.get("version").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let current = env!("CARGO_PKG_VERSION").to_string();
                Ok(serde_json::json!({
                    "ok": true,
                    "current": current,
                    "latest": latest,
                    "has_update": !latest.is_empty() && cmp_version(&latest, &current) > 0,
                    "url": j.get("url").and_then(|v| v.as_str()),
                    "notes": j.get("notes").and_then(|v| v.as_str()),
                }))
            }
            Err(e) => Ok(serde_json::json!({ "ok": false, "error": format!("清单格式错误：{e}") })),
        },
        Err(e) => Ok(serde_json::json!({ "ok": false, "error": format!("无法连接更新服务：{e}") })),
    }
}

/// 关机（默认 60 秒后，可用 shutdown /a 取消）
#[tauri::command]
fn shutdown_system(state: State<'_, AppState>, delay_secs: u64) -> Result<(), String> {
    let d = delay_secs.clamp(0, 3600);
    #[cfg(windows)]
    {
        std::process::Command::new("shutdown")
            .args(["/s", "/t", &d.to_string()])
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(windows))]
    {
        let _ = d;
        return Err("当前平台暂不支持自动关机".into());
    }
    logs::log_line(&state.0.dirs, "power", &format!("已计划 {d} 秒后关机"));
    Ok(())
}

/// 取消已计划的关机
#[tauri::command]
fn cancel_shutdown() -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("shutdown")
            .arg("/a")
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 全部任务完成 + 设置开启 → 计划关机
fn maybe_shutdown_when_done(s: &AppState, app: &tauri::AppHandle) {
    let settings = match s.0.settings.lock() {
        Ok(g) => g.clone(),
        Err(_) => return,
    };
    if !settings.shutdown_when_done {
        return;
    }
    if s.0.running.lock().map(|g| !g.is_empty()).unwrap_or(true) {
        return;
    }
    let busy = s
        .0
        .db
        .list_downloads()
        .map(|v| {
            v.iter().any(|t| {
                matches!(
                    t.status,
                    TaskStatus::Pending
                        | TaskStatus::Parsing
                        | TaskStatus::Downloading
                        | TaskStatus::Converting
                        | TaskStatus::Extracting
                        | TaskStatus::Transcribing
                )
            })
        })
        .unwrap_or(true);
    if busy {
        return;
    }
    logs::log_line(&s.0.dirs, "power", "全部任务完成 → 60 秒后关机");
    notify_finish("Umidl · 即将关机", "全部任务已完成，60 秒后关机");
    let _ = app.emit("power://shutdown-scheduled", serde_json::json!({ "delay": 60 }));
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("shutdown")
            .args(["/s", "/t", "60"])
            .spawn();
    }
}

'''
rep('        .invoke_handler(tauri::generate_handler![', NEW + '        .invoke_handler(tauri::generate_handler![', 1, '插入命令块')

# ── 7) 注册新命令
rep('''            list_test_cases,
            run_selftest,
        ])''',
    '''            list_test_cases,
            run_selftest,
            set_autostart,
            get_autostart,
            capture_restart,
            capture_status,
            engine_info,
            explain_route,
            export_logs,
            check_update,
            shutdown_system,
            cancel_shutdown,
        ])''', 1, '注册命令')

# ── 8) autostart 插件
rep('''        .plugin(tauri_plugin_opener::init())''',
    '''        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))''', 1, 'autostart 插件')

# ── 9) 需要用到 TaskStatus
if 'use models::TaskStatus' not in s and 'TaskStatus' in s:
    rep('use models::{', 'use models::{TaskStatus, ', 1, 'TaskStatus 导入')

out = s.replace('\n', '\r\n') if crlf else s
open(P, 'w', encoding='utf-8', newline='').write(out)
print('lib.rs 补丁：', '全部成功' if not fails else f'{len(fails)} 处失败')
for f in fails:
    print('  ❌', f)
