use std::path::Path;

use crate::capture;
use crate::ctx::AppDirs;
use crate::models::AppSettings;

pub fn load(dirs: &AppDirs) -> AppSettings {
    let mut s = std::fs::read_to_string(&dirs.settings_file)
        .ok()
        .and_then(|t| serde_json::from_str::<AppSettings>(&t).ok())
        .unwrap_or_default();
    if s.download_dir.trim().is_empty() {
        s.download_dir = dirs.downloads.to_string_lossy().to_string();
    }
    if s.whisper_model.trim().is_empty() {
        s.whisper_model = "base".into();
    }
    if s.concurrency <= 0 {
        s.concurrency = 3;
    }
    if s.theme.trim().is_empty() {
        s.theme = "dark".into();
    }
    // 捕获令牌（BUG-06）：启用捕获端口时**必须**有令牌 —— 缺失（首启 / 用户清空后保存）就
    // 现场生成一个随机令牌并落盘。放在 load() 里，设置文件、内存快照、捕获服务、
    // 设置页 UI 四处读到的永远是同一个令牌。
    if s.capture_port > 0 && s.capture_token.trim().is_empty() {
        s.capture_token = capture::generate_token();
        let _ = save(dirs, &s);
    }
    s
}

pub fn save(dirs: &AppDirs, s: &AppSettings) -> anyhow::Result<()> {
    dirs.ensure();
    let text = serde_json::to_string_pretty(s)?;
    std::fs::write(&dirs.settings_file, text)?;
    Ok(())
}

/// 校验用户指定的工具路径是否可用
pub fn validate_tool_path(p: &str) -> bool {
    !p.trim().is_empty() && Path::new(p.trim()).is_file()
}
