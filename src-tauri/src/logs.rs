//! 应用日志：落盘 + 导出
//!
//! 所有下载 / 转换 / 字幕 / 工具 / 捕获事件写一份到
//! `<data>/logs/umidl-YYYY-MM-DD.log`，单文件超过 4 MB 自动轮转。
//! 导出时把日志与环境摘要（版本、工具路径、关键设置）汇成一个目录。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::ctx::AppDirs;

const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;

static WRITE_LOCK: Mutex<()> = Mutex::new(());

fn now_stamp() -> (String, String) {
    // (YYYY-MM-DD, HH:MM:SS) —— 不引第三方时间库，用系统时间秒数换算
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, m, d, hh, mm, ss) = civil_from_unix(secs);
    (
        format!("{y:04}-{m:02}-{d:02}"),
        format!("{hh:02}:{mm:02}:{ss:02}"),
    )
}

/// 把 Unix 秒折算成东八区民用时间
fn civil_from_unix(secs: i64) -> (i64, i64, i64, i64, i64, i64) {
    // 先整体平移 8 小时，再拆分日期/时刻（否则跨日会算错）
    let shifted = secs + 8 * 3600;
    let days = shifted.div_euclid(86_400);
    let tod = shifted.rem_euclid(86_400);
    let (hh, mm, ss) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d, hh, mm, ss)
}

fn log_file(dirs: &AppDirs, date: &str) -> PathBuf {
    dirs.data.join("logs").join(format!("umidl-{date}.log"))
}

/// 追加一行日志；任何 IO 失败都静默（日志不能影响主流程）
pub fn log_line(dirs: &AppDirs, tag: &str, msg: &str) {
    let _g = WRITE_LOCK.lock();
    let (date, time) = now_stamp();
    let path = log_file(dirs, &date);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // 超限轮转：改名成 .1 覆盖式保留一份
    if std::fs::metadata(&path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(&path, path.with_extension("log.1"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "[{date} {time}] [{tag}] {msg}");
    }
}

/// 最近 N 天的日志文件（新的在前）
pub fn recent_logs(dirs: &AppDirs, days: usize) -> Vec<PathBuf> {
    let dir = dirs.data.join("logs");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("umidl-") && n.ends_with(".log"))
                        .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files.reverse();
    files.truncate(days.max(1));
    files
}

/// 导出：把日志 + 环境摘要写到 `dest`（目录或 .txt 文件路径）
pub fn export(dirs: &AppDirs, dest: &Path, summary: &str) -> anyhow::Result<PathBuf> {
    let files = recent_logs(dirs, 14);
    let mut body = String::from("=== Umidl 诊断日志包 ===\n\n");
    body.push_str(summary);
    body.push_str("\n\n");
    for f in &files {
        body.push_str(&format!("───── {} ─────\n", f.file_name().unwrap_or_default().to_string_lossy()));
        match std::fs::read_to_string(f) {
            Ok(t) => body.push_str(&t),
            Err(e) => body.push_str(&format!("(读取失败: {e})\n")),
        }
        body.push('\n');
    }
    if body.trim().is_empty() {
        body.push_str("(暂无日志)\n");
    }
    let out = if dest.extension().map(|e| e.eq_ignore_ascii_case("txt")).unwrap_or(false) {
        dest.to_path_buf()
    } else {
        let (date, time) = now_stamp();
        let name = format!("umidl-logs-{date}-{}.txt", time.replace(':', ""));
        let dir = if dest.is_dir() { dest.to_path_buf() } else { dirs.data.join("logs") };
        std::fs::create_dir_all(&dir).ok();
        dir.join(name)
    };
    std::fs::write(&out, body)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_time_roundtrip() {
        // 2026-09-27T00:00:00Z → 东八区 08:00
        let secs = 1_790_467_200;
        let (y, m, d, hh, _, _) = civil_from_unix(secs);
        assert_eq!((y, m, d), (2026, 9, 27));
        assert_eq!(hh, 8, "东八区应为 08 点");
        // 跨日边界：UTC 18:00 → 东八区次日 02:00
        let (y2, _, d2, hh2, _, _) = civil_from_unix(secs + 18 * 3600);
        assert_eq!((y2, d2, hh2), (2026, 28, 2));
    }
}
