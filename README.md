<div align="center">

<img src="assets/logo-256.png" alt="Umidl" width="150" />

# Umidl

**轻量化、美观、高性能的视频下载 / 格式转换 / AI 字幕桌面客户端**

下载 + 转换 + 字幕，一站式搞定。**Tauri 2 + Rust + Vue 3** 构建，安装包 **4.46 MiB**，全部处理在**本机**完成——不上传、不注册、不埋点。

[![License: AGPL-3.0-only](https://img.shields.io/badge/License-AGPL--3.0--only-2f81f7.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux-3fb950.svg)](#-平台支持)
[![Tauri 2](https://img.shields.io/badge/Tauri-2.x-24C8DB.svg?logo=tauri&logoColor=white)](https://tauri.app)
[![Vue 3](https://img.shields.io/badge/Vue-3.5-42B883.svg?logo=vuedotjs&logoColor=white)](https://vuejs.org)
[![Installer size](https://img.shields.io/badge/Installer-4.46%20MiB-brightgreen.svg)](https://github.com/HuanMoovo/umidl/releases)
[![i18n](https://img.shields.io/badge/i18n-%E4%B8%AD%20%C2%B7%20EN%20%C2%B7%20%E6%97%A5%20%C2%B7%20FR-8957e5.svg)](#界面与主题)

[快速开始](#-快速开始) · [它做什么](#-它做什么) · [功能](#-功能) · [平台支持](#-平台支持) · [参与贡献](CONTRIBUTING.md) · [开源介绍页](https://huanmoovo.github.io/umidl/) · [路线图](docs/ROADMAP.md)

<sub>简体中文 · <a href="README.en.md">English</a> · <a href="README.ja.md">日本語</a></sub>

<a href="https://huanmoovo.github.io/umidl/">
  <img src="docs/img/home.webp" alt="Umidl 1.8.11 主界面：链接输入框、四类任务计数与三个功能入口" width="860" />
</a>

<sub>Umidl 1.8.11 主界面（Windows 实机截图，沙箱数据目录）· <a href="https://huanmoovo.github.io/umidl/#screens">查看全部 7 张界面截图</a></sub>

</div>

---

## 🎯 它做什么

**在本机把「下载 → 转换 → 字幕」一条链做完**——安装包 4.46 MiB（1.8.11 Windows NSIS 实测）。

| 能力 | 现状（1.8.11；均可从仓库与程序界面核对） |
| --- | --- |
| **本地处理，不上传** | 全流程在本机进程内完成：无账号、无遥测、无云端解析；卸载或删除数据目录即清除全部状态 |
| **下载** | 两条链路按链接类型自动路由（yt-dlp 站点解析 / aria2c 分段并行）；分 P、合集、HLS/DASH 自动合并；暂停、断点续传、全局限速 |
| **格式转换** | 80 种目标格式（视频 22 · 音频 20 · 图片 20 · 文档 18），可用性由本机引擎能力探测决定；docx / xlsx / pptx / odt / ods / odp 为纯 Rust 原生解析；支持 NVENC / QSV / AMF 硬件编码 |
| **字幕** | Whisper.cpp 在本机听写；识别语言为自动检测 + 8 种可多选，可叠加「翻译成英文」轨道；导出 SRT / ASS / VTT / TXT / JSON；模型 6 档按需下载 |
| **插件** | QuickJS 沙箱，硬上限 16 MB 内存 / 512 KB 栈 / 200 ms 单次执行；白名单 API（无 `require` / `process` / `fetch`）；插件按内容寻址校验 sha256 |
| **跨平台** | 同一份源码可在 Windows / macOS / Linux 构建；各平台产物状态与验证程度见[平台支持](#-平台支持) |


---

## 🚀 快速开始

### Windows（已发布）

1. 到 [Releases](https://github.com/HuanMoovo/umidl/releases) 下载 `Umidl_x.y.z_x64-setup.exe`（1.8.11 安装包 **4.46 MiB**）
2. 首次启动会自动检测 yt-dlp / FFmpeg / Whisper 等运行时工具；缺失的在「设置 → 依赖工具」一键下载（装进 `%APPDATA%/umi-downloader/bin`，**不写入系统 PATH**）
3. 粘贴链接 → 选清晰度 → 开始下载

### macOS / Linux（源码构建；安装包由 CI 产出）

```bash
git clone https://github.com/HuanMoovo/umidl.git
cd umidl
npm ci

# Linux 需先装系统依赖（与 CI 一致）
sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf

npm run tauri:dev      # 开发模式（热更新）
npm run tauri:build    # 本地出包：.dmg / .AppImage / .deb
```

- **CI 产物位置**：Actions → `Release` 工作流 → 对应 run 的 Artifacts（`umidl-windows-x64` / `umidl-macos-arm64` / `umidl-macos-x64` / `umidl-linux-x64`）；四个平台的产物已随 [v1.8.11](https://github.com/HuanMoovo/umidl/releases) 挂到 Releases。Intel macOS 的 dmg 由 arm64 runner 交叉编译产出（命令见 [CONTRIBUTING.md](CONTRIBUTING.md)）。
- macOS / Linux 目前**只由 CI 编译产出**，尚未在真机做完整回归；aria2 在这两个平台没有官方静态包，安装引导会给出 `brew` / `apt` 指引。

### 环境要求（源码）

- **Node ≥ 20**、**Rust stable ≥ 1.77**，另需 Python 3（跑 `scripts/` 下的辅助脚本）
- Windows：MSVC 生成工具 + WebView2（Win11 自带）；macOS：Xcode Command Line Tools；Linux：见上面的 apt 依赖

完整开发流程见 [CONTRIBUTING.md](CONTRIBUTING.md)。

---

## 💻 平台支持

| 平台 | 架构 | 产物 | 状态 |
| --- | --- | --- | --- |
| Windows | x64 | NSIS 安装包 `.exe` | ✅ **已发布**（1.8.11，4.46 MiB，本机真机回归） |
| macOS | Apple Silicon (arm64) | `.dmg` | ✅ **已发布**（1.8.11，8.96 MiB，CI 构建，未真机回归） |
| macOS | Intel (x64) | `.dmg` | ✅ **已发布**（1.8.11，9.24 MiB，CI 在 arm64 runner 上交叉编译，未真机回归） |
| Linux | x64 | `.AppImage` / `.deb` | ✅ **已发布**（1.8.11，deb 6.85 MiB / AppImage 81.4 MiB，CI 构建，未真机回归） |
| Android | — | — | 🔴 实验性作业，需移动端适配，暂不提供产物 |

---

## ✨ 功能

### 下载引擎（双引擎自动路由）
- **aria2c 分段并行引擎**：直链 / FTP / BitTorrent 种子 / 磁力链接走 16 连接动态分段（1 – 16 可调），支持断电断点续传
- **yt-dlp 站点解析引擎**：1000+ 站点视频、**分P 与合集**、HLS/DASH 流媒体自动合并
- 引擎可在设置里指定「自动 / yt-dlp / aria2」，自动模式按链接类型路由
- **全局限速（令牌桶）**：设置带宽上限，网关级节流同时作用于 yt-dlp、aria2 与自身请求，下载在后台跑也不影响浏览
- **受管 ImageMagick（1.8 新增）**：ffmpeg 写不了的目标（PSD/DDS）交给随包 ImageMagick 完成，仍不污染系统 PATH
- **多链接 / txt 批量导入（1.6 新增）**：一次粘贴多行链接或导入 txt 文件批量入队，结果面板给出「已加入 / 跳过」与失败原因
- **多线程与并发（1.6 新增）**：同时任务数与每个下载的连接数/分段数/最小分片可调（aria2 与 yt-dlp 分别生效），越界自动限幅
- **下载队列**：单一表格式列表（V1.8.7 起移除卡片模式，操作精简为打开 / 定位 / 预览 / 删除)

### 插件沙箱（JavaScript · 1.5 新增）
- **QuickJS 沙箱运行时**：16 MB 内存上限、512 KB 栈上限、200 ms 单次执行时间预算，越权脚本被真中断（`while(true)` 也拦得住），不注入 `require` / `process` / `fetch` / 文件 API
- **白名单能力**：只暴露 `umi.log / resolve / registerResolver / on / retry / storage`
- **链接解析器**：插件注册的解析器参与「这条链接走哪个引擎」的判定，设置页可实时看到插件意见
- **通知钩子 + 命令式重试**：`download:done` / `download:error` / `app:start` 事件推给插件，插件可调 `umi.retry(taskId)` 让失败任务重试（自带次数上限，不会无限重试）
- **插件市场（内容寻址校验）**：内置示例插件一键安装；安装前比对 **sha256**，文件被改动一个字节即拒绝安装
- **从 GitHub / https 直链安装**：设置页粘贴仓库地址（自动探测仓库根目录 `plugin.json` / `manifest.json`）或 `.js` / 清单直链即可安装；仅 https、单文件 ≤ 5 MB、请求超时 60 s，下载内容只落盘不执行
- 插件管理界面位于 **设置 → 插件**（1.6 起从侧边栏独立页迁入）

#### 插件包与「从 GitHub 安装」约定

「设置 → 插件」顶部可直接粘贴地址安装，只接受 **https**（`http` 一律拒绝）：单文件 **≤ 5 MB**、请求超时 **60 s**；下载内容**只落盘、不执行**，并且只写入应用数据目录的 `plugins/<id>/` 内。

支持的两种地址：

- **GitHub 仓库**：`https://github.com/<owner>/<repo>`（可带 `.git` 后缀 / 尾部 `/`）→ 自动探测仓库根目录 `main` / `master` 分支下的 `plugin.json` / `manifest.json`
- **https 直链**：清单 JSON 地址（`entry` 按同目录解析）或 `.js` 脚本地址（插件 id 取文件名）；GitHub 的 `blob` 页面地址会自动转成 raw

**插件包最小约定**：仓库（或直链所在目录）放一个 `plugin.json`（`manifest.json` 同义）：

```json
{
  "id": "my-plugin",
  "name": "我的插件",
  "version": "1.0.0",
  "description": "一句话说明",
  "author": "your-name",
  "permissions": ["resolve"],
  "entry": "plugin.js"
}
```

- `id` **必填**：字母 / 数字 / `-` `_` `.`，≤ 64 字符；禁止 `..`、`/`、`\` 与绝对路径（净化不通过直接拒绝安装）
- `entry` **可选**：脚本路径（相对清单所在目录，或 https 直链），默认 `plugin.js`
- `name` / `version` / `description` / `author` / `permissions` 均可选（默认分别取 `id` / `0.0.0` / 空 / 空 / `[]`）

插件本体就是一个 JavaScript 文件（`entry`），API 与沙箱限制和市场安装完全一致（`umi.log / resolve / registerResolver / on / retry / storage`，无 `require` / `process` / `fetch`）。安装后可在插件页启用 / 停用 / 测试 / 卸载；安装来源记在插件目录 `manifest.json` 的 `source`（`github` / `url` / `market`）与 `source_url` 字段。

### 视频下载（yt-dlp 内核）
- 支持 YouTube / Bilibili / Vimeo / TikTok 等 1000+ 站点
- 解析标题、封面、作者、时长、清晰度、格式
- 可视化选择清晰度 / 仅音频（MP3 / M4A / FLAC / WAV / OPUS）
- 实时进度、速度、剩余时间
- **暂停 / 断点续传 / 取消 / 删除**
- 嵌入封面、嵌入字幕、自定义下载目录、限速、代理、Cookies

### 格式转换（80 种格式 · FFmpeg + 原生解析 + pandoc/poppler）
- **可用性 = 真实引擎能力（1.8）**：目录里 80 种目标格式的可用状态由真实探测决定（ffmpeg 编码器/复用器 + ImageMagick 格式表 + pandoc/poppler），本机全部可用；可用性由真实探测（ffmpeg 编码器/复用器 + ImageMagick 格式表 + pandoc/poppler）决定，不是写死的表
- **统一流程（1.7 起收敛为一张卡）**：文件区 → 单一格式选择器（类型芯片 + 搜索 + 目标格式网格）→ 按类型显隐的参数区 → 一行引擎状态条 → 队列；页面高度比 1.6 减少约 64%
- **类型选项**：全部 / 视频 / 音频 / 图片 / 文档 五档分段选择，网格里显示每种格式的可用状态（本机引擎不支持则置灰并给出原因），支持按格式名/说明搜索
- **共 80 种**（真机探测）：视频 22 · 音频 20 · 图片 20 · 文档 18；选不同类型自动切换对应参数面板（编码/分辨率/CRF 只在视频/音频目标下出现）
- **文档（1.5 新增）**：docx / xlsx / pptx / odt / ods / odp **原生纯 Rust 解析**为 txt / md / html / csv（不依赖任何外部运行时）；pandoc 负责富格式互转（docx·odt·rtf·epub·html·md）；poppler 负责抽取 PDF 文本；选中即显示页数 / 工作表 / 幻灯片 / 字符数
- **图片（1.5 新增）**：PNG / JPG / WEBP / BMP / TIFF / ICO
- 视频：MP4 / MKV / MOV / AVI / WEBM / FLV
- 音频：MP3 / AAC / FLAC / WAV / OPUS / M4A
- 编码：H.264 / H.265(HEVC) / AV1 / VP9
- 分辨率缩放、CRF 质量预设、提取音频、去除音轨
- 实时转码进度与取消

### AI 字幕（Whisper.cpp 内核）
- 视频 → FFmpeg 提取音频 → Whisper 语音识别 → 时间轴 → 导出
- 导出格式：SRT / ASS / VTT / TXT / JSON
- **识别语言可多选（1.6 新增）**：每个选中语言各起一个任务，可叠加"翻译成英文"再生成一份英文轨
- 自动语言检测，支持中 / 英 / 日 / 韩 / 法 / 德 / 西 / 俄
- 6 档模型（tiny 75 MB ~ large-v3 3.1 GB），按需下载

### 内置自动化自检
设置页「功能自检」可一键跑完整回归：真实调用 yt-dlp / FFmpeg / Whisper 逐项验证下载、转换、字幕、数据层与配置读写。

---

## 🚀 开发

```bash
npm install
npm run tauri:dev      # 开发模式（热更新）
npm run test           # 前端单元测试（vitest）
npm run tauri:build    # 打包桌面客户端（NSIS 安装包 + exe）
```

### 后端自检（无 GUI，可跑 CI）

```bash
cd src-tauri
cargo run --release -- --selftest          # 完整自检（含真实下载 / AI 识别）
cargo run --release -- --selftest --quick  # 快速自检（跳过耗时用例）
```

---

## 🧱 技术架构

```
┌─────────────────────────────┐
│ UI 层    Vue3 + TS + Tailwind + Naive UI │
└──────────────┬──────────────┘
               │ Tauri IPC（invoke / event）
┌──────────────▼──────────────┐
│ 应用核心层   Tauri 2 + Rust              │
│  · 任务调度 / 取消 / 进度解析            │
│  · SQLite 持久化（rusqlite）             │
└──────────────┬──────────────┘
               │
┌──────────────▼──────────────┐
│ 功能服务层   yt-dlp · FFmpeg · Whisper   │
└──────────────┬──────────────┘
               │
┌──────────────▼──────────────┐
│ 数据层   SQLite + 文件系统               │
└─────────────────────────────┘
```

### 目录结构

```
umi-downloader
├── src                  # 前端
│   ├── views            # Home / Download / Converter / Subtitle / Settings
│   ├── components       # SideBar / TitleBar / TaskCard / ParticleBg
│   ├── services         # ipc / downloader / ffmpeg / whisper / utils
│   ├── stores           # Pinia：tasks / settings
│   └── types            # 与 Rust 端一致的类型定义
├── src-tauri            # 后端
│   ├── src
│   │   ├── lib.rs       # Tauri 命令与任务调度
│   │   ├── downloader.rs# yt-dlp 集成（解析 / 参数 / 进度 / 续传）
│   │   ├── converter.rs # FFmpeg 集成（ffprobe / 参数 / 进度）
│   │   ├── subtitle.rs  # Whisper 集成（抽音轨 / 转写 / 字幕格式）
│   │   ├── tools.rs     # 依赖检测与一键安装
│   │   ├── db.rs        # SQLite 数据层
│   │   ├── selftest.rs  # 自动化自检系统
│   │   └── http_testserver.rs # 自检用本地 HTTP 服务
│   └── tauri.conf.json
└── scripts/make_brand_assets.py  # 品牌管线：母版 SVG → PNG/WebP/应用图标
```

---

## 🔧 依赖工具

应用启动时自动检测，缺失可在「设置 → 依赖工具」一键安装（下载到应用数据目录，不污染系统）：

| 工具 | 用途 | 大小 |
| --- | --- | --- |
| yt-dlp | 视频解析与下载 | ~17 MB |
| FFmpeg / FFprobe | 转码、合成、探测 | ~80 MB |
| whisper.cpp | AI 语音识别 | ~22 MB |
| Whisper 模型 | 语音识别模型 | 75 MB ~ 3.1 GB |

检测顺序：**用户指定路径 → 应用托管目录 → 主程序同级目录 → 系统 PATH**。

---

## 📄 数据存储

| 内容 | 位置 |
| --- | --- |
| 数据库 / 配置 | `%APPDATA%/umi-downloader/` |
| 托管工具 | `%APPDATA%/umi-downloader/bin/` |
| Whisper 模型 | `%APPDATA%/umi-downloader/models/` |
| 默认下载目录 | `%USERPROFILE%/Downloads/Umidl/` |

---

## 🆕 功能增强（v1.0.0 增强版）

### 界面与主题
- **昼夜主题系统**：标题栏一键切换「浅色 / 深色 / 跟随系统」，跟随系统时随 Windows 昼夜自动切换；全部界面元素走 CSS 变量，切换即时生效无需重启。
- **6 种强调色**：紫罗兰 / 青蓝 / 樱花粉 / 翡翠绿 / 琥珀金 / 天空蓝，整站配色（按钮、进度条、发光、滚动条）同步变化。
- **性能优化**：
  - 背景粒子改为预渲染发光贴图 + 空间分桶连线 + 30fps 限帧，并支持关闭 / 三档质量；
  - 下载 / 转换进度事件节流（≤4 次/秒），大幅降低高频 IPC 与界面重渲染；
  - 任务卡片懒加载封面、可选紧凑模式。

### 视频下载
- **封面**：解析后自动抓取封面并缓存到本地（离线也能显示），支持点击放大预览。
- **单独下载**：视频 / 音频 / 封面 / 字幕 四个独立入口，字幕支持多语言多选（含自动生成字幕标记）。
- **预览面板**：封面大图 + 标题 + 投稿人 / 播放量 / 点赞 / 时长 / 上传日期 + 全部清晰度列表 + 字幕轨道 + 简介，可一键在浏览器打开。
- **系统代理自动识别**：未手动填写代理时自动读取 Windows 系统代理（Clash / v2ray 等），本机与内网地址强制直连，避免 502。

### 格式转换
- **格式更丰富**：视频 12 种容器（mp4/mkv/mov/webm/avi/flv/ts/m4v/mpg/wmv/ogv/gif），音频 10 种（mp3/m4a/aac/flac/wav/opus/ogg/wma/ac3/aiff）。
- **编码器更多**：包含 libx264 / libx265 / libsvtav1 / libvpx-vp9 / ProRes，以及 NVENC / QSV / AMF 硬件编码（各自匹配正确的码控参数）。
- **自动检测格式**：载入文件后按源编码智能推荐输出方案——已是 H.264/AAC 直接无损封装 MP4、VP9/Opus 无损封装 WebM、无损音频保留 FLAC，避免无意义重编码。
- **高级选项**：视频/音频码率、采样率、声道、帧率、播放速度（0.25x~4x，含滤镜链）、硬件加速解码、faststart、清除元数据、分辨率缩放、静音提取。

### 设置
- 新增「外观」页（主题 / 强调色 / 动画与性能）与「默认参数」页（下载容器、音频格式、清晰度偏好、文件名模板、转换 CRF、自动检测、硬件加速、字幕语言 / 格式）。
- 通知与行为开关：完成后系统通知、完成后打开文件夹、解析后自动下载、保留原始文件、限速。

### 工程
- 端到端自检模块改为 `selftest` feature 编译开关，**正式发布的桌面客户端不包含自检代码**；开发 / CI 使用 `cargo build --release --features selftest` 即可通过 `--selftest` 跑全量验证。

---

## 🤝 参与贡献

欢迎 issue、PR、翻译与插件。动手之前请先读 [CONTRIBUTING.md](CONTRIBUTING.md)：里面有环境搭建、目录导览、四条门禁命令，以及 i18n 约定（**中文原文即 key**）。

```bash
npx vue-tsc --noEmit              # 类型检查
npx vitest run                    # 前端单元测试
cd src-tauri && cargo test --lib  # Rust 单元测试
python scripts/check_i18n_catalog.py  # 四语言 key 集合一致
```

- 报告问题：请附版本号（设置页可复制）、操作系统、复现步骤与日志；已知问题与清理进度见 [docs/BUG_SWEEP_1.8.md](docs/BUG_SWEEP_1.8.md)。
- 想做大改动：先在 issue 里对齐方向，或直接看 [docs/ROADMAP.md](docs/ROADMAP.md) 里还挂着的条目。
- 不想写代码也行：翻译、文档、插件示例、真机测试（尤其是 macOS / Linux）都是缺口。

## 📄 许可证

本项目以 **GNU Affero General Public License v3.0**（`AGPL-3.0-only`）开源，全文见 [LICENSE](LICENSE)。

一句话理由：Umidl 是纯本地桌面程序，但它带一个可编程插件沙箱——**AGPL 让任何把它包装成网络服务对外提供的人必须交出源码**，而普通用户在自己电脑上使用它不受任何额外限制。

### 第三方工具声明

Umidl 自身只负责调度，**不随包分发**下列第三方程序：它们在首次需要时由应用下载到 `%APPDATA%/umi-downloader/bin/`（或复用你系统里已有的副本），各自遵循其**原许可证**，与 AGPL-3.0 无关。

| 工具 | 用途 | 许可证（以各项目官方发布为准） |
| --- | --- | --- |
| yt-dlp | 站点解析与视频下载 | Unlicense（公开发布） |
| FFmpeg / FFprobe | 转码、封装、探测 | LGPL-2.1-or-later 或 GPL-2.0-or-later（取决于其构建配置） |
| aria2c | 分段并行下载（HTTP/FTP/BT/磁力） | GPL-2.0-or-later |
| ImageMagick | PSD / DDS 等 ffmpeg 写不了的图片格式 | ImageMagick License（Apache-2.0 风格） |
| pandoc | docx / odt / rtf / epub 等富文本互转 | GPL-2.0-or-later |
| poppler（pdftotext） | PDF 文本抽取 | GPL-2.0-or-later |
| whisper.cpp | 本地语音识别 | MIT |
| Whisper 模型权重 | 语音识别模型（按需下载，75 MB ~ 3.1 GB） | MIT / 各自模型仓库声明 |

> 若你要再分发 Umidl，请自行确认上述程序在你所在司法辖区的许可与专利条款（例如 FFmpeg 的 H.264 / H.265 编解码专利）。Umidl 只在运行时按需下载，不改变这些程序的许可，也不对其做任何修改。

---

## ⚖️ 合规提示

本工具仅用于下载你有权获取的内容。请遵守目标站点的服务条款与当地法律法规。
