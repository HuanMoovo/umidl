"""ROADMAP + README 更新到 1.7（ED2K 并入下载、转换页统一 UI）。幂等。"""
import sys

def edit(path, pairs):
    raw = open(path, encoding="utf-8", newline="").read()
    crlf = "\r\n" in raw
    s = raw.replace("\r\n", "\n")
    orig = s
    out = []
    for old, new in pairs:
        if new in s:
            out.append(f"跳过：{old[:24]}…")
            continue
        n = s.count(old)
        if n != 1:
            sys.exit(f"{path}: 锚点命中 {n} 次：{old[:50]}")
        s = s.replace(old, new, 1)
        out.append(f"已更新：{old[:28]}…")
    if s != orig:
        open(path, "w", encoding="utf-8", newline="").write(s.replace("\n", "\r\n") if crlf else s)
    return out

SEC = """## 1.7.0 新增（界面收敛）

| 需求 | 结果 | 实测证据 |
|------|------|----------|
| ED2K 电驴引擎整合到下载里 | ✅ | ED2K 面板抽成 `src/components/Ed2kPanel.vue` 并嵌入下载页（`SettingsEd2k.vue` 已删除）：引擎状态/一键安装（带 `tool://progress` 进度）/粘贴即时解析（文件名·大小·源数·AICH）/「交给引擎下载」/链接类型即时提示。实测：粘 ed2k 链接出现「检测到 ED2K 链接 · 将由 eMule 引擎接管」（粘 https 不出现），交接走到 `handoff: http` + 引擎回执 `result="OK"`；单链接按钮对 ed2k 也用，任务卡显示「ED2K · 已交由引擎接管」与链接里的真实文件名/大小；设置 → 系统与集成 **页面上 ED2K/eMule 文本为 0**，只剩 浏览器捕获 / 系统与电源 / 更新 / 诊断 四张卡 |
| 转换格式统一 UI + 自定义选类型/目标格式，减少页面占用 | ✅ | 转换页由 4 张大卡收敛为 **2 张**（统一流程卡 + 队列），页面高度 **2964 → 1071 px**（有任务时 1194）；一套「文件区 → 单一格式选择器（类型芯片 + 搜索 + 网格）→ 按类型显隐的参数区 → 一行引擎状态条 → 队列」；实测 `[data-role=format-grid]=1`、历史区块 `doc-capability/format-catalog/param-panel = 0`；类型筛选与后端目录逐类一致：全部 88 / 视频 22 / 音频 21 / 图片 22 / 文档 23（后端 22/21/22/23）；搜索 `mp4` 在视频类下 22 → 2 条；全部视图 13 项、音频视图 3 项按真实引擎能力置灰 |

### 1.7.0 验收证据

| 项目 | 结果 |
|------|------|
| `cargo test --lib` | 115 / 115 |
| `npx vue-tsc --noEmit` | 0 错误 |
| `npx vitest run` | 67 / 67 |
| i18n 一致性 | 四语言各 622 条，key 完全一致、运行时零缺失 |
| `scripts/verify_v17.py` | **22 / 22** |
| 安装包 | `Umidl_1.7.0_x64-setup.exe`（4.39 MiB） |

"""
print("\n".join(edit("docs/ROADMAP.md", [("## 仍未做完的部分（诚实清单）", SEC + "## 仍未做完的部分（诚实清单）")])))

print("\n".join(edit("README.md", [
    (
        "- **多链接 / txt 批量导入（1.6 新增）**",
        "- **ED2K 电驴引擎（1.7 起内置在下载页）**：`ed2k://` 链接交给托管 eMule 引擎接管，面板内可看引擎状态、一键安装、粘贴即时解析（文件名/大小/源/AICH）并交接\n"
        "- **多链接 / txt 批量导入（1.6 新增）**",
    ),
    (
        "- **类型选项**：全部 / 视频 / 音频 / 图片 / 文档 五档分段选择，网格里显示每种格式的可用状态（本机引擎不支持则置灰并给出原因）",
        "- **统一流程（1.7 起收敛为一张卡）**：文件区 → 单一格式选择器（类型芯片 + 搜索 + 目标格式网格）→ 按类型显隐的参数区 → 一行引擎状态条 → 队列；页面高度比 1.6 减少约 64%\n"
        "- **类型选项**：全部 / 视频 / 音频 / 图片 / 文档 五档分段选择，网格里显示每种格式的可用状态（本机引擎不支持则置灰并给出原因），支持按格式名/说明搜索",
    ),
])))
