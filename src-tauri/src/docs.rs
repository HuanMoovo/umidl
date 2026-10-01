//! 文档格式转换：原生 OOXML / ODF 解析（zip + quick-xml）+ pandoc / poppler 外部引擎
//!
//! 三条通路，按「原生 → pandoc → poppler」优先级路由（见 [`route_for`]）：
//!
//! | 输入 | 原生（纯 Rust，无需任何外部依赖） | 外部引擎 |
//! |------|--------------------------------|----------|
//! | docx | txt / md / html（标题、段落、表格） | pandoc → 任意 pandoc 目标 |
//! | xlsx | csv / txt / html（sharedStrings、inlineStr、多工作表） | pandoc |
//! | pptx | txt / md / html（按 slide 顺序） | pandoc |
//! | odt / ods / odp | txt / md / csv / html（content.xml） | pandoc |
//! | pdf  | — | poppler：pdftotext（txt）/ pdftoppm（png） |
//! | rtf / epub / html / md / rst / txt | html、txt、md、csv 直接读取 | pandoc |
//!
//! 外部引擎未安装时明确报错，提示到「设置 → 依赖工具」一键安装，绝不静默失败。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{anyhow, bail, Context, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use quick_xml::XmlVersion;
use serde_json::json;

use crate::ctx::{command_for, ensure_dir, Ctx};

/// 有 XML 前缀时用的默认版本（几乎所有 OOXML / ODF 部件都是 XML 1.0）
const XML_V1_0: XmlVersion = XmlVersion::Implicit1_0;

/* ==================================================================
 *  1. 扩展名 / 目标格式判定
 * ================================================================== */

/// 原生可解析的容器格式（zip + XML，无需外部引擎）
pub const NATIVE_KINDS: &[&str] = &["docx", "xlsx", "pptx", "odt", "ods", "odp"];

/// 纯文本类（原生直接读取，可剥离 HTML 标签）
const TEXT_KINDS: &[&str] = &["txt", "md", "csv", "tsv", "html"];

/// 旧版 Office 二进制格式（OLE2，无开源纯 Rust 解析器可用）
const LEGACY_KINDS: &[&str] = &["doc", "xls", "ppt"];

/// 是否文档类文件（按扩展名）
pub fn is_document_path(p: &str) -> bool {
    !kind_of(Path::new(p)).is_empty()
}

/// 归一化格式名：markdown → md、htm/xhtml → html
pub fn normalize_target(target: &str) -> String {
    match target.trim().to_ascii_lowercase().as_str() {
        "markdown" | "mdown" | "mkd" => "md".into(),
        "htm" | "xhtml" => "html".into(),
        other => other.to_string(),
    }
}

/// 文档类型：扩展名归一化后的短名，未知返回空串
pub fn kind_of(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    // 说明：这里用静态字符串表匹配，避免调用方持有临时字符串
    const ALL: &[&str] = &[
        "docx", "doc", "xlsx", "xls", "pptx", "ppt", "odt", "ods", "odp", "rtf", "epub", "pdf", "md",
        "markdown", "html", "htm", "xhtml", "txt", "csv", "tsv", "rst", "org",
    ];
    let ext = match ext.as_str() {
        "markdown" => "md",
        "htm" | "xhtml" => "html",
        other => other,
    };
    ALL.iter().find(|k| **k == ext).copied().unwrap_or("")
}

/// 原生支持的 (输入 → 输出) 组合
pub fn native_supports(kind: &str, target: &str) -> bool {
    let t = normalize_target(target);
    match kind {
        "docx" | "pptx" | "odt" | "odp" => matches!(t.as_str(), "txt" | "md" | "html"),
        "xlsx" | "ods" => matches!(t.as_str(), "csv" | "txt" | "html"),
        // 纯文本类：直接读取（html 会剥离标签）
        "txt" | "md" | "csv" | "tsv" | "html" => t == "txt",
        _ => false,
    }
}

/// pandoc 能读取的输入类型（不含旧版二进制与 pdf）
fn pandoc_reads(kind: &str) -> bool {
    matches!(
        kind,
        "docx" | "odt" | "rtf" | "epub" | "html" | "md" | "txt" | "csv" | "rst" | "org"
    )
}

/// pandoc 支持的输出格式（键为归一化后的目标名）
fn pandoc_writer(target: &str) -> Option<&'static str> {
    Some(match target {
        "md" => "gfm",
        "html" => "html5",
        "txt" => "plain",
        "docx" => "docx",
        "odt" => "odt",
        "rtf" => "rtf",
        "epub" => "epub",
        "pptx" => "pptx",
        "tex" | "latex" => "latex",
        "rst" => "rst",
        "org" => "org",
        "json" => "json",
        "opml" => "opml",
        "mediawiki" => "mediawiki",
        "asciidoc" => "asciidoc",
        "csv" => "csv",
        other => {
            // 未在表内的名称：只要形如合法标识符就交给 pandoc 自己判断
            if !other.is_empty()
                && other
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '_'))
            {
                return None; // 交给调用方原样透传
            }
            return None;
        }
    })
}

/// pandoc 输入格式（-f 参数）：从扩展名推断，未知则交给 pandoc 猜测
fn pandoc_reader(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "docx" => "docx",
        "odt" => "odt",
        "rtf" => "rtf",
        "epub" => "epub",
        "html" => "html",
        "md" => "gfm",
        "rst" => "rst",
        "org" => "org",
        "csv" => "csv",
        _ => return None,
    })
}

/// 原生可处理的输入扩展名清单（供「设置 → 文档转换」展示）
pub fn native_formats() -> Vec<String> {
    NATIVE_KINDS
        .iter()
        .chain(TEXT_KINDS.iter())
        .map(|s| s.to_string())
        .collect()
}

/// 当前引擎能力下可用的输出格式清单
pub fn supported_targets(pandoc: bool, poppler: bool) -> Vec<String> {
    let mut v: Vec<String> = ["txt", "md", "html", "csv"].iter().map(|s| s.to_string()).collect();
    if pandoc {
        v.extend(
            ["docx", "odt", "rtf", "epub", "pptx", "tex", "rst", "org"]
                .iter()
                .map(|s| s.to_string()),
        );
    }
    if poppler {
        v.push("png".into());
    }
    v
}

/// 文档类输入在「自动检测」时推荐的输出格式
pub fn auto_document_target(kind: &str) -> String {
    match kind {
        "xlsx" | "ods" | "csv" => "csv",
        "pptx" | "odp" => "md",
        "docx" | "odt" | "md" | "html" => "md",
        _ => "txt",
    }
    .to_string()
}

/// 转换路由
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// 纯 Rust 原生解析
    Native,
    /// pandoc 外部引擎
    Pandoc,
    /// poppler（pdf → txt/png）
    PopplerPdf,
    /// 无可用通路，附可读原因
    Unsupported(String),
}

/// 媒体容器/编码：文档绝不能输出成这些（route_for 立即判不支持，而不是转给 pandoc）
const NON_DOCUMENT_TARGETS: &[&str] = &[
    "mp4", "mkv", "mov", "avi", "webm", "flv", "wmv", "ts", "mpg", "mpeg", "m4v", "3gp", "mp3", "m4a",
    "aac", "flac", "wav", "ogg", "opus", "wma", "gif", "jpg", "jpeg", "png", "webp", "bmp", "tif", "tiff",
    "ico",
];

/// 根据输入类型与目标格式选择通路
pub fn route_for(src: &Path, target: &str) -> Route {
    let kind = kind_of(src);
    let t = normalize_target(target);
    if native_supports(kind, &t) {
        return Route::Native;
    }
    if kind == "pdf" {
        return if t == "txt" || t == "png" {
            Route::PopplerPdf
        } else {
            Route::Unsupported(format!(
                "PDF 仅支持转换为 txt（文本）或 png（图片），当前目标为 {t}"
            ))
        };
    }
    if NON_DOCUMENT_TARGETS.contains(&t.as_str()) {
        return Route::Unsupported(format!(
            "{kind} 不能转换为媒体格式 {t}：文档请选择 txt / md / html / csv 等文档输出"
        ));
    }
    if pandoc_reads(kind) && (pandoc_writer(&t).is_some() || is_plain_identifier(&t)) {
        return Route::Pandoc;
    }
    if LEGACY_KINDS.contains(&kind) {
        return Route::Unsupported(format!(
            "旧版二进制格式 .{kind} 不受支持：请先用 Office / WPS 另存为 .docx / .xlsx / .pptx，或转存为 rtf / odt / html 后再转换"
        ));
    }
    if kind.is_empty() {
        return Route::Unsupported(format!("不是支持的文档格式：{}", src.display()));
    }
    Route::Unsupported(format!(
        "不支持 {kind} → {t}：原生引擎可输出 txt / md / html / csv，更多格式请在「设置 → 依赖工具」安装 pandoc"
    ))
}

fn is_plain_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '_'))
}

/* ==================================================================
 *  2. 能力检测
 * ================================================================== */

fn exe_name_docs(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// 解析外部引擎路径：ctx.tools（启动时已解析）→ 受管目录 → PATH → tools::resolve_tool
///
/// 注意：`tools::resolve_tool(ctx, "poppler")` 只会命中受管目录 `bin/poppler/pdftotext.exe`，
/// 用户自己用系统包管理器安装的 poppler / xpdf（只有 `pdftotext` 在 PATH 里）需要这里补一次查找。
fn engine_path(ctx: &Ctx, name: &str) -> Option<PathBuf> {
    match name {
        "pandoc" => {
            if let Some(p) = ctx.tools.pandoc.clone().filter(|p| p.is_file()) {
                return Some(p);
            }
            for cand in [
                ctx.dirs.bin.join(exe_name_docs("pandoc")),
                ctx.dirs.bin.join("pandoc").join(exe_name_docs("pandoc")),
            ] {
                if cand.is_file() {
                    return Some(cand);
                }
            }
            which_on_path(&exe_name_docs("pandoc"))
                .or_else(|| crate::tools::resolve_tool(ctx, "pandoc", None).0)
        }
        "poppler" => {
            if let Some(p) = ctx.tools.poppler.clone().filter(|p| p.is_file()) {
                return Some(p);
            }
            for cand in [
                ctx.dirs.bin.join("poppler").join(exe_name_docs("pdftotext")),
                ctx.dirs.bin.join(exe_name_docs("pdftotext")),
            ] {
                if cand.is_file() {
                    return Some(cand);
                }
            }
            which_on_path(&exe_name_docs("pdftotext"))
                .or_else(|| crate::tools::resolve_tool(ctx, "poppler", None).0)
        }
        _ => None,
    }
}

/// 不依赖调用方 ctx 的引擎探测（extract_text 等无 ctx 的入口使用）
fn detect_engine(name: &str) -> Option<PathBuf> {
    let dirs = crate::ctx::AppDirs::new();
    let ctx = Ctx::new(dirs, crate::ctx::ToolPaths::default(), crate::models::AppSettings::default());
    engine_path(&ctx, name)
}

/// 引擎能力探测：`{"pandoc":bool,"pandoc_path":..,"poppler":bool,"poppler_path":..,"native":[..],"targets":[..]}`
pub fn capabilities(ctx: &Ctx) -> serde_json::Value {
    let pandoc = engine_path(ctx, "pandoc");
    let poppler = engine_path(ctx, "poppler");
    json!({
        "pandoc": pandoc.is_some(),
        "pandoc_path": pandoc.as_ref().map(|p| p.to_string_lossy().to_string()),
        "poppler": poppler.is_some(),
        "poppler_path": poppler.as_ref().map(|p| p.to_string_lossy().to_string()),
        "native": native_formats(),
        "targets": supported_targets(pandoc.is_some(), poppler.is_some()),
    })
}

/* ==================================================================
 *  3. XML 迷你 DOM（保留顺序，轻量容错）
 * ================================================================== */

#[derive(Debug, Clone)]
pub enum Child {
    Elem(Node),
    Text(String),
}

#[derive(Debug, Clone, Default)]
pub struct Node {
    /// 本地名（去掉 ns 前缀，如 `w:body` → `body`），所有比较都用它
    pub name: String,
    /// 原始限定名（含前缀，排查问题用）
    pub qname: String,
    /// 属性按原文顺序保存（全名优先，按本地名回退匹配）
    pub attrs: Vec<(String, String)>,
    pub content: Vec<Child>,
}

impl Node {
    /// 属性查找：先全名精确匹配（如 `r:id`），再按本地名匹配（如 `val` ↔ `w:val`）
    pub fn attr(&self, key: &str) -> Option<&str> {
        if let Some((_, v)) = self.attrs.iter().find(|(k, _)| k == key) {
            return Some(v.as_str());
        }
        self.attrs
            .iter()
            .find(|(k, _)| k.rsplit(':').next() == Some(key))
            .map(|(_, v)| v.as_str())
    }

    pub fn elems(&self) -> impl Iterator<Item = &Node> + '_ {
        self.content.iter().filter_map(|c| match c {
            Child::Elem(e) => Some(e),
            Child::Text(_) => None,
        })
    }

    pub fn elem(&self, name: &str) -> Option<&Node> {
        self.elems().find(|e| e.name == name)
    }

    /// 直接子元素文本（如 `<v>42</v>`）
    pub fn text(&self) -> String {
        let mut s = String::new();
        for c in &self.content {
            if let Child::Text(t) = c {
                s.push_str(t);
            }
        }
        s
    }

    /// 深度优先查找第一个同名后代
    pub fn find_desc(&self, name: &str) -> Option<&Node> {
        for e in self.elems() {
            if e.name == name {
                return Some(e);
            }
            if let Some(f) = e.find_desc(name) {
                return Some(f);
            }
        }
        None
    }

    /// 深度优先收集全部同名后代（保持文档顺序）
    pub fn all_desc(&self, name: &str) -> Vec<&Node> {
        let mut out = Vec::new();
        self.collect_desc(name, &mut out);
        out
    }

    pub fn collect_desc<'a>(&'a self, name: &str, out: &mut Vec<&'a Node>) {
        for e in self.elems() {
            if e.name == name {
                out.push(e);
            }
            e.collect_desc(name, out);
        }
    }

    /// 同名后代计数
    pub fn count_desc(&self, name: &str) -> usize {
        let mut n = 0;
        for e in self.elems() {
            if e.name == name {
                n += 1;
            }
            n += e.count_desc(name);
        }
        n
    }

    fn space_preserve(&self) -> bool {
        self.attr("xml:space") == Some("preserve") || self.attr("space") == Some("preserve")
    }
}

/// 携带文本的元素：其内部空白有意义（w:t / a:t / v=单元格值）
fn keeps_raw_ws(node: &Node) -> bool {
    matches!(node.name.as_str(), "t" | "v") || node.space_preserve()
}

/// 解析 XML 字符串为迷你 DOM。容错：不校验结束标签、允许游离 `&`。
pub fn parse_xml(xml: &str) -> Result<Node> {
    let mut reader = Reader::from_str(xml);
    {
        let cfg = reader.config_mut();
        cfg.check_end_names = false;
        cfg.allow_unmatched_ends = true;
        cfg.allow_dangling_amp = true;
        cfg.trim_text(false);
    }
    let mut root = Node {
        name: "#document".into(),
        ..Default::default()
    };
    let mut stack: Vec<Node> = Vec::new();
    loop {
        match reader.read_event() {
            Err(e) => return Err(anyhow!("XML 解析失败：{e}")),
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                if stack.len() > 512 {
                    return Err(anyhow!("XML 嵌套过深，已放弃解析"));
                }
                stack.push(node_from_start(&e));
            }
            Ok(Event::Empty(e)) => {
                let n = node_from_start(&e);
                attach_elem(&mut stack, &mut root, n);
            }
            Ok(Event::End(_)) => {
                if let Some(mut n) = stack.pop() {
                    normalize_text(&mut n);
                    attach_elem(&mut stack, &mut root, n);
                }
            }
            Ok(Event::Text(e)) => {
                let s = e.xml_content(XML_V1_0).into_owned();
                attach_text(&mut stack, &mut root, &s);
            }
            Ok(Event::CData(e)) => {
                let s = e.xml_content(XML_V1_0).into_owned();
                attach_text(&mut stack, &mut root, &s);
            }
            Ok(Event::GeneralRef(e)) => {
                // 字符引用（&#x4E2D;）或实体引用（&amp;）
                let name = e.xml_content(XML_V1_0).into_owned();
                let ch = match e.resolve_char_ref() {
                    Ok(Some(c)) => Some(c),
                    _ => match name.as_str() {
                        "amp" => Some('&'),
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "quot" => Some('"'),
                        "apos" => Some('\''),
                        _ => None,
                    },
                };
                if let Some(c) = ch {
                    let s = c.to_string();
                    attach_text(&mut stack, &mut root, &s);
                }
            }
            Ok(_) => {}
        }
    }
    Ok(root)
}

fn node_from_start(e: &BytesStart) -> Node {
    let qname = e.name().as_ref().to_string();
    let local = e.local_name().as_ref().to_string();
    let mut n = Node {
        name: if local.is_empty() { qname.clone() } else { local },
        qname,
        ..Default::default()
    };
    for a in e.attributes().flatten() {
        let key = a.key.as_ref().to_string();
        let val = a
            .normalized_value(XML_V1_0)
            .map(|c| c.into_owned())
            .unwrap_or_else(|_| a.value.to_string());
        n.attrs.push((key, val));
    }
    n
}

fn attach_elem(stack: &mut Vec<Node>, root: &mut Node, n: Node) {
    match stack.last_mut() {
        Some(p) => p.content.push(Child::Elem(n)),
        None => root.content.push(Child::Elem(n)),
    }
}

/// 追加文本：先原样累积（不逐块 trim，否则 `特殊字符 &amp; 符号` 的空格会被吃掉），
/// 元素结束时再由 [`normalize_text`] 统一裁掉边界空白
fn attach_text(stack: &mut Vec<Node>, _root: &mut Node, raw: &str) {
    if raw.is_empty() {
        return;
    }
    let Some(parent) = stack.last_mut() else {
        return;
    };
    // 相邻文本合并，避免碎片
    if let Some(Child::Text(last)) = parent.content.last_mut() {
        last.push_str(raw);
        return;
    }
    parent.content.push(Child::Text(raw.to_string()));
}

/// 元素闭合时整理文本：裁掉与元素边界相邻的空白、丢弃纯空白文本节点（XML 缩进）。
/// 文本携带元素（`w:t`/`a:t`/`v`、`xml:space="preserve"`）保持原样，空格全保留。
fn normalize_text(node: &mut Node) {
    if keeps_raw_ws(node) {
        return;
    }
    let n = node.content.len();
    for (i, c) in node.content.iter_mut().enumerate() {
        if let Child::Text(t) = c {
            let mut s: &str = t;
            if i == 0 {
                s = s.trim_start();
            }
            if i + 1 == n {
                s = s.trim_end();
            }
            if s.trim().is_empty() {
                t.clear();
            } else {
                *t = s.to_string();
            }
        }
    }
    node.content.retain(|c| match c {
        Child::Text(t) => !t.is_empty(),
        Child::Elem(_) => true,
    });
}

/* ==================================================================
 *  4. 内联文本（段落级）
 * ================================================================== */

#[derive(Debug, Clone, PartialEq)]
enum Inline {
    Text(String),
    Tab,
    Br,
}

/// 需要跳过的子树：样式 / 修订删除 / 域代码 / 日文注音
fn skip_inline(name: &str) -> bool {
    matches!(
        name,
        "pPr" | "rPr" | "tblPr" | "trPr" | "tcPr" | "sectPr" | "instrText" | "fldChar" | "del"
            | "delText" | "rPh" | "phoneticPr" | "noteRef" | "annotationRef"
    )
}

fn push_text(out: &mut Vec<Inline>, s: String) {
    if s.is_empty() {
        return;
    }
    if let Some(Inline::Text(last)) = out.last_mut() {
        last.push_str(&s);
        return;
    }
    out.push(Inline::Text(s));
}

/// 递归收集内联内容（保持文档顺序）
fn collect_inline(node: &Node, out: &mut Vec<Inline>) {
    for c in &node.content {
        match c {
            Child::Text(t) => push_text(out, t.clone()),
            Child::Elem(e) => match e.name.as_str() {
                "t" => push_text(out, e.text()),
                "tab" => out.push(Inline::Tab),
                "br" | "line-break" => out.push(Inline::Br),
                "s" => {
                    // ODF text:s：空格，可带数量
                    let n = e.attr("c").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).min(64);
                    push_text(out, " ".repeat(n));
                }
                n if skip_inline(n) => {}
                _ => collect_inline(e, out),
            },
        }
    }
}

/// 内联内容 → 字符串（`mode`=md 时换行转 `<br>`）
fn inline_text(node: &Node, md: bool) -> String {
    let mut v = Vec::new();
    collect_inline(node, &mut v);
    v.into_iter()
        .map(|i| match i {
            Inline::Text(t) => t,
            Inline::Tab => "\t".to_string(),
            Inline::Br => if md { "<br>".to_string() } else { "\n".to_string() },
        })
        .collect()
}

/* ==================================================================
 *  5. 文档模型与渲染
 * ================================================================== */

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// level=0 普通段落，>0 标题层级
    Para { level: u8, text: String },
    Table(Vec<Vec<String>>),
}

fn md_cell(s: &str) -> String {
    s.replace('\r', "").replace('\n', "<br>").replace('|', "\\|").trim().to_string()
}

/// 块模型 → Markdown
pub fn blocks_to_markdown(blocks: &[Block]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for b in blocks {
        match b {
            Block::Para { level, text } => {
                let t = text.replace('\r', "").replace('\n', "<br>");
                if *level > 0 {
                    parts.push(format!("{} {}", "#".repeat((*level).min(6) as usize), t.trim()));
                } else if !t.trim().is_empty() {
                    parts.push(t.trim_end().to_string());
                }
            }
            Block::Table(rows) => {
                let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
                if width == 0 {
                    continue;
                }
                let mut lines: Vec<String> = Vec::new();
                for (i, r) in rows.iter().enumerate() {
                    let cells: Vec<String> = (0..width)
                        .map(|c| md_cell(r.get(c).map(|s| s.as_str()).unwrap_or("")))
                        .collect();
                    lines.push(format!("| {} |", cells.join(" | ")));
                    if i == 0 {
                        lines.push(format!("| {} |", vec!["---"; width].join(" | ")));
                    }
                }
                parts.push(lines.join("\n"));
            }
        }
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("{}\n", parts.join("\n\n"))
}

/// 块模型 → 纯文本
pub fn blocks_to_text(blocks: &[Block]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for b in blocks {
        match b {
            Block::Para { text, .. } => {
                let t = text.trim_end();
                if !t.trim().is_empty() {
                    parts.push(t.to_string());
                }
            }
            Block::Table(rows) => {
                let lines: Vec<String> = rows
                    .iter()
                    .map(|r| r.iter().map(|c| c.replace(['\n', '\r'], " ")).collect::<Vec<_>>().join("\t"))
                    .collect();
                if !lines.is_empty() {
                    parts.push(lines.join("\n"));
                }
            }
        }
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("{}\n", parts.join("\n\n"))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn html_table(rows: &[Vec<String>]) -> String {
    let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut s = String::from("<table>");
    if let Some(head) = rows.first() {
        s.push_str("<thead><tr>");
        for c in head {
            s.push_str(&format!("<th>{}</th>", html_escape(c)));
        }
        s.push_str("</tr></thead>");
    }
    s.push_str("<tbody>");
    for r in rows.iter().skip(1) {
        s.push_str("<tr>");
        for c in 0..width {
            let v = r.get(c).map(|s| s.as_str()).unwrap_or("");
            s.push_str(&format!("<td>{}</td>", html_escape(v).replace('\n', "<br>")));
        }
        s.push_str("</tr>");
    }
    s.push_str("</tbody></table>");
    s
}

fn html_document(title: &str, body: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"zh-CN\">\n<head><meta charset=\"utf-8\"><title>{}</title></head>\n<body>\n{}\n</body>\n</html>\n",
        html_escape(title),
        body
    )
}

/// 块模型 → HTML 文档
pub fn blocks_to_html(blocks: &[Block], title: &str) -> String {
    let mut body: Vec<String> = Vec::new();
    for b in blocks {
        match b {
            Block::Para { level, text } => {
                if text.trim().is_empty() {
                    continue;
                }
                let t = html_escape(text.trim()).replace('\n', "<br>");
                if *level > 0 {
                    let lv = (*level).clamp(1, 6);
                    body.push(format!("<h{lv}>{t}</h{lv}>"));
                } else {
                    body.push(format!("<p>{t}</p>"));
                }
            }
            Block::Table(rows) => {
                if !rows.is_empty() {
                    body.push(html_table(rows));
                }
            }
        }
    }
    html_document(title, &body.join("\n"))
}

/* ==================================================================
 *  6. zip 辅助
 * ================================================================== */

fn zip_open(path: &Path) -> Result<zip::ZipArchive<std::fs::File>> {
    let f = std::fs::File::open(path).with_context(|| format!("打开文件失败：{}", path.display()))?;
    zip::ZipArchive::new(f).map_err(|e| anyhow!("不是有效的 OOXML / ODF 压缩包：{e}"))
}

fn zip_part(path: &Path, entry: &str) -> Result<String> {
    let mut z = zip_open(path)?;
    let mut f = z
        .by_name(entry)
        .map_err(|_| anyhow!("压缩包中缺少部件 {entry}"))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)
        .with_context(|| format!("读取部件 {entry} 失败"))?;
    if buf.starts_with(&[0xEF, 0xBB, 0xBF]) {
        buf.drain(..3);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn zip_part_opt(path: &Path, entry: &str) -> Option<String> {
    zip_part(path, entry).ok()
}

fn zip_names(path: &Path) -> Vec<String> {
    match zip_open(path) {
        Ok(z) => z.file_names().map(|s| s.to_string()).collect(),
        Err(_) => Vec::new(),
    }
}

/// 相对部件路径解析（`xl` + `worksheets/sheet1.xml`，支持 `../`）
fn resolve_part(base: &str, target: &str) -> String {
    if let Some(rest) = target.strip_prefix('/') {
        return rest.to_string();
    }
    let mut parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/* ==================================================================
 *  7. OOXML：docx / xlsx / pptx
 * ================================================================== */

/* ---------- docx ---------- */

/// word/styles.xml：styleId → 样式名（用于识别标题层级）
fn docx_styles(path: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some(xml) = zip_part_opt(path, "word/styles.xml") else {
        return map;
    };
    let Ok(root) = parse_xml(&xml) else { return map };
    for st in root.all_desc("style") {
        if st.attr("type").map(|t| t != "paragraph").unwrap_or(false) {
            continue;
        }
        let id = st.attr("styleId").unwrap_or("").to_string();
        let name = st
            .elem("name")
            .and_then(|n| n.attr("val"))
            .unwrap_or("")
            .to_string();
        if !id.is_empty() {
            map.insert(id, name);
        }
    }
    map
}

/// 标题层级：`heading 1` / `标题 1` / `Heading1` / `h2` → 1..9
fn heading_level(style_id: &str, styles: &HashMap<String, String>) -> u8 {
    let name = styles.get(style_id).map(|s| s.as_str()).unwrap_or("");
    for cand in [name, style_id] {
        let t = cand.trim().to_ascii_lowercase();
        if t.is_empty() {
            continue;
        }
        for pre in ["heading", "标题", "h"] {
            if let Some(rest) = t.strip_prefix(pre) {
                let rest = rest.trim().trim_start_matches(['.', '-', '_']);
                if let Ok(n) = rest.parse::<u8>() {
                    if (1..=9).contains(&n) {
                        return n;
                    }
                }
            }
        }
        if t == "title" {
            return 1;
        }
    }
    0
}

fn docx_blocks(path: &Path) -> Result<Vec<Block>> {
    let styles = docx_styles(path);
    let xml = zip_part(path, "word/document.xml")?;
    let root = parse_xml(&xml)?;
    let body = root.find_desc("body").ok_or_else(|| anyhow!("document.xml 缺少 w:body"))?;
    let mut blocks = Vec::new();
    walk_docx_body(body, &styles, &mut blocks);
    Ok(blocks)
}

fn walk_docx_body(node: &Node, styles: &HashMap<String, String>, out: &mut Vec<Block>) {
    for e in node.elems() {
        match e.name.as_str() {
            "p" => {
                let (level, text) = docx_paragraph(e, styles);
                if level > 0 || !text.trim().is_empty() {
                    out.push(Block::Para { level, text });
                }
            }
            "tbl" => {
                let rows = docx_table(e);
                if !rows.is_empty() {
                    out.push(Block::Table(rows));
                }
            }
            // 内容控件 / 结构化文档标签 / 表格单元格等容器：继续下钻
            "sdt" | "sdtContent" | "tc" | "txbxContent" | "body" => walk_docx_body(e, styles, out),
            _ => {}
        }
    }
}

fn docx_paragraph(p: &Node, styles: &HashMap<String, String>) -> (u8, String) {
    let level = p
        .elem("pPr")
        .and_then(|pp| pp.elem("pStyle"))
        .and_then(|s| s.attr("val"))
        .map(|v| heading_level(v, styles))
        .unwrap_or(0);
    (level, inline_text(p, false))
}

fn docx_table(tbl: &Node) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    for tr in docx_rows(tbl) {
        let mut row: Vec<String> = Vec::new();
        for tc in tr.elems().filter(|e| e.name == "tc") {
            let mut cell: Vec<String> = Vec::new();
            for p in tc.elems() {
                match p.name.as_str() {
                    "p" => cell.push(inline_text(p, false)),
                    "sdt" | "sdtContent" => {
                        if let Some(inner) = p.elem("sdtContent") {
                            for ip in inner.elems().filter(|e| e.name == "p") {
                                cell.push(inline_text(ip, false));
                            }
                        }
                    }
                    _ => {}
                }
            }
            row.push(cell.join(" ").trim().to_string());
        }
        if !row.is_empty() {
            rows.push(row);
        }
    }
    rows
}

fn docx_rows(tbl: &Node) -> Vec<&Node> {
    let mut rows = Vec::new();
    for e in tbl.elems() {
        match e.name.as_str() {
            "tr" => rows.push(e),
            "sdt" | "sdtContent" => rows.extend(docx_rows(e)),
            _ => {}
        }
    }
    rows
}

/* ---------- xlsx ---------- */

/// 工作表：(名称, 部件路径)
fn xlsx_sheets(path: &Path) -> Result<Vec<(String, String)>> {
    let wb = zip_part(path, "xl/workbook.xml")?;
    let root = parse_xml(&wb)?;
    let rels: HashMap<String, String> = zip_part_opt(path, "xl/_rels/workbook.xml.rels")
        .and_then(|x| parse_xml(&x).ok())
        .map(|r| {
            r.all_desc("Relationship")
                .into_iter()
                .filter_map(|e| Some((e.attr("Id")?.to_string(), e.attr("Target")?.to_string())))
                .collect()
        })
        .unwrap_or_default();

    let mut out = Vec::new();
    if let Some(sheets) = root.find_desc("sheets") {
        for (i, sh) in sheets.elems().filter(|e| e.name == "sheet").enumerate() {
            let name = sh.attr("name").unwrap_or("Sheet").to_string();
            let part = sh
                .attr("r:id")
                .or_else(|| sh.attr("id"))
                .and_then(|rid| rels.get(rid))
                .map(|t| resolve_part("xl", t))
                .unwrap_or_else(|| format!("xl/worksheets/sheet{}.xml", i + 1));
            out.push((name, part));
        }
    }
    if out.is_empty() {
        // 回退：按文件名扫描
        let mut names: Vec<String> = zip_names(path)
            .into_iter()
            .filter(|n| n.starts_with("xl/worksheets/sheet") && n.ends_with(".xml"))
            .collect();
        names.sort();
        for (i, n) in names.into_iter().enumerate() {
            out.push((format!("Sheet{}", i + 1), n));
        }
    }
    Ok(out)
}

fn xlsx_shared_strings(path: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Some(xml) = zip_part_opt(path, "xl/sharedStrings.xml") else {
        return out;
    };
    let Ok(root) = parse_xml(&xml) else { return out };
    for si in root.all_desc("si") {
        out.push(inline_text(si, false));
    }
    out
}

/// 单元格引用 → 列下标（0 起）：`B12` → 1
fn col_index(reference: &str) -> usize {
    let mut idx = 0usize;
    for ch in reference.chars() {
        if ch.is_ascii_alphabetic() {
            idx = idx * 26 + (ch.to_ascii_uppercase() as usize - 'A' as usize + 1);
        } else {
            break;
        }
    }
    idx.saturating_sub(1)
}

fn xlsx_cell_value(c: &Node, shared: &[String]) -> String {
    match c.attr("t").unwrap_or("") {
        "s" => c
            .elem("v")
            .map(|v| v.text())
            .and_then(|i| i.trim().parse::<usize>().ok())
            .and_then(|i| shared.get(i).cloned())
            .unwrap_or_default(),
        "inlineStr" => c.elem("is").map(|is| inline_text(is, false)).unwrap_or_default(),
        "b" => if c.elem("v").map(|v| v.text().trim() == "1").unwrap_or(false) {
            "TRUE".into()
        } else {
            "FALSE".into()
        },
        // n / str / d / e：直接取 <v>
        _ => c.elem("v").map(|v| v.text().trim().to_string()).unwrap_or_default(),
    }
}

fn xlsx_rows(path: &Path, part: &str) -> Result<Vec<Vec<String>>> {
    let shared = xlsx_shared_strings(path);
    let xml = zip_part(path, part)?;
    let root = parse_xml(&xml)?;
    let mut rows: Vec<Vec<String>> = Vec::new();
    let row_nodes: Vec<&Node> = root
        .find_desc("sheetData")
        .map(|sd| sd.elems().filter(|e| e.name == "row").collect())
        .unwrap_or_default();
    for rn in row_nodes {
        if let Some(ri) = rn.attr("r").and_then(|s| s.parse::<usize>().ok()) {
            // 稀疏行补空（上限 500，避免异常文件撑爆内存）
            while rows.len() + 1 < ri && rows.len() < 500 {
                rows.push(Vec::new());
            }
        }
        let mut row: Vec<String> = Vec::new();
        for c in rn.elems().filter(|e| e.name == "c") {
            let col = c.attr("r").map(col_index).unwrap_or(row.len());
            if col > 4096 {
                continue;
            }
            let v = xlsx_cell_value(c, &shared);
            while row.len() < col {
                row.push(String::new());
            }
            row.push(v);
        }
        rows.push(row);
    }
    Ok(rows)
}

/* ---------- pptx ---------- */

fn pptx_slide_parts(path: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let (Some(pres), Some(rels)) = (
        zip_part_opt(path, "ppt/presentation.xml"),
        zip_part_opt(path, "ppt/_rels/presentation.xml.rels"),
    ) {
        if let (Ok(pres), Ok(rels)) = (parse_xml(&pres), parse_xml(&rels)) {
            let map: HashMap<String, String> = rels
                .all_desc("Relationship")
                .into_iter()
                .filter_map(|e| Some((e.attr("Id")?.to_string(), e.attr("Target")?.to_string())))
                .collect();
            if let Some(lst) = pres.find_desc("sldIdLst") {
                for id in lst.elems().filter(|e| e.name == "sldId") {
                    if let Some(t) = id.attr("r:id").and_then(|rid| map.get(rid)) {
                        out.push(resolve_part("ppt", t));
                    }
                }
            }
        }
    }
    if out.is_empty() {
        let mut names: Vec<String> = zip_names(path)
            .into_iter()
            .filter(|n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml"))
            .collect();
        names.sort_by_key(|n| {
            n.trim_start_matches("ppt/slides/slide")
                .trim_end_matches(".xml")
                .parse::<u32>()
                .unwrap_or(0)
        });
        out = names;
    }
    out
}

fn pptx_shape_title(sp: &Node) -> bool {
    sp.elem("nvSpPr")
        .and_then(|n| n.elem("nvPr"))
        .and_then(|n| n.elem("ph"))
        .and_then(|ph| ph.attr("type"))
        .map(|t| t == "title" || t == "ctrTitle")
        .unwrap_or(false)
}

/// 单页：([(是否标题, 文本)], ...) 按形状顺序
fn pptx_slide_lines(slide: &Node) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    let mut shapes: Vec<&Node> = Vec::new();
    collect_shapes(slide, &mut shapes);
    for sp in shapes {
        let title = pptx_shape_title(sp);
        let Some(tx) = sp.elem("txBody") else { continue };
        for p in tx.elems().filter(|e| e.name == "p") {
            let t = inline_text(p, false);
            if !t.trim().is_empty() {
                out.push((title, t.trim().to_string()));
            }
        }
    }
    out
}

/// 收集 sp 元素（txBody 不包含 sp，直接全树收集即可）
fn collect_shapes<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
    for e in node.elems() {
        if e.name == "sp" {
            out.push(e);
        }
        collect_shapes(e, out);
    }
}

fn pptx_blocks(path: &Path) -> Result<Vec<Block>> {
    let parts = pptx_slide_parts(path);
    if parts.is_empty() {
        bail!("pptx 中没有找到幻灯片部件（ppt/slides/slideN.xml）");
    }
    let mut blocks = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let xml = zip_part(path, part)?;
        let root = parse_xml(&xml)?;
        blocks.push(Block::Para {
            level: 2,
            text: format!("幻灯片 {}", i + 1),
        });
        for (is_title, text) in pptx_slide_lines(&root) {
            blocks.push(Block::Para {
                level: if is_title { 3 } else { 0 },
                text,
            });
        }
    }
    Ok(blocks)
}

/* ==================================================================
 *  8. ODF：odt / ods / odp
 * ================================================================== */

fn odf_root(path: &Path) -> Result<Node> {
    let xml = zip_part(path, "content.xml")?;
    parse_xml(&xml)
}

fn odf_body(root: &Node) -> Option<&Node> {
    let body = root.find_desc("body")?;
    body.elems().next()
}

/* ---------- odt ---------- */

fn odt_blocks(path: &Path) -> Result<Vec<Block>> {
    let root = odf_root(path)?;
    let body = odf_body(&root).ok_or_else(|| anyhow!("content.xml 缺少 office:body"))?;
    let mut blocks = Vec::new();
    odf_walk_text(body, &mut blocks);
    Ok(blocks)
}

fn odf_walk_text(node: &Node, out: &mut Vec<Block>) {
    for e in node.elems() {
        match e.name.as_str() {
            "h" => {
                let level = e.attr("outline-level").and_then(|v| v.parse::<u8>().ok()).unwrap_or(1);
                let t = inline_text(e, false);
                if !t.trim().is_empty() {
                    out.push(Block::Para { level: level.clamp(1, 6), text: t.trim().to_string() });
                }
            }
            "p" => {
                let t = inline_text(e, false);
                if !t.trim().is_empty() {
                    out.push(Block::Para { level: 0, text: t });
                }
            }
            "table" => {
                let rows = odf_table(e);
                if !rows.is_empty() {
                    out.push(Block::Table(rows));
                }
            }
            "list" | "list-item" | "section" | "text" | "body" => odf_walk_text(e, out),
            _ => {}
        }
    }
}

fn odf_cell_text(cell: &Node) -> String {
    let mut parts: Vec<String> = Vec::new();
    for p in cell.elems() {
        if p.name == "p" || p.name == "h" {
            parts.push(inline_text(p, false));
        }
    }
    let joined = parts.join(" ").trim().to_string();
    if !joined.is_empty() {
        return joined;
    }
    for key in ["value", "date-value", "string-value", "boolean-value", "time-value"] {
        if let Some(v) = cell.attr(key) {
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    String::new()
}

/// ODF 表格 → 二维数组（处理重复行列，去掉行尾空单元格）
fn odf_table(table: &Node) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for tr in odf_rows(table) {
        let mut values: Vec<String> = Vec::new();
        for cell in tr
            .elems()
            .filter(|e| e.name == "table-cell" || e.name == "covered-table-cell")
        {
            let v = odf_cell_text(cell);
            let rep = cell
                .attr("number-columns-repeated")
                .and_then(|x| x.parse::<usize>().ok())
                .unwrap_or(1)
                .clamp(1, 512);
            for _ in 0..rep {
                values.push(v.clone());
            }
        }
        // 行尾空单元格（LibreOffice 会把表格补齐到极大列数）直接丢弃，避免噪声
        while values.last().map(|v| v.is_empty()).unwrap_or(false) {
            values.pop();
        }
        let repeat = tr
            .attr("number-rows-repeated")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1);
        let all_empty = values.iter().all(|v| v.is_empty());
        let repeat = if all_empty { repeat.min(1) } else { repeat.min(100) };
        for _ in 0..repeat.max(1) {
            rows.push(values.clone());
        }
    }
    rows
}

fn odf_rows(table: &Node) -> Vec<&Node> {
    let mut out = Vec::new();
    for e in table.elems() {
        match e.name.as_str() {
            "table-row" => out.push(e),
            "table-header-rows" | "table-row-group" | "table-rows" => out.extend(odf_rows(e)),
            _ => {}
        }
    }
    out
}

/* ---------- ods ---------- */

/// ODS 工作表：[(名称, 二维数组)]
fn ods_sheets(path: &Path) -> Result<Vec<(String, Vec<Vec<String>>)>> {
    let root = odf_root(path)?;
    let mut out = Vec::new();
    // 只取 office:body/office:spreadsheet 下的直接表格
    let sheet_root = root.find_desc("spreadsheet");
    let iter: Vec<&Node> = match sheet_root {
        Some(s) => s.elems().filter(|e| e.name == "table").collect(),
        None => return Ok(out),
    };
    for (i, t) in iter.iter().enumerate() {
        let name = t.attr("name").unwrap_or("Sheet1").to_string();
        let rows = odf_table(t);
        out.push((if name.is_empty() { format!("Sheet{}", i + 1) } else { name }, rows));
    }
    Ok(out)
}

/* ---------- odp ---------- */

fn odp_blocks(path: &Path) -> Result<Vec<Block>> {
    let root = odf_root(path)?;
    let body = odf_body(&root).ok_or_else(|| anyhow!("content.xml 缺少 office:body"))?;
    let pages: Vec<&Node> = body.elems().filter(|e| e.name == "page").collect();
    let mut blocks = Vec::new();
    for (i, page) in pages.iter().enumerate() {
        blocks.push(Block::Para {
            level: 2,
            text: format!("幻灯片 {}", i + 1),
        });
        let mut paras: Vec<String> = Vec::new();
        collect_paragraphs(page, &mut paras);
        for t in paras {
            if !t.trim().is_empty() {
                blocks.push(Block::Para { level: 0, text: t });
            }
        }
    }
    Ok(blocks)
}

/// 收集后代中的所有段落（遇到 p / h 即停止下钻，保持顺序）
fn collect_paragraphs(node: &Node, out: &mut Vec<String>) {
    for e in node.elems() {
        match e.name.as_str() {
            "p" | "h" => out.push(inline_text(e, false)),
            _ => collect_paragraphs(e, out),
        }
    }
}

/* ==================================================================
 *  9. 文本类与 PDF
 * ================================================================== */

fn read_text_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("读取文件失败：{}", path.display()))?;
    // UTF-8 优先，失败回落 GBK（中文 Windows 常见）
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => {
            let bytes = e.into_bytes();
            let (cow, _, _) = encoding_rs::GBK.decode(&bytes);
            cow.into_owned()
        }
    })
}

/// 极简 HTML → 文本（去标签、解码常见实体、块级标签换行）
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut chars = html.char_indices().peekable();
    let mut in_tag = false;
    let mut tag = String::new();
    let mut skip = false; // script / style
    while let Some((_, c)) = chars.next() {
        if in_tag {
            if c == '>' {
                in_tag = false;
                let name = tag.trim().trim_start_matches('/').split_whitespace().next().unwrap_or("").to_ascii_lowercase();
                if matches!(name.as_str(), "script" | "style") {
                    skip = !tag.starts_with('/');
                } else if matches!(name.as_str(), "p" | "div" | "br" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                }
                tag.clear();
            } else {
                tag.push(c);
            }
            continue;
        }
        match c {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '&' => {
                // 读取实体
                let mut ent = String::new();
                let mut it = chars.clone();
                while let Some((_, ec)) = it.next() {
                    if ec == ';' || ent.len() > 10 {
                        break;
                    }
                    ent.push(ec);
                }
                let consumed = ent.len() + 1;
                let decoded = match ent.as_str() {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some(' '),
                    other => {
                        if let Some(hex) = other.strip_prefix("#x").or_else(|| other.strip_prefix("#X")) {
                            u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
                        } else if let Some(dec) = other.strip_prefix('#') {
                            dec.parse::<u32>().ok().and_then(char::from_u32)
                        } else {
                            None
                        }
                    }
                };
                if let Some(d) = decoded {
                    if !skip {
                        out.push(d);
                    }
                    for _ in 0..consumed {
                        chars.next();
                    }
                } else if !skip {
                    out.push('&');
                }
            }
            _ => {
                if !skip {
                    out.push(c);
                }
            }
        }
    }
    // 折叠多余空白
    let mut text = String::with_capacity(out.len());
    let mut blank = 0;
    for line in out.lines() {
        let l = line.trim();
        if l.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        text.push_str(l);
        text.push('\n');
    }
    text.trim().to_string()
}

/// PDF 页数（扫描 `/Type /Page`，排除 `/Pages`）
fn pdf_page_count(path: &Path) -> Option<i64> {
    let bytes = std::fs::read(path).ok()?;
    let mut n = 0i64;
    let mut i = 0usize;
    while i + 5 <= bytes.len() {
        if &bytes[i..i + 5] == b"/Page" {
            let next = bytes.get(i + 5).copied().unwrap_or(b' ');
            if next != b's' && !next.is_ascii_alphanumeric() {
                n += 1;
            }
        }
        i += 1;
    }
    Some(n)
}

/// 从 PDF 原始字节里粗略取 /Title（可能带 UTF-16BE BOM）
fn pdf_title(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let key = b"/Title";
    let pos = bytes.windows(key.len()).position(|w| w == key)?;
    let rest = &bytes[pos + key.len()..];
    let start = rest.iter().position(|b| *b == b'(')?;
    let mut end = start + 1;
    let mut depth = 1;
    while end < rest.len() && depth > 0 {
        match rest[end] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        end += 1;
    }
    let raw = &rest[start + 1..end.saturating_sub(1)];
    if raw.len() >= 2 && raw[0] == 0xFE && raw[1] == 0xFF {
        let units: Vec<u16> = raw[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return Some(String::from_utf16_lossy(&units).trim().to_string());
    }
    let s = String::from_utf8_lossy(raw).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// 用 poppler 的 pdftotext 提取 PDF 文本（无外部依赖时明确报错）
fn pdf_text(path: &Path) -> Result<String> {
    let Some(pdftotext) = detect_engine("poppler") else {
        bail!("提取 PDF 文本需要 poppler（pdftotext），请在「设置 → 依赖工具」一键安装");
    };
    // 注意：必须显式给出 `-`（stdout）；部分 pdftotext 构建（如 Git for Windows 自带的 xpdf 版）
    // 在省略输出文件时什么都不写，只靠“省略即 stdout”的约定会静默拿到空串。
    let out = command_for(&pdftotext)
        .args(["-layout", "-enc", "UTF-8", "-nopgbrk"])
        .arg(path)
        .arg("-")
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("启动 pdftotext 失败：{}", pdftotext.display()))?;
    if !out.status.success() {
        bail!(
            "pdftotext 执行失败：{}",
            crate::ctx::decode_output(&out.stderr).trim()
        );
    }
    Ok(crate::ctx::decode_output(&out.stdout))
}

/* ==================================================================
 *  10. 对外 API
 * ================================================================== */

/// 文档探测：`{ok,kind,title,pages,sheets,slides,chars,error}`
pub fn probe_document(path: &str) -> serde_json::Value {
    let p = Path::new(path);
    let kind = kind_of(p);
    if kind.is_empty() {
        return probe_json(false, None, None, None, None, None, None, Some(format!("不是支持的文档格式：{path}")));
    }
    if !p.is_file() {
        return probe_json(false, Some(kind), None, None, None, None, None, Some(format!("文件不存在：{path}")));
    }

    let title = |part: &str| -> Option<String> {
        let xml = zip_part_opt(p, part)?;
        let root = parse_xml(&xml).ok()?;
        root.find_desc("title")
            .map(|t| t.text().trim().to_string())
            .filter(|s| !s.is_empty())
    };

    let mut pages = None;
    let mut sheets = None;
    let mut slides = None;
    let mut chars = None;
    let mut title_s = None;

    let parsed: Result<()> = (|| {
        match kind {
            "docx" => {
                title_s = title("docProps/core.xml");
                if let Some(app) = zip_part_opt(p, "docProps/app.xml") {
                    if let Ok(root) = parse_xml(&app) {
                        pages = root
                            .find_desc("Pages")
                            .and_then(|n| n.text().trim().parse::<i64>().ok());
                    }
                }
                chars = Some(docx_blocks(p)?.iter().map(block_chars).sum::<i64>());
            }
            "xlsx" => {
                title_s = title("docProps/core.xml");
                let sh = xlsx_sheets(p)?;
                sheets = Some(sh.len() as i64);
            }
            "pptx" => {
                title_s = title("docProps/core.xml");
                slides = Some(pptx_slide_parts(p).len() as i64);
            }
            "odt" => {
                if let Some(meta) = zip_part_opt(p, "meta.xml") {
                    if let Ok(root) = parse_xml(&meta) {
                        pages = root
                            .find_desc("document-statistic")
                            .and_then(|s| s.attr("page-count"))
                            .and_then(|v| v.parse::<i64>().ok());
                        title_s = root
                            .find_desc("title")
                            .map(|t| t.text().trim().to_string())
                            .filter(|s| !s.is_empty());
                    }
                }
                chars = Some(odt_blocks(p)?.iter().map(block_chars).sum::<i64>());
            }
            "ods" => {
                let sh = ods_sheets(p)?;
                sheets = Some(sh.len() as i64);
                chars = Some(
                    sh.iter()
                        .flat_map(|(_, rows)| rows.iter())
                        .flat_map(|r| r.iter())
                        .map(|c| c.chars().count() as i64)
                        .sum(),
                );
            }
            "odp" => {
                let root = odf_root(p)?;
                slides = Some(root.count_desc("page") as i64);
                if let Some(meta) = zip_part_opt(p, "meta.xml") {
                    if let Ok(m) = parse_xml(&meta) {
                        title_s = m
                            .find_desc("title")
                            .map(|t| t.text().trim().to_string())
                            .filter(|s| !s.is_empty());
                    }
                }
            }
            "pdf" => {
                pages = pdf_page_count(p);
                title_s = pdf_title(p);
            }
            _ => {
                // 纯文本类
                let text = read_text_file(p)?;
                let text = if kind == "html" { html_to_text(&text) } else { text };
                chars = Some(text.chars().count() as i64);
            }
        }
        Ok(())
    })();

    match parsed {
        Ok(()) => {
            if chars.is_none() && matches!(kind, "xlsx" | "pptx" | "txt" | "md" | "csv" | "tsv") {
                chars = extract_text(p).ok().map(|t| t.chars().count() as i64);
            }
            probe_json(true, Some(kind), title_s, pages, sheets, slides, chars, None)
        }
        Err(e) => probe_json(false, Some(kind), title_s, pages, sheets, slides, chars, Some(e.to_string())),
    }
}

fn block_chars(b: &Block) -> i64 {
    match b {
        Block::Para { text, .. } => text.chars().count() as i64,
        Block::Table(rows) => rows
            .iter()
            .flat_map(|r| r.iter())
            .map(|c| c.chars().count() as i64)
            .sum(),
    }
}

#[allow(clippy::too_many_arguments)]
fn probe_json(
    ok: bool,
    kind: Option<&str>,
    title: Option<String>,
    pages: Option<i64>,
    sheets: Option<i64>,
    slides: Option<i64>,
    chars: Option<i64>,
    error: Option<String>,
) -> serde_json::Value {
    json!({
        "ok": ok,
        "kind": kind,
        "title": title,
        "pages": pages,
        "sheets": sheets,
        "slides": slides,
        "chars": chars,
        "error": error,
    })
}

/// 原生解析 + 渲染：docx/xlsx/pptx/odt/ods/odp → txt/md/html/csv（纯 Rust）
pub fn native_convert(src: &Path, out: &Path, target: &str) -> Result<()> {
    let kind = kind_of(src);
    let t = normalize_target(target);
    if !native_supports(kind, &t) {
        bail!("原生引擎不支持 {kind} → {t}");
    }
    if !src.is_file() {
        bail!("输入文件不存在：{}", src.display());
    }
    if let Some(dir) = out.parent() {
        if !dir.as_os_str().is_empty() {
            ensure_dir(dir)?;
        }
    }
    let title = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "document".into());

    match kind {
        "docx" | "pptx" | "odt" | "odp" => {
            let blocks = match kind {
                "docx" => docx_blocks(src)?,
                "pptx" => pptx_blocks(src)?,
                "odt" => odt_blocks(src)?,
                _ => odp_blocks(src)?,
            };
            let content = match t.as_str() {
                "md" => blocks_to_markdown(&blocks),
                "html" => blocks_to_html(&blocks, &title),
                _ => blocks_to_text(&blocks),
            };
            write_output(out, &content, false)
        }
        "xlsx" | "ods" => {
            let sheets = if kind == "xlsx" {
                let list = xlsx_sheets(src)?;
                let (name, part) = list
                    .first()
                    .cloned()
                    .ok_or_else(|| anyhow!("工作簿中没有工作表"))?;
                vec![(name, xlsx_rows(src, &part)?)]
            } else {
                let list = ods_sheets(src)?;
                if list.is_empty() {
                    bail!("电子表格中没有工作表");
                }
                list
            };
            let (sheet_name, rows) = &sheets[0];
            let content = match t.as_str() {
                "csv" => grid_to_csv(rows, ','),
                "html" => html_document(
                    &format!("{title} - {sheet_name}"),
                    &html_table(rows),
                ),
                _ => grid_to_text(rows),
            };
            let bom = t == "csv" && !content.is_ascii();
            write_output(out, &content, bom)
        }
        // 纯文本类：直接读取（html 剥标签）
        _ => {
            let text = read_text_file(src)?;
            let text = if kind == "html" { html_to_text(&text) } else { text };
            write_output(out, &text, false)
        }
    }
}

fn grid_field(s: &str, delim: char) -> String {
    if s.contains(delim) || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// 二维数组 → CSV（CRLF 行结束，Windows/Excel 友好）
pub fn grid_to_csv(rows: &[Vec<String>], delim: char) -> String {
    let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut s = String::new();
    for r in rows {
        let line: Vec<String> = (0..width)
            .map(|c| grid_field(r.get(c).map(|x| x.as_str()).unwrap_or(""), delim).to_string())
            .collect();
        s.push_str(&line.join(&delim.to_string()));
        s.push_str("\r\n");
    }
    s
}

fn grid_to_text(rows: &[Vec<String>]) -> String {
    let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut s = String::new();
    for r in rows {
        let line: Vec<String> = (0..width)
            .map(|c| r.get(c).map(|x| x.replace(['\n', '\r'], " ")).unwrap_or_default())
            .collect();
        s.push_str(&line.join("\t"));
        s.push('\n');
    }
    s
}

fn write_output(path: &Path, content: &str, bom: bool) -> Result<()> {
    let mut f = std::fs::File::create(path)
        .with_context(|| format!("创建输出文件失败：{}", path.display()))?;
    if bom {
        f.write_all(&[0xEF, 0xBB, 0xBF])?;
    }
    f.write_all(content.as_bytes())?;
    f.flush()?;
    Ok(())
}

/// 提取文档纯文本（原生可解析的走原生，pdf 走 poppler）
pub fn extract_text(path: &Path) -> Result<String> {
    let kind = kind_of(path);
    match kind {
        "docx" => Ok(blocks_to_text(&docx_blocks(path)?)),
        "pptx" => Ok(blocks_to_text(&pptx_blocks(path)?)),
        "odt" => Ok(blocks_to_text(&odt_blocks(path)?)),
        "odp" => Ok(blocks_to_text(&odp_blocks(path)?)),
        "xlsx" => {
            let list = xlsx_sheets(path)?;
            let (_, part) = list.first().ok_or_else(|| anyhow!("工作簿中没有工作表"))?;
            Ok(grid_to_text(&xlsx_rows(path, part)?))
        }
        "ods" => {
            let list = ods_sheets(path)?;
            let (_, rows) = list.first().ok_or_else(|| anyhow!("电子表格中没有工作表"))?;
            Ok(grid_to_text(rows))
        }
        "pdf" => pdf_text(path),
        "txt" | "md" | "csv" | "tsv" => read_text_file(path),
        "html" => Ok(html_to_text(&read_text_file(path)?)),
        "doc" | "xls" | "ppt" => bail!(
            "旧版二进制格式 .{kind} 无法直接解析：请先用 Office / WPS 另存为 .{docx} 或转存为 rtf / html",
            docx = if kind == "doc" { "docx" } else if kind == "xls" { "xlsx" } else { "pptx" }
        ),
        "rtf" | "epub" => bail!(
            "提取 .{kind} 文本需要 pandoc，请在「设置 → 依赖工具」一键安装"
        ),
        _ => bail!("不是支持的文档格式：{}", path.display()),
    }
}

/* ==================================================================
 *  11. pandoc / poppler 外部引擎
 * ================================================================== */

/// 构建 pandoc 命令行（纯函数，便于单测与日志）
pub fn build_pandoc_args(src: &Path, out: &Path, target: &str) -> Vec<String> {
    let kind = kind_of(src);
    let t = normalize_target(target);
    let writer = pandoc_writer(&t).map(|s| s.to_string()).unwrap_or(t);
    let mut args: Vec<String> = Vec::new();
    if let Some(reader) = pandoc_reader(kind) {
        args.push("--from".into());
        args.push(reader.into());
    }
    args.push("--to".into());
    args.push(writer.clone());
    match writer.as_str() {
        "plain" | "gfm" | "markdown" | "rst" | "org" | "html5" | "html" | "latex" | "csv" | "json" => {
            args.push("--wrap=none".into());
        }
        _ => {}
    }
    match writer.as_str() {
        "html5" | "html" | "odt" | "docx" | "epub" | "rtf" | "pptx" => args.push("--standalone".into()),
        _ => {}
    }
    args.push("-o".into());
    args.push(out.to_string_lossy().to_string());
    args.push(src.to_string_lossy().to_string());
    args
}

/// 在 PATH 中查找可执行文件（外部引擎回退用）
fn which_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let full = dir.join(name);
        if full.is_file() {
            return Some(full);
        }
    }
    None
}

fn run_engine(program: &Path, args: &[String]) -> Result<()> {
    let out = command_for(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("启动外部引擎失败：{}", program.display()))?;
    if !out.status.success() {
        let tail: String = crate::ctx::decode_output(&out.stderr)
            .lines()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "{} 执行失败（退出码 {:?}）：{}",
            program.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(),
            out.status.code(),
            if tail.trim().is_empty() { "无错误输出" } else { tail.trim() }
        );
    }
    Ok(())
}

/// 外部引擎转换；返回实际执行的命令行（便于测试与日志）
pub fn external_convert(ctx: &Ctx, src: &Path, out: &Path, target: &str) -> Result<Vec<String>> {
    let kind = kind_of(src);
    let t = normalize_target(target);
    if !src.is_file() {
        bail!("输入文件不存在：{}", src.display());
    }
    if let Some(dir) = out.parent() {
        if !dir.as_os_str().is_empty() {
            ensure_dir(dir)?;
        }
    }
    match route_for(src, &t) {
        Route::Native => {
            native_convert(src, out, &t)?;
            return Ok(vec!["<native>".into(), src.display().to_string(), out.display().to_string()]);
        }
        Route::Unsupported(msg) => bail!("{msg}"),
        Route::PopplerPdf => return poppler_convert(ctx, src, out, &t),
        Route::Pandoc => {}
    }

    let Some(pandoc) = engine_path(ctx, "pandoc") else {
        bail!("未找到 pandoc 文档转换引擎（{kind} → {t} 需要它），请在「设置 → 依赖工具」一键安装");
    };
    let args = build_pandoc_args(src, out, &t);
    run_engine(&pandoc, &args)?;
    check_output(out, &pandoc, &args)?;
    Ok(args)
}

fn poppler_convert(ctx: &Ctx, src: &Path, out: &Path, target: &str) -> Result<Vec<String>> {
    let Some(pdftotext) = engine_path(ctx, "poppler") else {
        bail!("未找到 poppler（pdftotext / pdftoppm），请在「设置 → 依赖工具」一键安装");
    };
    let dir = pdftotext.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    match target {
        "txt" => {
            let args: Vec<String> = vec![
                "-layout".into(),
                "-enc".into(),
                "UTF-8".into(),
                "-nopgbrk".into(),
                src.to_string_lossy().to_string(),
                out.to_string_lossy().to_string(),
            ];
            run_engine(&pdftotext, &args)?;
            // 扫描件（纯图片、无文本层）会得到一个空文件：这是「成功执行但没内容」，
            // 直接说清楚原因，比让用户以为程序坏了要好
            if std::fs::metadata(out).map(|m| m.len() == 0).unwrap_or(false) {
                bail!(
                    "pdftotext 输出为空：{} 可能没有文本层（扫描件 / 纯图片 PDF）。可改为导出 PNG 后自行 OCR，或先用 Office / 在线工具做文字识别",
                    src.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()
                );
            }
            check_output(out, &pdftotext, &args)?;
            Ok(args)
        }
        "png" => {
            // 受管目录里 pdftoppm 与 pdftotext 同目录；PATH 安装时按名字查找
            let name = if cfg!(windows) { "pdftoppm.exe" } else { "pdftoppm" };
            let sibling = dir.join(name);
            let program = if sibling.is_file() {
                sibling
            } else if let Some(p) = which_on_path(name) {
                p
            } else {
                bail!("未找到 pdftoppm（PDF 转 PNG 需要 poppler 完整包），请在「设置 → 依赖工具」重新安装 poppler");
            };
            let prefix = out.with_extension("");
            let args: Vec<String> = vec![
                "-r".into(),
                "150".into(),
                "-png".into(),
                src.to_string_lossy().to_string(),
                prefix.to_string_lossy().to_string(),
            ];
            run_engine(&program, &args)?;
            finalize_pdf_png(out, &program, &args)?;
            Ok(args)
        }
        other => bail!("poppler 不支持输出 {other}（PDF 仅支持 txt / png）"),
    }
}

/// pdftoppm 输出 `前缀-N.png`：单页直接改名为目标文件；
/// 多页时第 1 页命名为目标文件，其余保留 `前缀-2.png`、`前缀-3.png` …（不丢页）
fn finalize_pdf_png(out: &Path, program: &Path, args: &[String]) -> Result<()> {
    if out.is_file() {
        return Ok(());
    }
    let dir = out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let stem = out.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let mut pages: Vec<(u32, PathBuf)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(rest) = name.strip_prefix(&format!("{stem}-")) {
                if let Some(num) = rest.strip_suffix(".png").and_then(|n| n.parse::<u32>().ok()) {
                    pages.push((num, e.path()));
                }
            }
        }
    }
    if pages.is_empty() {
        check_output(out, program, args)?; // 触发统一报错
        bail!("pdftoppm 未生成任何 PNG 文件");
    }
    pages.sort();
    std::fs::rename(&pages[0].1, out)
        .with_context(|| format!("重命名 PDF 首页图片失败：{}", pages[0].1.display()))?;
    Ok(())
}

fn check_output(out: &Path, program: &Path, args: &[String]) -> Result<()> {
    match std::fs::metadata(out) {
        Ok(m) if m.len() > 0 => Ok(()),
        Ok(_) => bail!(
            "{} 未生成有效输出（文件为空）：{}",
            program.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(),
            out.display()
        ),
        Err(_) => bail!(
            "转换未生成输出文件：{}（命令：{} {}）",
            out.display(),
            program.display(),
            args.join(" ")
        ),
    }
}

/* ==================================================================
 *  测试
 * ================================================================== */

#[cfg(test)]
pub mod testing {
    //! 测试基础设施：现场生成真实的 OOXML / ODF / PDF 文件（不是 mock，是真能被 zip 解析的容器）

    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    /// 独立临时目录（进程内唯一）
    pub fn tmp_dir(tag: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir()
            .join("umidl_docs_tests")
            .join(format!("{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// 用 zip crate 写出真实压缩包（模拟 OOXML / ODF 的「zip + XML 部件」结构）
    pub fn write_zip(path: &Path, files: &[(&str, &str)]) -> std::io::Result<()> {
        let f = std::fs::File::create(path)?;
        let mut zw = zip::ZipWriter::new(f);
        for (name, body) in files {
            zw.start_file(
                *name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )?;
            zw.write_all(body.as_bytes())?;
        }
        zw.finish()?;
        Ok(())
    }

    const CONTENT_TYPES_DOCX: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>
<Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>
</Types>"#;

    /// 生成最小但结构合法的 docx（含标题样式、中英文、`xml:space`、实体、表格）
    pub fn min_docx(path: &Path) -> std::io::Result<()> {
        let document = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>季度总结</w:t></w:r></w:p>
<w:p><w:r><w:t>第一段：销售增长 12%</w:t><w:t xml:space="preserve"> 同比 +3%</w:t></w:r><w:r><w:t>（数据来自 ERP）</w:t></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="2"/></w:pPr><w:r><w:t>第二节 明细</w:t></w:r></w:p>
<w:p><w:r><w:t>特殊字符 &amp; 中文 &#x6D4B;&#x8BD5;</w:t></w:r></w:p>
<w:p><w:r><w:tab/><w:t>制表符与换行</w:t><w:br/><w:t>第二行</w:t></w:r></w:p>
<w:tbl>
<w:tr><w:tc><w:p><w:r><w:t>姓名</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>销售额</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>备注</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>张三</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>1234</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>华东区 | 主力</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>李四</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>987.5</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>&lt;新增&gt;</w:t></w:r></w:p></w:tc></w:tr>
</w:tbl>
<w:p><w:r><w:t>结束语</w:t></w:r></w:p>
</w:body>
</w:document>"#;
        // 样式：Heading1（英文名）与 styleId=2（名 name="heading 2"）两条路径都覆盖
        let styles = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/></w:style>
<w:style w:type="paragraph" w:styleId="2"><w:name w:val="heading 2"/></w:style>
</w:styles>"#;
        let core = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:title>季度总结文档</dc:title><dc:creator>umi</dc:creator></cp:coreProperties>"#;
        let app = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">
<Pages>3</Pages><Words>42</Words></Properties>"#;
        let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;
        write_zip(
            path,
            &[
                ("[Content_Types].xml", CONTENT_TYPES_DOCX),
                ("_rels/.rels", rels),
                ("word/document.xml", document),
                ("word/styles.xml", styles),
                ("docProps/core.xml", core),
                ("docProps/app.xml", app),
            ],
        )
    }

    /// 生成最小但结构合法的 xlsx（sharedStrings + inlineStr + 数字 + 公式结果 + 多表）
    pub fn min_xlsx(path: &Path) -> std::io::Result<()> {
        let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
<Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>
</Types>"#;
        let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#;
        let workbook = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets><sheet name="销售数据" sheetId="1" r:id="rId1"/><sheet name="汇总" sheetId="2" r:id="rId2"/></sheets>
</workbook>"#;
        let wb_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/>
</Relationships>"#;
        let shared = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="5" uniqueCount="5">
<si><t>姓名</t></si>
<si><t>销售额</t></si>
<si><t>张三</t></si>
<si><t>李四, 副总</t></si>
<si><r><rPr/><t>富</t></r><r><t>文本</t></r><rPh sb="0" eb="1"><t>ふ</t></rPh></si>
</sst>"#;
        let sheet1 = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData>
<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1" t="inlineStr"><is><t>备注</t></is></c></row>
<row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2"><v>1234.5</v></c><c r="C2" t="s"><v>3</v></c></row>
<row r="3"><c r="A3" t="s"><v>4</v></c><c r="B3" t="str"><f>SUM(B2:B2)</f><v>1234.5</v></c><c r="C3" t="b"><v>1</v></c></row>
</sheetData>
</worksheet>"#;
        let sheet2 = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>第二张表</t></is></c><c r="B1"><v>7</v></c></row></sheetData>
</worksheet>"#;
        let core = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:title>销售台账</dc:title></cp:coreProperties>"#;
        write_zip(
            path,
            &[
                ("[Content_Types].xml", content_types),
                ("_rels/.rels", rels),
                ("xl/workbook.xml", workbook),
                ("xl/_rels/workbook.xml.rels", wb_rels),
                ("xl/sharedStrings.xml", shared),
                ("xl/worksheets/sheet1.xml", sheet1),
                ("xl/worksheets/sheet2.xml", sheet2),
                ("docProps/core.xml", core),
            ],
        )
    }

    /// 生成最小 pptx（2 页，含标题占位符与正文占位符）
    pub fn min_pptx(path: &Path) -> std::io::Result<()> {
        let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
<Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
<Override PartName="/ppt/slides/slide2.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>
</Types>"#;
        let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"#;
        let presentation = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<p:sldIdLst><p:sldId id="256" r:id="rId2"/><p:sldId id="257" r:id="rId3"/></p:sldIdLst>
</p:presentation>"#;
        let pres_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide2.xml"/>
</Relationships>"#;
        let slide1 = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<p:cSld><p:spTree>
<p:sp><p:nvSpPr><p:cNvPr id="2" name="标题 1"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>
<p:txBody><a:bodyPr/><a:p><a:r><a:t>一季度回顾</a:t></a:r></a:p></p:txBody></p:sp>
<p:sp><p:nvSpPr><p:cNvPr id="3" name="内容占位符 2"/><p:cNvSpPr/><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr>
<p:txBody><a:bodyPr/><a:p><a:r><a:t>营收同比增长 20%</a:t></a:r></a:p><a:p><a:r><a:t>新增客户 18 家</a:t></a:r></a:p></p:txBody></p:sp>
</p:spTree></p:cSld></p:sld>"#;
        let slide2 = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<p:cSld><p:spTree>
<p:sp><p:nvSpPr><p:cNvPr id="2" name="标题 1"/><p:cNvSpPr/><p:nvPr><p:ph type="ctrTitle"/></p:nvPr></p:nvSpPr>
<p:txBody><a:bodyPr/><a:p><a:r><a:t>谢谢观看</a:t></a:r></a:p></p:txBody></p:sp>
</p:spTree></p:cSld></p:sld>"#;
        let core = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:title>季度汇报</dc:title></cp:coreProperties>"#;
        write_zip(
            path,
            &[
                ("[Content_Types].xml", content_types),
                ("_rels/.rels", rels),
                ("ppt/presentation.xml", presentation),
                ("ppt/_rels/presentation.xml.rels", pres_rels),
                ("ppt/slides/slide1.xml", slide1),
                ("ppt/slides/slide2.xml", slide2),
                ("docProps/core.xml", core),
            ],
        )
    }

    /// 生成最小 odt（标题、段落、实体、表格）
    pub fn min_odt(path: &Path) -> std::io::Result<()> {
        let content = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" office:version="1.2">
<office:body><office:text>
<text:h text:outline-level="1">第一章 概述</text:h>
<text:p>正文一：中文段落<text:span>（重点）</text:span>与 空格<s/>分隔</text:p>
<text:p>特殊字符 &amp; 符号</text:p>
<table:table>
<table:table-row><table:table-cell><text:p>项目</text:p></table:table-cell><table:table-cell><text:p>数量</text:p></table:table-cell></table:table-row>
<table:table-row><table:table-cell><text:p>苹果</text:p></table:table-cell><table:table-cell><text:p>12</text:p></table:table-cell></table:table-row>
</table:table>
</office:text></office:body></office:document-content>"#;
        let meta = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-meta xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0">
<office:meta><dc:title>中文报告</dc:title><meta:document-statistic meta:page-count="2" meta:table-count="1"/></office:meta>
</office:document-meta>"#;
        write_zip(
            path,
            &[
                ("mimetype", "application/vnd.oasis.opendocument.text"),
                ("content.xml", content),
                ("meta.xml", meta),
            ],
        )
    }

    /// 生成最小 ods（两张表、数字/字符串/重复列）
    pub fn min_ods(path: &Path) -> std::io::Result<()> {
        let content = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" office:version="1.2">
<office:body><office:spreadsheet>
<table:table table:name="库存">
<table:table-row><table:table-cell office:value-type="string"><text:p>商品</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>数量</text:p></table:table-cell></table:table-row>
<table:table-row><table:table-cell office:value-type="string"><text:p>苹果</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="12"><text:p>12</text:p></table:table-cell></table:table-row>
<table:table-row><table:table-cell office:value-type="string"><text:p>香蕉, 青</text:p></table:table-cell><table:table-cell table:number-columns-repeated="3"/></table:table-row>
</table:table>
<table:table table:name="汇总">
<table:table-row><table:table-cell office:value-type="string"><text:p>合计</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="12.5"/></table:table-row>
</table:table>
</office:spreadsheet></office:body></office:document-content>"#;
        write_zip(
            path,
            &[
                ("mimetype", "application/vnd.oasis.opendocument.spreadsheet"),
                ("content.xml", content),
            ],
        )
    }

    /// 生成最小 odp（2 页，draw:text-box 内的段落）
    pub fn min_odp(path: &Path) -> std::io::Result<()> {
        let content = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" office:version="1.2">
<office:body><office:presentation>
<draw:page draw:name="第一页">
<draw:frame><draw:text-box><text:p>年度目标</text:p><text:p>增长 30%</text:p></draw:text-box></draw:frame>
</draw:page>
<draw:page draw:name="第二页">
<draw:frame><draw:text-box><text:p>谢谢</text:p></draw:text-box></draw:frame>
</draw:page>
</office:presentation></office:body></office:document-content>"#;
        write_zip(
            path,
            &[
                ("mimetype", "application/vnd.oasis.opendocument.presentation"),
                ("content.xml", content),
            ],
        )
    }

    /// 手写最小可解析 PDF（ASCII 文本，Helvetica 字体，交叉引用表偏移量正确）
    pub fn min_pdf(path: &Path, text: &str) -> std::io::Result<()> {
        let safe: String = text.chars().filter(|c| c.is_ascii() && !"()\\".contains(*c)).collect();
        let stream = format!("BT /F1 24 Tf 72 700 Td ({safe}) Tj ET");
        let objects: Vec<String> = vec![
            "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".into(),
            "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".into(),
            "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>\nendobj\n".into(),
            "4 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n".into(),
            format!("5 0 obj\n<< /Length {} >>\nstream\n{}\nendstream\nendobj\n", stream.len(), stream),
        ];
        let mut out = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for o in &objects {
            offsets.push(out.len());
            out.push_str(o);
        }
        let xref_pos = out.len();
        out.push_str(&format!("xref\n0 {}\n", objects.len() + 1));
        out.push_str("0000000000 65535 f \n");
        for off in &offsets {
            out.push_str(&format!("{:010} 00000 n \n", off));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            objects.len() + 1,
            xref_pos
        ));
        std::fs::write(path, out)
    }

    /// 测试用 ctx：全部路径落在临时目录，且不带任何外部工具
    pub fn test_ctx(base: &Path) -> crate::ctx::Ctx {
        let dirs = crate::ctx::AppDirs {
            data: base.join("data"),
            bin: base.join("bin"),
            models: base.join("models"),
            cache: base.join("cache"),
            downloads: base.join("downloads"),
            db_file: base.join("umi.db"),
            settings_file: base.join("settings.json"),
        };
        crate::ctx::Ctx::new(dirs, crate::ctx::ToolPaths::default(), crate::models::AppSettings::default())
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    /* ---------- 扩展名与路由 ---------- */

    #[test]
    fn document_path_detection() {
        for p in [
            "a.docx", "a.XLSX", "b.pptx", "c.odt", "d.ods", "e.odp", "f.pdf", "g.rtf", "h.epub", "i.md",
            "j.HTML", "k.txt", "l.csv", "m.doc", "n.xls", "o.ppt", "p.rst",
        ] {
            assert!(is_document_path(p), "{p} 应识别为文档");
        }
        for p in ["video.mp4", "song.mp3", "image.png", "archive.zip", "noext", "x.tar.gz"] {
            assert!(!is_document_path(p), "{p} 不应识别为文档");
        }
    }

    #[test]
    fn routing_priority_native_then_pandoc_then_poppler() {
        assert_eq!(route_for(Path::new("a.docx"), "md"), Route::Native);
        assert_eq!(route_for(Path::new("a.xlsx"), "csv"), Route::Native);
        assert_eq!(route_for(Path::new("a.odt"), "txt"), Route::Native);
        // 原生覆盖不了的组合 → pandoc
        assert_eq!(route_for(Path::new("a.docx"), "docx"), Route::Pandoc);
        assert_eq!(route_for(Path::new("a.md"), "docx"), Route::Pandoc);
        assert_eq!(route_for(Path::new("a.rtf"), "md"), Route::Pandoc);
        // pdf 走 poppler
        assert_eq!(route_for(Path::new("a.pdf"), "txt"), Route::PopplerPdf);
        assert_eq!(route_for(Path::new("a.pdf"), "png"), Route::PopplerPdf);
        // 明确报错
        match route_for(Path::new("a.pdf"), "docx") {
            Route::Unsupported(m) => assert!(m.contains("PDF")),
            other => panic!("pdf→docx 应不支持，得到 {other:?}"),
        }
        match route_for(Path::new("a.doc"), "md") {
            Route::Unsupported(m) => assert!(m.contains("旧版二进制"), "消息应说明原因：{m}"),
            other => panic!("doc 应不支持，得到 {other:?}"),
        }
        match route_for(Path::new("a.docx"), "mp4") {
            Route::Unsupported(m) => assert!(m.contains("媒体格式"), "消息应说明原因：{m}"),
            other => panic!("docx→mp4 应不支持，得到 {other:?}"),
        }
    }

    /* ---------- docx ---------- */

    #[test]
    fn docx_to_markdown_headings_text_table() {
        let dir = tmp_dir("docx_md");
        let src = dir.join("季度总结.docx");
        min_docx(&src).unwrap();
        let out = dir.join("季度总结.md");
        native_convert(&src, &out, "md").unwrap();
        let md = std::fs::read_to_string(&out).unwrap();

        assert!(md.starts_with("# 季度总结\n"), "一级标题（styles.xml 映射）：{md}");
        assert!(md.contains("\n## 第二节 明细\n"), "二级标题：{md}");
        // xml:space="preserve" 的空格必须保留，且多个 run 的字要无缝拼接
        assert!(md.contains("第一段：销售增长 12% 同比 +3%（数据来自 ERP）"), "段落拼接：{md}");
        // 实体与字符引用
        assert!(md.contains("特殊字符 & 中文 测试"), "实体/字符引用：{md}");
        // 表格（含 | 转义）
        assert!(md.contains("| 姓名 | 销售额 | 备注 |"), "表头：{md}");
        assert!(md.contains("| --- | --- | --- |"), "表格分隔行：{md}");
        assert!(md.contains("| 张三 | 1234 | 华东区 \\| 主力 |"), "表格内容与转义：{md}");
        assert!(md.contains("| 李四 | 987.5 | <新增> |"), "表格内容：{md}");
        assert!(md.trim_end().ends_with("结束语"), "结尾段落：{md}");
        // 制表符与换行
        assert!(md.contains("制表符与换行<br>第二行"), "w:tab / w:br：{md}");
    }

    #[test]
    fn docx_to_txt_and_html() {
        let dir = tmp_dir("docx_txt_html");
        let src = dir.join("报告.docx");
        min_docx(&src).unwrap();

        let txt_out = dir.join("报告.txt");
        native_convert(&src, &txt_out, "txt").unwrap();
        let txt = std::fs::read_to_string(&txt_out).unwrap();
        assert!(txt.contains("季度总结"), "{txt}");
        assert!(txt.contains("姓名\t销售额\t备注"), "txt 表格用制表符：{txt}");
        assert!(!txt.contains("# "), "txt 不应出现 Markdown 标记");

        let html_out = dir.join("报告.html");
        native_convert(&src, &html_out, "html").unwrap();
        let html = std::fs::read_to_string(&html_out).unwrap();
        assert!(html.contains("<h1>季度总结</h1>"), "{html}");
        assert!(html.contains("<title>报告</title>"), "标题用文件名：{html}");
        assert!(html.contains("<th>姓名</th>"), "{html}");
        assert!(html.contains("<td>张三</td>"), "{html}");
        assert!(html.contains("&lt;新增&gt;"), "HTML 转义：{html}");
    }

    #[test]
    fn docx_layout_heading_only_by_filename_style_id() {
        // styles.xml 缺失时，styleId 形如 HeadingN / N 也要能识别标题
        let dir = tmp_dir("docx_styles");
        let src = dir.join("nostyles.docx");
        let doc = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>无样式表标题</w:t></w:r></w:p>
<w:p><w:r><w:t>正文</w:t></w:r></w:p>
</w:body></w:document>"#;
        write_zip(
            &src,
            &[
                ("[Content_Types].xml", r#"<Types/>"#),
                ("word/document.xml", doc),
            ],
        )
        .unwrap();
        let md = blocks_to_markdown(&docx_blocks(&src).unwrap());
        assert!(md.contains("## 无样式表标题"), "{md}");
    }

    /* ---------- xlsx ---------- */

    #[test]
    fn xlsx_to_csv_shared_strings_numbers_and_quoting() {
        let dir = tmp_dir("xlsx_csv");
        let src = dir.join("销售台账.xlsx");
        min_xlsx(&src).unwrap();
        let out = dir.join("销售台账.csv");
        native_convert(&src, &out, "csv").unwrap();

        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF], "含中文的 CSV 应带 UTF-8 BOM（Excel 友好）");
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(
            text,
            "\u{feff}姓名,销售额,备注\r\n张三,1234.5,\"李四, 副总\"\r\n富文本,1234.5,TRUE\r\n",
            "CSV 内容（含逗号字段引号、inlineStr、共享字符串富文本、布尔、公式结果）"
        );
    }

    #[test]
    fn xlsx_to_txt_and_html_first_sheet_only() {
        let dir = tmp_dir("xlsx_txt");
        let src = dir.join("台账.xlsx");
        min_xlsx(&src).unwrap();

        let txt_out = dir.join("台账.txt");
        native_convert(&src, &txt_out, "txt").unwrap();
        let txt = std::fs::read_to_string(&txt_out).unwrap();
        assert!(txt.starts_with("姓名\t销售额\t备注\n"), "{txt}");
        assert!(txt.contains("张三\t1234.5\t李四, 副总"), "{txt}");
        assert!(!txt.contains("第二张表"), "默认只导出第一张工作表：{txt}");

        let html_out = dir.join("台账.html");
        native_convert(&src, &html_out, "html").unwrap();
        let html = std::fs::read_to_string(&html_out).unwrap();
        assert!(html.contains("<th>姓名</th>"), "{html}");
        assert!(html.contains("<td>张三</td>"), "{html}");
        assert!(html.contains("销售数据"), "HTML 标题含工作表名：{html}");
    }

    #[test]
    fn xlsx_rejects_csv_target_for_docx_engine() {
        let dir = tmp_dir("xlsx_bad");
        let src = dir.join("x.xlsx");
        min_xlsx(&src).unwrap();
        // xlsx → md 不在原生范围（表格请用 csv/txt/html）
        assert!(!native_supports("xlsx", "md"));
        let err = native_convert(&src, &dir.join("x.md"), "md").unwrap_err().to_string();
        assert!(err.contains("原生引擎不支持"), "{err}");
    }

    /* ---------- pptx ---------- */

    #[test]
    fn pptx_to_markdown_slide_order_and_titles() {
        let dir = tmp_dir("pptx_md");
        let src = dir.join("季度汇报.pptx");
        min_pptx(&src).unwrap();
        let out = dir.join("季度汇报.md");
        native_convert(&src, &out, "md").unwrap();
        let md = std::fs::read_to_string(&out).unwrap();

        assert!(md.starts_with("## 幻灯片 1\n"), "{md}");
        assert!(md.contains("### 一季度回顾"), "标题占位符 → 3 级标题：{md}");
        assert!(md.contains("营收同比增长 20%"), "{md}");
        assert!(md.contains("新增客户 18 家"), "{md}");
        let p2 = md.find("## 幻灯片 2").expect("第二页");
        assert!(p2 > md.find("营收同比增长 20%").unwrap(), "应按 slide 顺序输出：{md}");
        assert!(md.contains("### 谢谢观看"), "ctrTitle 也要识别：{md}");
    }

    #[test]
    fn pptx_to_txt_flattens_headings() {
        let dir = tmp_dir("pptx_txt");
        let src = dir.join("d.pptx");
        min_pptx(&src).unwrap();
        let out = dir.join("d.txt");
        native_convert(&src, &out, "txt").unwrap();
        let txt = std::fs::read_to_string(&out).unwrap();
        assert!(txt.contains("幻灯片 1"), "{txt}");
        assert!(txt.contains("一季度回顾"), "{txt}");
        assert!(!txt.contains("#"), "txt 无 Markdown 标记：{txt}");
    }

    /* ---------- ODF ---------- */

    #[test]
    fn odt_to_markdown_heading_span_and_table() {
        let dir = tmp_dir("odt_md");
        let src = dir.join("中文报告.odt");
        min_odt(&src).unwrap();
        let out = dir.join("中文报告.md");
        native_convert(&src, &out, "md").unwrap();
        let md = std::fs::read_to_string(&out).unwrap();

        assert!(md.starts_with("# 第一章 概述\n"), "{md}");
        assert!(md.contains("正文一：中文段落（重点）与 空格 分隔"), "text:span / text:s：{md}");
        assert!(md.contains("特殊字符 & 符号"), "{md}");
        assert!(md.contains("| 项目 | 数量 |"), "{md}");
        assert!(md.contains("| 苹果 | 12 |"), "{md}");
    }

    #[test]
    fn odt_to_txt_and_html() {
        let dir = tmp_dir("odt_txt");
        let src = dir.join("中文报告.odt");
        min_odt(&src).unwrap();
        let html_out = dir.join("r.html");
        native_convert(&src, &html_out, "html").unwrap();
        let html = std::fs::read_to_string(&html_out).unwrap();
        assert!(html.contains("<h1>第一章 概述</h1>"), "{html}");
        assert!(html.contains("<td>苹果</td>"), "{html}");

        let txt_out = dir.join("r.txt");
        native_convert(&src, &txt_out, "txt").unwrap();
        let txt = std::fs::read_to_string(&txt_out).unwrap();
        assert!(txt.contains("第一章 概述\n"), "{txt}");
    }

    #[test]
    fn ods_to_csv_handles_value_types_and_repeats() {
        let dir = tmp_dir("ods_csv");
        let src = dir.join("库存.ods");
        min_ods(&src).unwrap();
        let out = dir.join("库存.csv");
        native_convert(&src, &out, "csv").unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert_eq!(
            text,
            "\u{feff}商品,数量\r\n苹果,12\r\n\"香蕉, 青\",\r\n",
            "重复列展开（行尾空单元格省略）、office:value 兜底、逗号字段加引号"
        );
    }

    #[test]
    fn odp_to_markdown_pages() {
        let dir = tmp_dir("odp_md");
        let src = dir.join("目标.odp");
        min_odp(&src).unwrap();
        let out = dir.join("目标.md");
        native_convert(&src, &out, "md").unwrap();
        let md = std::fs::read_to_string(&out).unwrap();
        assert!(md.starts_with("## 幻灯片 1\n"), "{md}");
        assert!(md.contains("年度目标"), "{md}");
        assert!(md.contains("增长 30%"), "{md}");
        assert!(md.contains("## 幻灯片 2"), "{md}");
        assert!(md.contains("谢谢"), "{md}");
    }

    /* ---------- 探测 ---------- */

    #[test]
    fn probe_document_reports_metadata() {
        let dir = tmp_dir("probe");

        let docx = dir.join("a.docx");
        min_docx(&docx).unwrap();
        let v = probe_document(docx.to_str().unwrap());
        assert_eq!(v["ok"], true);
        assert_eq!(v["kind"], "docx");
        assert_eq!(v["title"], "季度总结文档");
        assert_eq!(v["pages"], 3);
        assert!(v["chars"].as_i64().unwrap_or(0) > 30, "{v}");

        let xlsx = dir.join("b.xlsx");
        min_xlsx(&xlsx).unwrap();
        let v = probe_document(xlsx.to_str().unwrap());
        assert_eq!(v["ok"], true);
        assert_eq!(v["kind"], "xlsx");
        assert_eq!(v["title"], "销售台账");
        assert_eq!(v["sheets"], 2);

        let pptx = dir.join("c.pptx");
        min_pptx(&pptx).unwrap();
        let v = probe_document(pptx.to_str().unwrap());
        assert_eq!(v["ok"], true);
        assert_eq!(v["slides"], 2);
        assert_eq!(v["title"], "季度汇报");

        let odt = dir.join("d.odt");
        min_odt(&odt).unwrap();
        let v = probe_document(odt.to_str().unwrap());
        assert_eq!(v["ok"], true);
        assert_eq!(v["kind"], "odt");
        assert_eq!(v["title"], "中文报告");
        assert_eq!(v["pages"], 2);

        let ods = dir.join("e.ods");
        min_ods(&ods).unwrap();
        let v = probe_document(ods.to_str().unwrap());
        assert_eq!(v["ok"], true);
        assert_eq!(v["sheets"], 2);

        let odp = dir.join("f.odp");
        min_odp(&odp).unwrap();
        let v = probe_document(odp.to_str().unwrap());
        assert_eq!(v["ok"], true);
        assert_eq!(v["slides"], 2);

        let pdf = dir.join("g.pdf");
        min_pdf(&pdf, "Hello Umidl").unwrap();
        let v = probe_document(pdf.to_str().unwrap());
        assert_eq!(v["ok"], true);
        assert_eq!(v["kind"], "pdf");
        assert_eq!(v["pages"], 1);

        let txt = dir.join("h.txt");
        std::fs::write(&txt, "纯文本内容 abc").unwrap();
        let v = probe_document(txt.to_str().unwrap());
        assert_eq!(v["ok"], true);
        assert_eq!(v["chars"], 9);
    }

    #[test]
    fn probe_document_failure_modes() {
        let dir = tmp_dir("probe_fail");
        let missing = dir.join("nope.docx");
        let v = probe_document(missing.to_str().unwrap());
        assert_eq!(v["ok"], false);
        assert_eq!(v["kind"], "docx");
        assert!(v["error"].as_str().unwrap().contains("文件不存在"), "{v}");

        let broken = dir.join("broken.docx");
        std::fs::write(&broken, b"not a zip at all").unwrap();
        let v = probe_document(broken.to_str().unwrap());
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().len() > 4, "{v}");

        let unknown = dir.join("x.mp4");
        std::fs::write(&unknown, b"x").unwrap();
        let v = probe_document(unknown.to_str().unwrap());
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("不是支持的文档格式"), "{v}");
    }

    /* ---------- extract_text ---------- */

    #[test]
    fn extract_text_native_and_text_files() {
        let dir = tmp_dir("extract");
        let docx = dir.join("a.docx");
        min_docx(&docx).unwrap();
        let text = extract_text(&docx).unwrap();
        assert!(text.contains("季度总结"), "{text}");
        assert!(text.contains("张三\t1234"), "表格转制表符：{text}");

        let html = dir.join("b.html");
        std::fs::write(
            &html,
            "<html><head><title>T</title><style>p{color:red}</style></head><body><h1>标题</h1><p>中文 &amp; 段落</p><script>var x=1;</script></body></html>",
        )
        .unwrap();
        let text = extract_text(&html).unwrap();
        assert!(text.contains("标题"), "{text}");
        assert!(text.contains("中文 & 段落"), "实体解码：{text}");
        assert!(!text.contains("color:red"), "style 应丢弃：{text}");
        assert!(!text.contains("var x"), "script 应丢弃：{text}");

        let gbk = dir.join("c.txt");
        // GBK 编码的“你好”（符合中文 Windows 常见来源）
        std::fs::write(&gbk, [0xC4u8, 0xE3, 0xBA, 0xC3]).unwrap();
        assert_eq!(extract_text(&gbk).unwrap().trim(), "你好");

        let legacy = dir.join("d.doc");
        std::fs::write(&legacy, b"\xD0\xCF\x11\xE0").unwrap();
        let err = extract_text(&legacy).unwrap_err().to_string();
        assert!(err.contains("旧版二进制"), "{err}");

        let rtf = dir.join("e.rtf");
        std::fs::write(&rtf, b"{\\rtf1}").unwrap();
        let err = extract_text(&rtf).unwrap_err().to_string();
        assert!(err.contains("pandoc"), "{err}");
    }

    /* ---------- pandoc 参数（纯函数） ---------- */

    #[test]
    fn build_pandoc_args_docx_to_md() {
        let args = build_pandoc_args(Path::new("C:/tmp/报告.docx"), Path::new("C:/tmp/报告.md"), "md");
        assert_eq!(
            args,
            vec![
                "--from",
                "docx",
                "--to",
                "gfm",
                "--wrap=none",
                "-o",
                "C:/tmp/报告.md",
                "C:/tmp/报告.docx"
            ]
        );
    }

    #[test]
    fn build_pandoc_args_md_to_docx_standalone() {
        let args = build_pandoc_args(Path::new("a.md"), Path::new("b.docx"), "docx");
        assert_eq!(args, vec!["--from", "gfm", "--to", "docx", "--standalone", "-o", "b.docx", "a.md"]);
    }

    #[test]
    fn build_pandoc_args_html_and_unknown_target() {
        let args = build_pandoc_args(Path::new("a.html"), Path::new("b.md"), "markdown");
        assert_eq!(
            args,
            vec!["--from", "html", "--to", "gfm", "--wrap=none", "-o", "b.md", "a.html"],
            "markdown 归一化为 gfm"
        );
        // 未知但合法的格式名：原样透传，且不追加 --standalone
        let args = build_pandoc_args(Path::new("a.docx"), Path::new("b.fb2"), "fb2");
        assert_eq!(args, vec!["--from", "docx", "--to", "fb2", "-o", "b.fb2", "a.docx"]);
        // 文本输入：不猜 -f，交给 pandoc 自动识别
        let args = build_pandoc_args(Path::new("a.txt"), Path::new("b.html"), "html");
        assert_eq!(args, vec!["--to", "html5", "--wrap=none", "--standalone", "-o", "b.html", "a.txt"]);
    }

    /* ---------- 能力探测 ---------- */

    #[test]
    fn capabilities_shape_and_native_list() {
        let dir = tmp_dir("caps");
        let ctx = test_ctx(&dir);
        let v = capabilities(&ctx);
        assert_eq!(v["pandoc"], v["pandoc"].as_bool().unwrap_or(false));
        assert!(v["pandoc_path"].is_null() || v["pandoc_path"].is_string());
        assert!(v["poppler_path"].is_null() || v["poppler_path"].is_string());
        let native: Vec<String> = v["native"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect();
        for k in ["docx", "xlsx", "pptx", "odt", "ods", "odp", "html", "txt"] {
            assert!(native.contains(&k.to_string()), "native 缺少 {k}：{native:?}");
        }
        let targets: Vec<String> = v["targets"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect();
        for t in ["txt", "md", "html", "csv"] {
            assert!(targets.contains(&t.to_string()), "targets 缺少 {t}：{targets:?}");
        }
        // 路径与布尔必须一致
        assert_eq!(v["pandoc"].as_bool().unwrap(), !v["pandoc_path"].is_null());
        assert_eq!(v["poppler"].as_bool().unwrap(), !v["poppler_path"].is_null());
    }

    #[test]
    fn unsupported_route_reports_install_hint_when_engine_missing() {
        let dir = tmp_dir("noengine");
        let ctx = test_ctx(&dir);
        let caps = capabilities(&ctx);
        let src = dir.join("a.docx");
        min_docx(&src).unwrap();
        let out = dir.join("a.rtf");
        let res = external_convert(&ctx, &src, &out, "rtf");
        if caps["pandoc"].as_bool().unwrap() {
            eprintln!("[skip] 本机已安装 pandoc（{}），跳过「未安装」报错分支的断言", caps["pandoc_path"]);
            return;
        }
        let err = res.unwrap_err().to_string();
        assert!(err.contains("依赖工具"), "错误信息应引导安装：{err}");
    }

    /* ---------- 真实引擎（存在则跑，不存在明确跳过） ---------- */

    /// 真实引擎入口：环境变量 UMIDL_TEST_PANDOC / UMIDL_TEST_POPPLER 可显式指定，
    /// 否则走正式解析逻辑（受管目录 → PATH）
    fn real_engine(ctx: &mut Ctx, name: &str) -> Option<PathBuf> {
        let env_key = if name == "pandoc" { "UMIDL_TEST_PANDOC" } else { "UMIDL_TEST_POPPLER" };
        if let Ok(p) = std::env::var(env_key) {
            let pb = PathBuf::from(p.trim());
            if pb.is_file() {
                if name == "pandoc" {
                    ctx.tools.pandoc = Some(pb.clone());
                } else {
                    ctx.tools.poppler = Some(pb.clone());
                }
                return Some(pb);
            }
        }
        engine_path(ctx, name)
    }

    #[test]
    fn real_poppler_pdf_to_text_when_available() {
        let dir = tmp_dir("real_poppler");
        let mut ctx = test_ctx(&dir);
        let Some(path) = real_engine(&mut ctx, "poppler") else {
            eprintln!(
                "[skip] 未检测到 poppler：%APPDATA%/umi-downloader/bin/poppler/pdftotext.exe、PATH 与 UMIDL_TEST_POPPLER 均无 pdftotext，跳过真实 PDF 转换"
            );
            return;
        };
        eprintln!("[real] 使用 pdftotext：{}", path.display());
        let pdf = dir.join("真实.pdf");
        min_pdf(&pdf, "Hello Umidl Real Engine").unwrap();
        let out = dir.join("真实.txt");
        let args = external_convert(&ctx, &pdf, &out, "txt").expect("pdftotext 转换应成功");
        eprintln!("[real] 命令行：{args:?}");
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("Hello Umidl Real Engine"), "pdftotext 输出：{text}");
        // 探测的页数也要对得上
        assert_eq!(probe_document(pdf.to_str().unwrap())["pages"], 1);
        // extract_text（stdout 通路，必须显式传 `-`）：与文件输出内容一致
        let direct = extract_text(&pdf).expect("pdftotext stdout 通路应成功");
        assert!(
            direct.contains("Hello Umidl Real Engine"),
            "extract_text(stdout) 输出：{direct:?}"
        );
    }

    #[test]
    fn real_pandoc_markdown_roundtrip_when_available() {
        let dir = tmp_dir("real_pandoc");
        let mut ctx = test_ctx(&dir);
        let Some(path) = real_engine(&mut ctx, "pandoc") else {
            eprintln!(
                "[skip] 未检测到 pandoc：%APPDATA%/umi-downloader/bin/pandoc.exe、PATH 与 UMIDL_TEST_PANDOC 均无 pandoc，跳过真实文档转换"
            );
            return;
        };
        eprintln!("[real] 使用 pandoc：{}", path.display());

        // ① docx → rtf：pandoc 真的去解析我们现场生成的 docx（验证 OOXML 合法性 + pandoc 参数）
        let docx = dir.join("真实.docx");
        min_docx(&docx).unwrap();
        let rtf_out = dir.join("真实.rtf");
        let args = external_convert(&ctx, &docx, &rtf_out, "rtf").expect("pandoc docx→rtf 应成功");
        eprintln!("[real] 命令行：{args:?}");
        assert_eq!(
            args.first().map(|s| s.as_str()),
            Some("--from"),
            "必须真的调用 pandoc（原生分支会返回 <native>）：{args:?}"
        );
        assert!(args.contains(&"docx".to_string()));
        let rtf = std::fs::read_to_string(&rtf_out).unwrap();
        assert!(rtf.starts_with("{\\rtf1"), "RTF 魔数：{}", &rtf[..rtf.len().min(40)]);

        // ② md → docx：原生引擎没有这条通路，必须由 pandoc 写二进制 OOXML
        let md_in = dir.join("输入.md");
        std::fs::write(&md_in, "# 季度总结\n\n正文：中文内容\n\n| 姓名 | 销售额 |\n| --- | --- |\n| 张三 | 1234 |\n").unwrap();
        let docx_out = dir.join("回写.docx");
        let args = external_convert(&ctx, &md_in, &docx_out, "docx").expect("pandoc md→docx 应成功");
        eprintln!("[real] 命令行：{args:?}");
        assert!(args.contains(&"docx".to_string()));
        let meta = std::fs::metadata(&docx_out).unwrap();
        assert!(meta.len() > 1000, "docx 应为真实压缩包，大小 {}", meta.len());
        let names = zip_names(&docx_out);
        assert!(names.iter().any(|n| n == "word/document.xml"), "docx 结构应完整：{names:?}");

        // ③ 回到原生引擎读 pandoc 写出的 docx（外部写 / 原生读 互通）
        let txt_out = dir.join("回读.txt");
        native_convert(&docx_out, &txt_out, "txt").unwrap();
        let txt = std::fs::read_to_string(&txt_out).unwrap();
        assert!(txt.contains("张三"), "原生读取 pandoc 产物：{txt}");
    }

    #[test]
    fn min_pdf_is_really_parseable() {
        // PDF 生成器本身的质量保证：结构合法（供真实引擎测试使用）
        let dir = tmp_dir("pdf_gen");
        let pdf = dir.join("t.pdf");
        min_pdf(&pdf, "Hello World").unwrap();
        let bytes = std::fs::read(&pdf).unwrap();
        assert!(bytes.starts_with(b"%PDF-1.4"));
        assert!(bytes.ends_with(b"%%EOF\n"));
        assert!(String::from_utf8_lossy(&bytes).contains("startxref"));
        assert_eq!(pdf_page_count(&pdf), Some(1));
    }

    #[test]
    fn pdf_png_finalize_renames_first_page_and_keeps_rest() {
        // pdftoppm 输出 `前缀-N.png`：单页改名成目标文件，多页时首页命名目标文件、其余保留
        let dir = tmp_dir("pdf_png_names");
        let out = dir.join("文档.png");
        for n in 1..=3 {
            std::fs::write(dir.join(format!("文档-{n}.png")), b"\x89PNG\r\n\x1a\n").unwrap();
        }
        finalize_pdf_png(&out, Path::new("pdftoppm"), &[]).unwrap();
        assert!(out.is_file(), "首页应改名为目标文件");
        assert!(dir.join("文档-2.png").is_file(), "其余页不能丢");
        assert!(dir.join("文档-3.png").is_file());
        assert!(!dir.join("文档-1.png").exists(), "首页原名应消失");

        // 单页场景：只有一个 `前缀-1.png`
        let dir2 = tmp_dir("pdf_png_single");
        let out2 = dir2.join("单页.png");
        std::fs::write(dir2.join("单页-1.png"), b"\x89PNG\r\n\x1a\n").unwrap();
        finalize_pdf_png(&out2, Path::new("pdftoppm"), &[]).unwrap();
        assert!(out2.is_file());
        assert!(!dir2.join("单页-1.png").exists());

        // 什么都没生成 → 明确报错，不静默成功
        let dir3 = tmp_dir("pdf_png_empty");
        let err = finalize_pdf_png(&dir3.join("空.png"), Path::new("pdftoppm"), &[]).unwrap_err().to_string();
        assert!(err.contains("未生成") || err.contains("未生成输出文件"), "{err}");
    }

    #[test]
    fn pdf_to_png_without_pdftoppm_reports_install_hint() {
        let dir = tmp_dir("pdf_png_hint");
        let ctx = test_ctx(&dir);
        if which_on_path(&exe_name_docs("pdftoppm")).is_some() || engine_path(&ctx, "poppler").map(|p| p.with_file_name(exe_name_docs("pdftoppm")).is_file()).unwrap_or(false) {
            eprintln!("[skip] 本机存在 pdftoppm，跳过「缺少 pdftoppm」提示分支");
            return;
        }
        let pdf = dir.join("a.pdf");
        min_pdf(&pdf, "Hello Umidl").unwrap();
        let err = external_convert(&ctx, &pdf, &dir.join("a.png"), "png")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("pdftoppm") || err.contains("poppler"),
            "缺少 pdftoppm 时应提示安装 poppler：{err}"
        );
    }

    #[test]
    fn xml_entities_and_char_refs() {
        let doc = parse_xml(
            r#"<root><a>a&amp;b</a><b>&#x4E2D;&#25991;</b><c xml:space="preserve"> 空格 保留 </c><d>  缩进忽略  </d></root>"#,
        )
        .unwrap();
        let a = doc.find_desc("a").unwrap();
        let b = doc.find_desc("b").unwrap();
        let c = doc.find_desc("c").unwrap();
        let d = doc.find_desc("d").unwrap();
        assert_eq!(a.text(), "a&b");
        assert_eq!(b.text(), "中文");
        assert_eq!(c.text(), " 空格 保留 ");
        assert_eq!(d.text(), "缩进忽略");
    }

    #[test]
    fn auto_document_target_picks_sensible_format() {
        assert_eq!(auto_document_target("xlsx"), "csv");
        assert_eq!(auto_document_target("ods"), "csv");
        assert_eq!(auto_document_target("pptx"), "md");
        assert_eq!(auto_document_target("docx"), "md");
        assert_eq!(auto_document_target("pdf"), "txt");
        assert_eq!(auto_document_target("epub"), "txt");
    }
}
