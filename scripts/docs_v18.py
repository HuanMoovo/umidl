"""ROADMAP + README 更新到 1.8（格式全面可用 / UI 收敛 / bug 修复轮）。幂等。"""
import sys

def edit(path, pairs):
    raw = open(path, encoding="utf-8", newline="").read()
    crlf = "\r\n" in raw
    s = raw.replace("\r\n", "\n")
    out = []
    for old, new in pairs:
        if new in s:
            out.append(f"跳过：{old[:26]}…")
            continue
        if s.count(old) != 1:
            sys.exit(f"{path}: 锚点命中 {s.count(old)} 次：{old[:50]}")
        s = s.replace(old, new, 1)
        out.append(f"已更新：{old[:26]}…")
    open(path, "w", encoding="utf-8", newline="").write(s.replace("\n", "\r\n") if crlf else s)
    return out

SEC = """## 1.8.0 新增（格式全面可用 / UI 收敛 / bug 修复轮）

| 需求 | 结果 | 实测证据 |
|------|------|----------|
| 修复转换队列预览 | ✅ | 队列卡片的“转换预览”此前是死按钮、且会把存在的产物误报成「文件已丢失」、源/目标名还会错位。现改为显示 `源文件名 真实大小 → 目标格式 产物大小`（新增 `file_sizes` 命令批量取真实字节数），预览按钮可用，文件真被移走时才提示 |
| 灰掉的格式也要能用（要全面） | ✅ 13 → 8 | 根因是能力判定把「格式名」当编码器名比对：ALAC 编码器其实存在（写 m4a）、DTS 的编码器真名是 `dca`（实验性标志，需补 `-strict experimental`）、PNM 走 `-f image2 -c:v ppm`、ICO 要强制 bmp/png、m2v/y4m/dv 容器只认单一编码器（原来会写出空文件）、tga/ppm/pgm/pbm/xbm/xwd/pcx/exr 无输出路径。另接入**受管 ImageMagick**（`%APPDATA%/umi-downloader/bin/imagemagick`）补上 ffmpeg 写不了的 PSD/DDS。真机产物：真 PSD 6,922 字节（魔数 38425053=8BPS）、真 DDS 38,528 字节、DTS 178,176 字节、ALAC m4a 18,399 字节、PNM 230,415 字节。仍不可用的 8 种是引擎真的写不出来的（ape/heic/svg 与 pandoc 目标外的 doc/xls/ppt/json/xml），原因写在目录里 |
| 全面分析布局 + 合并功能与集成 + 更简洁凝练 | ✅ | 卡片 **46 → 30**（首页 9→2、下载 5→3、设置 28→18），设置页签 **7 → 5**（依赖工具+下载引擎→「引擎」；通用设置+默认参数→「通用」；系统与集成+更新+诊断→「系统」），旧深链 `?tab=tools`/`?tab=defaults` 仍能落到对应内容；11 个视图里除「引擎」页签 1.38 屏外**全部恰好一屏**；新增 `.umi-page/.umi-card-title/.umi-inner/.umi-hint/.umi-btn-sm/.umi-btn-xs` 等设计令牌把 5 种手写卡头、4 种魔改按钮收敛成唯一写法，并用测试固化（`uiLayout.test.ts` 出现旧写法即失败）；英文界面真 CJK 泄漏 3 → 0 |
| 全面检查 BUG | ✅ 14 项 | 只读排查 + 真机执行验证，产出 `docs/BUG_SWEEP_1.8.md`（每项含严重度/复现/原始输出/文件行号），并全部修复见下 |

### 1.8.0 修掉的关键 bug

| BUG | 严重度 | 症状 | 修复与实测 |
|-----|--------|------|-----------|
| BUG-01 | P1 | **字幕功能在生产路径必然失败**：whisper 产出 `<基名>.<语言>.srt`，代码用 `with_extension("srt")` 校验成 `<基名>.srt`，任何字幕任务都报“未生成字幕文件”，自检因用无语言后缀的名而掩盖 | 改为基名直接拼 `.srt` + 回退扫描同名产物 + 真实路径回写任务；真机 `status=done`、产物 `clip12s.zh.srt` 46 字节 |
| BUG-09 | P2 | `extract_audio` 不排空 stderr → 管道写满后**永久挂起**（实测 791 秒冻结、管道残留 4081B；排空对照组 720 秒正常完成） | 起双 reader 线程实时排空 + `-nostats`；看门狗改为「输出 5 分钟无增长即判停滞」，真机 6.5 秒判死并给出原因 |
| BUG-02 | P1 | 删除“下载中”的任务后记录被运行线程 upsert 回库（任务复活），引擎继续下载，`delete_file` 也删不掉半成品 | 先墓碑+`kill_tree` 再删库，并按输出目录+URL 基名清 `<name>/<name>.aria2/<name>.part`；真机 30 秒后不复活、半成品与 `.aria2` 已清 |
| BUG-05 | P2 | 并发数设置形同虚设（判断成立后是空注释块） | 实现真排队：超并发置 `pending`（界面「排队中」），槽位释放自动唤醒；真机 6 个任务 → `{downloading:3, pending:3}`、aria2c 恰 3 个、释放后 pending 3→2 |
| BUG-03 | P2 | 退出不回收引擎子进程（实测 4 个 aria2c 变孤儿、退出后仍在写盘） | 退出路径遍历 `state.pids` 执行 `kill_tree` + 启动清扫父进程已死的受管引擎；真机 `aria2_after=[]` |
| BUG-04 | P2 | 取消下载最终显示“已暂停”并残留 `.aria2` | 用 `StopIntent` 区分 pause/cancel；取消 → `canceled` 并清理控制文件与半成品 |
| BUG-06 | P2（安全） | 捕获端口默认无令牌 + 响应带 `ACAO: *` → 任意网站可越站入队（实测 `Origin: https://evil.example` 真入队） | 强制令牌（首启自动生成 32 位 hex）、去掉全部 CORS 头、校验 Origin/Referer；真机 evil Origin → **403 且不入队**，带令牌 → 200 queued |
| BUG-07 | P2 | 粘贴 100 万行让主线程冻结 10 秒（另有约 403 秒的极端复现） | 512KB 上限（按行截断+提示）、`@paste` 拦截（36.9MB 不进 DOM）、分块解析；真机 10,034ms → **243.8ms** |
| BUG-08 | P2 | 目标路径 >260 字符下载失败，且只看到 aria2 的 `Download aborted`，真因（errorCode=18）被丢弃 | 入队预检（>259 直接拒绝并说明）+ 错误展示保留 errorCode/Exception 行 |
| BUG-10/11/12/13/14 | P3 | 限速器锁中毒会连锁 panic；捕获端口冲突无界面提示；`/capture` 恒回 `queued`；字幕失败残留产物无指引；ED2K 入队即显示“完成/100%” | 分别：安全取锁 + 中毒锁单测；设置页可见警告与「重启/换端口」；按真实结果返回 queued/skipped/blocked/rejected；失败时清无效产物、保留有效产物并回写路径；新增 `handed_off` 状态（不再复用 done），四语言与卡片映射同步 |

### 1.8.0 验收证据

| 项目 | 结果 |
|------|------|
| `cargo test --lib` | 153 / 153（+4 ignored 为真机端到端用例） |
| `npx vue-tsc --noEmit` | 0 错误 |
| `npx vitest run` | 87 / 87 |
| i18n 一致性 | 四语言各 655 条，key 完全一致、运行时零缺失 |
| `scripts/verify_v18.py` | 全部通过（格式 80/88 可用、PSD 真产物、队列预览、字幕真出 SRT、排队/取消/删除/长路径/ED2K 状态、20 万行粘贴、越站入队拒绝、退出清扫、UI 收敛回归） |
| 安装包 | `Umidl_1.8.0_x64-setup.exe`（4.43 MiB） |
| bug 报告 | `docs/BUG_SWEEP_1.8.md` |

"""
print("\n".join(edit("docs/ROADMAP.md", [("## 仍未做完的部分（诚实清单）", SEC + "## 仍未做完的部分（诚实清单）")])))

print("\n".join(edit("README.md", [
    (
        "- **多链接 / txt 批量导入（1.6 新增）**：",
        "- **受管 ImageMagick（1.8 新增）**：ffmpeg 写不了的目标（PSD/DDS）交给随包 ImageMagick 完成，仍不污染系统 PATH\n"
        "- **多链接 / txt 批量导入（1.6 新增）**：",
    ),
    (
        "- **统一流程（1.7 起收敛为一张卡）**",
        "- **可用性 = 真实引擎能力（1.8）**：88 种里 80 种在本机可用（视频 22/22），其余 8 种为引擎真写不出来的格式并在目录里给出原因；可用性由真实探测（ffmpeg 编码器/复用器 + ImageMagick 格式表 + pandoc/poppler）决定，不是写死的表\n"
        "- **统一流程（1.7 起收敛为一张卡）**",
    ),
])))
