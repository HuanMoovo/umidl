use std::path::Path;

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
    // 「跟随系统」已移除：历史配置里的 'system'（及空值）一律回退 dark，不报错
    let theme = s.theme.trim().to_ascii_lowercase();
    if theme.is_empty() || theme == "system" {
        s.theme = "dark".into();
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
