"""ROADMAP + README 更新到 1.6。幂等。"""
import sys

def edit(path, pairs, required=True):
    raw = open(path, encoding="utf-8", newline="").read()
    crlf = "\r\n" in raw
    s = raw.replace("\r\n", "\n")
    orig = s
    out = []
    for old, new in pairs:
        if new and new in s:
            out.append(f"跳过（已更新）：{old[:26]}…")
            continue
        n = s.count(old)
        if n != 1:
            if required:
                sys.exit(f"{path}: 锚点命中 {n} 次（期望 1）：{old[:60]}")
            out.append(f"!! 未命中：{old[:40]}")
            continue
        s = s.replace(old, new, 1)
        out.append(f"已更新：{old[:30]}…")
    if s != orig:
        open(path, "w", encoding="utf-8", newline="").write(s.replace("\n", "\r\n") if crlf else s)
    return out

# ── ROADMAP：追加 1.6 章节 ────────────────────────────────────────────────
ROADMAP_SEC = """## 1.6.0 新增（本轮 5 项需求）

| # | 需求 | 结果 | 实测证据 |
|---|------|------|----------|
| 1 | 转换模块整理通用功能 + 格式类型选项，主流音/视/图/文档格式 60 种以上 | ✅ | `convert_formats()` 真机探测 **88 种**：视频 22 / 音频 21 / 图片 22 / 文档 23（可用性来自本机 ffmpeg / pandoc / poppler 真实能力，13 种标为不可用并置灰）；转换页顶部显示"共 88 种格式"与四类计数，类型分段（全部/视频/音频/图片/文档）可切换，选不同类型只显示对应参数面板 |
| 2 | 插件模块移动到设置里 | ✅ | 侧边栏只剩 首页/下载/转换/字幕/设置；`#/plugins` 重定向到 `#/settings?tab=plugins`（旧链接不死）；设置页第 7 个页签"插件"渲染沙箱信息/已装插件/市场/事件日志，既有开关·测试·安装逻辑原样可用 |
| 3 | 字幕模块导出语言可选多个 | ✅ | 「识别语言（可多选）」芯片组；选 3 个语言 → 按钮"开始转写（3 个任务）"→ 真实发起 3 次 `start_subtitle`；再开「翻译成英文」→ 6 个任务（每语言额外一份英文）；队列卡片逐条标注语言 |
| 4 | 视频下载支持多链接导入与 txt 导入自动下载 | ✅ | 新增 `enqueue_links(text, output_dir)` 后端命令 + 下载页批量导入面板：5 行文本（含注释/空行/重复/被过滤项）→ 识别 4 条/去重 3 条 → 后端返回 `added=2 skipped=1`，任务真实下载完成 **2,475,379 字节**；txt 导入走真实文件读取并回填文本框；后端错误原文直接展示 |
| 5 | 设置里可调整多线程数 | ✅ | 新增 5 个可调参数：同时任务数 `concurrency`(1-8)、aria2 单服务器连接数 `aria2_connections`(1-16)、分段数 `aria2_split`(1-16)、最小分片 MB `aria2_min_split_mb`(1-64)、yt-dlp 分片并发 `ytdlp_concurrency`(1-16)；真实写入 aria2 `--max-connection-per-server/--split/--min-split-size` 与 yt-dlp `--concurrent-fragments`；越界值实测被限幅（999→16、0→1、99→8） |

### 1.6.0 验收证据

| 项目 | 结果 |
|------|------|
| `cargo test --lib` | **115 / 115**（1.5 的 101 + 本轮 14） |
| `npx vue-tsc --noEmit` | 0 错误 |
| `npx vitest run` | **67 / 67**（1.5 的 60 + 批量导入解析 7） |
| i18n 一致性 | 四语言各 604 条，key 完全一致、运行时零缺失 |
| `scripts/verify_v16.py` | **25 / 25** |
| 安装包 | `Umidl_1.6.0_x64-setup.exe` |

本轮修掉的**真 bug**：`enqueue_links` 最初按"空白+逗号"切分整段文本，导致注释行 `# 这是一行注释，应被跳过` 被逗号切断、后半截 `应被跳过` 被当成链接入队（前端解析器当时已在自己的注释里标注了这个后端问题）。改为**按整行**忽略注释、仅在行内切词后，同一份输入从 `added=4`（3 条垃圾链接）变为 `added=2 skipped=1`。

"""
try:
    print("\n".join(edit("docs/ROADMAP.md", [("## 仍未做完的部分（诚实清单）", ROADMAP_SEC + "## 仍未做完的部分（诚实清单）")])))
except SystemExit as e:
    print("ROADMAP:", e)

# ── README ───────────────────────────────────────────────────────────────
READ_PAIRS = [
    (
        "### 格式转换（FFmpeg + 原生解析 + pandoc/poppler）",
        "### 格式转换（88 种格式 · FFmpeg + 原生解析 + pandoc/poppler）\n"
        "- **类型选项**：全部 / 视频 / 音频 / 图片 / 文档 五档分段选择，网格里显示每种格式的可用状态（本机引擎不支持则置灰并给出原因）\n"
        "- **共 88 种**（真机探测）：视频 22 · 音频 21 · 图片 22 · 文档 23；选不同类型自动切换对应参数面板（编码/分辨率/CRF 只在视频/音频目标下出现）",
    ),
    (
        "- **浏览器捕获**：本地回环接口",
        "- **多链接 / txt 批量导入（1.6 新增）**：一次粘贴多行链接或导入 txt 文件，按扩展名/域名/大小过滤后批量入队，结果面板给出\"已加入 / 拦截 / 跳过\"与逐条原因\n"
        "- **多线程与并发（1.6 新增）**：同时任务数与每个下载的连接数/分段数/最小分片可调（aria2 与 yt-dlp 分别生效），越界自动限幅\n"
        "- **浏览器捕获**：本地回环接口",
    ),
    (
        "- **插件市场（内容寻址校验）**：内置示例插件一键安装；安装前比对 **sha256**，文件被改动一个字节即拒绝安装",
        "- **插件市场（内容寻址校验）**：内置示例插件一键安装；安装前比对 **sha256**，文件被改动一个字节即拒绝安装\n"
        "- 插件管理界面位于 **设置 → 插件**（1.6 起从侧边栏独立页迁入）",
    ),
    (
        "- 导出格式：SRT / ASS / VTT / TXT / JSON",
        "- 导出格式：SRT / ASS / VTT / TXT / JSON\n"
        "- **识别语言可多选（1.6 新增）**：每个选中语言各起一个任务，可叠加\"翻译成英文\"再生成一份英文轨",
    ),
]
try:
    print("\n".join(edit("README.md", READ_PAIRS)))
except SystemExit as e:
    print("README:", e)
