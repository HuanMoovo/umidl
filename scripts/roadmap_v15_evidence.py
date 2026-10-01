"""ROADMAP 收尾：加 1.5.0 验收证据表，并重写"未实现清单"（ED2K/文档/插件已完成）。幂等。"""
import sys

P = "docs/ROADMAP.md"
raw = open(P, encoding="utf-8", newline="").read()
crlf = "\r\n" in raw
s = raw.replace("\r\n", "\n")
orig = s
done = []

TABLE = """## 验收证据（1.5.0 · 本机实机）

| 项目 | 结果 |
|------|------|
| `npx vue-tsc --noEmit` | 0 错误 |
| `npx vitest run` | 60 / 60 通过 |
| `cargo test --lib` | **101 / 101 通过**（原有 20 + 文档 33 + 插件 13 + ED2K 35） |
| i18n 一致性 | 四语言各 544 条，key 集合完全一致、运行时文案零缺失 |
| `scripts/verify_v15.py` | **38 / 38 通过** |
| 安装包 | `Umidl_1.5.0_x64-setup.exe`（4.36 MiB） |

1.5 新增三项功能各自的**真证据**：

**文档转换（第 4 项补齐）**
- 真实 `docx → md`：产出 95 字符，首行 `# 文档转换验收标题`（pandoc 生成的真 docx）
- 真实 `PDF → txt`：产出 39 字符，含 `Umidl Doc Verify` / `token bucket 300 KB/s`（poppler `pdftotext`）
- 真实 `xlsx → csv`：产出 `项目,数值` / `令牌桶,300 KB/s`（**纯 Rust 原生解析**，不经任何外部程序）
- `probe_document` 对三种真文件都识别出正确元信息（页数 / 工作表 / 字符数）
- 实测揪出的真 bug：部分 `pdftotext` 构建（Git for Windows 自带）在省略输出文件时**什么都不写**，必须显式传 `-`，否则抽取静默返回空串

**ED2K（第 2 项补齐）**
- 解析：hash / 文件名 / 大小 / AICH / 源；非法链接报"大小段不是数字：`notanumber`（应为字节数，例如 734003200）"
- 引擎接管：冷启动 eMule（pid 13456）→ WebServer `127.0.0.1:4711` 就绪 → HTTP 远程加链 → 引擎回执 `result="OK"` → **回读传输列表确认 hash 在列**（`verified: true`）
- 队列联动：ed2k 链接的路由判定为 `ed2k`（不再落到 yt-dlp/aria2），任务卡写回链接里的真实文件名与大小
- 实测确认的引擎行为（写进了代码注释）：eMule 0.72a **不支持 `-c` 指定配置目录**（源码 `ProcessCommandline` 只认 `-ignoreinstances` / `-AutoStart`），改用"程序目录便携模式"；**冷启动会丢弃 argv 里的链接**，必须等 WebServer 就绪后走 HTTP 加链

**插件沙箱（第 6 项补齐）**
- 沙箱自述：`quickjs 0.16.2`、内存上限 16 MB、单脚本时间预算 200 ms
- 越权隔离：插件侧 `require` / `process` / `fetch` / `XMLHttpRequest` 全为 undefined；恶意插件抛错后**宿主进程存活**
- 真中断：`while(true){}` 在 199.47 ms 被中断（预算 200 ms）；16 MB 上限下分配大数组报 `out of memory`
- 内容寻址：市场文件被改动一个字节 → 安装被拒（期望 `a8ac1782…`，实际 `2b…`）
- 端到端：市场 2 条（`direct-link-sniffer` / `retry-hook`）→ 安装 → 列出 → 沙箱试跑 → 解析器对 URL 表态 → 卸载

"""

anchor = "## 未实现清单（如需要请明确优先级）"
if "## 验收证据（1.5.0 · 本机实机）" in s:
    done.append("跳过证据表（已存在）")
else:
    if s.count(anchor) != 1:
        sys.exit(f"锚点命中 {s.count(anchor)} 次")
    s = s.replace(anchor, TABLE + anchor, 1)
    done.append("已插入 1.5.0 证据表")

OLD_LIST = """## 未实现清单（如需要请明确优先级）

1. **插件沙箱与插件市场**（第 6 项）——最大的缺口，建议单独立项。
2. **ED2K 电驴链接**（第 2 项）——无可靠开源引擎。
3. **文档格式转换**（第 4 项）——需要 LibreOffice/pandoc 级依赖。
4. **浏览器扩展本体**（第 8 项）——接口已就绪，扩展可另行开发（可与插件层一起做）。
5. **随包自带的 macOS / Linux 安装包**（第 10 项）——需 CI 或对应平台机器。"""

NEW_LIST = """## 仍未做完的部分（诚实清单）

1. **mlDonkey 分支未实测**（ED2K 的备用引擎）——本机没有可用二进制，代码与返回里都显式标注"未实测"，接入点按官方 Browser Integration 文档实现。
2. **浏览器扩展本体**（第 8 项）——捕获接口、过滤、真实入队、插件重试钩子都已就绪，扩展 UI 尚未开发（可用插件层写）。
3. **随包自带的 macOS / Linux 安装包**（第 10 项）——`.github/workflows/release.yml` 已覆盖三平台，但**只在本机验证过 Windows**；macOS/Linux 的 aria2 无官方静态包，安装引导已给出 brew/apt 指引。
4. **插件市场的远端分发**——当前是内置市场（首次运行生成 + sha256 真实计算），远端索引拉取与签名发布链路尚未接。
5. **文档格式的边界**——扫描件 PDF 的 OCR、加密 PDF、宏文档（docm/xlsm）的执行语义不在支持范围内（前者需 OCR 引擎，后者按安全策略不执行宏）。
6. **一处编译告警**——`plugins.rs` 里的 `eval_text` 目前未被调用（保留给后续"插件调试台"），`cargo build` 会报一条 dead_code 警告。"""

if OLD_LIST in s:
    s = s.replace(OLD_LIST, NEW_LIST, 1)
    done.append("已重写未实现清单")
elif NEW_LIST in s:
    done.append("跳过清单（已重写）")
else:
    sys.exit("未实现清单锚点未命中")

if s != orig:
    open(P, "w", encoding="utf-8", newline="").write(s.replace("\n", "\r\n") if crlf else s)
print("\n".join(done))
