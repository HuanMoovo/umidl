/*! GitHub 直链 → 可下载地址 / 安全文件名（自定义模型入口用）
 *
 * 支持四种写法（用户直接粘贴仓库地址即可）：
 *   1) https://github.com/<o>/<r>/releases/download/<tag>/<file>   → 原样下载（Release 资产）
 *   2) https://github.com/<o>/<r>/blob/<ref>/<path>               → 转 raw.githubusercontent.com
 *   3) https://raw.githubusercontent.com/<o>/<r>/<ref>/<path>     → 原样下载
 *   4) 上面 2) 带 `?raw=true` 的写法（GitHub 页面上「Download」按钮给出的地址）→ 同 2)
 *
 * 设计要点：
 *   - 解析是纯函数：便于单测四种写法 + 各种脏输入（多余空白、`#`、大小写域名、目录地址）。
 *   - 文件名一律从 URL 末段安全化提取（去查询串 / 去锚点 / 去引号包裹 / 防 `..` 越级）——
 *     下载落盘只用这个值，绝不拿 URL 原文当文件名。
 *   - **不套用 HuggingFace 镜像**：GitHub 有独立的源与可达性，镜像硬套过去只会两头不讨好。
 */

/// GitHub 地址的解析结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubDownload {
    /// 真正去下载的地址（blob → raw 已转换；Release 资产保持原样）
    pub url: String,
    /// 安全文件名（末段，已去掉查询串并做字符白名单）
    pub file: String,
    pub owner: String,
    pub repo: String,
    /// Release tag 或 git ref（能取到就带上，便于日志里说清「装的是哪个版本」）
    pub git_ref: Option<String>,
    /// 地址形态，日志/界面里回显用
    pub kind: GhUrlKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhUrlKind {
    /// releases/download/<tag>/<file>
    ReleaseAsset,
    /// github.com/.../blob/<ref>/<path> → raw
    BlobToRaw,
    /// raw.githubusercontent.com/...
    Raw,
}

impl GhUrlKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            GhUrlKind::ReleaseAsset => "release",
            GhUrlKind::BlobToRaw => "blob-raw",
            GhUrlKind::Raw => "raw",
        }
    }
}

impl GithubDownload {
    /// 下载地址列表：**主地址在前**（用户给的那个），后面是可达性更好时的备份源。
    /// 镜像只做兜底，绝不改变「下载的是用户指定的那个文件」这一事实。
    pub fn download_urls(&self) -> Vec<String> {
        let mut out = vec![self.url.clone()];
        match self.kind {
            GhUrlKind::Raw | GhUrlKind::BlobToRaw => {
                if let Some(jd) = self.jsdelivr_url() {
                    out.push(jd);
                }
                out.push(format!("https://gh-proxy.com/{}", self.url));
            }
            GhUrlKind::ReleaseAsset => {
                out.push(format!("https://gh-proxy.com/{}", self.url));
            }
        }
        out.retain(|u| !u.trim().is_empty() && u.contains("://"));
        out.dedup();
        out
    }

    /// jsDelivr 镜像地址（只对 raw 文件有意义）：`raw.githubusercontent.com/o/r/ref/p/a` →
    /// `cdn.jsdelivr.net/gh/o/r@ref/p/a`。
    pub fn jsdelivr_url(&self) -> Option<String> {
        let rest = self.url.strip_prefix("https://raw.githubusercontent.com/")?;
        let mut parts = rest.splitn(3, '/');
        let owner = parts.next()?;
        let repo = parts.next()?;
        let tail = parts.next()?;
        let (git_ref, path) = tail.split_once('/')?;
        if owner.is_empty() || repo.is_empty() || git_ref.is_empty() || path.is_empty() {
            return None;
        }
        Some(format!(
            "https://cdn.jsdelivr.net/gh/{owner}/{repo}@{git_ref}/{path}"
        ))
    }
}

pub fn is_github_url(raw: &str) -> bool {
    let u = normalize_input(raw).to_lowercase();
    u.starts_with("https://github.com/")
        || u.starts_with("http://github.com/")
        || u.starts_with("https://raw.githubusercontent.com/")
        || u.starts_with("http://raw.githubusercontent.com/")
        || u.starts_with("https://www.github.com/")
        || u.starts_with("github.com/")
        || u.starts_with("raw.githubusercontent.com/")
}

/// 去掉用户粘贴时常见的包裹与空白：`<url>`、引号、行首尾空白
fn normalize_input(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    loop {
        let t = s.trim();
        let changed = if (t.starts_with('<') && t.ends_with('>'))
            || (t.starts_with('"') && t.ends_with('"') && t.len() > 1)
            || (t.starts_with('\'') && t.ends_with('\'') && t.len() > 1)
        {
            t[1..t.len() - 1].trim().to_string()
        } else if let Some(rest) = t.strip_prefix("github.com/") {
            format!("https://github.com/{rest}")
        } else if let Some(rest) = t.strip_prefix("raw.githubusercontent.com/") {
            format!("https://raw.githubusercontent.com/{rest}")
        } else {
            break;
        };
        s = changed;
    }
    s
}

/// URL → (scheme+host, 路径段)。查询串 / 锚点会被丢掉（GitHub 的 `?raw=true` 正是靠这一步吸收）。
fn split_url(url: &str) -> Option<(String, Vec<String>)> {
    let no_frag = url.split('#').next().unwrap_or(url);
    let no_query = no_frag.split('?').next().unwrap_or(no_frag);
    let (host, rest) = no_query.split_once("://")?;
    let host = host.trim().to_lowercase();
    if host.is_empty() {
        return None;
    }
    let path = rest.split('/').next().unwrap_or("");
    let _ = path;
    // rest = "github.com/a/b/blob/main/c.bin" → 取 host 之后的所有段
    let after_host = rest.split_once('/').map(|(h, r)| (h, r)).unwrap_or((rest, ""));
    let segments: Vec<String> = after_host
        .1
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    let full_host = format!("{}://{}", host, after_host.0.to_lowercase());
    Some((full_host, segments))
}

/// 解析 GitHub 直链（四种写法 + 宽松清理）。非 GitHub 地址返回 Err（调用方回落到普通 http 下载）。
pub fn parse_github_url(raw: &str) -> Result<GithubDownload, String> {
    let input = normalize_input(raw);
    if input.is_empty() {
        return Err("请输入模型链接或本地文件路径".into());
    }
    let (host, segs) = split_url(&input)
        .ok_or_else(|| format!("无法解析地址（缺少 http(s):// 前缀）：{input}"))?;
    let h = host.to_lowercase();
    let is_github = h.ends_with("github.com") && !h.contains("raw.githubusercontent.com");
    let is_raw = h.ends_with("raw.githubusercontent.com");
    if !is_github && !is_raw {
        return Err(format!("不是 GitHub 地址：{input}"));
    }
    if is_raw {
        // raw.githubusercontent.com/<o>/<r>/<ref>/<path...>
        if segs.len() < 4 {
            return Err(format!(
                "GitHub raw 地址至少要 4 段（<用户>/<仓库>/<分支>/<文件>）：{input}"
            ));
        }
        let file = safe_file_name_from_url(&input);
        if file.is_empty() {
            return Err(format!("从地址里取不到文件名（是不是目录地址？）：{input}"));
        }
        return Ok(GithubDownload {
            url: build_url(&input, &segs, &segs[..3].join("/")),
            file,
            owner: segs[0].clone(),
            repo: segs[1].clone(),
            git_ref: Some(segs[2].clone()),
            kind: GhUrlKind::Raw,
        });
    }

    // github.com/<o>/<r>/<kind>/...  至少 5 段
    if segs.len() < 5 {
        return Err(format!(
            "看不懂的 GitHub 地址：{input}（支持 releases/download/<tag>/<文件>、blob/<分支>/<文件>、raw.githubusercontent.com/<用户>/<仓库>/<分支>/<文件>）"
        ));
    }
    let (owner, repo) = (segs[0].clone(), segs[1].clone());
    match segs[2].as_str() {
        "releases" => {
            if segs[3] != "download" {
                return Err(format!(
                    "只支持 Release 的「下载」地址（…/releases/download/<tag>/<文件>）：{input}"
                ));
            }
            let tag = segs[4].clone();
            let file = safe_file_name_from_url(&input);
            if file.is_empty() {
                return Err(format!("Release 地址里取不到文件名：{input}"));
            }
            Ok(GithubDownload {
                url: input,
                file,
                owner,
                repo,
                git_ref: Some(tag),
                kind: GhUrlKind::ReleaseAsset,
            })
        }
        "blob" => {
            // segs = [o, r, "blob", <ref>, <path...>] → 分支/标签在 segs[3]
            let git_ref = segs[3].clone();
            let file = safe_file_name_from_url(&input);
            if file.is_empty() {
                return Err(format!("从地址里取不到文件名（是不是目录地址？）：{input}"));
            }
            // 分支名要留在路径里：segs = [o, r, "blob", <ref>, <path...>]
            let path = segs[3..].join("/");
            let raw = format!("https://raw.githubusercontent.com/{owner}/{repo}/{path}");
            Ok(GithubDownload {
                url: raw,
                file,
                owner,
                repo,
                git_ref: Some(git_ref),
                kind: GhUrlKind::BlobToRaw,
            })
        }
        "raw" => {
            // github.com/<o>/<r>/raw/<ref>/<path>（老写法，同样归一到 raw）
            if segs.len() < 6 {
                return Err(format!("GitHub raw 地址要带分支与文件：{input}"));
            }
            let path = segs[3..].join("/");
            let raw = format!("https://raw.githubusercontent.com/{owner}/{repo}/{path}");
            Ok(GithubDownload {
                url: raw,
                file: safe_file_name_from_url(&input),
                owner,
                repo,
                // segs = [o, r, "raw", <ref>, <path...>]
                git_ref: Some(segs[3].clone()),
                kind: GhUrlKind::Raw,
            })
        }
        other => Err(format!(
            "这个 GitHub 地址暂时用不了（/{}）——请给 Release 资产地址或 raw/blob 文件地址：{input}",
            other
        )),
    }
}

/// raw 地址原样保留（仅去掉查询串/锚点/包裹），保证下载的是用户给的那个文件
fn build_url(input: &str, _segs: &[String], _keep: &str) -> String {
    let no_frag = input.split('#').next().unwrap_or(input);
    no_frag.split('?').next().unwrap_or(no_frag).trim().to_string()
}

/// URL → 安全文件名（单测覆盖四种写法 + 脏输入）：
///   * 只取末段（`…/model.int8.onnx?raw=true` → `model.int8.onnx`）
///   * 去查询串 / 锚点 / URL 编码残留的引号与空白
///   * 字符白名单 `[A-Za-z0-9._-]`，其余换成 `-`
///   * 去掉全部前导 `.`（`..`、`.env` 都变成普通名字，杜绝越级）
///   * 长度上限 120，空值返回空串（调用方按「取不到文件名」报错）
pub fn safe_file_name_from_url(url: &str) -> String {
    let no_frag = url.split('#').next().unwrap_or(url);
    let no_query = no_frag.split('?').next().unwrap_or(no_frag);
    let trimmed = no_query.trim().trim_matches(|c| c == '"' || c == '\'' || c == '<' || c == '>');
    let raw = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed).trim();
    let decoded = percent_decode_basic(raw);
    let mut s: String = decoded
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+') { c } else { '-' })
        .collect();
    while s.starts_with('.') || s.starts_with('-') {
        if s.starts_with('.') {
            s.remove(0);
        } else {
            s.remove(0);
        }
    }
    let s = s.trim_end_matches(['.', '-', '+']).to_string();
    if s.len() > 120 {
        // 保留扩展名，前面的部分截断（超长文件名在 Windows 上会直接失败）
        let ext = std::path::Path::new(&s)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let keep = 120usize.saturating_sub(ext.len());
        let head: String = s.chars().take(keep).collect();
        return format!("{head}{ext}");
    }
    s
}

/// 只解 %20 / %2E 这类常见编码（GitHub 文件名里偶见空格），其余保持原样后再走白名单
fn percent_decode_basic(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("").to_lowercase();
            // 只解这三个：空格 / 斜杠 / 点。斜杠要解（解完再走白名单变成 '-'，
            // 「..%2Fetc%2Fpasswd」才不会留下裸的 '..'）；%23('#') 之类一律不解，
            // 免得把实体数字吞掉（a%23b → a-23b）。
            match hex.as_str() {
                "20" => {
                    out.push(' ');
                    i += 3;
                    continue;
                }
                "2f" => {
                    out.push('/');
                    i += 3;
                    continue;
                }
                "2e" => {
                    out.push('.');
                    i += 3;
                    continue;
                }
                _ => {}
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// 该地址看着像不像「点进去是目录」（末段为空 / 是已知的目录段）
pub fn looks_like_directory(url: &str) -> bool {
    let no_frag = url.split('#').next().unwrap_or(url);
    let no_query = no_frag.split('?').next().unwrap_or(no_frag);
    let t = no_query.trim().trim_end_matches('/');
    t.ends_with("/blob") || t.ends_with("/tree") || t.ends_with("/releases") || t.ends_with("/download")
}

/* ============================ 测试 ============================ */

#[cfg(test)]
mod tests {
    use super::*;

    /// 四种写法都要解析成可下载地址（本轮的核心契约）
    #[test]
    fn parses_four_github_url_forms() {
        // 1) Release 资产：原样下载
        let a = parse_github_url(
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx",
        )
        .unwrap();
        assert_eq!(a.kind, GhUrlKind::ReleaseAsset);
        assert_eq!(a.file, "silero_vad.onnx");
        assert_eq!(a.owner, "k2-fsa");
        assert_eq!(a.repo, "sherpa-onnx");
        assert_eq!(a.git_ref.as_deref(), Some("asr-models"));
        assert!(a.url.contains("/releases/download/asr-models/silero_vad.onnx"));
        assert!(!a.url.contains("hf-mirror"), "GitHub 资产绝不能套 HF 镜像：{}", a.url);

        // 2) blob → raw
        let b = parse_github_url("https://github.com/ggml-org/whisper.cpp/blob/master/models/ggml-tiny.bin")
            .unwrap();
        assert_eq!(b.kind, GhUrlKind::BlobToRaw);
        assert_eq!(b.file, "ggml-tiny.bin");
        assert_eq!(
            b.url,
            "https://raw.githubusercontent.com/ggml-org/whisper.cpp/master/models/ggml-tiny.bin"
        );
        assert_eq!(b.git_ref.as_deref(), Some("master"));

        // 3) raw.githubusercontent.com
        let c = parse_github_url(
            "https://raw.githubusercontent.com/ggml-org/whisper.cpp/master/models/ggml-tiny.bin",
        )
        .unwrap();
        assert_eq!(c.kind, GhUrlKind::Raw);
        assert_eq!(c.file, "ggml-tiny.bin");
        assert_eq!(
            c.url,
            "https://raw.githubusercontent.com/ggml-org/whisper.cpp/master/models/ggml-tiny.bin",
            "raw 地址原样使用（不带查询串）"
        );

        // 4) blob + ?raw=true（GitHub 网页上的「Download」按钮地址）
        let d = parse_github_url(
            "https://github.com/ggml-org/whisper.cpp/blob/master/models/ggml-tiny.bin?raw=true",
        )
        .unwrap();
        assert_eq!(d.kind, GhUrlKind::BlobToRaw);
        assert_eq!(d.file, "ggml-tiny.bin", "查询串不能混进文件名");
        assert!(!d.url.contains("?raw=true"), "转 raw 后要丢掉查询串：{}", d.url);
    }

    /// 脏输入：包裹引号 / 缺 scheme / 分支名带斜杠 / 锚点 / 大小写域名
    #[test]
    fn parses_dirty_but_common_inputs() {
        let a = parse_github_url("  <https://github.com/o/r/blob/main/sub/model.int8.onnx>  ").unwrap();
        assert_eq!(a.file, "model.int8.onnx");
        assert_eq!(a.url, "https://raw.githubusercontent.com/o/r/main/sub/model.int8.onnx");

        let b = parse_github_url("github.com/o/r/releases/download/v1/x.tar.bz2").unwrap();
        assert_eq!(b.kind, GhUrlKind::ReleaseAsset);
        assert_eq!(b.file, "x.tar.bz2");
        assert!(b.url.starts_with("https://github.com/o/r/releases/download/v1/"));

        let c = parse_github_url("HTTPS://GitHub.com/o/r/blob/feature/x/model.onnx").unwrap();
        assert_eq!(c.file, "model.onnx");
        assert_eq!(c.url, "https://raw.githubusercontent.com/o/r/feature/x/model.onnx");

        // 分支名里带斜杠：整体归一到 raw（分支部分不解析，直接拼路径）
        let d = parse_github_url("https://github.com/o/r/blob/feature/foo/model.onnx").unwrap();
        assert_eq!(d.file, "model.onnx");

        // 非 GitHub 地址必须明确拒绝（调用方据此走普通 http 链路）
        assert!(parse_github_url("https://hf-mirror.com/a/b/resolve/main/c.bin").is_err());
        assert!(!is_github_url("https://hf-mirror.com/a/b"));
        assert!(is_github_url("https://github.com/o/r/blob/main/a.bin"));

        // 目录 / 残缺地址给的是「看得懂」的错，而不是静默失败
        assert!(parse_github_url("https://github.com/o/r").unwrap_err().contains("看不懂"));
        assert!(parse_github_url("https://github.com/o/r/tree/main/models").is_err());
        assert!(looks_like_directory("https://github.com/o/r/releases"));
    }

    /// 文件名安全化：查询串 / 编码 / 越级 / 长名 / 空名
    #[test]
    fn safe_file_name_never_escapes_or_keeps_query() {
        assert_eq!(safe_file_name_from_url("https://a.com/x/model.onnx?download=1"), "model.onnx");
        assert_eq!(safe_file_name_from_url("https://a.com/x/model%20int8.onnx"), "model-int8.onnx");
        assert_eq!(safe_file_name_from_url("https://a.com/x/..%2Fetc%2Fpasswd"), "etc-passwd");
        assert_eq!(safe_file_name_from_url("https://a.com/x/../../evil.bin"), "evil.bin");
        assert_eq!(safe_file_name_from_url("https://a.com/x/.hidden"), "hidden");
        assert_eq!(safe_file_name_from_url("https://a.com/"), "");
        assert_eq!(safe_file_name_from_url("https://a.com/x/a%23b.tar.bz2#frag"), "a-23b.tar.bz2");
        assert!(safe_file_name_from_url(&format!("https://a.com/{}", "长".repeat(200))).len() <= 120);
        assert!(!safe_file_name_from_url("https://a.com/x/..%2F..%2Fevil").contains('/'));
    }
}
