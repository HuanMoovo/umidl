"""README 更新到 1.5：ED2K、文档转换、插件沙箱。幂等。"""
import sys

P = "README.md"
raw = open(P, encoding="utf-8", newline="").read()
crlf = "\r\n" in raw
s = raw.replace("\r\n", "\n")
orig = s
done = []

EDITS = [
    # 标题：双引擎 → 三引擎
    (
        "### 下载引擎（双引擎自动路由 · 1.4 新增）",
        "### 下载引擎（三引擎自动路由 · 1.5 更新）",
    ),
    # ED2K 补进引擎清单
    (
        "- 引擎可在设置里指定「自动 / yt-dlp / aria2」，自动模式按链接类型路由",
        "- **eMule 电驴引擎（1.5 新增）**：ed2k 链接（HTTP/HTTPS/FTP/BT/磁力/HLS/DASH 之后的又一种协议）"
        "由托管 eMule 引擎接管，解析链接里的文件名 / 大小 / hash / AICH，交接后回读引擎任务列表核对\n"
        "- 引擎可在设置里指定「自动 / yt-dlp / aria2」，自动模式按链接类型路由（ed2k 恒走电驴引擎）",
    ),
    # 格式转换：补文档与图片
    (
        "### 格式转换（FFmpeg 内核）\n- 视频：MP4 / MKV / MOV / AVI / WEBM / FLV",
        "### 格式转换（FFmpeg + 原生解析 + pandoc/poppler）\n"
        "- **文档（1.5 新增）**：docx / xlsx / pptx / odt / ods / odp **原生纯 Rust 解析**为 txt / md / html / csv（不依赖任何外部运行时）；"
        "pandoc 负责富格式互转（docx·odt·rtf·epub·html·md）；poppler 负责抽取 PDF 文本；选中即显示页数 / 工作表 / 幻灯片 / 字符数\n"
        "- **图片（1.5 新增）**：PNG / JPG / WEBP / BMP / TIFF / ICO\n"
        "- 视频：MP4 / MKV / MOV / AVI / WEBM / FLV",
    ),
    # 新增插件沙箱章节（放到视频下载之前）
    (
        "### 视频下载（yt-dlp 内核）",
        "### 插件沙箱（JavaScript · 1.5 新增）\n"
        "- **QuickJS 沙箱运行时**：16 MB 内存上限、512 KB 栈上限、200 ms 单次执行时间预算，"
        "越权脚本被真中断（`while(true)` 也拦得住），不注入 `require` / `process` / `fetch` / 文件 API\n"
        "- **白名单能力**：只暴露 `umi.log / resolve / registerResolver / on / retry / storage`\n"
        "- **链接解析器**：插件注册的解析器参与「这条链接走哪个引擎」的判定，设置页可实时看到插件意见\n"
        "- **通知钩子 + 命令式重试**：`download:done` / `download:error` / `app:start` 事件推给插件，"
        "插件可调 `umi.retry(taskId)` 让失败任务重试（自带次数上限，不会无限重试）\n"
        "- **插件市场（内容寻址校验）**：内置示例插件一键安装；安装前比对 **sha256**，文件被改动一个字节即拒绝安装\n"
        "\n"
        "### 视频下载（yt-dlp 内核）",
    ),
]

for old, new in EDITS:
    if old in s and new in s:
        done.append(f"跳过（已更新）：{old[:30]}…")
        continue
    n = s.count(old)
    if n != 1:
        sys.exit(f"锚点命中 {n} 次（期望 1）：{old[:60]}")
    s = s.replace(old, new, 1)
    done.append(f"已更新：{old[:34]}…")

if s != orig:
    open(P, "w", encoding="utf-8", newline="").write(s.replace("\n", "\r\n") if crlf else s)
print("\n".join(done))
