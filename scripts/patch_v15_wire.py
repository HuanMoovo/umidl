"""把 1.5 新功能（文档转换 / ED2K / 插件沙箱）接进 lib.rs。

三处改动：
  ① run_download_job 里加 ED2K 分支（电驴链接不走 yt-dlp / aria2，交给 eMule 引擎接管）
  ② 加 14 个新命令（doc_* / ed2k_* / plugin_*）
  ③ generate_handler 注册 + explain_route 增加 ed2k 与插件意见字段

幂等：已接过的锚点会直接跳过，重复运行不会写入两次。
"""
import sys

P = "src/lib.rs"
raw = open(P, encoding="utf-8", newline="").read()
crlf = "\r\n" in raw
s = raw.replace("\r\n", "\n")
orig = s
done = []

# ── ① ED2K 分支 ───────────────────────────────────────────────────────────
anchor1 = "    let engine_label = if downloader::wants_aria2(&req, &ctx.settings) {"
branch = '''    // ED2K：程序不内嵌电驴协议栈，链接交给受管的 eMule 引擎接管（进度在引擎自己的窗口查看）
    if crate::ed2k::is_ed2k(&req.url) {
        let submitted = crate::ed2k::submit(&ctx, &req.url);
        s.take_pid(&task_id);
        s.unmark_running(&task_id);
        let mut t = task.clone();
        t.speed = None;
        t.eta = None;
        if let Ok(link) = crate::ed2k::parse(&req.url) {
            t.total = Some(link.size);
            t.title = link.name.clone();
        }
        match submitted {
            Ok(v) => {
                t.status = TaskStatus::Done;
                t.error = None;
                t.format_note = Some("ED2K · 已交由引擎接管".to_string());
                logs::log_line(
                    &ctx.dirs,
                    "download",
                    &format!("ED2K · 已交给引擎接管 · {v} · {}", req.url),
                );
            }
            Err(e) => {
                t.status = TaskStatus::Error;
                t.error = Some(format!("ED2K 引擎接管失败：{e}"));
                logs::log_line(
                    &ctx.dirs,
                    "download",
                    &format!("ED2K · 接管失败 · {e} · {}", req.url),
                );
            }
        }
        emit_task(&app, &t);
        let _ = s.db.upsert_download(&t, None);
        maybe_shutdown_when_done(&s, &app);
        return;
    }

'''
if "crate::ed2k::is_ed2k(&req.url)" in s:
    done.append("① 已存在，跳过")
elif s.count(anchor1) == 1:
    s = s.replace(anchor1, branch + anchor1, 1)
    done.append("① ED2K 分支已插入")
else:
    sys.exit(f"① 锚点命中 {s.count(anchor1)} 次，放弃")

# ── ② 新命令 ─────────────────────────────────────────────────────────────
anchor2 = "/// 解释一条链接会走哪个引擎、是否会被过滤规则拦下"
cmds = '''/* ═════════════════════ 新增命令：文档转换 / ED2K / 插件沙箱 ═════════════════════ */

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

/// 解析 ed2k 链接（粘贴即时预览）
#[tauri::command]
fn ed2k_parse(link: String) -> Result<serde_json::Value, String> {
    match crate::ed2k::parse(&link) {
        Ok(l) => Ok(serde_json::to_value(l).unwrap_or(serde_json::Value::Null)),
        Err(e) => Err(e.to_string()),
    }
}

/// ED2K 引擎状态（eMule / mlnet 是否安装、是否在运行）
#[tauri::command]
fn ed2k_engine_status(state: State<'_, AppState>) -> serde_json::Value {
    let s = state.0.clone();
    let ctx = build_ctx(&s, None);
    crate::ed2k::engine_status(&ctx)
}

/// 把 ed2k 链接交给引擎接管
#[tauri::command]
fn ed2k_submit(state: State<'_, AppState>, link: String) -> Result<serde_json::Value, String> {
    let s = state.0.clone();
    let ctx = build_ctx(&s, None);
    match crate::ed2k::submit(&ctx, &link) {
        Ok(v) => Ok(v),
        Err(e) => Err(e.to_string()),
    }
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
fn plugin_market_list(state: State<'_, AppState>) -> serde_json::Value {
    let s = state.0.clone();
    let ctx = build_ctx(&s, None);
    crate::plugins::market_list(&ctx)
}

/// 从市场安装插件（内容寻址校验：sha256 不一致直接拒绝）
#[tauri::command]
fn plugin_install(state: State<'_, AppState>, id: String) -> Result<serde_json::Value, String> {
    let s = state.0.clone();
    let ctx = build_ctx(&s, None);
    crate::plugins::install_from_market(&ctx, &id).map_err(|e| e.to_string())
}

/// 卸载插件
#[tauri::command]
fn plugin_uninstall(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let s = state.0.clone();
    let ctx = build_ctx(&s, None);
    crate::plugins::uninstall_plugin(&ctx, &id).map_err(|e| e.to_string())
}

/// 启用 / 禁用插件
#[tauri::command]
fn plugin_set_enabled(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    let s = state.0.clone();
    let ctx = build_ctx(&s, None);
    crate::plugins::set_plugin_enabled(&ctx, &id, enabled).map_err(|e| e.to_string())
}

/// 在沙箱里试跑插件（含 selfTest）
#[tauri::command]
fn plugin_test(state: State<'_, AppState>, id: String) -> serde_json::Value {
    let s = state.0.clone();
    let ctx = build_ctx(&s, None);
    crate::plugins::test_plugin(&ctx, &id)
}

/// 让所有启用插件的解析器对 URL 表态
#[tauri::command]
fn plugin_run_resolvers(state: State<'_, AppState>, url: String) -> serde_json::Value {
    let s = state.0.clone();
    let ctx = build_ctx(&s, None);
    crate::plugins::run_resolvers(&ctx, &url)
}

'''
if "fn doc_capabilities" in s:
    done.append("② 已存在，跳过")
elif s.count(anchor2) == 1:
    s = s.replace(anchor2, cmds + anchor2, 1)
    done.append("② 14 个新命令已插入")
else:
    sys.exit(f"② 锚点命中 {s.count(anchor2)} 次，放弃")

# ── ③ 注册 ───────────────────────────────────────────────────────────────
reg_old = "            engine_info,\n            explain_route,"
reg_new = """            engine_info,
            explain_route,
            doc_capabilities,
            probe_document,
            ed2k_parse,
            ed2k_engine_status,
            ed2k_submit,
            plugin_sandbox_info,
            plugin_list,
            plugin_market_list,
            plugin_install,
            plugin_uninstall,
            plugin_set_enabled,
            plugin_test,
            plugin_run_resolvers,"""
if "doc_capabilities," in s:
    done.append("③ 已注册，跳过")
elif s.count(reg_old) == 1:
    s = s.replace(reg_old, reg_new, 1)
    done.append("③ 命令已注册")
else:
    sys.exit(f"③ 锚点命中 {s.count(reg_old)} 次，放弃")

# ── ④ explain_route 扩展 ─────────────────────────────────────────────────
old_route = '''    let aria2 = downloader::wants_aria2(&req, &s);
    serde_json::json!({
        "engine": if aria2 { "aria2" } else { "ytdlp" },
        "engine_label": if aria2 { "aria2 · 16 连接分段并行（支持断点续传 / BT / 磁力 / FTP）" } else { "yt-dlp · 站点解析（1000+ 站点 / HLS / DASH）" },
        "blocked": verdict.is_block(),
        "reason": verdict.reason(),
    })'''
new_route = '''    let aria2 = downloader::wants_aria2(&req, &s);
    let ed2k = crate::ed2k::is_ed2k(&url);
    let ctx = build_ctx(&s, None);
    let plugins = crate::plugins::run_resolvers(&ctx, &url);
    serde_json::json!({
        "engine": if ed2k { "ed2k" } else if aria2 { "aria2" } else { "ytdlp" },
        "engine_label": if ed2k {
            "eMule · 电驴引擎接管（ed2k）"
        } else if aria2 {
            "aria2 · 16 连接分段并行（支持断点续传 / BT / 磁力 / FTP）"
        } else {
            "yt-dlp · 站点解析（1000+ 站点 / HLS / DASH）"
        },
        "blocked": verdict.is_block(),
        "reason": verdict.reason(),
        "ed2k": ed2k,
        "plugins": plugins,
    })'''
if '"ed2k": ed2k,' in s:
    done.append("④ 已扩展，跳过")
elif s.count(old_route) == 1:
    s = s.replace(old_route, new_route, 1)
    done.append("④ explain_route 已扩展（ed2k + plugins）")
else:
    sys.exit(f"④ 锚点命中 {s.count(old_route)} 次，放弃")

if s != orig:
    open(P, "w", encoding="utf-8", newline="").write(s.replace("\n", "\r\n") if crlf else s)
    print("已写入 lib.rs")
print("\n".join(done))
