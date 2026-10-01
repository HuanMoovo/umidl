"""1.6 接线：lib.rs 增加 enqueue_links（批量导入入队）与 convert_formats 命令 + 注册。幂等。"""
import sys

P = "src/lib.rs"
raw = open(P, encoding="utf-8", newline="").read()
crlf = "\r\n" in raw
s = raw.replace("\r\n", "\n")
orig = s
done = []

anchor = "/* ═════════════════════ 新增命令：文档转换 / ED2K / 插件沙箱 ═════════════════════ */"
cmds = '''/* ═════════════════════ 1.6 新增命令：批量导入 / 转换格式目录 ═════════════════════ */

/// 批量导入链接并入队（多行文本 / txt 导入）：逐条过过滤规则后入队。
/// 返回 { added, blocked, skipped, tasks, blocked_reasons, errors }
#[tauri::command]
async fn enqueue_links(
    app: AppHandle,
    state: State<'_, AppState>,
    text: String,
    output_dir: Option<String>,
) -> Result<serde_json::Value, String> {
    let s = state.0.clone();
    let cfg = s.settings_snapshot();
    let filters = crate::filters::Filters::from_settings(&cfg);

    let mut added = 0usize;
    let mut blocked = 0usize;
    let mut skipped = 0usize;
    let mut count = 0usize;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut tasks: Vec<serde_json::Value> = Vec::new();
    let mut blocked_reasons: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for raw in text.split(|c: char| {
        c == '\\n' || c == '\\r' || c == '\\t' || c == ' ' || c == ',' || c == '，'
    }) {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        if count >= 200 {
            break;
        }
        if !seen.insert(line.to_string()) {
            skipped += 1;
            continue;
        }
        let verdict = filters.check_url(line);
        if let Some(r) = verdict.reason() {
            blocked += 1;
            blocked_reasons.push(format!("{line} · {r}"));
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
        "blocked": blocked,
        "skipped": skipped,
        "tasks": tasks,
        "blocked_reasons": blocked_reasons,
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

'''
if "async fn enqueue_links" in s:
    done.append("① 已存在，跳过")
elif s.count(anchor) == 1:
    s = s.replace(anchor, cmds + anchor, 1)
    done.append("① enqueue_links + convert_formats 已插入")
else:
    sys.exit(f"① 锚点命中 {s.count(anchor)} 次，放弃")

reg_old = """            ed2k_submit,
            plugin_sandbox_info,"""
reg_new = """            ed2k_submit,
            enqueue_links,
            convert_formats,
            plugin_sandbox_info,"""
if "            enqueue_links,\n" in s:
    done.append("② 已注册，跳过")
elif s.count(reg_old) == 1:
    s = s.replace(reg_old, reg_new, 1)
    done.append("② 命令已注册")
else:
    sys.exit(f"② 注册锚点命中 {s.count(reg_old)} 次，放弃")

if s != orig:
    open(P, "w", encoding="utf-8", newline="").write(s.replace("\n", "\r\n") if crlf else s)
print("\n".join(done))
