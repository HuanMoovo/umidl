//! ED2K（电驴）链接支持：链接解析 + eMule（社区版）/ mlDonkey 引擎检测与接管
//!
//! # 本文件的实现依据（全部为**实测**结论，不是推测）
//!
//! 被测引擎：eMule v0.72a community（irwir/eMule），官方 Windows zip
//! <https://github.com/irwir/eMule/releases/download/eMule_v0.72a-community/eMule0.72a.zip>，
//! 由 `tools.rs` 的 `install_tool("emule")` 整包解压到 `%APPDATA%/umi-downloader/bin/emule/`。
//! 源码对照：仓库 irwir/eMule 的 tag `eMule_v0.72a-community`（ac3d52ea，代码在 `srchybrid/`）。
//!
//! ## 1) 配置目录：eMule **不支持** `-c <dir>`
//!
//! - 源码 `srchybrid/Emule.cpp::CemuleApp::ProcessCommandline()`：只识别 `-ignoreinstances`
//!   和 `-AutoStart` 两个开关，其余 `-x` / `/x` 参数一律忽略，**没有** `-c`；
//!   实测 `emule.exe -c <dir>` 不会把配置写到该目录。
//! - eMule 的目录选择逻辑在 `srchybrid/Preferences.cpp::CPreferences::GetDefaultDirectory()`：
//!   由注册表 `HKCU\Software\eMule\UsePublicUserDirectories`（0=每用户 / 1=公共 / 2=程序目录）
//!   覆盖；没有该值时（Windows Vista+）依次判断
//!   `%LOCALAPPDATA%\eMule\config\preferences.ini` → 1(公共) `%ProgramData%\eMule\config\preferences.ini`
//!   → 2(程序目录) `<exe目录>\config\preferences.ini` → 否则回落到 0(每用户)。
//! - 因此**受管目录方案 = 在 `<exe目录>/config/` 放一份 preferences.ini**，eMule 即进入
//!   “程序目录”模式（mode 2）。实测：受管目录 `bin/emule/config/` 里出现
//!   `preferences.ini / cryptkey.dat / downloads.txt / known2_64.met / statistics.ini`，
//!   而 `%LOCALAPPDATA%\eMule` 完全不被创建（自带配置成功隔离在受管目录内）。
//!
//! ## 2) 投递通道：**不能**依赖“命令行带链接启动”，要分冷/热两种走法
//!
//! eMule 的命令行确实接受 ED2K 链接（`srchybrid/Emule.cpp::ProcessCommandline`）：
//! 参数含 `://` → 打包成 `OP_ED2KLINK`；还有若干 CLI 命令（`exit`/`restore`/`connect`/
//! `disconnect`/`resume`/`reloadipf`/`limits=`）→ `OP_CLCOMMAND`
//! （处理函数见 `srchybrid/EmuleDlg.cpp::CemuleDlg::OnWMData`）。但**分两种情况**，
//! 本模块把两种都实测过：
//!
//! - **引擎已在运行**（热）：再执行一次 `emule.exe "<ed2k链接>"`，第二个进程 0.03s 就退出，
//!   链接经 `WM_COPYDATA` 转发给已有实例（`SearchEmuleWindow` 广播 `UWM_ARE_YOU_EMULE` 找窗口），
//!   实测传输列表出现该任务 ✅。`rundll32 url.dll,FileProtocolHandler "<ed2k链接>"` 同理（0.15s）✅。
//! - **引擎未运行**（冷）：**不能用“带链接启动”**——`ProcessCommandline` 只有在
//!   `maininst != NULL` 时才设置 `sendstruct.dwData = OP_ED2KLINK`；冷启动没有 maininst，
//!   于是 `m_strPendingLink` 被赋值、但 `sendstruct.dwData` 仍是 0，而启动末尾
//!   （`CemuleDlg::OnInitDialog` 尾部）是拿同一个 `sendstruct` 去调
//!   `OnWMData(NULL, &theApp.sendstruct)`，`dwData == 0` 直接被丢掉。
//!   实测：杀掉引擎后带链接冷启动，**30s 内链接都没有进队列**（WebServer 早就 200 了）❌。
//!
//! 所以本实现的投递策略（与实测一致）：
//! 1. 引擎未运行 → **先启动引擎**（不带链接），等 WebServer 就绪（实测 1.5–3s，最多等 20s）；
//! 2. 在**已在运行**的引擎上首选 **HTTP 远程加链**（`srchybrid/WebServer.cpp` 的 `justAddLink` 分支）：
//!    `GET /?w=password&p=<明文密码>&c=<urlencoded ed2k 链接>`，
//!    返回 `<status result="OK">…</status>` 即确认收到（实测 0.02s 返回、3s 内进队列 ✅）；
//! 3. HTTP 通道不可用（例如用户自己关掉了 WebServer）→ 退回 `emule.exe "<链接>"` 命令行转发
//!    （此时引擎已在运行，属“热”路径，实测有效 ✅）；
//! 4. 投递后**回读传输列表**核对 hash 是否真的在队列里（`verified` 字段），不靠“应该成功了”。
//!
//! 协议注册确实存在：eMule 会把 `HKCU\Software\Classes\ed2k\shell\open\command` 指向
//! 本程序目录的 `emule.exe "%1"`（实测注册表可见），`rundll32 url.dll,FileProtocolHandler`
//! 因此也能用（实测 ✅），本模块把它当备用通道，而不是主通道。
//!
//! ## 3) WebServer：可以只用受管 preferences.ini 打开，且能读到进度
//!
//! `[WebServer]` 段的键见 `srchybrid/Preferences.cpp`（`Enabled` / `Port` / `Password` /
//! `UseGzip` …）。其中 `Password` 存的不是明文，而是
//! `MD5Sum::GetHashString()` → `md4str()` → `EncodeBase16()`（`srchybrid/MD5Sum.cpp`、
//! `OtherFunctions.cpp`，base16Chars = `"0123456789ABCDEF"`，即**大写**十六进制），
//! 且 `MD5Sum::Calculate(CString)` 哈希的是 `sSource.GetLength() * sizeof(TCHAR)` 字节，
//! 即 Unicode 构建下**明文密码的 UTF-16LE 字节**。实测：写
//! `Password=<大写十六进制(MD5(密码的 UTF-16LE 字节))>` 后，
//! `GET /?w=password&p=<明文>` 能拿到会话（页面里带 `ses=<id>`），
//! `GET /?ses=<id>&w=transfer` 返回传输列表，可用正则读出每个文件的名字/hash/状态/进度。
//! `[WebServer] UseGzip=0` 让页面保持明文（默认 `UseGzip=1` 时传输页是 gzip 压缩流，
//! 而本项目依赖里没有可用的 gzip 解压实现，故显式关掉）。
//! 实测：`curl http://127.0.0.1:4711/` → 200 + `eMule 0.72a - Web Interface`。
//!
//! ## 4) 首次启动的坑（已规避，均已实测）
//!
//! - 不写 `[eMule] AppVersion` 时 eMule 认为“首次运行”并弹 `eMule First Runtime Wizard`；
//! - 不写 `[eMule] Language` 时，非英语系统上会弹一个只有“确定”按钮的提示框
//!   （"The language was set to 'English'."），**该模态框会挡住 WebServer 启动**。
//!   写入 `Language=1033` 后实测：启动 3 秒内 WebServer 就绪且无任何弹窗。
//!
//! ## 5) mlDonkey 分支（**未实测**）
//!
//! 本机没有 mlnet 二进制，无法实测；`bin/mlnet.exe` 若存在（用户自备）可走它的 HTTP 接口。
//! 查询串依据 MLDonkey 官方文档 “Browser Integration”（
//! <https://bugfreeblog.duckdns.org/wp-content/uploads/mldonkey-website/Browser_Integration.html>，
//! 脚本写法 `http://user:pass@host:4080/submit?q=dllink+<urlencoded link>`）以及社区脚本
//! （`wget "http://localhost:4080/submit?q=dllink+$tor"`）；服务端实现见
//! `mldonkey/src/daemon/driver/driverControlers.ml`：URL 参数 `q` 交给命令解释器，
//! 其中 `ed2k://` 前缀会被自动补成 `dllink <link>`，`+` 用于分隔参数。
//! 相关代码路径与返回结构**均未在真机验证**，注释与返回值里显式标注“未实测”。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::ctx::{command_for, Ctx};

/// eMule WebServer 默认端口（`srchybrid/Preferences.cpp`: `ini.GetInt("Port", 4711)`）
pub const EMULE_WEB_PORT: u16 = 4711;
/// mlDonkey HTTP 接口默认端口（官方文档 http_port 默认值）
pub const MLNET_HTTP_PORT: u16 = 4080;

/* ==================== 链接解析 ==================== */

/// 解析后的 ed2k 文件链接
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ed2kLink {
    /// 文件 hash（32 位十六进制，统一小写）
    pub hash: String,
    /// 文件名（已 URL 解码）
    pub name: String,
    /// 文件大小（字节）
    pub size: i64,
    /// `s=<http 源>`（可能多个，按出现顺序）
    pub sources: Vec<String>,
    /// `h=<AICH 根哈希>`（base32 文本，原样保留）
    pub aich: Option<String>,
}

/// `ed2k://|server|ip|port|/` 形式的服务器链接
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ed2kServer {
    pub host: String,
    pub port: u16,
}

/// 是否为 ed2k 链接（file / server / serverlist / nodeslist 都算）
pub fn is_ed2k(url: &str) -> bool {
    url.trim().to_ascii_lowercase().starts_with("ed2k://")
}

/// eMule 的 `URLDecode`（`srchybrid/OtherFunctions.cpp`）只解 `%XX`，**不把 `+` 当空格**，
/// 这里保持一致（`+` 是合法文件名字符）。
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                let v = (hi * 16 + lo) as u8;
                // eMule 会丢弃 <= 0x1F 的控制字符（换行除外，这里也丢弃）
                if v > 0x1f {
                    out.push(v);
                }
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // 链接里的文件名是 UTF-8（eMule 会再过一遍 OptUtf8ToStr）
    String::from_utf8_lossy(&out).into_owned()
}

fn is_hex32(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 把链接（去掉 `ed2k://` 之后的正文）切成 `|` 分段。
///
/// eMule 的做法（`srchybrid/EmuleDlg.cpp::ProcessED2KLink`）是先把 `%7c` 还原成 `|`，
/// 再对整条链接做 URL 解码，最后才按 `|` 切分；这里等价实现：先整体解码再切分，
/// 于是 `%7c` / `%7C` 都会成为分隔符（与 eMule 一致）。
///
/// 注意：**保留空段**，这样“某一段为空（缺失）”才能被区分出来并给出准确报错。
fn split_segments(link: &str) -> Vec<String> {
    let rest = match link.trim().get(7..) {
        Some(r) => r,
        None => "",
    };
    let decoded = percent_decode(rest);
    let mut seg: Vec<String> = decoded.split('|').map(|s| s.to_string()).collect();
    // 规范写法是 `ed2k://|file|…`，切分后首段是空串，去掉它
    if seg.first().map(|s| s.is_empty()).unwrap_or(false) {
        seg.remove(0);
    }
    seg
}

/// 解析 ed2k **文件**链接。
///
/// 支持 `ed2k://|file|<name>|<size>|<hash>|/`、带 `h=<AICH>`、带若干 `s=<http 源>`；
/// `ed2k://|server|...` 请用 [`parse_server`]（本函数会给出明确错误）。
pub fn parse(link: &str) -> anyhow::Result<Ed2kLink> {
    let raw = link.trim();
    if !is_ed2k(raw) {
        anyhow::bail!("不是 ed2k 链接（应以 ed2k:// 开头）：`{}`", raw);
    }
    let seg = split_segments(raw);
    let kind = seg
        .first()
        .map(|s| s.as_str())
        .ok_or_else(|| anyhow::anyhow!("ed2k 链接缺少类型段（应为 ed2k://|file|…|/）：`{raw}`"))?;
    if kind.eq_ignore_ascii_case("server") {
        anyhow::bail!("这是 ed2k 服务器链接（ed2k://|server|ip|port|/），不是文件链接；请用 parse_server() 或「连接服务器」功能");
    }
    if !kind.eq_ignore_ascii_case("file") {
        anyhow::bail!(
            "暂不支持该类型的 ed2k 链接：`{kind}`（仅支持文件链接 ed2k://|file|<文件名>|<大小>|<hash>|/）"
        );
    }

    let name = seg
        .get(1)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("ed2k 链接缺少文件名段（ed2k://|file|<这里>|<大小>|<hash>|/）：`{raw}`"))?;

    let size_raw = seg
        .get(2)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("ed2k 链接缺少大小段（ed2k://|file|<文件名>|<这里>|<hash>|/）：`{raw}`"))?;
    let size: i64 = size_raw.parse().map_err(|_| {
        anyhow::anyhow!("ed2k 链接的大小段不是数字：`{size_raw}`（应为字节数，例如 734003200）")
    })?;

    let hash_raw = seg
        .get(3)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("ed2k 链接缺少 hash 段（ed2k://|file|<文件名>|<大小>|<这里>|/）：`{raw}`"))?;
    if !is_hex32(&hash_raw) {
        anyhow::bail!("ed2k 链接的 hash 段不是 32 位十六进制：`{hash_raw}`");
    }

    let mut sources: Vec<String> = Vec::new();
    let mut aich: Option<String> = None;
    // 第 5 段起是扩展参数（`/` 之后是 eMule 的私有扩展，忽略）
    for s in seg.iter().skip(4) {
        if s == "/" {
            break;
        }
        let lower = s.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("h=") {
            let v = &s[s.len() - v.len()..];
            if !v.trim().is_empty() {
                aich = Some(v.trim().to_string());
            }
        } else if let Some(v) = lower.strip_prefix("s=") {
            let v = &s[s.len() - v.len()..];
            if !v.trim().is_empty() {
                sources.push(v.trim().to_string());
            }
        }
        // `p=` 等其它参数按 eMule 的做法忽略
    }

    Ok(Ed2kLink {
        hash: hash_raw.to_ascii_lowercase(),
        name,
        size,
        sources,
        aich,
    })
}

/// 解析 `ed2k://|server|<ip 或域名>|<port>|/`
pub fn parse_server(link: &str) -> anyhow::Result<Ed2kServer> {
    let raw = link.trim();
    if !is_ed2k(raw) {
        anyhow::bail!("不是 ed2k 链接（应以 ed2k:// 开头）：`{raw}`");
    }
    let seg = split_segments(raw);
    let kind = seg.first().map(|s| s.as_str()).unwrap_or("");
    if !kind.eq_ignore_ascii_case("server") {
        anyhow::bail!("不是 ed2k 服务器链接（应为 ed2k://|server|ip|port|/）：`{raw}`");
    }
    let host = seg
        .get(1)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("ed2k 服务器链接缺少地址段：`{raw}`"))?;
    let port_raw = seg
        .get(2)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("ed2k 服务器链接缺少端口段：`{raw}`"))?;
    let port: u16 = port_raw.parse().map_err(|_| {
        anyhow::anyhow!("ed2k 服务器链接的端口段不是合法端口（1-65535）：`{port_raw}`")
    })?;
    if port == 0 {
        anyhow::bail!("ed2k 服务器链接的端口段不是合法端口：`{port_raw}`");
    }
    Ok(Ed2kServer { host, port })
}

/* ==================== MD5（仅用于 WebServer 密码） ==================== */

/// 极简 MD5（RFC 1321）。项目依赖里没有 md5 实现（只有 sha2），
/// 而 eMule WebServer 的密码校验用的是 MD5，所以这里自带一份。
fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    let (mut a0, mut b0, mut c0, mut d0) =
        (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);

    for chunk in msg.chunks(64) {
        let mut m = [0u32; 16];
        for (i, w) in chunk.chunks(4).enumerate() {
            m[i] = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let tmp = d;
            d = c;
            c = b;
            let sum = a
                .wrapping_add(f)
                .wrapping_add(K[i])
                .wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(S[i]));
            a = tmp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    for (i, v) in [a0, b0, c0, d0].iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

fn hex_upper(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02X}"));
    }
    s
}

/// eMule WebServer 存的密码：**大写十六进制 MD5(密码的 UTF-16LE 字节)**
/// （`MD5Sum::Calculate(CString)` 哈希 `len * sizeof(TCHAR)` 字节，
/// `GetHashString()` → `md4str()` → `EncodeBase16()` 用 `0123456789ABCDEF`）。
pub fn web_password_hash(password: &str) -> String {
    let mut wide: Vec<u8> = Vec::with_capacity(password.len() * 2);
    for u in password.encode_utf16() {
        wide.extend_from_slice(&u.to_le_bytes());
    }
    hex_upper(&md5(&wide))
}

/* ==================== 引擎发现 / 进程检测 ==================== */

fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// 受管 eMule 目录：`%APPDATA%/umi-downloader/bin/emule/`
pub fn emule_home(ctx: &Ctx) -> PathBuf {
    ctx.dirs.bin.join("emule")
}

/// 受管 eMule 可执行文件（优先用已解析的工具路径，其次直接看受管目录）
pub fn emule_exe(ctx: &Ctx) -> Option<PathBuf> {
    if let Some(p) = ctx.tools.emule.as_ref() {
        if p.is_file() {
            return Some(p.clone());
        }
    }
    let p = emule_home(ctx).join(exe_name("emule"));
    p.is_file().then_some(p)
}

/// mlDonkey 可执行文件（用户自备：受管目录或 PATH）
pub fn mlnet_exe(ctx: &Ctx) -> Option<PathBuf> {
    let p = ctx.dirs.bin.join(exe_name("mlnet"));
    if p.is_file() {
        return Some(p);
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let full = dir.join(exe_name("mlnet"));
        if full.is_file() {
            return Some(full);
        }
    }
    None
}

/// 进程是否在运行：Windows 用 `tasklist /FI`，其它平台用 `pgrep -x`
fn process_running(image: &str) -> bool {
    #[cfg(windows)]
    {
        let mut cmd = command_for(Path::new("tasklist"));
        cmd.args(["/FI", &format!("IMAGENAME eq {image}"), "/NH", "/FO", "CSV"]);
        cmd.stdin(Stdio::null());
        match cmd.output() {
            Ok(o) => {
                let text = crate::ctx::decode_output(&o.stdout).to_ascii_lowercase();
                text.contains(&image.to_ascii_lowercase())
            }
            Err(_) => false,
        }
    }
    #[cfg(not(windows))]
    {
        let stem = image.trim_end_matches(".exe");
        Command::new("pgrep")
            .args(["-x", stem])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// 后台拉起引擎（分离进程，隐藏控制台；GUI 窗口正常显示）
fn spawn_engine(exe: &Path, workdir: &Path, args: &[String]) -> anyhow::Result<u32> {
    let mut cmd = Command::new(exe);
    cmd.args(args);
    cmd.current_dir(workdir);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let child = cmd.spawn()?;
    Ok(child.id())
}

/* ==================== 受管配置：preferences.ini + 密码 ==================== */

/// 受管 WebServer 密码（明文写在我们自己的 JSON 里，哈希写进 eMule 的 preferences.ini）
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WebSecret {
    port: u16,
    password: String,
}

fn secret_path(ctx: &Ctx) -> PathBuf {
    emule_home(ctx).join("umi_ed2k.json")
}

/// 生成 12 位随机密码（uuid v4 十六进制前 12 位）；
/// 长度控制在 12 以兼容 eMule Web 界面登录框 `maxlength=12`。
fn random_password() -> String {
    let s = uuid::Uuid::new_v4().simple().to_string();
    s[..12].to_string()
}

fn read_secret(ctx: &Ctx) -> Option<WebSecret> {
    let raw = std::fs::read_to_string(secret_path(ctx)).ok()?;
    let s: WebSecret = serde_json::from_str(&raw).ok()?;
    if s.password.is_empty() {
        return None;
    }
    Some(s)
}

/// 读取 ini 里某个 section 下 key 的值（大小写不敏感，容忍 CRLF）
fn read_ini_key(text: &str, section: &str, key: &str) -> Option<String> {
    let want = format!("[{section}]").to_ascii_lowercase();
    let mut in_sec = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_sec = l.to_ascii_lowercase() == want;
            continue;
        }
        if !in_sec {
            continue;
        }
        let (k, v) = match l.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        if k.trim().eq_ignore_ascii_case(key) {
            return Some(v.trim().to_string());
        }
    }
    None
}

/// 在 ini 的指定 section 写入 / 覆盖 key=value（保留其它内容）
fn upsert_ini_key(text: &str, section: &str, key: &str, value: &str) -> String {
    let want = format!("[{section}]").to_ascii_lowercase();
    let lines: Vec<String> = text.lines().map(|s| s.trim_end_matches('\r').to_string()).collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 2);
    let mut in_sec = false;
    let mut sec_found = false;
    let mut key_written = false;

    for line in lines.iter() {
        let l = line.trim();
        if l.starts_with('[') {
            if in_sec && !key_written {
                out.push(format!("{key}={value}"));
                key_written = true;
            }
            in_sec = l.to_ascii_lowercase() == want;
            sec_found |= in_sec;
            out.push(line.clone());
            continue;
        }
        if in_sec {
            if let Some((k, _)) = l.split_once('=') {
                if k.trim().eq_ignore_ascii_case(key) {
                    out.push(format!("{key}={value}"));
                    key_written = true;
                    continue;
                }
            }
        }
        out.push(line.clone());
    }
    if in_sec && !key_written {
        out.push(format!("{key}={value}"));
    }
    if !sec_found {
        out.push(String::new());
        out.push(format!("[{section}]"));
        out.push(format!("{key}={value}"));
    }
    out.join("\r\n") + "\r\n"
}

/// 确保受管 eMule 配置就绪，返回 (preferences.ini 路径, web 端口, 明文密码)
///
/// 写出的 `config/preferences.ini` 同时起两个作用：
/// 1. 让 eMule 进入“程序目录”模式（配置全部落在受管目录里，不污染 `%LOCALAPPDATA%`）；
/// 2. 打开 WebServer（进度读取）并跳过首次运行向导 / 语言提示框。
fn ensure_emule_config(ctx: &Ctx) -> anyhow::Result<(PathBuf, u16, String)> {
    let home = emule_home(ctx);
    let config_dir = home.join("config");
    std::fs::create_dir_all(&config_dir)?;
    let ini_path = config_dir.join("preferences.ini");

    let mut secret = read_secret(ctx).unwrap_or(WebSecret {
        port: EMULE_WEB_PORT,
        password: random_password(),
    });

    let existing = std::fs::read_to_string(&ini_path).unwrap_or_default();
    // 端口：沿用已有配置（用户可能改过），否则用默认
    let port: u16 = std::fs::read_to_string(&ini_path)
        .ok()
        .and_then(|t| read_ini_key(&t, "WebServer", "Port"))
        .and_then(|v| v.parse().ok())
        .filter(|p| *p > 0)
        .unwrap_or(secret.port.max(1));
    secret.port = port;

    let mut ini = existing;
    if ini.trim().is_empty() {
        ini = String::new();
    }
    // 关键键：缺失才写，已有内容一律保留
    if read_ini_key(&ini, "eMule", "AppVersion").unwrap_or_default().trim().is_empty() {
        ini = upsert_ini_key(&ini, "eMule", "AppVersion", "0.72a");
    }
    if read_ini_key(&ini, "eMule", "Language").unwrap_or_default().trim().is_empty() {
        // 1033 = en-US：避免非英语系统弹出“语言已设为 English”的模态提示框
        ini = upsert_ini_key(&ini, "eMule", "Language", "1033");
    }
    if read_ini_key(&ini, "eMule", "Nick").unwrap_or_default().trim().is_empty() {
        ini = upsert_ini_key(&ini, "eMule", "Nick", "umi-downloader");
    }
    // WebServer：这几项由本程序管理（受管引擎目录由本程序安装，见模块文档）
    let hash = web_password_hash(&secret.password);
    ini = upsert_ini_key(&ini, "WebServer", "Enabled", "1");
    ini = upsert_ini_key(&ini, "WebServer", "Port", &port.to_string());
    ini = upsert_ini_key(&ini, "WebServer", "Password", &hash);
    // 关掉 gzip：项目依赖里没有可用的 gzip 解压实现，明文页面才能被解析
    ini = upsert_ini_key(&ini, "WebServer", "UseGzip", "0");

    if ini != std::fs::read_to_string(&ini_path).unwrap_or_default() {
        std::fs::write(&ini_path, &ini)?;
    }
    std::fs::write(secret_path(ctx), serde_json::to_string_pretty(&secret)?)?;
    Ok((ini_path, port, secret.password))
}

/* ==================== HTTP（本地 loopback，短超时） ==================== */

fn http_get(url: &str) -> anyhow::Result<String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let resp = client.get(url).send()?;
    let status = resp.status();
    let body = resp.text()?;
    if !status.is_success() {
        anyhow::bail!("HTTP {status}");
    }
    Ok(body)
}

/// percent-encode（RFC 3986，保留 `A-Za-z0-9-_.~`），用于把 ed2k 链接塞进查询串
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.as_bytes() {
        let c = *b as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
            out.push(c);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// eMule WebServer 远程加链（无会话通道，`srchybrid/WebServer.cpp` 的 `justAddLink` 分支）：
/// `GET /?w=password&p=<明文密码>&c=<urlencoded ed2k 链接>`
/// 成功返回 `<status result="OK">…</status>`，失败返回
/// `<status result="FAILED" reason="WRONG_PASSWORD">…`。
fn emule_web_add_link(port: u16, password: &str, link: &str) -> anyhow::Result<String> {
    let url = format!(
        "http://127.0.0.1:{port}/?w=password&p={}&c={}",
        percent_encode(password),
        percent_encode(link)
    );
    http_get(&url)
}

/// eMule WebServer 登录，返回会话号（页面里 `ses=<id>`）
fn emule_web_login(port: u16, password: &str) -> anyhow::Result<String> {
    let url = format!(
        "http://127.0.0.1:{port}/?w=password&p={}",
        percent_encode(password)
    );
    let body = http_get(&url)?;
    let re = regex::Regex::new(r"ses=(\d+)")?;
    re.captures(&body)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| anyhow::anyhow!("登录响应里没有会话号（密码可能与引擎内不一致）"))
}

/// 一个传输条目（从 Web 界面传输列表的悬浮提示里解析，字段与页面一致）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TransferItem {
    pub name: String,
    pub hash: String,
    pub size: String,
    pub size_on_disk: String,
    pub status: String,
    pub completed: String,
    pub percent: f64,
    pub sources: u32,
}

/// 解析传输列表页面（`?w=transfer`）。
///
/// eMule 把每个文件的详细信息放在 `downmenu(event,'admin','<详情>','ed2k://…',…)` 的
/// 单引号字符串里，字段用 `\n`（字面反斜杠+n）分隔，形如：
/// ```text
/// <文件名>
/// Hash: <32 位大写十六进制>
/// Size: 2.00 MB    (Size on disk: 0 Bytes)
///
/// part.met file: 002.part.met
/// Status: Waiting
/// Completed: 0 Bytes/2.00 MB (0.0%)
/// Sources: 0  (Useful: 0, NNP: 0);  A4AF: 0
/// Parts: 1, Available: 0 (0.0%)
/// ```
/// 该格式取自实测页面（见模块文档第 3 节）。
pub fn parse_transfer_page(html: &str) -> Vec<TransferItem> {
    let re = regex::Regex::new(r"downmenu\(event,'(?:admin|guest)','(.*?)','ed2k://").unwrap();
    let mut out = Vec::new();
    for cap in re.captures_iter(html) {
        let tip = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let lines: Vec<&str> = tip.split("\\n").collect();
        let name = lines.first().map(|s| s.trim()).unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        let field = |prefix: &str| -> String {
            lines
                .iter()
                .find(|l| l.trim_start().to_ascii_lowercase().starts_with(prefix))
                .map(|l| {
                    let v = l.trim_start()[prefix.len()..].trim().to_string();
                    v
                })
                .unwrap_or_default()
        };
        let hash = field("hash:");
        let size_line = field("size:");
        let (size, size_on_disk) = match size_line.split_once('(') {
            Some((s, rest)) => (
                s.trim().to_string(),
                rest.trim_end_matches(')').trim().trim_start_matches("Size on disk:").trim().to_string(),
            ),
            None => (size_line.clone(), String::new()),
        };
        let completed = field("completed:");
        // percent：取 Completed 行最后一个括号里的数字
        let percent = completed
            .rsplit_once('(')
            .and_then(|(_, r)| r.split(')').next())
            .map(|p| p.trim().trim_end_matches('%').trim().to_string())
            .and_then(|p| p.parse::<f64>().ok())
            .unwrap_or(0.0);
        let src_line = field("sources:");
        let sources = src_line
            .split_whitespace()
            .next()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        out.push(TransferItem {
            name,
            hash,
            size,
            size_on_disk,
            status: field("status:"),
            completed,
            percent,
            sources,
        });
    }
    out
}

/* ==================== 对外 API ==================== */

/// 引擎类型
fn engine_kind(ctx: &Ctx) -> Option<(&'static str, PathBuf)> {
    if let Some(p) = emule_exe(ctx) {
        return Some(("emule", p));
    }
    if let Some(p) = mlnet_exe(ctx) {
        return Some(("mlnet", p));
    }
    None
}

/// 引擎状态：`{engine,path,installed,running,ready,web_port,note}`
///
/// 诚实约定：找不到引擎时 `engine=null` 且 `note` 说明原因；
/// `ready` 表示“已安装且正在运行”，即可以由 [`submit`] 直接接管链接。
pub fn engine_status(ctx: &Ctx) -> serde_json::Value {
    let (kind, path) = match engine_kind(ctx) {
        Some(kv) => kv,
        None => {
            return serde_json::json!({
                "engine": serde_json::Value::Null,
                "path": serde_json::Value::Null,
                "installed": false,
                "running": false,
                "ready": false,
                "web_port": serde_json::Value::Null,
                "note": "未安装 ED2K 引擎：请到「设置 → 依赖工具」安装 eMule（社区版），或把自备的 mlnet.exe 放进工具目录",
            });
        }
    };
    let image = exe_name(kind);
    let running = process_running(&image);
    let (web_port, extra_note) = match kind {
        "emule" => {
            // 端口以受管 preferences.ini 为准（本程序写它）
            let ini = std::fs::read_to_string(emule_home(ctx).join("config").join("preferences.ini"))
                .unwrap_or_default();
            let port = read_ini_key(&ini, "WebServer", "Port")
                .and_then(|v| v.parse::<u16>().ok())
                .unwrap_or(EMULE_WEB_PORT);
            let note = if ctx.tools.emule.is_none() {
                " 注：eMule 路径直接取自受管目录（工具检测缓存未包含它）"
            } else {
                ""
            };
            (Some(port), note.to_string())
        }
        _ => (
            Some(MLNET_HTTP_PORT),
            " 注：mlDonkey 的 HTTP 提交/进度读取接口**未实测**（本机无 mlnet 二进制）".to_string(),
        ),
    };
    let note = if running {
        format!("{kind} 已在运行，可直接接管链接{extra_note}")
    } else {
        format!("{kind} 已安装但未运行，提交链接时会自动启动{extra_note}")
    };
    serde_json::json!({
        "engine": kind,
        "path": path.to_string_lossy().to_string(),
        "installed": true,
        "running": running,
        "ready": running,
        "web_port": web_port,
        "note": note,
    })
}

/// 把链接交给引擎（这是 ED2K 支持的入口）。
///
/// 流程：解析校验 → 引擎就绪检查（未安装报「设置 → 依赖工具」）→ 未运行则启动
/// → 投递链接 → 回读传输列表核对。
/// 投递通道：
/// - eMule：HTTP 远程加链（`handoff="http"`，能拿到 `result="OK"` 回执），
///   不可用时退回命令行转发（`handoff="protocol"`，仅在引擎已运行时有效）；
/// - mlDonkey：HTTP `/submit?q=dllink+<link>`（`handoff="http"`，**未实测**）。
/// eMule 未运行 → 启动并等 WebServer 就绪；然后在运行中的引擎上投递链接。
///
/// 顺序（每一步都有真机实测支撑，见模块文档第 2 节）：
/// 1. 先启动引擎（**不带链接**：0.72a 冷启动时命令行里的链接会被丢弃）；
/// 2. 等 WebServer 就绪（实测 1.5–3s，这里最多等 20s）；
/// 3. HTTP 远程加链；不可用则退回命令行转发（此时引擎已在运行，属有效的“热”路径）；
/// 4. 回读传输列表核对 hash，返回 `verified`，不靠“应该成功了”。
pub fn submit(ctx: &Ctx, link: &str) -> anyhow::Result<serde_json::Value> {
    // 1) 先校验链接（错误信息要说清哪一段不对）
    let parsed = parse(link)?;
    // 2) 引擎
    let (kind, exe) = engine_kind(ctx).ok_or_else(|| {
        anyhow::anyhow!("未找到 ED2K 引擎，请在「设置 → 依赖工具」安装 ED2K 引擎（eMule 社区版）")
    })?;
    let raw_link = link.trim().to_string();

    match kind {
        "emule" => {
            let (_ini, port, password) = ensure_emule_config(ctx)?;
            let home = emule_home(ctx);
            let mut started = false;
            let mut notes: Vec<String> = Vec::new();

            if !process_running(&exe_name("emule")) {
                let pid = spawn_engine(&exe, &home, &[])?;
                started = true;
                notes.push(format!("已启动 eMule（pid {pid}）"));
                if wait_for_webserver(port, Duration::from_secs(20)) {
                    notes.push(format!("WebServer 127.0.0.1:{port} 已就绪"));
                } else {
                    notes.push("WebServer 20s 内未就绪".to_string());
                }
            }

            // 3) 投递：HTTP 优先，命令行转发兜底
            let mut handoff = "protocol";
            match emule_web_add_link(port, &password, &raw_link) {
                Ok(body) if body.contains("result=\"OK\"") => {
                    handoff = "http";
                    notes.push(format!("已通过 WebServer(127.0.0.1:{port}) 远程加链，引擎回执 result=\"OK\""));
                }
                Ok(body) => {
                    let brief: String = body.chars().take(160).collect();
                    spawn_engine(&exe, &home, std::slice::from_ref(&raw_link))?;
                    notes.push(format!("WebServer 加链未回 OK（{brief}），已改用命令行转发"));
                }
                Err(e) => {
                    spawn_engine(&exe, &home, std::slice::from_ref(&raw_link))?;
                    notes.push(format!("WebServer 不可用（{e}），已改用命令行转发"));
                }
            }

            // 4) 回读核对（最多 3 次，约 6s）
            let verified = verify_in_queue(port, &password, &parsed.hash, 3);
            if handoff == "protocol" && !verified {
                notes.push("命令行转发未能确认入队（eMule 冷启动不接受带链接启动，热转发才有效）".into());
            }
            Ok(serde_json::json!({
                "engine": "emule",
                "started": started,
                "handoff": handoff,
                "web_port": port,
                "verified": verified,
                "note": format!(
                    "{}；链接 {}（{} 字节，hash {}）{}",
                    notes.join("；"),
                    parsed.name,
                    parsed.size,
                    parsed.hash,
                    if verified { "已在引擎传输列表中确认" } else { "未能在传输列表中确认（请打开 eMule 查看）" }
                ),
            }))
        }
        _ => {
            let port = MLNET_HTTP_PORT;
            let url = format!(
                "http://127.0.0.1:{port}/submit?q=dllink+{}",
                percent_encode(&raw_link)
            );
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(3))
                // MLDonkey 的 HTTP 接口用 Basic 认证，用户名 admin、密码即配置的口令
                .build()?;
            let resp = client.get(&url).basic_auth("admin", Some("")).send();
            match resp {
                Ok(r) => Ok(serde_json::json!({
                    "engine": "mlnet",
                    "started": false,
                    "handoff": "http",
                    "web_port": port,
                    "note": format!(
                        "已向 MLDonkey HTTP 接口提交（HTTP {}）：{url}；该分支**未实测**（本机无 mlnet 二进制），请以引擎实际队列为准",
                        r.status()
                    ),
                })),
                Err(e) => anyhow::bail!(
                    "MLDonkey HTTP 接口不可用（127.0.0.1:{port}：{e}）；请确认 mlnet 已启动并开启 http_port（默认 4080）。该分支未实测"
                ),
            }
        }
    }
}

/// 等 WebServer 起来（实测冷启动 1.5–3s 就绪）
fn wait_for_webserver(port: u16, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if http_get(&format!("http://127.0.0.1:{port}/")).is_ok() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// 登录并取传输列表页面
fn fetch_transfer_page(port: u16, password: &str) -> anyhow::Result<String> {
    let ses = emule_web_login(port, password)?;
    http_get(&format!("http://127.0.0.1:{port}/?ses={ses}&w=transfer"))
}

/// 传输列表页面里是否已出现某个 hash（大小写不敏感）
fn queue_has_hash(body: &str, hash: &str) -> bool {
    let want = hash.to_ascii_lowercase();
    parse_transfer_page(body)
        .iter()
        .any(|f| f.hash.to_ascii_lowercase() == want)
}

/// 回读传输列表，确认某个 hash 真的在引擎队列里（最多尝试 `tries` 次，每次隔 2s）
fn verify_in_queue(port: u16, password: &str, hash: &str, tries: u32) -> bool {
    for i in 0..tries.max(1) {
        if let Ok(body) = fetch_transfer_page(port, password) {
            if queue_has_hash(&body, hash) {
                return true;
            }
        }
        if i + 1 < tries {
            std::thread::sleep(Duration::from_secs(2));
        }
    }
    false
}

/// 尽力而为地读取进度（eMule WebServer）。
///
/// 拿不到就返回 `ok=false` + 说明，**绝不编造进度**。
/// 结构：`{ok, engine, web_port, files:[{name,hash,size,size_on_disk,status,completed,percent,sources}], note}`
pub fn poll_progress(ctx: &Ctx) -> serde_json::Value {
    let empty = |note: String| {
        serde_json::json!({
            "ok": false,
            "engine": serde_json::Value::Null,
            "web_port": serde_json::Value::Null,
            "files": Vec::<TransferItem>::new(),
            "note": note,
        })
    };
    let (kind, _exe) = match engine_kind(ctx) {
        Some(kv) => kv,
        None => return empty("未安装 ED2K 引擎，无法读取进度".into()),
    };
    if kind != "emule" {
        return serde_json::json!({
            "ok": false,
            "engine": kind,
            "web_port": MLNET_HTTP_PORT,
            "files": Vec::<TransferItem>::new(),
            "note": "mlDonkey 进度读取未实现（未实测，本机无 mlnet 二进制）",
        });
    }
    if !process_running(&exe_name("emule")) {
        return serde_json::json!({
            "ok": false,
            "engine": "emule",
            "web_port": serde_json::Value::Null,
            "files": Vec::<TransferItem>::new(),
            "note": "eMule 未在运行，暂无进度可读",
        });
    }
    let secret = match read_secret(ctx) {
        Some(s) => s,
        None => {
            return serde_json::json!({
                "ok": false,
                "engine": "emule",
                "web_port": serde_json::Value::Null,
                "files": Vec::<TransferItem>::new(),
                "note": "未找到受管 WebServer 配置（先提交一个链接或启动一次引擎即可生成）",
            })
        }
    };
    let body = match fetch_transfer_page(secret.port, &secret.password) {
        Ok(b) => b,
        Err(e) => {
            return serde_json::json!({
                "ok": false,
                "engine": "emule",
                "web_port": secret.port,
                "files": Vec::<TransferItem>::new(),
                "note": format!("读取传输列表失败（127.0.0.1:{}：{e}）", secret.port),
            })
        }
    };
    let files = parse_transfer_page(&body);
    serde_json::json!({
        "ok": true,
        "engine": "emule",
        "web_port": secret.port,
        "files": files,
        "note": format!("来自 eMule WebServer 传输列表（{} 个任务）", files.len()),
    })
}

/* ==================== 单元测试 ==================== */

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::{AppDirs, ToolPaths};
    use crate::models::AppSettings;

    fn temp_ctx(tag: &str) -> (Ctx, PathBuf) {
        let base = std::env::temp_dir().join(format!("umi-ed2k-test-{tag}-{}", uuid::Uuid::new_v4()));
        let data = base.join("data");
        let dirs = AppDirs {
            data: data.clone(),
            bin: data.join("bin"),
            models: data.join("models"),
            cache: data.join("cache"),
            downloads: base.join("downloads"),
            db_file: data.join("umi.db"),
            settings_file: data.join("settings.json"),
        };
        std::fs::create_dir_all(&dirs.data).expect("创建测试目录");
        let ctx = Ctx::new(dirs, ToolPaths::default(), AppSettings::default());
        (ctx, base)
    }

    /* ---------- 链接解析 ---------- */

    #[test]
    fn parses_plain_file_link() {
        let l = parse("ed2k://|file|ubuntu.iso|734003200|31D6CFE0D16AE931B73C59D7E0C089C0|/").unwrap();
        assert_eq!(l.name, "ubuntu.iso");
        assert_eq!(l.size, 734003200);
        assert_eq!(l.hash, "31d6cfe0d16ae931b73c59d7e0c089c0", "hash 应归一为小写");
        assert!(l.sources.is_empty());
        assert!(l.aich.is_none());
    }

    #[test]
    fn hash_is_case_insensitive_but_normalised() {
        for raw in ["31D6CFE0D16AE931B73C59D7E0C089C0", "31d6cfe0d16ae931b73c59d7e0c089c0"] {
            let l = parse(&format!("ed2k://|file|a.bin|1|{raw}|/")).unwrap();
            assert_eq!(l.hash, "31d6cfe0d16ae931b73c59d7e0c089c0");
        }
    }

    #[test]
    fn url_decodes_file_name() {
        // %20 空格、%E4%B8%AD%E6%96%87 中文、%7c 竖线（eMule 会先还原成分隔符，文件名里不能再出现）
        let l = parse("ed2k://|file|My%20Movie%20%E4%B8%AD%E6%96%87.mkv|1048576|31d6cfe0d16ae931b73c59d7e0c089c0|/").unwrap();
        assert_eq!(l.name, "My Movie 中文.mkv");
        // `+` 在 ed2k 链接里不是空格（eMule URLDecode 只解 %XX）
        let l2 = parse("ed2k://|file|a+b%2Bc.txt|10|31d6cfe0d16ae931b73c59d7e0c089c0|/").unwrap();
        assert_eq!(l2.name, "a+b+c.txt");
    }

    #[test]
    fn parses_aich_and_sources() {
        let link = "ed2k://|file|big.mkv|1073741824|a1b2c3d4e5f60718293a4b5c6d7e8f90|h=2GQFLQIJGMFHHZGXKZ2FVXVOZ3EYWPUZ|s=http://mirror1.example/a.mkv|s=http://mirror2.example/a.mkv|/";
        let l = parse(link).unwrap();
        assert_eq!(l.name, "big.mkv");
        assert_eq!(l.aich.as_deref(), Some("2GQFLQIJGMFHHZGXKZ2FVXVOZ3EYWPUZ"));
        assert_eq!(l.sources, vec!["http://mirror1.example/a.mkv", "http://mirror2.example/a.mkv"]);
    }

    #[test]
    fn parses_aich_and_source_in_any_order() {
        let l = parse("ed2k://|file|x.bin|7|a1b2c3d4e5f60718293a4b5c6d7e8f90|s=http://a/x|h=ABCDEFGHIJKLMNOPQRSTUVWXYZ234567|/").unwrap();
        assert_eq!(l.size, 7);
        assert_eq!(l.sources, vec!["http://a/x"]);
        assert_eq!(l.aich.as_deref(), Some("ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"));
    }

    #[test]
    fn tolerates_missing_trailing_slash_and_stray_whitespace() {
        let l = parse("  ed2k://|file|x.bin|10|a1b2c3d4e5f60718293a4b5c6d7e8f90  ").unwrap();
        assert_eq!(l.name, "x.bin");
        assert_eq!(l.size, 10);
    }

    #[test]
    fn reports_missing_name_segment() {
        let e = parse("ed2k://|file||10|a1b2c3d4e5f60718293a4b5c6d7e8f90|/").unwrap_err().to_string();
        assert!(e.contains("文件名段"), "错误信息应指出缺哪一段：{e}");
    }

    #[test]
    fn reports_missing_size_segment() {
        let e = parse("ed2k://|file|x.bin||a1b2c3d4e5f60718293a4b5c6d7e8f90|/").unwrap_err().to_string();
        assert!(e.contains("大小段"), "{e}");
    }

    #[test]
    fn reports_missing_hash_segment() {
        let e = parse("ed2k://|file|x.bin|10||/").unwrap_err().to_string();
        assert!(e.contains("hash 段"), "{e}");
    }

    #[test]
    fn reports_non_numeric_size() {
        let e = parse("ed2k://|file|x.bin|12MB|a1b2c3d4e5f60718293a4b5c6d7e8f90|/").unwrap_err().to_string();
        assert!(e.contains("不是数字") && e.contains("12MB"), "{e}");
    }

    #[test]
    fn reports_bad_hash() {
        let e = parse("ed2k://|file|x.bin|10|NOTAHASH|/").unwrap_err().to_string();
        assert!(e.contains("32 位十六进制"), "{e}");
        // 长度对但不是十六进制
        let e2 = parse("ed2k://|file|x.bin|10|zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz|/").unwrap_err().to_string();
        assert!(e2.contains("32 位十六进制"), "{e2}");
    }

    #[test]
    fn rejects_non_ed2k_link() {
        let e = parse("https://example.com/a.bin").unwrap_err().to_string();
        assert!(e.contains("不是 ed2k 链接"), "{e}");
        let e2 = parse("").unwrap_err().to_string();
        assert!(e2.contains("不是 ed2k 链接"), "{e2}");
    }

    #[test]
    fn server_link_gives_clear_error_and_dedicated_parser() {
        let e = parse("ed2k://|server|195.245.244.243|4661|/").unwrap_err().to_string();
        assert!(e.contains("服务器链接") && e.contains("parse_server"), "{e}");
        let s = parse_server("ed2k://|server|195.245.244.243|4661|/").unwrap();
        assert_eq!(s.host, "195.245.244.243");
        assert_eq!(s.port, 4661);
    }

    #[test]
    fn server_link_errors_are_specific() {
        let e = parse_server("ed2k://|server|1.2.3.4||/").unwrap_err().to_string();
        assert!(e.contains("端口段"), "{e}");
        let e2 = parse_server("ed2k://|server|1.2.3.4|70000|/").unwrap_err().to_string();
        assert!(e2.contains("端口"), "{e2}");
        let e3 = parse_server("ed2k://|file|x|1|a1b2c3d4e5f60718293a4b5c6d7e8f90|/").unwrap_err().to_string();
        assert!(e3.contains("不是 ed2k 服务器链接"), "{e3}");
    }

    #[test]
    fn rejects_other_link_kinds_with_clear_message() {
        for kind in ["serverlist", "nodeslist", "search"] {
            let e = parse(&format!("ed2k://|{kind}|whatever|/")).unwrap_err().to_string();
            assert!(e.contains("暂不支持"), "{kind}: {e}");
        }
    }

    #[test]
    fn is_ed2k_matches_all_kinds() {
        assert!(is_ed2k("ed2k://|file|a|1|a1b2c3d4e5f60718293a4b5c6d7e8f90|/"));
        assert!(is_ed2k("  ED2K://|server|1.2.3.4|4661|/"));
        assert!(!is_ed2k("magnet:?xt=urn:ed2k:abc"));
        assert!(!is_ed2k("https://example.com/ed2k://"));
        assert!(!is_ed2k(""));
    }

    #[test]
    fn percent_decode_matches_emule_url_decode() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("100%25"), "100%");
        assert_eq!(percent_decode("%E4%B8%AD%E6%96%87"), "中文");
        assert_eq!(percent_decode("a+b"), "a+b"); // `+` 不是空格
        assert_eq!(percent_decode("bad%zz"), "bad%zz"); // 非法转义原样保留
        assert_eq!(percent_decode("%00x"), "x"); // 控制字符被丢弃
    }

    /* ---------- 密码哈希（对照 eMule 源码与实测值） ---------- */

    #[test]
    fn md5_known_vectors() {
        assert_eq!(hex_upper(&md5(b"")), "D41D8CD98F00B204E9800998ECF8427E");
        assert_eq!(hex_upper(&md5(b"abc")), "900150983CD24FB0D6963F7D28E17F72");
        assert_eq!(hex_upper(&md5(b"message digest")), "F96B697D7CB7938D525A2F31AAF161D0");
        assert_eq!(
            hex_upper(&md5(b"12345678901234567890123456789012345678901234567890123456789012345678901234567890")),
            "57EDF4A22BE3C955AC49DA2E2107B67A"
        );
    }

    #[test]
    fn webserver_password_hash_matches_real_emule() {
        // 实测：preferences.ini 的 Password 值就是 MD5(明文密码的 UTF-16LE 字节) 的大写十六进制。
        // 下面这组值是在真机 eMule 0.72a 上验证通过的（用它可以登录 WebServer）。
        assert_eq!(
            web_password_hash("umilocal"),
            "88D23F7FF9916BD3C685DCD1F1E3BA96"
        );
        // 与 UTF-16LE 无关的 ASCII MD5 必须不同，防止退化成错误的实现
        assert_ne!(web_password_hash("umilocal"), hex_upper(&md5(b"umilocal")));
    }

    /* ---------- 受管配置 ---------- */

    #[test]
    fn ini_upsert_creates_section_and_key() {
        let ini = upsert_ini_key("", "WebServer", "Enabled", "1");
        assert!(ini.contains("[WebServer]") && ini.contains("Enabled=1"), "{ini}");
    }

    #[test]
    fn ini_upsert_replaces_without_touching_other_keys() {
        let src = "[eMule]\r\nNick=someone\r\n[WebServer]\r\nEnabled=0\r\nPort=1234\r\n";
        let out = upsert_ini_key(src, "WebServer", "Port", "4711");
        assert!(out.contains("Port=4711"), "{out}");
        assert!(!out.contains("Port=1234"), "{out}");
        assert!(out.contains("Enabled=0"), "其它键必须保留：{out}");
        assert!(out.contains("Nick=someone"), "{out}");
    }

    #[test]
    fn ensure_config_writes_managed_preferences_and_secret() {
        let (ctx, base) = temp_ctx("prefs");
        let (ini_path, port, pw) = ensure_emule_config(&ctx).unwrap();
        assert_eq!(port, EMULE_WEB_PORT);
        assert_eq!(pw.len(), 12);
        let text = std::fs::read_to_string(&ini_path).unwrap();
        // 1) 防首次向导 / 语言弹窗  2) 打开 WebServer  3) 关 gzip  4) 密码是大写 MD5(UTF-16LE)
        assert!(text.contains("AppVersion=0.72a"), "{text}");
        assert!(text.contains("Language=1033"), "{text}");
        assert!(read_ini_key(&text, "WebServer", "Enabled").as_deref() == Some("1"), "{text}");
        assert!(read_ini_key(&text, "WebServer", "UseGzip").as_deref() == Some("0"), "{text}");
        assert_eq!(
            read_ini_key(&text, "WebServer", "Password").unwrap(),
            web_password_hash(&pw)
        );
        // 幂等：再跑一次不应改变密码/端口，也不破坏用户已有键
        let with_user_key = format!("{text}[Extra]\r\nKeep=1\r\n");
        std::fs::write(&ini_path, &with_user_key).unwrap();
        let (_p, port2, pw2) = ensure_emule_config(&ctx).unwrap();
        assert_eq!((port2, pw2.clone()), (port, pw.clone()));
        let text2 = std::fs::read_to_string(&ini_path).unwrap();
        assert!(text2.contains("Keep=1"), "不应破坏用户已有的键：{text2}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /* ---------- 引擎状态（诚实返回） ---------- */

    #[test]
    fn engine_status_is_honest_without_engine() {
        let (ctx, base) = temp_ctx("noengine");
        let v = engine_status(&ctx);
        assert!(v["engine"].is_null(), "{v}");
        assert_eq!(v["installed"], false);
        assert_eq!(v["running"], false);
        assert_eq!(v["ready"], false);
        assert!(v["web_port"].is_null(), "{v}");
        let note = v["note"].as_str().unwrap();
        assert!(note.contains("未安装") || note.contains("未找到"), "{note}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn engine_status_reports_installed_but_not_running() {
        let (ctx, base) = temp_ctx("installed");
        let dir = emule_home(&ctx);
        std::fs::create_dir_all(&dir).unwrap();
        // 只放一个占位文件（不执行），避免测试去跑真实引擎
        std::fs::write(dir.join(exe_name("emule")), b"stub").unwrap();
        let v = engine_status(&ctx);
        assert_eq!(v["engine"], "emule");
        assert_eq!(v["installed"], true);
        assert_eq!(v["web_port"], EMULE_WEB_PORT as u64);
        // `running` 反映的是本机真实进程（可能真有 eMule 在跑），所以这里断言的是**不变量**，
        // 而不是写死的 false —— 否则本机开着 eMule 时测试就会假失败。
        let real = process_running(&exe_name("emule"));
        assert_eq!(v["running"], real, "running 必须与本机真实进程状态一致");
        assert_eq!(v["ready"], real, "ready = 已安装且正在运行");
        // `path` 必须**正好**是受管目录里的引擎可执行文件。
        // 可执行文件名的平台差异（Windows 带 `.exe`，类 Unix 不带）由 `exe_name()` 统一表达，
        // 这里写死 "emule.exe" 只会在 Linux 上假失败 —— 那边受管目录里就是 `.../emule/emule`。
        let path = v["path"].as_str().unwrap();
        assert_eq!(
            std::path::Path::new(path),
            dir.join(exe_name("emule")).as_path(),
            "path 应指向受管目录里的引擎可执行文件（受管目录 {dir:?}）"
        );
        // Windows 的平台专属保证照旧钉死：不能被「跨平台化」放松
        #[cfg(windows)]
        assert!(
            path.to_lowercase().ends_with("emule.exe"),
            "Windows 上受管引擎必须是 emule.exe：{path}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn submit_without_engine_tells_user_where_to_install() {
        let (ctx, base) = temp_ctx("noengine-submit");
        let e = submit(&ctx, "ed2k://|file|a.bin|1|a1b2c3d4e5f60718293a4b5c6d7e8f90|/")
            .unwrap_err()
            .to_string();
        assert!(e.contains("设置 → 依赖工具"), "{e}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn submit_validates_link_before_engine_lookup() {
        let (ctx, base) = temp_ctx("badlink-submit");
        let e = submit(&ctx, "ed2k://|file|a.bin|abc|a1b2c3d4e5f60718293a4b5c6d7e8f90|/")
            .unwrap_err()
            .to_string();
        assert!(e.contains("不是数字"), "应先报链接错误：{e}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn poll_progress_is_honest_without_engine() {
        let (ctx, base) = temp_ctx("poll");
        let v = poll_progress(&ctx);
        assert_eq!(v["ok"], false);
        assert!(v["files"].as_array().unwrap().is_empty());
        assert!(!v["note"].as_str().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    /* ---------- 传输列表解析（用真机抓到的页面片段） ---------- */

    #[test]
    fn parses_transfer_page_from_real_capture() {
        // 这段 HTML 是 eMule 0.72a 真机页面（?ses=…&w=transfer）的原样片段：
        // 注意工具提示用**字面反斜杠+n** 作分隔符（原始字符串里写作 \n）
        let html = r##"<tr>
  <td valign=top class="down-line-waiting-left" nowrap><table><tr><td>
  <a href="#" onMouseover="downmenu(event,'admin','umi-ed2k-handoff-cli.txt\nHash: A1B2C3D4E5F60718293A4B5C6D7E8F90\nSize: 2.00 MB    (Size on disk: 0 Bytes)\n\npart.met file: 002.part.met\nStatus: Waiting\nCompleted: 0 Bytes/2.00 MB (0.0%)\nSources: 0  (Useful: 0, NNP: 0);  A4AF: 0\nParts: 1, Available: 0 (0.0%)\n\nSeen complete: Never\nLast reception: 星期日, 2026/9/27 18:18:03','ed2k://|file|umi-ed2k-handoff-cli.txt|2097152|A1B2C3D4E5F60718293A4B5C6D7E8F90|/','waiting','disabled')" onMouseout="delayhidemenu()">
  <img src="t_waiting.gif"></a>
  </td></tr></table></td>
</tr>"##;
        let items = parse_transfer_page(html);
        assert_eq!(items.len(), 1, "应解析出 1 个任务");
        let it = &items[0];
        assert_eq!(it.name, "umi-ed2k-handoff-cli.txt");
        assert_eq!(it.hash, "A1B2C3D4E5F60718293A4B5C6D7E8F90");
        assert_eq!(it.size, "2.00 MB");
        assert_eq!(it.size_on_disk, "0 Bytes");
        assert_eq!(it.status, "Waiting");
        assert_eq!(it.completed, "0 Bytes/2.00 MB (0.0%)");
        assert_eq!(it.percent, 0.0);
        assert_eq!(it.sources, 0);
    }

    #[test]
    fn parses_transfer_page_with_progress_and_sources() {
        let html = r#"downmenu(event,'admin','movie.mkv\nHash: 31D6CFE0D16AE931B73C59D7E0C089C0\nSize: 700.00 MB    (Size on disk: 12.50 MB)\n\npart.met file: 001.part.met\nStatus: Downloading\nCompleted: 12.50 MB/700.00 MB (1.8%)\nSources: 42  (Useful: 30, NNP: 2);  A4AF: 3\nParts: 700, Available: 12 (1.8%)','ed2k://|file|movie.mkv|734003200|31d6cfe0d16ae931b73c59d7e0c089c0|/',"#;
        let items = parse_transfer_page(html);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].status, "Downloading");
        assert_eq!(items[0].sources, 42);
        assert!((items[0].percent - 1.8).abs() < 1e-9, "{:?}", items[0]);
        assert_eq!(items[0].size_on_disk, "12.50 MB");
    }

    #[test]
    fn transfer_page_parser_ignores_login_screen() {
        let items = parse_transfer_page("<html><body>Enter your password here</body></html>");
        assert!(items.is_empty());
    }

    #[test]
    fn percent_encode_escapes_link_chars() {
        let enc = percent_encode("ed2k://|file|a b.bin|1|abc|/");
        assert!(!enc.contains(' '), "{enc}");
        assert!(enc.starts_with("ed2k%3A%2F%2F%7Cfile%7C"), "{enc}");
    }

    #[test]
    fn secret_round_trip() {
        let (ctx, base) = temp_ctx("secret");
        assert!(read_secret(&ctx).is_none());
        let s = WebSecret { port: 4711, password: "abcdefghijkl".into() };
        std::fs::create_dir_all(emule_home(&ctx)).unwrap();
        std::fs::write(secret_path(&ctx), serde_json::to_string(&s).unwrap()).unwrap();
        let back = read_secret(&ctx).unwrap();
        assert_eq!((back.port, back.password), (s.port, s.password));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn random_password_is_stable_length() {
        let p = random_password();
        assert_eq!(p.len(), 12);
        assert!(p.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(random_password(), random_password());
    }

    #[test]
    fn queue_has_hash_uses_real_transfer_page() {
        // 真机页面片段（同一个 hash 出现在工具提示里）
        let html = r##"downmenu(event,'admin','chan-http.txt\nHash: C1B2C3D4E5F60718293A4B5C6D7E8F92\nSize: 2.00 MB    (Size on disk: 0 Bytes)\n\npart.met file: 002.part.met\nStatus: Waiting\nCompleted: 0 Bytes/2.00 MB (0.0%)\nSources: 0','ed2k://|file|chan-http.txt|2097152|c1b2c3d4e5f60718293a4b5c6d7e8f92|/',"##;
        assert!(queue_has_hash(html, "c1b2c3d4e5f60718293a4b5c6d7e8f92"), "大写传入也要命中");
        assert!(queue_has_hash(html, "C1B2C3D4E5F60718293A4B5C6D7E8F92"));
        assert!(!queue_has_hash(html, "00000000000000000000000000000000"), "不该误报");
        assert!(!queue_has_hash("<html>login</html>", "c1b2c3d4e5f60718293a4b5c6d7e8f92"));
    }

    #[test]
    fn wait_for_webserver_times_out_honestly() {
        // 没人监听的端口：必须如实返回 false（不能假装成功）
        let t0 = std::time::Instant::now();
        let ok = wait_for_webserver(45999, Duration::from_secs(1));
        let el = t0.elapsed();
        assert!(!ok, "无监听端口不应返回 true");
        assert!(el >= Duration::from_millis(900) && el < Duration::from_secs(5), "耗时 {el:?}");
    }
}
