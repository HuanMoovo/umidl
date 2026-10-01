//! 智能过滤：按扩展名、域名黑白名单、最小文件大小决定是否下载
//!
//! 纯逻辑模块，可单测。域名规则：
//! - `example.com` 命中自身及任意子域（`www.example.com`）
//! - `*.example.com` 只命中子域
//! 扩展名规则忽略大小写与前导点：`mp4` 与 `.MP4` 等价。

use crate::models::AppSettings;

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Allow,
    Block(String),
}

impl Verdict {
    pub fn is_block(&self) -> bool {
        matches!(self, Verdict::Block(_))
    }
    pub fn reason(&self) -> Option<&str> {
        match self {
            Verdict::Block(r) => Some(r.as_str()),
            Verdict::Allow => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Filters {
    pub ext_block: Vec<String>,
    pub domain_block: Vec<String>,
    pub domain_allow: Vec<String>,
    pub min_size_bytes: u64,
}

fn split_list(s: &str) -> Vec<String> {
    s.split([',', '，', ';', '\n'])
        .map(|x| x.trim().to_ascii_lowercase())
        .filter(|x| !x.is_empty())
        .collect()
}

fn norm_ext(e: &str) -> String {
    e.trim().trim_start_matches('.').to_ascii_lowercase()
}

impl Filters {
    pub fn from_settings(s: &AppSettings) -> Self {
        Self {
            ext_block: split_list(&s.filter_ext_block).into_iter().map(|e| norm_ext(&e)).collect(),
            domain_block: split_list(&s.filter_domain_block),
            domain_allow: split_list(&s.filter_domain_allow),
            min_size_bytes: if s.filter_min_size_mb > 0 {
                s.filter_min_size_mb as u64 * 1024 * 1024
            } else {
                0
            },
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ext_block.is_empty()
            && self.domain_block.is_empty()
            && self.domain_allow.is_empty()
            && self.min_size_bytes == 0
    }

    /// 从 URL 取主机名（小写，去端口）
    pub fn host_of(url: &str) -> Option<String> {
        let after_scheme = url.split("://").nth(1).unwrap_or(url);
        let host = after_scheme
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("")
            .rsplit('@') // 去掉 user:pass@
            .next()
            .unwrap_or("");
        // IPv6 形如 [::1]:8080
        let host = if host.starts_with('[') {
            host.split(']').next().unwrap_or("").trim_start_matches('[')
        } else {
            host.split(':').next().unwrap_or("")
        };
        if host.is_empty() {
            None
        } else {
            Some(host.to_ascii_lowercase())
        }
    }

    /// 从 URL / 路径里取扩展名
    pub fn ext_of(url: &str) -> Option<String> {
        let path = url.split(['?', '#']).next().unwrap_or(url);
        let last = path.rsplit('/').next().unwrap_or(path);
        match last.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && ext.len() <= 8 => {
                Some(norm_ext(ext))
            }
            _ => None,
        }
    }

    fn domain_matches(pattern: &str, host: &str) -> bool {
        if let Some(suffix) = pattern.strip_prefix("*.") {
            host.len() > suffix.len() && host.ends_with(suffix) && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
        } else {
            host == pattern || host.ends_with(&format!(".{pattern}"))
        }
    }

    /// 入队前检查（域名 + URL 扩展名）。大小未知，故不检查最小体积。
    pub fn check_url(&self, url: &str) -> Verdict {
        // 磁力 / 种子：无域名与扩展名语义，只按协议放过
        if url.starts_with("magnet:") || url.starts_with("ed2k:") {
            return Verdict::Allow;
        }
        if let Some(host) = Self::host_of(url) {
            if !self.domain_allow.is_empty()
                && !self.domain_allow.iter().any(|p| Self::domain_matches(p, &host))
            {
                return Verdict::Block(format!("域名不在白名单：{host}"));
            }
            if let Some(hit) = self
                .domain_block
                .iter()
                .find(|p| Self::domain_matches(p, &host))
            {
                return Verdict::Block(format!("域名命中黑名单：{hit}"));
            }
        }
        if let Some(ext) = Self::ext_of(url) {
            if self.ext_block.contains(&ext) {
                return Verdict::Block(format!("扩展名被过滤：.{ext}"));
            }
        }
        Verdict::Allow
    }

    /// 探测到大小之后检查最小体积
    pub fn check_size(&self, bytes: Option<u64>) -> Verdict {
        if self.min_size_bytes == 0 {
            return Verdict::Allow;
        }
        match bytes {
            Some(b) if b < self.min_size_bytes => Verdict::Block(format!(
                "小于最小体积（{:.1} MB < {} MB）",
                b as f64 / 1_048_576.0,
                self.min_size_bytes / 1_048_576
            )),
            _ => Verdict::Allow,
        }
    }

    /// 下载完成后按落盘文件复核扩展名（yt-dlp 场景 URL 看不出真实后缀）
    pub fn check_path(&self, file: &str) -> Verdict {
        match Self::ext_of(file) {
            Some(ext) if self.ext_block.contains(&ext) => {
                Verdict::Block(format!("扩展名被过滤：.{ext}"))
            }
            _ => Verdict::Allow,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(ext: &str, block: &str, allow: &str, min: i64) -> Filters {
        let s = AppSettings {
            filter_ext_block: ext.into(),
            filter_domain_block: block.into(),
            filter_domain_allow: allow.into(),
            filter_min_size_mb: min,
            ..Default::default()
        };
        Filters::from_settings(&s)
    }

    #[test]
    fn host_and_ext_parsing() {
        assert_eq!(Filters::host_of("https://www.Example.com:8443/a/b.mp4?x=1").unwrap(), "www.example.com");
        assert_eq!(Filters::host_of("http://user:pw@a.b.c/x").unwrap(), "a.b.c");
        assert_eq!(Filters::host_of("http://[::1]:8080/x").unwrap(), "::1");
        assert_eq!(Filters::ext_of("https://a.com/v.MP4?x=.txt"), Some("mp4".into()));
        assert_eq!(Filters::ext_of("https://a.com/dir.v2/file"), None, "目录名不算扩展名");
        assert_eq!(Filters::ext_of("https://a.com/dir.v2/file.MP3"), Some("mp3".into()));
        assert_eq!(Filters::ext_of("https://a.com/"), None);
    }

    #[test]
    fn extension_block_works() {
        let fl = f("mp4,.EXE", "", "", 0);
        assert!(fl.check_url("https://a.com/v.mp4").is_block());
        assert!(fl.check_url("https://a.com/v.exe").is_block());
        assert!(!fl.check_url("https://a.com/v.mkv").is_block());
        assert!(fl.check_path("D:/x/y/setup.EXE").is_block());
    }

    #[test]
    fn domain_rules() {
        let fl = f("", "*.ads.com, tracker.net", "", 0);
        assert!(fl.check_url("https://cdn.ads.com/v.mp4").is_block());
        assert!(!fl.check_url("https://ads.com/v.mp4").is_block(), "带 * 只匹配子域");
        assert!(fl.check_url("https://sub.tracker.net/x").is_block());
        assert!(fl.check_url("https://tracker.net/x").is_block(), "裸域匹配自身");
        assert!(!fl.check_url("https://notracker.net/x").is_block());
    }

    #[test]
    fn whitelist_restricts() {
        let fl = f("", "", "bilibili.com, *.youtube.com", 0);
        assert!(!fl.check_url("https://www.bilibili.com/video/BV1").is_block());
        assert!(!fl.check_url("https://m.youtube.com/watch?v=1").is_block());
        assert!(fl.check_url("https://vimeo.com/123").is_block());
        assert!(!fl.check_url("magnet:?xt=urn:btih:abc").is_block(), "磁力不做域名过滤");
    }

    #[test]
    fn min_size_and_empty() {
        let fl = f("", "", "", 50);
        assert!(fl.check_size(Some(10 * 1024 * 1024)).is_block());
        assert!(!fl.check_size(Some(60 * 1024 * 1024)).is_block());
        assert!(!fl.check_size(None).is_block(), "大小未知时放过");
        assert!(Filters::default().is_empty());
        assert!(!fl.is_empty());
    }
}
