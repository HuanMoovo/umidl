"""ROADMAP 三项状态更新（1.5：ED2K / 文档转换 / 插件沙箱）。幂等。"""
import sys

P = "docs/ROADMAP.md"
raw = open(P, encoding="utf-8", newline="").read()
crlf = "\r\n" in raw
s = raw.replace("\r\n", "\n")
orig = s
done = []

EDITS = [
    # 需求 2：全协议——ED2K 补上
    (
        "HLS/DASH、分段并行、断点续传 | 🟡 |",
        "HLS/DASH、分段并行、断点续传 | ✅ |",
    ),
    (
        "**ED2K 无可用开源引擎，未实现**（界面明确报错）",
        "**ED2K 已实现**：ed2k 链接解析（hash / 大小 / 文件名 / AICH / 源，逐段错误定位）+ **eMule 0.72a 社区版引擎托管接管**"
        "（受管目录 + 预置 `config/preferences.ini` 便携模式 + WebServer 就绪后 HTTP 远程加链，并回读传输列表核对 hash；"
        "冷启动丢弃 argv 链接的引擎行为已实测确认并绕开）",
    ),
    # 需求 4：文档格式——补上
    (
        "自动识别 ✅；**文档格式（docx/xlsx/pdf）未实现**；队列双布局 ✅",
        "自动识别 ✅；**文档 ✅**（原生纯 Rust 解析 docx / xlsx / pptx / odt / ods / odp → txt / md / html / csv，"
        "pandoc 富格式互转 docx·odt·rtf·epub·html·md，poppler 抽取 PDF 文本与页面）；队列双布局 ✅",
    ),
    (
        "双布局 | 🟡 | 音频 ✅",
        "双布局 | ✅ | 音频 ✅",
    ),
    # 需求 6：插件沙箱——从 ⛔ 到 ✅
    (
        "| 6 | JavaScript 插件沙箱 + 链接解析器 + 通知钩子 + 插件市场（内容寻址校验） | ⛔ | 未实现。需要嵌入 JS 运行时（boa/deno_core）+ 能力白名单 + 清单签名与内容寻址校验，工作量约等于本次 1.4 全部内容，建议独立立项 |",
        "| 6 | JavaScript 插件沙箱 + 链接解析器 + 通知钩子 + 插件市场（内容寻址校验） | ✅ | **rquickjs（QuickJS）沙箱**：16 MB 内存上限 + 512 KB 栈上限 + 200 ms 墙钟时间预算（interrupt handler 真中断 `while(true)`）；"
        "不注入任何宿主对象（`require`/`process`/`fetch`/`XMLHttpRequest` 全部 undefined，恶意插件被隔离且宿主存活），只暴露白名单 `umi.*`；"
        "**链接解析器**（`umi.registerResolver`，参与路由判定）、**通知钩子与命令式重试**（`download:error` → `umi.retry`，自带次数上限不无限重试）、"
        "**插件市场**：内置两个可运行示例 + 以文件 sha256 做内容寻址校验，**改一个字节即拒绝安装** |",
    ),
]

for old, new in EDITS:
    if new.split("|")[0][:12] in s and old not in s:
        done.append(f"跳过（已更新）：{old[:36]}…")
        continue
    n = s.count(old)
    if n != 1:
        sys.exit(f"锚点命中 {n} 次（期望 1）：{old[:60]}")
    s = s.replace(old, new, 1)
    done.append(f"已更新：{old[:40]}…")

if s != orig:
    open(P, "w", encoding="utf-8", newline="").write(s.replace("\n", "\r\n") if crlf else s)
print("\n".join(done))
