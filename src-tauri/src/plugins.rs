/*!
JavaScript 插件沙箱（QuickJS 运行时 + 能力白名单 + 内容寻址校验）

设计要点
--------
1. **沙箱**：每个插件的一次调用都在独立的 `rquickjs` Runtime 中执行，施加三重限制：
   - 内存上限 `MEMORY_LIMIT_MB`（`JS_SetMemoryLimit`）
   - 栈上限 `STACK_LIMIT_KB`（`JS_SetMaxStackSize`）
   - 墙钟时间预算 `SCRIPT_TIMEOUT_MS`（`JS_SetInterruptHandler` 读共享的 deadline 原子量）
   超时/超内存都表现为「异常」而不是死循环或进程崩溃。
2. **能力白名单**：宿主不注入 `require` / `process` / `fetch` / `XMLHttpRequest` /
   任何文件或网络 API；只注入一个 `umi` 对象：
   `umi.log`、`umi.version`、`umi.resolve`、`umi.registerResolver`、`umi.on`、
   `umi.retry`、`umi.storage.get/set`（外加 `umi.export` 用于导出 `selfTest` 等钩子）。
3. **事件**：`download:done`、`download:error`、`app:start`（宿主可派发任意事件名，插件自行过滤）。
4. **内容寻址市场**：`plugins/market/index.json` 里每个插件都带真实 sha256；
   安装时对拿到字节重新计算 sha256 并与索引比对，不一致一律拒绝安装。
5. **目录布局**（`%APPDATA%/umi-downloader/`，测试可用 `UMI_DATA_DIR` 覆盖）：
   ```text
   plugins/registry.json
   plugins/<id>/plugin.js
   plugins/<id>/manifest.json
   plugins/<id>/storage.json        （插件 umi.storage 的落盘位置，由宿主代写）
   plugins/market/index.json
   plugins/market/files/<id>-<ver>.js
   ```

返回结构（供前端 / selftest 使用）
--------------------------------
* `sandbox_selftest()` → `{available, engine, version, memory_limit_mb, script_timeout_ms}`
* `list_plugins()`     → `[{id,name,version,description,enabled,sha256,source,installed_at}]`
* `market_list()`      → `plugins/market/index.json` 原文
* `test_plugin(id)`    → `{ok, logs, result, error}`
* `run_resolvers(url)` → `{url, queried, opinions:[{plugin,ok,matched,direct,result,error}], direct, best_url}`
* `dispatch_event(e)`  → `{event, notified, retries:[{plugin,task_id,attempt,reason}], errors:[{plugin,error}]}`
*/

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use rquickjs::context::EvalOptions;
use rquickjs::function::Opt;
use rquickjs::{Array, Context as JsContext, Ctx as JsCtx, Function, Object, Runtime, Type, Value};
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};

use crate::ctx::Ctx;

/* ==================== 常量 ==================== */

/// 沙箱引擎名
pub const ENGINE: &str = "quickjs";
/// 插件 API 版本（暴露为 `umi.version`）
pub const PLUGIN_API_VERSION: &str = "1.0";
/// 内存上限（MB）
pub const MEMORY_LIMIT_MB: usize = 16;
/// 内存上限（字节）
pub const MEMORY_LIMIT_BYTES: usize = MEMORY_LIMIT_MB * 1024 * 1024;
/// 栈上限（KB）
pub const STACK_LIMIT_KB: usize = 512;
/// 单次钩子（脚本求值 / 单次回调）的墙钟时间预算（ms）
pub const SCRIPT_TIMEOUT_MS: u64 = 200;
/// 单次运行里单个插件最多保留的日志行数
const LOG_LIMIT: usize = 200;
/// 单次运行里单个插件最多收集的重试请求数
const RETRY_LIMIT: usize = 64;
/// 插件 id 允许的字符集（防目录穿越）
const MAX_ID_LEN: usize = 64;
/// 宿主支持的事件名（文档用途；派发时不强制）
pub const PLUGIN_EVENTS: [&str; 3] = ["download:done", "download:error", "app:start"];

const VIDEO_EXTS: &[&str] = &["mp4", "mkv", "avi", "mov", "webm", "flv", "wmv", "m4v", "ts", "mpg", "mpeg", "rmvb"];
const AUDIO_EXTS: &[&str] = &["mp3", "flac", "wav", "aac", "m4a", "ogg", "opus", "wma", "ape"];
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff", "heic"];
const ARCHIVE_EXTS: &[&str] = &["zip", "7z", "rar", "tar", "gz", "bz2", "xz", "iso", "zst"];
/* ==================== 事件日志（前端「设置 → 插件 → 事件日志」） ==================== */

/// 把插件相关动作发到前端的 `plugin://event`。页面按 `{id, event, level, message}`
/// 宽松解析（字段缺失也能显示，未知级别按 info 处理），最多保留 200 条。
pub fn log_event(ctx: &Ctx, level: &str, id: &str, event: &str, message: &str) {
    ctx.emit_json(
        "plugin://event",
        &json!({ "id": id, "event": event, "level": level, "message": message }),
    );
}

/// 从 `dispatch_event` 的结果里取出「请求重试的任务 id」（只认本次任务，去重）。
/// 抽成纯函数：宿主拿到它之后把任务重新入队（见 `dispatch_plugin_download_event`）。
pub fn retry_task_ids(result: &Json, task_id: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(list) = result.get("retries").and_then(|v| v.as_array()) {
        for r in list {
            let id = r
                .get("task_id")
                .and_then(|v| v.as_str())
                .or_else(|| r.get("taskId").and_then(|v| v.as_str()));
            if let Some(id) = id {
                if id == task_id && !out.iter().any(|x| x == id) {
                    out.push(id.to_string());
                }
            }
        }
    }
    out
}

const DOC_EXTS: &[&str] = &["pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "epub", "mobi", "txt", "md", "csv"];
const EXE_EXTS: &[&str] = &["exe", "msi", "apk", "dmg", "deb", "rpm", "appx"];
const SUB_EXTS: &[&str] = &["srt", "ass", "ssa", "vtt", "sub"];
const STREAM_EXTS: &[&str] = &["m3u8", "mpd", "m3u"];
const TORRENT_EXTS: &[&str] = &["torrent"];
const PAGE_EXTS: &[&str] = &["html", "htm", "php", "asp", "aspx", "jsp", "do", "action", "cgi", "shtml"];
const PAGE_MARKERS: &[&str] = &["/watch", "/play", "/video/", "/vod/", "/detail", "/album", "/post/", "/item/"];
const DOWNLOAD_QUERY_KEYS: &[&str] = &["filename", "file", "download", "dl", "response-content-disposition", "attname"];

/// 沙箱是否可用（真的建一个 Runtime 试一下）
pub fn runtime_available() -> bool {
    Runtime::new().is_ok()
}

/* ==================== 内置市场源码 ==================== */

const BUILTIN_DIRECT_LINK_SNIFFER_JS: &str = include_str!("plugins_builtin/direct_link_sniffer.js");
const BUILTIN_RETRY_HOOK_JS: &str = include_str!("plugins_builtin/retry_hook.js");

struct BuiltinPlugin {
    id: &'static str,
    name: &'static str,
    version: &'static str,
    description: &'static str,
    author: &'static str,
    permissions: &'static [&'static str],
    source: &'static str,
}

fn builtin_plugins() -> Vec<BuiltinPlugin> {
    vec![
        BuiltinPlugin {
            id: "direct-link-sniffer",
            name: "直链嗅探器",
            version: "1.0.0",
            description: "用 umi.resolve 判断链接是否为可直下的直链，并给出媒体类型与置信度",
            author: "umi",
            permissions: &["resolve"],
            source: BUILTIN_DIRECT_LINK_SNIFFER_JS,
        },
        BuiltinPlugin {
            id: "retry-hook",
            name: "失败重试钩子",
            version: "1.0.0",
            description: "监听 download:error，首次失败请求 umi.retry 一次，并记住次数避免无限重试",
            author: "umi",
            permissions: &["events", "storage", "retry"],
            source: BUILTIN_RETRY_HOOK_JS,
        },
    ]
}

/* ==================== 目录与哈希 ==================== */

fn plugins_dir(ctx: &Ctx) -> PathBuf {
    ctx.dirs.data.join("plugins")
}
fn market_dir(ctx: &Ctx) -> PathBuf {
    plugins_dir(ctx).join("market")
}
fn market_files_dir(ctx: &Ctx) -> PathBuf {
    market_dir(ctx).join("files")
}
fn market_index_file(ctx: &Ctx) -> PathBuf {
    market_dir(ctx).join("index.json")
}
fn registry_file(ctx: &Ctx) -> PathBuf {
    plugins_dir(ctx).join("registry.json")
}
fn plugin_dir(ctx: &Ctx, id: &str) -> PathBuf {
    plugins_dir(ctx).join(id)
}
fn plugin_script(ctx: &Ctx, id: &str) -> PathBuf {
    plugin_dir(ctx, id).join("plugin.js")
}
fn plugin_storage_file(ctx: &Ctx, id: &str) -> PathBuf {
    plugin_dir(ctx, id).join("storage.json")
}

/// 校验插件 id：只允许 `[a-z0-9-_]`，避免目录穿越
fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > MAX_ID_LEN {
        bail!("非法插件 id（长度必须在 1..={MAX_ID_LEN}）：{id:?}");
    }
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
        bail!("非法插件 id（只允许字母/数字/-/_/.）：{id:?}");
    }
    if id.contains("..") {
        bail!("非法插件 id（不允许 ..）：{id:?}");
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 计算字节内容的 sha256（小写十六进制）
pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex(&hasher.finalize())
}

/// 计算文件的 sha256（小写十六进制）
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hex(&hasher.finalize()))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp-write");
    std::fs::write(&tmp, bytes)?;
    if std::fs::rename(&tmp, path).is_err() {
        // Windows 上 rename 不能覆盖已存在的目标文件
        let _ = std::fs::remove_file(path);
        std::fs::rename(&tmp, path).map_err(|e| anyhow!("写入 {} 失败: {e}", path.display()))?;
    }
    Ok(())
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/* ==================== 注册表 ==================== */

fn empty_registry() -> Json {
    json!({ "version": 1, "plugins": [] })
}

fn read_registry(ctx: &Ctx) -> Json {
    let path = registry_file(ctx);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|_| empty_registry()),
        Err(_) => empty_registry(),
    }
}

fn write_registry(ctx: &Ctx, reg: &Json) -> Result<()> {
    let text = serde_json::to_string_pretty(reg)?;
    write_atomic(&registry_file(ctx), text.as_bytes())
}

fn registry_entries(reg: &Json) -> Vec<Json> {
    reg.get("plugins")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default()
}

fn find_entry(reg: &Json, id: &str) -> Option<Json> {
    registry_entries(reg).into_iter().find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id))
}

/// 注册表条目 → 对外暴露的 7 个字段
fn public_record(entry: &Json) -> Json {
    let s = |k: &str| entry.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    json!({
        "id": s("id"),
        "name": s("name"),
        "version": s("version"),
        "description": s("description"),
        "enabled": entry.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false),
        "sha256": s("sha256"),
        "source": s("source"),
        "installed_at": s("installed_at"),
    })
}

fn upsert_entry(reg: &mut Json, entry: Json) -> Result<()> {
    let id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let obj = reg
        .as_object_mut()
        .ok_or_else(|| anyhow!("注册表格式损坏"))?;
    let list = obj
        .entry("plugins".to_string())
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| anyhow!("注册表格式损坏（plugins 不是数组）"))?;
    list.retain(|e| e.get("id").and_then(|v| v.as_str()) != Some(id.as_str()));
    list.push(entry);
    list.sort_by(|a, b| {
        a.get("id").and_then(|v| v.as_str()).unwrap_or("").cmp(b.get("id").and_then(|v| v.as_str()).unwrap_or(""))
    });
    Ok(())
}

/* ==================== 市场（内容寻址） ==================== */

/// 首次运行生成内置市场：index.json + files/<id>-<ver>.js，sha256 现算现写
fn ensure_market(ctx: &Ctx) -> Result<()> {
    let index_path = market_index_file(ctx);
    if let Ok(text) = std::fs::read_to_string(&index_path) {
        if serde_json::from_str::<Json>(&text).map(|v| v.get("plugins").is_some()).unwrap_or(false) {
            return Ok(());
        }
    }
    std::fs::create_dir_all(market_files_dir(ctx))?;
    let mut entries = Vec::new();
    for p in builtin_plugins() {
        let file_name = format!("{}-{}.js", p.id, p.version);
        let bytes = p.source.as_bytes();
        let digest = sha256_bytes(bytes);
        write_atomic(&market_files_dir(ctx).join(&file_name), bytes)?;
        entries.push(json!({
            "id": p.id,
            "name": p.name,
            "version": p.version,
            "description": p.description,
            "author": p.author,
            "permissions": p.permissions,
            "sha256": digest,
            "size": bytes.len(),
            "file": file_name,
            "url": Json::Null,
            "builtin": true,
        }));
    }
    let index = json!({
        "version": 1,
        "generated_at": now_iso(),
        "description": "Umidl 内置插件市场（内容寻址：sha256 为文件真实摘要）",
        "plugins": entries,
    });
    write_atomic(&index_path, serde_json::to_string_pretty(&index)?.as_bytes())?;
    Ok(())
}

/// 市场索引原文
pub fn market_list(ctx: &Ctx) -> Json {
    if let Err(e) = ensure_market(ctx) {
        crate::ctx::cwarn(&format!("生成内置市场失败: {e}"));
    }
    match std::fs::read_to_string(market_index_file(ctx)) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|_| json!({ "version": 1, "plugins": [] })),
        Err(_) => json!({ "version": 1, "plugins": [] }),
    }
}

fn http_get_bytes(url: &str) -> Result<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let resp = client.get(url).send()?;
    let status = resp.status();
    if !status.is_success() {
        bail!("下载插件失败：HTTP {status}");
    }
    Ok(resp.bytes()?.to_vec())
}

/// 从市场安装：取文件 → **sha256 与索引比对** → 落盘 → 登记
pub fn install_from_market(ctx: &Ctx, id: &str) -> Result<Json> {
    validate_id(id)?;
    ensure_market(ctx)?;
    let index = market_list(ctx);
    let entry = index
        .get("plugins")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.iter().find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id)))
        .cloned()
        .ok_or_else(|| anyhow!("市场中不存在插件 {id}"))?;

    let expected = entry
        .get("sha256")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if expected.len() != 64 {
        bail!("市场索引缺少有效 sha256，拒绝安装 {id}");
    }

    // 1) 取文件：优先 http(s)，否则本地 market/files/
    let bytes = match entry.get("url").and_then(|v| v.as_str()) {
        Some(u) if u.starts_with("http://") || u.starts_with("https://") => http_get_bytes(u)?,
        _ => {
            let file_name = entry
                .get("file")
                .and_then(|v| v.as_str())
                .filter(|f| !f.contains('/') && !f.contains('\\') && !f.contains(".."))
                .map(|f| f.to_string())
                .unwrap_or_else(|| format!("{id}-{}.js", entry.get("version").and_then(|v| v.as_str()).unwrap_or("0")));
            let path = market_files_dir(ctx).join(&file_name);
            std::fs::read(&path).map_err(|e| anyhow!("读取市场文件 {} 失败: {e}", path.display()))?
        }
    };

    // 2) 内容寻址校验：一个字节不同就拒绝
    let actual = sha256_bytes(&bytes);
    if actual != expected {
        bail!("sha256 校验失败，拒绝安装 {id}：期望 {expected}，实际 {actual}（内容寻址校验不通过）");
    }

    // 3) 落盘
    let dir = plugin_dir(ctx, id);
    std::fs::create_dir_all(&dir)?;
    write_atomic(&plugin_script(ctx, id), &bytes)?;
    let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or(id).to_string();
    let version = entry.get("version").and_then(|v| v.as_str()).unwrap_or("0.0.0").to_string();
    let description = entry.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let author = entry.get("author").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let permissions = entry.get("permissions").cloned().unwrap_or_else(|| json!([]));
    let manifest = json!({
        "id": id,
        "name": name,
        "version": version,
        "description": description,
        "author": author,
        "sha256": actual,
        "permissions": permissions,
        "installed_at": now_iso(),
        "source": "market",
    });
    write_atomic(&dir.join("manifest.json"), serde_json::to_string_pretty(&manifest)?.as_bytes())?;

    // 4) 登记
    let record = json!({
        "id": id,
        "name": name,
        "version": version,
        "description": description,
        "author": author,
        "sha256": actual,
        "source": "market",
        "enabled": true,
        "installed_at": manifest.get("installed_at").cloned().unwrap_or(Json::Null),
    });
    let mut reg = read_registry(ctx);
    upsert_entry(&mut reg, record.clone())?;
    write_registry(ctx, &reg)?;
    log_event(
        ctx,
        "info",
        id,
        "install",
        &format!("已安装 {name} v{version}（sha256 {}）", &actual[..12.min(actual.len())]),
    );
    Ok(public_record(&record))
}

/* ==================== 插件管理 ==================== */

/// 已安装插件列表
pub fn list_plugins(ctx: &Ctx) -> Json {
    let reg = read_registry(ctx);
    let mut out: Vec<Json> = registry_entries(&reg).iter().map(public_record).collect();
    out.sort_by(|a, b| {
        a.get("id").and_then(|v| v.as_str()).unwrap_or("").cmp(b.get("id").and_then(|v| v.as_str()).unwrap_or(""))
    });
    Json::Array(out)
}

/// 卸载插件（删除目录 + 注销）
pub fn uninstall_plugin(ctx: &Ctx, id: &str) -> Result<()> {
    validate_id(id)?;
    let mut reg = read_registry(ctx);
    if find_entry(&reg, id).is_none() {
        bail!("插件未安装：{id}");
    }
    let dir = plugin_dir(ctx, id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| anyhow!("删除插件目录 {} 失败: {e}", dir.display()))?;
    }
    if let Some(obj) = reg.as_object_mut() {
        if let Some(list) = obj.get_mut("plugins").and_then(|v| v.as_array_mut()) {
            list.retain(|e| e.get("id").and_then(|v| v.as_str()) != Some(id));
        }
    }
    write_registry(ctx, &reg)?;
    log_event(ctx, "info", id, "uninstall", "已卸载");
    Ok(())
}

/// 启用 / 停用插件，返回更新后的记录
pub fn set_plugin_enabled(ctx: &Ctx, id: &str, enabled: bool) -> Result<Json> {
    validate_id(id)?;
    let mut reg = read_registry(ctx);
    let mut entry = find_entry(&reg, id).ok_or_else(|| anyhow!("插件未安装：{id}"))?;
    if let Some(obj) = entry.as_object_mut() {
        obj.insert("enabled".to_string(), json!(enabled));
    }
    upsert_entry(&mut reg, entry.clone())?;
    write_registry(ctx, &reg)?;
    log_event(
        ctx,
        "info",
        id,
        "enable",
        if enabled { "已启用（解析器 / 事件钩子随之下一次调用生效）" } else { "已停用" },
    );
    Ok(public_record(&entry))
}

fn enabled_ids(ctx: &Ctx) -> Vec<String> {
    registry_entries(&read_registry(ctx))
        .into_iter()
        .filter(|e| e.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false))
        .filter_map(|e| e.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .filter(|id| plugin_script(ctx, id).exists())
        .collect()
}

/* ==================== 沙箱 ==================== */

/// 宿主共享给 JS 的收集器（日志 / 重试请求 / 存储 / 宿主级错误）
#[derive(Default)]
struct SandboxState {
    plugin_id: String,
    logs: Mutex<Vec<String>>,
    retries: Mutex<Vec<Json>>,
    errors: Mutex<Vec<String>>,
    storage: Mutex<HashMap<String, Json>>,
    dirty: AtomicBool,
}

impl SandboxState {
    fn log(&self, line: impl Into<String>) {
        if let Ok(mut logs) = self.logs.lock() {
            if logs.len() < LOG_LIMIT {
                logs.push(line.into());
            } else if logs.len() == LOG_LIMIT {
                logs.push(format!("…（日志超过 {LOG_LIMIT} 行，已截断）"));
            }
        }
    }
    fn error(&self, line: impl Into<String>) {
        if let Ok(mut errs) = self.errors.lock() {
            errs.push(line.into());
        }
    }
    fn take_logs(&self) -> Vec<String> {
        self.logs.lock().map(|mut l| std::mem::take(&mut *l)).unwrap_or_default()
    }
    fn take_retries(&self) -> Vec<Json> {
        self.retries.lock().map(|mut r| std::mem::take(&mut *r)).unwrap_or_default()
    }
    fn take_errors(&self) -> Vec<String> {
        self.errors.lock().map(|mut e| std::mem::take(&mut *e)).unwrap_or_default()
    }
    fn snapshot_storage(&self) -> HashMap<String, Json> {
        self.storage.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

/// 一次插件调用结束后的产物
struct SandboxRun {
    logs: Vec<String>,
    retries: Vec<Json>,
    errors: Vec<String>,
}

/// 插件沙箱：一个 Runtime + 一个 Context + 一条 deadline
struct Sandbox {
    /// 持有 Runtime（Context 内部也持有引用；显式保留避免依赖内部实现）
    #[allow(dead_code)]
    runtime: Runtime,
    context: JsContext,
    deadline: Arc<AtomicU64>,
    state: Arc<SandboxState>,
}

impl Sandbox {
    fn new(plugin_id: &str, storage: HashMap<String, Json>) -> Result<Self> {
        let runtime = Runtime::new().map_err(|e| anyhow!("创建 QuickJS 运行时失败: {e}"))?;
        // 限制一：内存
        runtime.set_memory_limit(MEMORY_LIMIT_BYTES);
        // 限制二：栈
        runtime.set_max_stack_size(STACK_LIMIT_KB * 1024);
        // 限制三：墙钟时间预算（interrupt handler 返回 true → 抛不可捕获异常）
        let deadline = Arc::new(AtomicU64::new(u64::MAX));
        let watch = Arc::clone(&deadline);
        runtime.set_interrupt_handler(Some(Box::new(move || now_ms() >= watch.load(Ordering::Relaxed))));
        let context = JsContext::full(&runtime).map_err(|e| anyhow!("创建 QuickJS 上下文失败: {e}"))?;
        let state = Arc::new(SandboxState {
            plugin_id: plugin_id.to_string(),
            storage: Mutex::new(storage),
            ..Default::default()
        });
        let sandbox = Self { runtime, context, deadline, state };
        sandbox.context.with(|ctx| {
            install_prelude(&ctx).map_err(|e| anyhow!("沙箱预置失败: {e}"))?;
            install_umi(&ctx, &sandbox.state).map_err(|e| anyhow!("注入 umi 能力白名单失败: {e}"))?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(sandbox)
    }

    /// 重新开始计时
    fn arm(&self, ms: u64) {
        self.deadline.store(now_ms().saturating_add(ms), Ordering::Relaxed);
    }

    /// 关闭计时（不再打断）
    fn disarm(&self) {
        self.deadline.store(u64::MAX, Ordering::Relaxed);
    }

    fn run_state(&self) -> SandboxRun {
        SandboxRun {
            logs: self.state.take_logs(),
            retries: self.state.take_retries(),
            errors: self.state.take_errors(),
        }
    }

    /// 在沙箱里求值一段脚本（global 模式）
    fn eval(&self, source: &str, filename: &str) -> Result<Json> {
        self.arm(SCRIPT_TIMEOUT_MS);
        let out = self.context.with(|ctx| {
            let mut options = EvalOptions::default();
            options.global = true;
            options.filename = Some(filename.to_string());
            let value: Value = ctx
                .eval_with_options(source, options)
                .map_err(|e| js_error(&ctx, e))?;
            Ok::<Json, anyhow::Error>(js_to_json(&value))
        });
        self.disarm();
        out
    }

    /// 在沙箱里求值并转成字符串
    fn eval_text(&self, source: &str) -> Result<String> {
        let v = self.eval(source, "probe.js")?;
        Ok(match v {
            Json::String(s) => s,
            other => other.to_string(),
        })
    }

    /// 调用已注册的解析器，返回每个解析器的原始结论
    fn call_resolvers(&self, url: &str) -> Result<Vec<Json>> {
        let info = analyze_url(url);
        self.context.with(|ctx| {
            let umi: Object = ctx.globals().get("umi").map_err(|e| js_error(&ctx, e))?;
            let resolvers: Array = umi.get("__resolvers").map_err(|e| js_error(&ctx, e))?;
            let url_arg: Value = ctx
                .json_parse(serde_json::to_string(url)?)
                .map_err(|e| js_error(&ctx, e))?;
            let info_arg: Value = ctx
                .json_parse(serde_json::to_string(&info)?)
                .map_err(|e| js_error(&ctx, e))?;
            let mut out = Vec::new();
            for i in 0..resolvers.len() {
                let f: Function = match resolvers.get(i) {
                    Ok(f) => f,
                    Err(e) => {
                        out.push(json!({ "error": format!("解析器 #{} 不可调用: {e}", i) }));
                        continue;
                    }
                };
                self.arm(SCRIPT_TIMEOUT_MS);
                match f.call::<_, Value>((url_arg.clone(), info_arg.clone())) {
                    Ok(v) => out.push(js_to_json(&v)),
                    Err(e) => {
                        let msg = js_error(&ctx, e).to_string();
                        out.push(json!({ "error": msg }));
                    }
                }
            }
            self.disarm();
            Ok::<Vec<Json>, anyhow::Error>(out)
        })
    }

    /// 派发事件给已注册的回调，返回成功回调数
    fn call_event(&self, event: &str, payload: &Json) -> Result<i32> {
        self.context.with(|ctx| {
            let umi: Object = ctx.globals().get("umi").map_err(|e| js_error(&ctx, e))?;
            let handlers: Object = umi.get("__handlers").map_err(|e| js_error(&ctx, e))?;
            let slot: Value = handlers.get(event).map_err(|e| js_error(&ctx, e))?;
            let Some(list) = slot.as_array() else {
                return Ok(0);
            };
            let arg: Value = ctx
                .json_parse(serde_json::to_string(payload)?)
                .map_err(|e| js_error(&ctx, e))?;
            let mut count = 0;
            for i in 0..list.len() {
                let f: Function = match list.get(i) {
                    Ok(f) => f,
                    Err(e) => {
                        self.state.error(format!("事件 {event} 回调 #{i} 不可调用: {e}"));
                        continue;
                    }
                };
                self.arm(SCRIPT_TIMEOUT_MS);
                match f.call::<_, Value>((arg.clone(),)) {
                    Ok(_) => count += 1,
                    Err(e) => self.state.error(js_error(&ctx, e).to_string()),
                }
            }
            self.disarm();
            Ok::<i32, anyhow::Error>(count)
        })
    }

    /// 寻找插件导出的 selfTest（`umi.export("selfTest", fn)` 或全局函数 `selfTest`）
    fn call_selftest(&self) -> Result<Option<Json>> {
        self.context.with(|ctx| {
            let umi: Object = ctx.globals().get("umi").map_err(|e| js_error(&ctx, e))?;
            let exported: Object = umi.get("__exports").map_err(|e| js_error(&ctx, e))?;
            let mut hook: Option<Function> = exported
                .get::<_, Value>("selfTest")
                .ok()
                .and_then(|v| v.into_function());
            if hook.is_none() {
                hook = ctx
                    .globals()
                    .get::<_, Value>("selfTest")
                    .ok()
                    .and_then(|v| v.into_function());
            }
            let Some(f) = hook else { return Ok(None) };
            self.arm(SCRIPT_TIMEOUT_MS);
            let value: Value = f.call::<_, Value>(()).map_err(|e| js_error(&ctx, e))?;
            self.disarm();
            Ok::<Option<Json>, anyhow::Error>(Some(js_to_json(&value)))
        })
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        self.deadline.store(0, Ordering::Relaxed);
    }
}

/* ==================== 沙箱内的宿主接口 ==================== */

/// 把 rquickjs 错误转成带 JS 异常文本的 anyhow 错误（超时会给出明确文案）
fn js_error(ctx: &JsCtx<'_>, err: rquickjs::Error) -> anyhow::Error {
    if !matches!(err, rquickjs::Error::Exception) {
        return anyhow!("JS 引擎错误: {err}");
    }
    let text = catch_text(ctx);
    let lower = text.to_ascii_lowercase();
    if lower.contains("interrupted") {
        anyhow!("沙箱超时：脚本超过 {SCRIPT_TIMEOUT_MS} ms 时间预算，已被中断（interrupted）")
    } else if lower.contains("out of memory") {
        anyhow!("沙箱内存超限：超过 {MEMORY_LIMIT_MB} MB 上限（out of memory）")
    } else {
        anyhow!("插件异常: {text}")
    }
}

/// 取出并消费当前挂起的 JS 异常，转成文本
fn catch_text(ctx: &JsCtx<'_>) -> String {
    let value = ctx.catch();
    if value.is_null() || value.is_undefined() {
        return "unknown javascript error".to_string();
    }
    let rendered = ctx
        .globals()
        .get::<_, Value>("__umi_errstr")
        .ok()
        .and_then(|v| v.into_function())
        .and_then(|f| f.call::<_, Value>((value.clone(),)).ok())
        .and_then(|v| v.into_string().and_then(|s| s.to_string().ok()));
    match rendered {
        Some(s) if !s.is_empty() => s,
        _ => js_to_json(&value).to_string(),
    }
}

/// 沙箱预置：一个把异常渲染成字符串的纯 JS 工具（不含任何宿主能力）
fn install_prelude<'js>(ctx: &JsCtx<'js>) -> rquickjs::Result<()> {
    ctx.eval::<Value, _>(
        r#"
        globalThis.__umi_errstr = function (e) {
          try {
            if (e && typeof e.message === "string") {
              var s = e.message;
              if (e.stack && typeof e.stack === "string") {
                var lines = String(e.stack).split("\n");
                if (lines.length > 1) { s += " | " + lines[1].trim(); }
              }
              return s;
            }
            return String(e);
          } catch (_) { return "unprintable javascript error"; }
        };
        "#,
    )?;
    Ok(())
}

fn value_to_text(v: &Value<'_>) -> String {
    match js_to_json(v) {
        Json::String(s) => s,
        other => other.to_string(),
    }
}

/// `umi.resolve` 的入参宽容处理：字符串直接用，对象取 `url` 字段
fn coerce_url_arg(v: &Value<'_>) -> String {
    if let Some(s) = v.as_string() {
        if let Ok(s) = s.to_string() {
            return s;
        }
    }
    if let Some(obj) = v.as_object() {
        if let Ok(s) = obj.get::<_, std::string::String>("url") {
            return s;
        }
    }
    value_to_text(v)
}

fn push_log(state: &SandboxState, line: String) {
    state.log(format!("[{}] {line}", state.plugin_id));
}

/// 注入能力白名单：只有 `umi`
fn install_umi<'js>(ctx: &JsCtx<'js>, state: &Arc<SandboxState>) -> rquickjs::Result<()> {
    let umi = Object::new(ctx.clone())?;
    umi.set("version", PLUGIN_API_VERSION)?;
    umi.set("__resolvers", Array::new(ctx.clone())?)?;
    umi.set("__handlers", Object::new(ctx.clone())?)?;
    umi.set("__exports", Object::new(ctx.clone())?)?;

    // umi.log(msg)
    let s = Arc::clone(state);
    umi.set(
        "log",
        Function::new(ctx.clone(), move |value: Value<'js>| {
            push_log(&s, value_to_text(&value));
        })?,
    )?;

    // umi.resolve(url | {url}) → 宿主给出的 URL 结构化分析
    let s = Arc::clone(state);
    umi.set(
        "resolve",
        Function::new(
            ctx.clone(),
            move |ctx: JsCtx<'js>, input: Value<'js>| -> rquickjs::Result<Value<'js>> {
                let url = coerce_url_arg(&input);
                let info = analyze_url(&url);
                match ctx.json_parse(serde_json::to_string(&info).unwrap_or_else(|_| "{}".into())) {
                    Ok(v) => Ok(v),
                    Err(e) => {
                        s.error(format!("umi.resolve 失败: {e}"));
                        Ok(Value::new_undefined(ctx.clone()))
                    }
                }
            },
        )?,
    )?;

    // umi.registerResolver(fn)
    umi.set(
        "registerResolver",
        Function::new(
            ctx.clone(),
            move |ctx: JsCtx<'js>, f: Function<'js>| -> rquickjs::Result<()> {
                let umi: Object = ctx.globals().get("umi")?;
                let list: Array = umi.get("__resolvers")?;
                list.set(list.len(), f)?;
                Ok(())
            },
        )?,
    )?;

    // umi.on(event, cb)
    umi.set(
        "on",
        Function::new(
            ctx.clone(),
            move |ctx: JsCtx<'js>, event: String, cb: Function<'js>| -> rquickjs::Result<()> {
                let umi: Object = ctx.globals().get("umi")?;
                let handlers: Object = umi.get("__handlers")?;
                let slot: Value = handlers.get(event.as_str())?;
                let list = match slot.into_array() {
                    Some(a) => a,
                    None => {
                        let a = Array::new(ctx.clone())?;
                        handlers.set(event.as_str(), a.clone())?;
                        a
                    }
                };
                list.set(list.len(), cb)?;
                Ok(())
            },
        )?,
    )?;

    // umi.retry(taskId[, reason])
    let s = Arc::clone(state);
    umi.set(
        "retry",
        Function::new(
            ctx.clone(),
            move |task: Value<'js>, reason: Opt<Value<'js>>| {
                let task_id = value_to_text(&task);
                if task_id.is_empty() || task_id == "null" || task_id == "undefined" {
                    s.error("umi.retry 缺少 taskId，已忽略");
                    return;
                }
                let plugin = s.plugin_id.clone();
                let reason_text = reason.0.as_ref().map(|v| match value_to_text(v) {
                    t if t == "null" || t == "undefined" => String::new(),
                    t => t,
                });
                if let Ok(mut retries) = s.retries.lock() {
                    if retries.len() >= RETRY_LIMIT {
                        return;
                    }
                    let attempt = retries
                        .iter()
                        .filter(|r| r.get("task_id").and_then(|v| v.as_str()) == Some(task_id.as_str()))
                        .count()
                        + 1;
                    retries.push(json!({
                        "plugin": plugin,
                        "task_id": task_id,
                        "attempt": attempt,
                        "reason": reason_text,
                    }));
                }
            },
        )?,
    )?;

    // umi.export(name, value) —— 导出 selfTest 等钩子
    umi.set(
        "export",
        Function::new(
            ctx.clone(),
            move |ctx: JsCtx<'js>, name: String, value: Value<'js>| -> rquickjs::Result<()> {
                let umi: Object = ctx.globals().get("umi")?;
                let exports: Object = umi.get("__exports")?;
                exports.set(&name, value)?;
                Ok(())
            },
        )?,
    )?;

    // umi.storage.get/set —— 由宿主落盘，插件自己碰不到文件系统
    let storage = Object::new(ctx.clone())?;
    let s = Arc::clone(state);
    storage.set(
        "get",
        Function::new(
            ctx.clone(),
            move |ctx: JsCtx<'js>, key: String| -> rquickjs::Result<Value<'js>> {
                let hit = s.storage.lock().ok().and_then(|m| m.get(&key).cloned());
                match hit {
                    Some(v) => ctx
                        .json_parse(serde_json::to_string(&v).unwrap_or_else(|_| "null".into())),
                    None => Ok(Value::new_null(ctx.clone())),
                }
            },
        )?,
    )?;
    let s = Arc::clone(state);
    storage.set(
        "set",
        Function::new(
            ctx.clone(),
            move |key: String, value: Value<'js>| {
                let json = js_to_json(&value);
                if let Ok(mut map) = s.storage.lock() {
                    map.insert(key, json);
                    s.dirty.store(true, Ordering::Relaxed);
                }
            },
        )?,
    )?;
    let s = Arc::clone(state);
    storage.set(
        "remove",
        Function::new(ctx.clone(), move |key: String| {
            if let Ok(mut map) = s.storage.lock() {
                map.remove(&key);
                s.dirty.store(true, Ordering::Relaxed);
            }
        })?,
    )?;
    let s = Arc::clone(state);
    storage.set(
        "keys",
        Function::new(ctx.clone(), move |ctx: JsCtx<'js>| -> rquickjs::Result<Array<'js>> {
            let keys: Vec<String> = s.storage.lock().map(|m| m.keys().cloned().collect()).unwrap_or_default();
            let out = Array::new(ctx.clone())?;
            for (i, k) in keys.iter().enumerate() {
                out.set(i, k.as_str())?;
            }
            Ok(out)
        })?,
    )?;
    umi.set("storage", storage)?;

    ctx.globals().set("umi", umi)?;
    Ok(())
}

/* ==================== URL 分析（umi.resolve 的宿主实现） ==================== */

fn kind_of(ext: &str) -> &'static str {
    if STREAM_EXTS.contains(&ext) {
        "stream"
    } else if VIDEO_EXTS.contains(&ext) {
        "video"
    } else if AUDIO_EXTS.contains(&ext) {
        "audio"
    } else if IMAGE_EXTS.contains(&ext) {
        "image"
    } else if ARCHIVE_EXTS.contains(&ext) {
        "archive"
    } else if DOC_EXTS.contains(&ext) {
        "document"
    } else if EXE_EXTS.contains(&ext) {
        "executable"
    } else if SUB_EXTS.contains(&ext) {
        "subtitle"
    } else if TORRENT_EXTS.contains(&ext) {
        "torrent"
    } else {
        "unknown"
    }
}

/// 判断 URL 能否直接交给下载引擎（供 `umi.resolve` 与解析器嗅探使用）
pub fn analyze_url(raw: &str) -> Json {
    let url = raw.trim();
    if url.is_empty() {
        return json!({ "ok": false, "url": raw, "direct": false, "kind": "unknown", "confidence": 0.0,
                       "reason": "空 URL" });
    }
    // 非 http(s)：磁力 / ed2k / ftp 等本身即「直链」
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => (String::new(), ""),
    };
    if scheme.is_empty() {
        let head = url.split_once(':').map(|(s, _)| s.to_ascii_lowercase()).unwrap_or_default();
        if matches!(head.as_str(), "magnet" | "ed2k" | "thunder" | "ftp" | "ftps") {
            return json!({
                "ok": true, "url": url, "scheme": head, "host": "", "path": "", "filename": "", "ext": "",
                "kind": "p2p", "direct": true, "confidence": 0.95,
                "reason": format!("{head} 链接可直接交给下载引擎"),
            });
        }
        return json!({ "ok": false, "url": url, "direct": false, "kind": "unknown", "confidence": 0.0,
                       "reason": "不是有效的绝对 URL" });
    }
    if !matches!(scheme.as_str(), "http" | "https" | "ftp" | "ftps") {
        return json!({ "ok": false, "url": url, "scheme": scheme, "direct": false, "kind": "unknown",
                       "confidence": 0.0, "reason": "不支持的协议" });
    }

    let (authority, tail) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let host = authority.rsplit('@').next().unwrap_or(authority).to_ascii_lowercase();
    let tail = tail.split('#').next().unwrap_or(tail);
    let (path, query) = match tail.split_once('?') {
        Some((p, q)) => (p, q),
        None => (tail, ""),
    };
    let filename = path.rsplit('/').next().unwrap_or("").to_string();
    let mut ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    let params: Vec<(String, String)> = query
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (k.to_ascii_lowercase(), v.to_string()),
            None => (pair.to_ascii_lowercase(), String::new()),
        })
        .collect();
    // 路径没有扩展名时，从查询参数里找 filename=xxx.mp4
    if ext.is_empty() {
        for (k, v) in &params {
            if DOWNLOAD_QUERY_KEYS.contains(&k.as_str()) {
                if let Some((_, e)) = v.rsplit_once('.') {
                    ext = e.to_ascii_lowercase();
                    break;
                }
            }
        }
    }
    let kind = kind_of(&ext);
    let download_param = params
        .iter()
        .any(|(k, v)| DOWNLOAD_QUERY_KEYS.contains(&k.as_str()) && v != "0" && v != "false");
    let page_like = path.ends_with('/')
        || PAGE_EXTS.contains(&ext.as_str())
        || PAGE_MARKERS.iter().any(|m| path.contains(m))
        || (ext.is_empty() && !download_param && path != "/");

    let (direct, confidence, reason) = if !ext.is_empty() && kind != "unknown" {
        let c = if download_param { 0.95 } else if kind == "stream" { 0.7 } else { 0.85 };
        (true, c, format!("路径扩展名 .{ext} 属于可直下资源（{kind}）"))
    } else if download_param {
        (true, 0.6, "查询参数表明这是下载型链接".to_string())
    } else if page_like {
        (false, 0.2, "网页 / 接口地址，需要站点解析器进一步解析".to_string())
    } else {
        (false, 0.3, "无直链特征（无可识别扩展名）".to_string())
    };

    json!({
        "ok": true,
        "url": url,
        "scheme": scheme,
        "host": host,
        "path": path,
        "filename": filename,
        "ext": ext,
        "kind": kind,
        "direct": direct,
        "confidence": confidence,
        "reason": reason,
    })
}

/* ==================== JS ↔ JSON ==================== */

fn js_to_json(value: &Value<'_>) -> Json {
    let mut budget = 4096usize;
    js_to_json_inner(value, 0, &mut budget)
}

fn js_to_json_inner(value: &Value<'_>, depth: usize, budget: &mut usize) -> Json {
    if *budget == 0 || depth > 8 {
        return Json::Null;
    }
    *budget -= 1;
    match value.type_of() {
        Type::Uninitialized | Type::Undefined | Type::Null => Json::Null,
        Type::Bool => Json::Bool(value.as_bool().unwrap_or(false)),
        Type::Int => json!(value.as_int().unwrap_or(0)),
        Type::Float => json!(value.as_float().unwrap_or(0.0)),
        Type::String => value
            .as_string()
            .and_then(|s| s.to_string().ok())
            .map(Json::String)
            .unwrap_or(Json::Null),
        Type::BigInt => value
            .as_big_int()
            .and_then(|b| b.clone().to_i64().ok())
            .map(|i| json!(i))
            .unwrap_or(Json::Null),
        Type::Array => {
            let Some(arr) = value.as_array() else { return Json::Null };
            let mut out = Vec::with_capacity(arr.len().min(*budget));
            for item in arr.iter::<Value>() {
                match item {
                    Ok(v) => out.push(js_to_json_inner(&v, depth + 1, budget)),
                    Err(_) => out.push(Json::Null),
                }
            }
            Json::Array(out)
        }
        Type::Object | Type::Function | Type::Constructor | Type::Promise | Type::Exception | Type::Proxy => {
            let Some(obj) = value.as_object() else { return Json::Null };
            let mut map = serde_json::Map::new();
            for entry in obj.props::<String, Value>() {
                if *budget == 0 {
                    break;
                }
                match entry {
                    Ok((k, v)) => {
                        map.insert(k, js_to_json_inner(&v, depth + 1, budget));
                    }
                    Err(_) => continue,
                }
            }
            Json::Object(map)
        }
        _ => Json::Null,
    }
}

/* ==================== 通用：跑一个插件的脚本 ==================== */

fn load_storage(ctx: &Ctx, id: &str) -> HashMap<String, Json> {
    let path = plugin_storage_file(ctx, id);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return HashMap::new(),
    };
    serde_json::from_str::<Json>(&text)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .map(|o| o.into_iter().collect())
        .unwrap_or_default()
}

fn save_storage(ctx: &Ctx, id: &str, storage: &HashMap<String, Json>) -> Result<()> {
    let json = Json::Object(storage.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
    write_atomic(&plugin_storage_file(ctx, id), serde_json::to_string_pretty(&json)?.as_bytes())
}

/// 建沙箱 → 载入插件脚本 → 执行闭包 → 收尾（落盘 storage）
///
/// 返回 `(闭包结果, 运行产物)`：即使钩子抛错，日志 / 重试 / 错误也照样回传。
fn with_plugin<R>(
    ctx: &Ctx,
    id: &str,
    f: impl FnOnce(&Sandbox) -> Result<R>,
) -> Result<(Result<R>, SandboxRun)> {
    validate_id(id)?;
    let path = plugin_script(ctx, id);
    let source = std::fs::read_to_string(&path)
        .map_err(|e| anyhow!("读取插件脚本 {} 失败: {e}", path.display()))?;
    let storage = load_storage(ctx, id);
    let had_storage = !storage.is_empty();
    let sandbox = Sandbox::new(id, storage.clone())?;
    let out = match sandbox.eval(&source, "plugin.js") {
        Ok(_) => f(&sandbox),
        Err(e) => Err(e),
    };
    let run = sandbox.run_state();
    if had_storage || sandbox.state.dirty.load(Ordering::Relaxed) {
        save_storage(ctx, id, &sandbox.state.snapshot_storage())?;
    }
    Ok((out, run))
}

/* ==================== 对外 API ==================== */

/// 沙箱自检：真的建 Runtime、真的跑一段 JS
pub fn sandbox_selftest() -> Json {
    let version = engine_version();
    let mut available = false;
    let mut probe_note = String::new();
    match Runtime::new() {
        Ok(rt) => {
            rt.set_memory_limit(MEMORY_LIMIT_BYTES);
            rt.set_max_stack_size(STACK_LIMIT_KB * 1024);
            let deadline = Arc::new(AtomicU64::new(u64::MAX));
            let watch = Arc::clone(&deadline);
            rt.set_interrupt_handler(Some(Box::new(move || now_ms() >= watch.load(Ordering::Relaxed))));
            match JsContext::full(&rt) {
                Ok(c) => {
                    deadline.store(now_ms().saturating_add(SCRIPT_TIMEOUT_MS), Ordering::Relaxed);
                    let ok = c.with(|ctx| -> bool {
                        let probe: Result<Value, _> = ctx.eval(
                            r#"({
                                sum: 1 + 1,
                                isolated: (typeof require === "undefined" && typeof process === "undefined"
                                           && typeof fetch === "undefined" && typeof XMLHttpRequest === "undefined"),
                                engine: (typeof globalThis === "object")
                            })"#,
                        );
                        match probe {
                            Ok(v) => {
                                let j = js_to_json(&v);
                                j.get("sum").and_then(|s| s.as_i64()) == Some(2)
                                    && j.get("isolated").and_then(|s| s.as_bool()) == Some(true)
                                    && j.get("engine").and_then(|s| s.as_bool()) == Some(true)
                            }
                            Err(e) => {
                                probe_note = e.to_string();
                                false
                            }
                        }
                    });
                    available = ok;
                }
                Err(e) => probe_note = e.to_string(),
            }
        }
        Err(e) => probe_note = e.to_string(),
    }
    let mut out = json!({
        "available": available,
        "engine": ENGINE,
        "version": version,
        "memory_limit_mb": MEMORY_LIMIT_MB as i64,
        "script_timeout_ms": SCRIPT_TIMEOUT_MS as i64,
        "stack_limit_kb": STACK_LIMIT_KB as i64,
    });
    if !probe_note.is_empty() {
        if let Some(obj) = out.as_object_mut() {
            obj.insert("error".to_string(), json!(probe_note));
        }
    }
    out
}

/// 引擎版本（来自 QuickJS 本身）
fn engine_version() -> String {
    unsafe {
        let ptr = rquickjs::qjs::JS_GetVersion();
        if ptr.is_null() {
            return "unknown".to_string();
        }
        std::ffi::CStr::from_ptr(ptr).to_string_lossy().to_string()
    }
}

/// 在沙箱里跑插件的 `selfTest()`（如有）——不崩不挂
pub fn test_plugin(ctx: &Ctx, id: &str) -> Json {
    let out = match test_plugin_inner(ctx, id) {
        Ok(v) => v,
        Err(e) => json!({ "ok": false, "logs": [], "result": Json::Null, "error": e.to_string() }),
    };
    let ok = out.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    log_event(
        ctx,
        if ok { "info" } else { "error" },
        id,
        "test",
        &if ok {
            "沙箱自检通过".to_string()
        } else {
            format!("沙箱自检失败：{}", out.get("error").and_then(|v| v.as_str()).unwrap_or("未知原因"))
        },
    );
    out
}

fn test_plugin_inner(ctx: &Ctx, id: &str) -> Result<Json> {
    let reg = read_registry(ctx);
    if find_entry(&reg, id).is_none() {
        bail!("插件未安装：{id}");
    }
    if !plugin_script(ctx, id).exists() {
        bail!("插件脚本缺失：{}", plugin_script(ctx, id).display());
    }
    let (result, run) = with_plugin(ctx, id, |sandbox| sandbox.call_selftest())?;
    let error_text = if run.errors.is_empty() {
        None
    } else {
        Some(run.errors.join("; "))
    };
    let mut hook_present = false;
    let (ok, result, error) = match result {
        // 载入或 selfTest 抛错：照样回传日志，明确失败
        Err(e) => (false, Json::Null, Some(e.to_string())),
        Ok(None) => (true, Json::Null, None),
        Ok(Some(value)) => {
            hook_present = true;
            let ok = error_text.is_none() && value != json!(false);
            (ok, value, error_text.clone())
        }
    };
    let mut out = json!({
        "ok": ok,
        "logs": run.logs,
        "result": result,
        "error": error,
    });
    if !hook_present && ok {
        if let Some(obj) = out.as_object_mut() {
            obj.insert("note".to_string(), json!("插件未导出 selfTest() 钩子"));
        }
    }
    Ok(out)
}

/// 把所有已启用插件的解析器跑一遍，收集意见（自动嗅探直链）
pub fn run_resolvers(ctx: &Ctx, url: &str) -> Json {
    let mut opinions: Vec<Json> = Vec::new();
    let mut errors: Vec<Json> = Vec::new();
    let mut queried = 0i64;
    for id in enabled_ids(ctx) {
        queried += 1;
        match with_plugin(ctx, &id, |sandbox| sandbox.call_resolvers(url)) {
            Ok((inner, run)) => {
                for e in run.errors {
                    errors.push(json!({ "plugin": id, "error": e }));
                }
                match inner {
                    Ok(list) => {
                        for item in list {
                            let err = item.get("error").cloned().filter(|v| !v.is_null());
                            if let Some(e) = err.clone() {
                                errors.push(json!({ "plugin": id, "error": e }));
                            }
                            opinions.push(json!({
                                "plugin": id,
                                "ok": err.is_none(),
                                "matched": item.get("matched").and_then(|v| v.as_bool()).unwrap_or(false),
                                "direct": item.get("direct").and_then(|v| v.as_bool()).unwrap_or(false),
                                "result": item,
                                "error": err,
                            }));
                        }
                    }
                    Err(e) => errors.push(json!({ "plugin": id, "error": e.to_string() })),
                }
            }
            Err(e) => errors.push(json!({ "plugin": id, "error": e.to_string() })),
        }
    }
    // 事件日志：谁命中、谁报错、有没有插件可用
    for o in &opinions {
        let pid = o.get("plugin").and_then(|v| v.as_str()).unwrap_or("");
        let matched = o.get("matched").and_then(|v| v.as_bool()).unwrap_or(false);
        match o.get("error").and_then(|v| v.as_str()) {
            Some(err) => log_event(ctx, "error", pid, "resolver", &format!("解析 {url} 出错：{err}")),
            None => log_event(
                ctx,
                "info",
                pid,
                "resolver",
                &format!("解析 {url}：{}", if matched { "命中" } else { "未命中" }),
            ),
        }
    }
    if queried == 0 {
        log_event(ctx, "warn", "", "resolver", &format!("解析 {url}：没有已启用的插件"));
    }
    let best = opinions
        .iter()
        .filter(|o| o.get("direct").and_then(|v| v.as_bool()).unwrap_or(false))
        .max_by(|a, b| {
            let ca = a.get("result").and_then(|r| r.get("confidence")).and_then(|c| c.as_f64()).unwrap_or(0.0);
            let cb = b.get("result").and_then(|r| r.get("confidence")).and_then(|c| c.as_f64()).unwrap_or(0.0);
            ca.partial_cmp(&cb).unwrap_or(std::cmp::Ordering::Equal)
        })
        .and_then(|o| o.get("result").and_then(|r| r.get("url")).and_then(|u| u.as_str()).map(|s| s.to_string()))
        .or_else(|| {
            opinions
                .iter()
                .find(|o| o.get("direct").and_then(|v| v.as_bool()).unwrap_or(false))
                .map(|_| url.to_string())
        });
    json!({
        "url": url,
        "queried": queried,
        "direct": best.is_some(),
        "best_url": best,
        "opinions": opinions,
        "errors": errors,
    })
}

/// 派发事件，收集 `umi.retry(taskId)` 请求
pub fn dispatch_event(ctx: &Ctx, event: &str, payload: Json) -> Json {
    let mut notified = 0i64;
    let mut retries: Vec<Json> = Vec::new();
    let mut errors: Vec<Json> = Vec::new();
    for id in enabled_ids(ctx) {
        match with_plugin(ctx, &id, |sandbox| sandbox.call_event(event, &payload)) {
            Ok((inner, run)) => {
                retries.extend(run.retries);
                for e in run.errors {
                    errors.push(json!({ "plugin": id, "error": e }));
                }
                match inner {
                    Ok(count) => {
                        if count > 0 {
                            notified += 1;
                        }
                    }
                    Err(e) => errors.push(json!({ "plugin": id, "error": e.to_string() })),
                }
            }
            Err(e) => errors.push(json!({ "plugin": id, "error": e.to_string() })),
        }
    }
    // 事件日志：派发结果（谁被通知、谁报错、谁请求重试）
    for r in &retries {
        let pid = r.get("plugin").and_then(|v| v.as_str()).unwrap_or("");
        let tid = r.get("task_id").and_then(|v| v.as_str()).unwrap_or("");
        let reason = r.get("reason").and_then(|v| v.as_str()).unwrap_or("");
        log_event(
            ctx,
            "warn",
            pid,
            event,
            &if reason.is_empty() {
                format!("请求重试任务 {tid}")
            } else {
                format!("请求重试任务 {tid}（{reason}）")
            },
        );
    }
    for e in &errors {
        let pid = e.get("plugin").and_then(|v| v.as_str()).unwrap_or("");
        let err = e.get("error").and_then(|v| v.as_str()).unwrap_or("");
        log_event(ctx, "error", pid, event, &format!("派发失败：{err}"));
    }
    if notified > 0 || !retries.is_empty() || !errors.is_empty() {
        log_event(
            ctx,
            "info",
            "",
            event,
            &format!(
                "事件 {event}：通知 {notified} 个插件，重试请求 {}，错误 {}",
                retries.len(),
                errors.len()
            ),
        );
    }
    json!({
        "event": event,
        "notified": notified,
        "retries": retries,
        "errors": errors,
    })
}

/* ==================== 测试 ==================== */

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::{AppDirs, ToolPaths};
    use crate::models::AppSettings;
    use std::sync::atomic::AtomicU64 as TestCounter;
    use std::time::Instant;

    static COUNTER: TestCounter = TestCounter::new(0);

    /// 每个测试一个独立的临时「应用数据目录」
    struct TempEnv {
        root: PathBuf,
    }

    impl TempEnv {
        fn new(tag: &str) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let root = std::env::temp_dir().join(format!(
                "umi-plugins-test-{}-{}-{}-{}",
                tag,
                std::process::id(),
                n,
                now_ms()
            ));
            std::fs::create_dir_all(&root).expect("创建临时目录");
            Self { root }
        }

        fn ctx(&self) -> Ctx {
            let data = self.root.join("data");
            let dirs = AppDirs {
                data: data.clone(),
                bin: data.join("bin"),
                models: data.join("models"),
                cache: data.join("cache"),
                downloads: self.root.join("downloads"),
                db_file: data.join("umi.db"),
                settings_file: data.join("settings.json"),
            };
            std::fs::create_dir_all(&dirs.data).expect("创建 data 目录");
            Ctx::new(dirs, ToolPaths::default(), AppSettings::default())
        }

        fn install_builtins(&self, ctx: &Ctx) {
            install_from_market(ctx, "direct-link-sniffer").expect("安装直链嗅探器");
            install_from_market(ctx, "retry-hook").expect("安装重试钩子");
        }
    }

    impl Drop for TempEnv {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /* ---------- 沙箱隔离 / 三重限制 ---------- */

    #[test]
    fn selftest_reports_quickjs_engine() {
        let v = sandbox_selftest();
        assert_eq!(v.get("available").and_then(|b| b.as_bool()), Some(true), "沙箱应可用: {v}");
        assert_eq!(v.get("engine").and_then(|e| e.as_str()), Some("quickjs"));
        assert_eq!(v.get("memory_limit_mb").and_then(|m| m.as_i64()), Some(MEMORY_LIMIT_MB as i64));
        assert_eq!(v.get("script_timeout_ms").and_then(|m| m.as_i64()), Some(SCRIPT_TIMEOUT_MS as i64));
        let version = v.get("version").and_then(|s| s.as_str()).unwrap_or("");
        assert!(!version.is_empty() && version != "unknown", "应拿到真实 QuickJS 版本: {v}");
    }

    #[test]
    fn sandbox_isolates_host_objects() {
        let sb = Sandbox::new("isolation", HashMap::new()).expect("建沙箱");
        let probe = sb
            .eval_text(
                r#"[
                    typeof require, typeof process, typeof fetch, typeof XMLHttpRequest,
                    typeof globalThis.require, typeof globalThis.process, typeof module,
                    typeof globalThis.print === "function" ? "print" : "no-print",
                    typeof umi, typeof umi.log, typeof umi.storage.get
                ].join(",")"#,
            )
            .expect("探测脚本应能执行");
        assert_eq!(
            probe,
            "undefined,undefined,undefined,undefined,undefined,undefined,undefined,no-print,object,function,function",
            "沙箱不应注入任何宿主对象，只注入 umi"
        );
        println!("[sandbox-isolation] typeof probe = {probe}");

        let err = sb
            .eval_text("require('fs').readFileSync('C:/Windows/win.ini')")
            .expect_err("require 必须不存在");
        assert!(
            err.to_string().contains("require is not defined"),
            "调用 require 应报 ReferenceError，实际: {err}"
        );
        let err2 = sb.eval_text("process.exit(0)").expect_err("process 必须不存在");
        assert!(err2.to_string().contains("process is not defined"), "实际: {err2}");
        let err3 = sb.eval_text("fetch('http://127.0.0.1/')").expect_err("fetch 必须不存在");
        assert!(err3.to_string().contains("fetch is not defined"), "实际: {err3}");
        println!(
            "[sandbox-isolation] require('fs') → {err} | process.exit → {err2} | fetch → {err3}"
        );
        // 沙箱还能继续用（异常被消费掉，不污染后续调用）
        assert_eq!(sb.eval_text("1 + 1").unwrap(), "2");
    }

    #[test]
    fn sandbox_interrupts_infinite_loop_within_budget() {
        let sb = Sandbox::new("timeout", HashMap::new()).expect("建沙箱");
        let started = Instant::now();
        let err = sb.eval_text("while (true) {}").expect_err("死循环必须被中断");
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(SCRIPT_TIMEOUT_MS * 10),
            "沙箱没有在时间预算内中断，耗时 {elapsed:?}"
        );
        let msg = err.to_string();
        assert!(msg.contains("interrupted"), "错误应说明被中断: {msg}");
        assert!(msg.contains("超时"), "错误应是超时语义: {msg}");
        println!("[sandbox-timeout] while(true){{}} 在 {elapsed:?} 内被中断（预算 {SCRIPT_TIMEOUT_MS} ms）→ {msg}");
        // 中断后沙箱仍可用
        assert_eq!(sb.eval_text("'alive'").unwrap(), "alive");
    }

    /// 内存上限：跑飞的大数组分配必须在 16 MB 上限处被拦下，且拦下它的原因必须是「内存」。
    ///
    /// 断言**不能只看错误文案**（旧写法就是这么挂在 Linux CI 上的）：
    /// quickjs 的 `JS_ThrowOutOfMemory` → `JS_ThrowInternalError(ctx, "out of memory")`
    /// → `JS_ThrowError2`，而后者在「连错误对象本身都分配不出来」时会退化成**抛 `JS_NULL`**
    /// （源码注释：`/* out of memory: throw JS_NULL to avoid recursing */`）。
    /// 这时 JS 侧 `catch (e)` 拿到的 `e` 就是 null，`String(e)` 只剩 "null" —— 上限明明生效了，
    /// 却因为文案里没有 "memory" 被判失败。错误对象能不能分配出来取决于 C 分配器的块记账
    /// （`js_arena_usable_size + MALLOC_OVERHEAD`）与失败分配的大小，所以这条路径**是机器相关的**。
    ///
    /// 现在断言可观测的事实（两种降级都接受，但不许「超时」冒充内存）：
    /// 1. 循环不可能跑完 —— 跑完就说明上限没生效；
    /// 2. 确实是「分配了一部分之后才被拦下」；
    /// 3. 拦下它的是**可捕获**异常 —— quickjs 的中断异常是不可捕获的
    ///    （`JS_ThrowInterrupted` → `JS_SetUncatchableError`），所以能 catch 到就不是超时；
    /// 4. 直接量运行时的 `malloc_size` / `malloc_limit`：占用顶到 16 MB 上限，且从未越界。
    #[test]
    fn sandbox_enforces_memory_limit() {
        let sb = Sandbox::new("memory", HashMap::new()).expect("建沙箱");

        // ① JS 侧 catch 住 OOM：回报「分配了多少才被拦下」与原因文本
        let started = Instant::now();
        let report = sb
            .eval_text(
                r#"(function () {
                     var sink = [];
                     var completed = false;
                     var allocated = -1;
                     var reason = "";
                     try {
                       for (var i = 0; i < 200000; i++) { sink.push(new Array(1024).fill(i)); }
                       completed = true;
                     } catch (e) {
                       allocated = sink.length;
                       sink = null;                     // 先放掉引用，避免「回报」这一步二次 OOM
                       if (e === null || e === undefined) { reason = ""; }
                       else if (typeof e === "string") { reason = e; }
                       else if (e && typeof e.message === "string") { reason = e.message; }
                       else { reason = String(e); }
                     }
                     return JSON.stringify({ completed: completed, allocated: allocated, reason: reason });
                   })()"#,
            )
            .expect("内存超限应作为可捕获的 JS 异常返回（quickjs 的中断异常不可捕获）");
        let elapsed = started.elapsed();
        let v: Json = serde_json::from_str(&report).expect("探针应返回 JSON");
        assert_eq!(
            v["completed"], false,
            "内存上限没生效：200000 个 1024 元数组全部分配成功 → {v}"
        );
        let allocated = v["allocated"].as_i64().unwrap_or(-1);
        assert!(
            allocated > 0 && allocated < 200_000,
            "应当是「分配了一部分之后被拦下」，实际已分配 {allocated} 个数组 → {v}"
        );
        let reason = v["reason"].as_str().unwrap_or("");
        assert!(
            !reason.to_ascii_lowercase().contains("interrupt"),
            "拦下它的是超时而不是内存上限：{reason}"
        );
        assert!(
            elapsed < Duration::from_millis(SCRIPT_TIMEOUT_MS * 10),
            "内存限制没生效（耗时 {elapsed:?}）"
        );

        // ② 不放 catch：OOM 必须被宿主看到；大块内存仍挂在 global 上，于是可以直接量沙箱占用
        let escaped = sb.eval_text(
            r#"(function () {
                 globalThis.__umi_mem_probe = [];
                 for (var i = 0; i < 200000; i++) {
                   globalThis.__umi_mem_probe.push(new Array(1024).fill(i));
                 }
                 return "unexpected: no oom";
               })()"#,
        );
        assert!(
            escaped.is_err(),
            "不 catch 的 OOM 必须作为错误回到宿主：{escaped:?}"
        );
        let usage = sb.runtime.memory_usage();
        assert_eq!(
            usage.malloc_limit, MEMORY_LIMIT_BYTES as i64,
            "沙箱内存上限必须正好是 {MEMORY_LIMIT_MB} MB"
        );
        assert!(
            usage.malloc_size <= MEMORY_LIMIT_BYTES as i64,
            "沙箱占用越过了上限：{} > {MEMORY_LIMIT_BYTES}",
            usage.malloc_size
        );
        assert!(
            usage.malloc_size >= (MEMORY_LIMIT_BYTES as i64) * 3 / 4,
            "沙箱占用没有顶到上限就停了（只用了 {} 字节）",
            usage.malloc_size
        );
        println!(
            "[sandbox-memory] {MEMORY_LIMIT_MB} MB 上限：{} 个数组（{} ms）后被拦下，原因 {:?}；runtime malloc_size={} / malloc_limit={}；宿主看到：{}",
            allocated,
            elapsed.as_millis(),
            reason,
            usage.malloc_size,
            usage.malloc_limit,
            escaped.unwrap_err()
        );
    }

    /* ---------- 内容寻址 ---------- */

    #[test]
    fn sha256_file_matches_known_digest_and_detects_tamper() {
        let env = TempEnv::new("sha");
        let file = env.root.join("hello.txt");
        std::fs::write(&file, b"hello").unwrap();
        assert_eq!(
            sha256_file(&file).unwrap(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        std::fs::write(&file, b"hellp").unwrap(); // 改一个字节
        assert_ne!(
            sha256_file(&file).unwrap(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn market_index_has_two_real_hashes() {
        let env = TempEnv::new("market");
        let ctx = env.ctx();
        let index = market_list(&ctx);
        let plugins = index.get("plugins").and_then(|p| p.as_array()).cloned().unwrap_or_default();
        assert!(plugins.len() >= 2, "内置市场应至少有两个示例插件: {index}");
        for p in &plugins {
            let id = p.get("id").and_then(|v| v.as_str()).unwrap();
            let file = p.get("file").and_then(|v| v.as_str()).unwrap();
            let sha = p.get("sha256").and_then(|v| v.as_str()).unwrap();
            assert_eq!(sha.len(), 64, "{id} 的 sha256 应是 64 位十六进制");
            let actual = sha256_file(&market_files_dir(&ctx).join(file)).unwrap();
            assert_eq!(actual, sha, "{id} 索引里的 sha256 必须是文件的真实摘要");
        }
    }

    #[test]
    fn tampered_market_file_is_rejected() {
        let env = TempEnv::new("tamper");
        let ctx = env.ctx();
        // 正向对照：未篡改时可以正常安装
        install_from_market(&ctx, "direct-link-sniffer").expect("原始文件应能安装");

        // 篡改市场里的另一个文件（只改一个字节）
        let victim = market_files_dir(&ctx).join("retry-hook-1.0.0.js");
        let mut bytes = std::fs::read(&victim).unwrap();
        let idx = bytes.iter().position(|b| *b == b'(').unwrap_or(0);
        bytes[idx] = b'[';
        std::fs::write(&victim, &bytes).unwrap();

        let err = install_from_market(&ctx, "retry-hook").expect_err("篡改后必须拒绝安装");
        let msg = err.to_string();
        let expected = market_list(&ctx)
            .get("plugins")
            .and_then(|p| p.as_array())
            .unwrap()
            .iter()
            .find(|p| p.get("id").and_then(|v| v.as_str()) == Some("retry-hook"))
            .and_then(|p| p.get("sha256"))
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let actual = sha256_file(&victim).unwrap();
        assert_ne!(expected, actual, "篡改后摘要必须不同");
        println!("[tamper] 市场文件改一个字节 → 索引期望 {expected} / 文件实际 {actual} → 安装被拒绝: {msg}");
        assert!(msg.contains("sha256"), "拒绝原因应是内容寻址校验: {msg}");
        assert!(msg.contains("拒绝安装"), "拒绝原因应是内容寻址校验: {msg}");
        assert!(
            !plugin_script(&ctx, "retry-hook").exists(),
            "被拒绝的插件不应落盘"
        );
        assert!(
            find_entry(&read_registry(&ctx), "retry-hook").is_none(),
            "被拒绝的插件不应登记"
        );
        // 恢复原文件后可以安装
        let original = builtin_plugins()
            .into_iter()
            .find(|p| p.id == "retry-hook")
            .map(|p| p.source.as_bytes().to_vec())
            .unwrap();
        std::fs::write(&victim, &original).unwrap();
        install_from_market(&ctx, "retry-hook").expect("恢复原文件后应能安装");
    }

    /* ---------- 端到端 ---------- */

    #[test]
    fn end_to_end_install_list_test_resolve_dispatch() {
        let env = TempEnv::new("e2e");
        let ctx = env.ctx();
        env.install_builtins(&ctx);

        // 1) list_plugins
        let list = list_plugins(&ctx);
        let ids: Vec<String> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.get("id").and_then(|v| v.as_str()).unwrap().to_string())
            .collect();
        assert_eq!(ids, vec!["direct-link-sniffer".to_string(), "retry-hook".to_string()]);
        for p in list.as_array().unwrap() {
            for key in ["id", "name", "version", "description", "enabled", "sha256", "source", "installed_at"] {
                assert!(p.get(key).is_some(), "list_plugins 缺少字段 {key}: {p}");
            }
            assert_eq!(p.get("enabled").and_then(|v| v.as_bool()), Some(true));
            assert_eq!(p.get("source").and_then(|v| v.as_str()), Some("market"));
            // 列表里的 sha256 必须等于落盘文件的真实摘要
            let id = p.get("id").and_then(|v| v.as_str()).unwrap();
            assert_eq!(
                p.get("sha256").and_then(|v| v.as_str()).unwrap(),
                sha256_file(&plugin_script(&ctx, id)).unwrap()
            );
        }

        // 2) test_plugin（跑插件导出的 selfTest）
        for id in ["direct-link-sniffer", "retry-hook"] {
            let r = test_plugin(&ctx, id);
            assert_eq!(r.get("ok").and_then(|v| v.as_bool()), Some(true), "{id} selfTest 应通过: {r}");
            assert_eq!(r.get("result").and_then(|v| v.as_bool()), Some(true), "{id} 应返回 true: {r}");
            assert!(r.get("error").map(|e| e.is_null()).unwrap_or(false), "{id} 不应有错误: {r}");
            let logs = r.get("logs").and_then(|l| l.as_array()).unwrap();
            assert!(!logs.is_empty(), "{id} 应产生日志: {r}");
        }

        // 3) run_resolvers —— 自动嗅探直链
        let direct = run_resolvers(&ctx, "https://cdn.example.com/media/movie-1080p.mp4");
        assert_eq!(direct.get("queried").and_then(|v| v.as_i64()), Some(2), "{direct}");
        assert_eq!(direct.get("direct").and_then(|v| v.as_bool()), Some(true), "{direct}");
        assert_eq!(
            direct.get("best_url").and_then(|v| v.as_str()),
            Some("https://cdn.example.com/media/movie-1080p.mp4")
        );
        let opinions = direct.get("opinions").and_then(|o| o.as_array()).unwrap();
        assert_eq!(opinions.len(), 1, "只有嗅探器注册了解析器: {direct}");
        assert_eq!(opinions[0].get("plugin").and_then(|v| v.as_str()), Some("direct-link-sniffer"));
        assert_eq!(opinions[0].get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(opinions[0].get("direct").and_then(|v| v.as_bool()), Some(true));

        let page = run_resolvers(&ctx, "https://example.com/watch?v=12345");
        assert_eq!(page.get("direct").and_then(|v| v.as_bool()), Some(false), "网页不是直链: {page}");
        let page_opinion = &page.get("opinions").and_then(|o| o.as_array()).unwrap()[0];
        assert_eq!(page_opinion.get("direct").and_then(|v| v.as_bool()), Some(false));

        // 4) dispatch_event —— download:error 触发重试
        let ev = dispatch_event(&ctx, "download:error", json!({ "taskId": "task-42" }));
        assert_eq!(ev.get("notified").and_then(|v| v.as_i64()), Some(1), "只有 retry-hook 监听错误: {ev}");
        let retries = ev.get("retries").and_then(|r| r.as_array()).unwrap();
        assert_eq!(retries.len(), 1, "首次失败应产生 1 个重试请求: {ev}");
        assert_eq!(retries[0].get("plugin").and_then(|v| v.as_str()), Some("retry-hook"));
        assert_eq!(retries[0].get("task_id").and_then(|v| v.as_str()), Some("task-42"));
        assert_eq!(retries[0].get("attempt").and_then(|v| v.as_i64()), Some(1));
        assert!(ev.get("errors").and_then(|e| e.as_array()).unwrap().is_empty(), "{ev}");

        // 再次失败：不应无限重试
        let ev2 = dispatch_event(&ctx, "download:error", json!({ "taskId": "task-42" }));
        assert_eq!(ev2.get("notified").and_then(|v| v.as_i64()), Some(1));
        assert!(ev2.get("retries").and_then(|r| r.as_array()).unwrap().is_empty(), "不应重复重试: {ev2}");

        // download:done 清零计数 → 又能重试一次
        let done = dispatch_event(&ctx, "download:done", json!({ "taskId": "task-42" }));
        assert_eq!(done.get("notified").and_then(|v| v.as_i64()), Some(1), "{done}");
        let ev3 = dispatch_event(&ctx, "download:error", json!({ "taskId": "task-42" }));
        assert_eq!(ev3.get("retries").and_then(|r| r.as_array()).unwrap().len(), 1, "{ev3}");

        // app:start 无人监听
        let start = dispatch_event(&ctx, "app:start", json!({}));
        assert_eq!(start.get("notified").and_then(|v| v.as_i64()), Some(0), "{start}");
    }

    #[test]
    fn retry_hook_never_loops_forever() {
        let env = TempEnv::new("retry");
        let ctx = env.ctx();
        env.install_builtins(&ctx);
        let mut total = 0;
        for i in 0..6 {
            let ev = dispatch_event(&ctx, "download:error", json!({ "taskId": "flaky-1" }));
            total += ev.get("retries").and_then(|r| r.as_array()).map(|a| a.len()).unwrap_or(0);
            assert!(i < 10);
        }
        assert_eq!(total, 1, "同一任务的 6 次失败只应产生 1 个重试请求");
    }

    #[test]
    fn toggle_and_uninstall_plugin() {
        let env = TempEnv::new("toggle");
        let ctx = env.ctx();
        env.install_builtins(&ctx);

        let rec = set_plugin_enabled(&ctx, "retry-hook", false).unwrap();
        assert_eq!(rec.get("enabled").and_then(|v| v.as_bool()), Some(false));
        let list = list_plugins(&ctx);
        let hook = list
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p.get("id").and_then(|v| v.as_str()) == Some("retry-hook"))
            .unwrap()
            .clone();
        assert_eq!(hook.get("enabled").and_then(|v| v.as_bool()), Some(false));

        // 停用后事件不再派发给它
        let ev = dispatch_event(&ctx, "download:error", json!({ "taskId": "t1" }));
        assert_eq!(ev.get("notified").and_then(|v| v.as_i64()), Some(0), "{ev}");
        assert!(ev.get("retries").and_then(|r| r.as_array()).unwrap().is_empty());

        // 重新启用
        let rec2 = set_plugin_enabled(&ctx, "retry-hook", true).unwrap();
        assert_eq!(rec2.get("enabled").and_then(|v| v.as_bool()), Some(true));
        let ev2 = dispatch_event(&ctx, "download:error", json!({ "taskId": "t1" }));
        assert_eq!(ev2.get("retries").and_then(|r| r.as_array()).unwrap().len(), 1, "{ev2}");

        // 卸载
        uninstall_plugin(&ctx, "retry-hook").expect("卸载应成功");
        assert!(!plugin_dir(&ctx, "retry-hook").exists(), "目录应被删除");
        assert_eq!(list_plugins(&ctx).as_array().unwrap().len(), 1);
        assert!(uninstall_plugin(&ctx, "retry-hook").is_err(), "重复卸载应报错");

        // 停用嗅探器后，解析器不再参与
        set_plugin_enabled(&ctx, "direct-link-sniffer", false).unwrap();
        let r = run_resolvers(&ctx, "https://cdn.example.com/a.mp4");
        assert_eq!(r.get("queried").and_then(|v| v.as_i64()), Some(0), "{r}");
        assert!(r.get("opinions").and_then(|o| o.as_array()).unwrap().is_empty());
        assert_eq!(r.get("direct").and_then(|v| v.as_bool()), Some(false));
    }

    /* ---------- 恶意插件：隔离在真实安装路径上生效 ---------- */

    #[test]
    fn hostile_plugin_fails_cleanly_without_crashing_host() {
        let env = TempEnv::new("hostile");
        let ctx = env.ctx();
        env.install_builtins(&ctx);

        // 手写一个想逃逸的「插件」：读文件 / 起进程 / 发请求 / 死循环
        let evil = r#"
            (function () {
              var report = [];
              try { require("fs"); report.push("require-ok"); } catch (e) { report.push("require-blocked"); }
              try { process.exit(1); report.push("process-ok"); } catch (e) { report.push("process-blocked"); }
              try { fetch("http://127.0.0.1/"); report.push("fetch-ok"); } catch (e) { report.push("fetch-blocked"); }
              try { new XMLHttpRequest(); report.push("xhr-ok"); } catch (e) { report.push("xhr-blocked"); }
              try { umi.storage.set("probe", 1); report.push("umi-storage-ok"); } catch (e) { report.push("umi-storage-blocked"); }
              umi.log("escape-report: " + report.join("/"));
              umi.export("selfTest", function () { while (true) {} });
            })();
        "#;
        let dir = plugin_dir(&ctx, "evil");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.js"), evil).unwrap();
        let mut reg = read_registry(&ctx);
        upsert_entry(
            &mut reg,
            json!({
                "id": "evil", "name": "逃逸尝试", "version": "0.0.1", "description": "测试用",
                "author": "test", "sha256": sha256_bytes(evil.as_bytes()), "source": "local",
                "enabled": true, "installed_at": now_iso(),
            }),
        )
        .unwrap();
        write_registry(&ctx, &reg).unwrap();

        let started = Instant::now();
        let r = test_plugin(&ctx, "evil");
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_millis(SCRIPT_TIMEOUT_MS * 10), "恶意插件拖死宿主: {elapsed:?}");
        assert_eq!(r.get("ok").and_then(|v| v.as_bool()), Some(false), "必须判定为失败: {r}");
        let err = r.get("error").and_then(|v| v.as_str()).unwrap_or("");
        assert!(err.contains("interrupted") || err.contains("超时"), "死循环应被中断: {r}");
        let logs = r.get("logs").and_then(|l| l.as_array()).unwrap().clone();
        let joined = logs
            .iter()
            .filter_map(|l| l.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("escape-report"), "即使失败也必须回传日志: {r}");
        assert!(joined.contains("require-blocked"), "require 必须不可用: {joined}");
        assert!(joined.contains("process-blocked"), "process 必须不可用: {joined}");
        assert!(joined.contains("fetch-blocked"), "fetch 必须不可用: {joined}");
        assert!(joined.contains("xhr-blocked"), "XMLHttpRequest 必须不可用: {joined}");
        assert!(joined.contains("umi-storage-ok"), "白名单能力应可用: {joined}");
        // 宿主还活着
        let ok = test_plugin(&ctx, "direct-link-sniffer");
        assert_eq!(ok.get("ok").and_then(|v| v.as_bool()), Some(true), "宿主应继续正常工作: {ok}");
    }

    #[test]
    fn analyze_url_classification() {
        let m = analyze_url("https://cdn.example.com/media/movie.mp4");
        assert_eq!(m.get("direct").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(m.get("kind").and_then(|v| v.as_str()), Some("video"));
        let page = analyze_url("https://example.com/watch?v=1");
        assert_eq!(page.get("direct").and_then(|v| v.as_bool()), Some(false));
        let magnet = analyze_url("magnet:?xt=urn:btih:abcdef");
        assert_eq!(magnet.get("direct").and_then(|v| v.as_bool()), Some(true));
        let bad = analyze_url("not a url");
        assert_eq!(bad.get("ok").and_then(|v| v.as_bool()), Some(false));
        let hls = analyze_url("https://cdn.example.com/live/index.m3u8");
        assert_eq!(hls.get("direct").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(hls.get("kind").and_then(|v| v.as_str()), Some("stream"));
    }

    #[test]
    fn plugin_id_validation_blocks_traversal() {
        assert!(validate_id("../../etc/passwd").is_err());
        assert!(validate_id("a/b").is_err());
        assert!(validate_id("").is_err());
        assert!(validate_id("ok-plugin_1.0").is_ok());
    }

    /// 事件日志：插件相关动作要真的发到前端（此前一条都不发，面板永远是空的）
    #[test]
    fn plugin_actions_emit_frontend_event_log() {
        let env = TempEnv::new("evtlog");
        let seen: Arc<Mutex<Vec<Json>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let ctx = env.ctx().with_emit(Arc::new(move |event: &str, payload: Json| {
            assert_eq!(event, "plugin://event", "只该往 plugin://event 发");
            sink.lock().unwrap().push(payload);
        }));

        install_from_market(&ctx, "direct-link-sniffer").expect("安装内置插件");
        set_plugin_enabled(&ctx, "direct-link-sniffer", false).expect("停用");
        let _ = test_plugin(&ctx, "direct-link-sniffer");
        let _ = run_resolvers(&ctx, "https://cdn.example.com/media/movie.mp4");
        uninstall_plugin(&ctx, "direct-link-sniffer").expect("卸载");

        let events: Vec<String> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|p| p.get("event").and_then(|v| v.as_str()).unwrap_or("").to_string())
            .collect();
        for want in ["install", "enable", "test", "resolver", "uninstall"] {
            assert!(events.iter().any(|e| e == want), "缺少 {want} 日志：{events:?}");
        }
        // 字段形状与前端解析一致（id / level / message 都要在）
        let first = seen.lock().unwrap().first().cloned().unwrap();
        assert!(first.get("level").and_then(|v| v.as_str()).is_some());
        assert!(first.get("message").and_then(|v| v.as_str()).is_some());
        assert!(first.get("id").is_some());
    }

    /// 重试请求的提取：只认本次任务、去重、缺字段不炸
    #[test]
    fn retry_task_ids_filters_and_dedupes() {
        let res = json!({
            "event": "download:error",
            "notified": 1,
            "retries": [
                { "plugin": "retry-hook", "task_id": "t1", "attempt": 1, "reason": "首次失败" },
                { "plugin": "retry-hook", "task_id": "t1", "attempt": 1, "reason": "重复请求" },
                { "plugin": "other", "task_id": "t2", "attempt": 1, "reason": "别的任务" }
            ],
            "errors": []
        });
        assert_eq!(retry_task_ids(&res, "t1"), vec!["t1".to_string()]);
        assert!(retry_task_ids(&res, "t9").is_empty(), "不认识的任务不能被重试");
        assert!(retry_task_ids(&json!({}), "t1").is_empty(), "缺 retries 字段不炸");
        assert!(retry_task_ids(&json!({ "retries": "oops" }), "t1").is_empty(), "类型不对也不炸");
    }

    /// 派发 download:error 时，重试钩子插件被调用且日志里能看到它的重试请求
    #[test]
    fn dispatch_error_reports_retry_request_and_logs_it() {
        let env = TempEnv::new("dispatchlog");
        env.install_builtins(&env.ctx());
        let seen: Arc<Mutex<Vec<Json>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let ctx = env.ctx().with_emit(Arc::new(move |_e: &str, payload: Json| {
            sink.lock().unwrap().push(payload);
        }));

        let res = dispatch_event(&ctx, "download:error", json!({ "taskId": "task-7", "error": "网络中断" }));
        assert_eq!(res.get("notified").and_then(|v| v.as_i64()), Some(1), "{res}");
        assert_eq!(retry_task_ids(&res, "task-7"), vec!["task-7".to_string()]);
        let logs: Vec<String> = seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|p| p.get("message").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .collect();
        assert!(
            logs.iter().any(|l| l.contains("请求重试任务 task-7")),
            "重试请求要写进日志：{logs:?}"
        );
        assert!(logs.iter().any(|l| l.contains("事件 download:error")), "派发小结要写进日志：{logs:?}");
    }
}
