# 参与贡献 · Contributing to Umidl

感谢你愿意花时间。Umidl 是纯本地、无账号、无遥测的桌面工具，贡献方式不止写代码：**翻译、文档、插件示例、真机测试（尤其 macOS / Linux）都是缺口**。

- 🐞 发现 bug：开 issue，附版本号（设置页可复制）、操作系统、复现步骤、日志；已知问题清单见 [docs/BUG_SWEEP_1.8.md](docs/BUG_SWEEP_1.8.md)
- 💡 提功能建议：开 issue 说明「要解决什么问题」，而不是「要加什么按钮」
- 🔧 想改大块头：先在 issue 里对齐方向（避免写完才发现撞了 [docs/ROADMAP.md](docs/ROADMAP.md) 里的已定路线）
- 🌍 语言：中文原文即 key，补 `src/i18n/locales/*.ts` 即可，不需要动代码

---

## 行为准则摘要

参与本项目即表示你同意：

1. **对人客气**——批评代码可以，针对人不行；不接受人身攻击、歧视性言论、性骚扰、恶意刷屏。
2. **对事认真**——不要为了让 CI 变绿去删测试、改断言、加 `@ts-ignore` 掩盖问题；不确定就在 PR 里说明。
3. **尊重隐私与边界**——不要在本仓库提交任何他人的个人信息、下载日志、真实站点凭据（Cookies / token 一律用占位符）。
4. **不要提交你无权开源的代码**——拷贝来的 GPL 不兼容代码会让整个项目陷入许可问题。

维护者会对违反者做提醒、关闭 PR，必要时封禁。完整版参考 [Contributor Covenant v2.1](https://www.contributor-covenant.org/version/2/1/code_of_conduct/)。

---

## 开发环境

| 依赖 | 版本 | 说明 |
| --- | --- | --- |
| Node.js | ≥ 20（CI 用 20） | 前端构建与测试 |
| Rust | stable ≥ 1.77 | `rustup` 安装即可 |
| Python | 3.x | 只用于 `scripts/` 下的辅助脚本（i18n 检查等） |

**平台系统依赖**

- **Windows**：MSVC 生成工具（VS 2022 Build Tools，勾选「使用 C++ 的桌面开发」）+ WebView2（Win11 自带）。Tauri 走 MSVC 工具链。
- **macOS**：Xcode Command Line Tools（`xcode-select --install`）。
- **Linux**：与 CI 一致 —— `sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`。

```bash
git clone https://github.com/HuanMoovo/umidl.git
cd umidl
npm ci                 # 严格按 package-lock.json 安装

npm run tauri:dev      # 开发模式（Vite 热更新 + Rust 侧重建）
npm run tauri:build    # 本地打包：Windows .exe(NSIS) / macOS .dmg / Linux AppImage+deb

# 指定目标打包。例：在 Apple Silicon 上产出 Intel(x86_64) dmg —— CI 就是这么做的，
# 因为 GitHub 的 macos-13（Intel）runner 长期排队，交叉编译不必等它。
npm run tauri:build -- --target x86_64-apple-darwin --bundles dmg
```

运行时外部工具（yt-dlp / FFmpeg / Whisper …）**不会随包分发**，应用首次使用时会下载到 `%APPDATA%/umi-downloader/bin/`（macOS/Linux 在应用数据目录下的同名子目录）。调试时可在「设置 → 依赖工具」里指定你自己的路径。

### 后端自检（无 GUI，可在 CI 跑）

```bash
cd src-tauri
cargo run --release --features selftest -- --selftest          # 完整自检（含真实下载 / 语音识别）
cargo run --release --features selftest -- --selftest --quick  # 快速自检（跳过耗时用例）
```

> 自检模块由 `selftest` feature 门控，**正式发布的客户端里不含自检代码**。

---

## 目录结构导览

```
umi-downloader
├── src/                      # 前端（Vue 3 + TS + Tailwind + Naive UI）
│   ├── views/                # Home / Download / Converter / Subtitle / Settings 五个页面
│   ├── components/           # SideBar / TitleBar / TaskCard / TaskTable / ParticleBg …
│   ├── services/             # ipc.ts（IPC 封装）· downloader · ffmpeg · whisper · docEngine
│   │                         # formatCatalog（80 种格式目录）· batch（批量导入）· plugins · theme
│   ├── stores/               # Pinia：tasks / settings
│   ├── i18n/                 # index.ts + locales/{zh,en,ja,fr}.ts
│   └── types/                # 与 Rust 端一致的类型定义
├── src-tauri/                # 后端（Tauri 2 + Rust）
│   ├── src/lib.rs            # Tauri 命令注册与任务调度
│   ├── src/downloader.rs     # yt-dlp / aria2 集成（解析 · 参数 · 进度 · 续传）
│   ├── src/converter.rs      # FFmpeg 集成（ffprobe · 参数 · 进度）
│   ├── src/docs.rs           # 文档解析（docx/xlsx/pptx 纯 Rust 解析 + pandoc/poppler 调度）
│   ├── src/subtitle.rs       # Whisper.cpp 集成（抽音轨 · 转写 · 字幕格式）
│   ├── src/plugins.rs        # QuickJS 沙箱 · 插件市场（sha256）· GitHub / 直链安装
│   ├── src/tools.rs          # 外部工具检测与托管安装
│   ├── src/ratelimit.rs      # 全局限速令牌桶
│   ├── src/db.rs             # SQLite 数据层（rusqlite）
│   ├── src/selftest.rs       # 自动化自检（feature = "selftest"）
│   └── tauri.conf.json       # 应用配置（identifier / 窗口 / 打包）
├── assets/                   # logo / 图标（提交）
├── docs/                     # ROADMAP · BUG_SWEEP · 开源介绍页 index.html（提交）
├── scripts/                  # 辅助脚本：i18n 检查、图标生成、往期版本迁移脚本
└── .github/workflows/        # release.yml：Windows / macOS(arm64+x64) / Linux 四目标构建
```

---

## 提交前门禁（四条，缺一不可）

```bash
npx vue-tsc --noEmit               # 1. 前端类型检查
npx vitest run                     # 2. 前端单元测试（用例与源码同目录，*.test.ts）
cd src-tauri && cargo test --lib   # 3. Rust 单元测试（令牌桶 / 下载参数解析 / 文档 / 字幕 / 插件沙箱）
python scripts/check_i18n_catalog.py  # 4. i18n 校对：四语言包 key 集合必须一致（脚本内做 JS 转义还原）
```

前三条在 `.github/workflows/release.yml` 里同样会跑，本地能过 CI 才能过。

> 当前基线（1.8.10）：`cargo test --lib` → **230 passed / 0 failed**（Windows 本机与 CI）· `vitest run` → **21 文件 / 176 例** · 四语言包各 **714** 条 key。数字对不上说明有东西被改动了，别直接放宽断言。改了自检相关代码再补一条快速自检：

```bash
cd src-tauri && cargo run --release --features selftest -- --selftest --quick
```

> 别用「跳过测试 / 放宽断言」的方式让门禁变绿——真有问题就修问题，或者在 PR 里明说是已知缺陷。

---

## i18n 约定（重要）

界面文案全部走 **「中文原文即 key」**：`t('保存')`，语言包里的 key 就是中文原文本身。

- 语言包位置：`src/i18n/locales/{zh,en,ja,fr}.ts`（中 / 英 / 日 / 法四语言）
- **缩进 2 空格**；**语言包统一 LF 行尾**（与仓库主分支一致），不要整文件重排行尾或重排顺序——那会产生几千行的噪音 diff，review 什么都看不出来
- **先补语言包，再在代码里用**：先给四个语言包都加上 `'新文案': '...'`，再在 `.vue` / `.ts` 里写 `t('新文案')`。只在代码里用、语言包没跟进 → 英/日/法界面会裸露出中文
- 缺失翻译时 vue-i18n 会回退到 `zh`，也就是原样显示中文，**永远不会出现空白或 key 泄漏**——所以漏翻译不会被用户当场发现，必须靠第 4 条门禁把关
- 不要在模板里写死界面中文；新增界面文案一律走 `$t()` / `t()`
- 生成 / 追加 key 用脚本（`scripts/` 下已有多个往期示例），避免手写导致四语言漏项

---

## 跨平台注意事项

同一份代码要在 Windows / macOS / Linux 上跑，下面这条是**已踩过的坑**，改相关代码时两个平台都要想一遍：

1. **Unix 上杀超时进程必须按进程组**（`libc::kill(-pgid)`，配合 spawn 时建组）。只杀直接子进程时，`sh -c "sleep 5"` 这类命令的孙进程仍持有管道写端，读线程要等它自然退出——实测「400 ms 超时」的用例拖到 5.01 秒才返回。Windows 侧对应 `taskkill /T`。

另外：**本机只能验证本机平台的产物**。macOS / Linux 的结论以 CI 与容器为准——Linux 可以照 `scripts/docker/` 下的脚本在 `ubuntu:22.04` 里跑全量单测复现。

---

## 本地真机验收（推荐）

改完界面或下载/转换/字幕链路，建议用远程调试端口驱动真实界面走一遍，而不是只看单测：

```bash
# Windows：设好远程调试端口再启动（9222 或 9700；9223 在部分 Windows 上属保留段，绑不上且不报错）
WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 <安装目录>/umidl.exe

python scripts/drive_app.py shot out.png        # 截图
python scripts/drive_app.py eval "document.title"
python scripts/drive_app.py click "selector"
```

- 验收完结束进程（`Stop-Process -Force`），并确认调试端口已释放。
- **要把截图放进公开文档时**，先给应用换个沙箱数据目录，避免把个人下载记录、文件路径与用户名拍进去：
  `UMI_DATA_DIR=<沙箱>/data UMI_DOWNLOAD_DIR=<沙箱>/dl`（见 `src-tauri/src/ctx.rs`；`scripts/capture_docs_shots.py` 是现成的批量截图示例）。
- 改 `.exe` 之前先结束正在运行的 `umidl.exe`，否则 `cargo build` 会报 `failed to remove file …exe`。

---

## 提交与 PR 流程

1. **Fork** 仓库，从 `main` 切分支：`feat/xxx`、`fix/xxx`、`docs/xxx`、`i18n/xxx`
2. 小步提交，提交信息写明「做了什么 + 为什么」：

   ```
   fix: 下载队列折叠后不刷新进度

   原因：折叠状态下的定时刷新被 v-if 卸载打断。
   现在改为父层持有定时器，折叠只切显示。
   ```
3. 本地跑完[四条门禁](#提交前门禁四条缺一不可)
4. 推分支 → 开 PR，描述里写清：**改了什么 / 为什么这么改 / 怎么验证的（贴命令或截图）/ 有没有已知遗留**
5. CI 会跑四个作业：Windows x64 / macOS arm64 / macOS x64（在 arm64 runner 上交叉编译）/ Linux x64（另有 Android 实验性作业，失败不阻塞）；CI 红先自己看日志，不要直接请求 review
6. 维护者 review 后 squash 合并；大改动请提前在 issue 对齐

**README 有三份语言版本**：`README.md`（简体中文）· `README.en.md`（English）· `README.ja.md`（日本語）。改了功能、平台或版本相关的描述，三份要一起改；英文/日文版里的内部锚点链接（`](#...)`）必须指向各自语言的小节标题，改完随手点一遍确认不跳空。

**PR 里不要出现的文件**：`dist/`、`node_modules/`、`src-tauri/target/`、`target-selftest/`、`.tmp/`、安装包 `.exe`/`.dmg`/`.AppImage`/`.deb`、模型权重、`i18n-*.json` 与 `fr_new.json` 这类中间产物、任何 `.env` 或站点凭据（`.gitignore` 已覆盖，`git status` 提交前自己看一眼）。

---

## 硬约定（这些不能改）

改动下面任何一条都会破坏用户数据或安全边界，**PR 会被直接拒绝**（确有需要请先开 issue 讨论）：

1. **Tauri identifier 固定为 `com.umi.downloader`**（`src-tauri/tauri.conf.json`）。改了等于换了一个应用：更新链断裂，用户数据/配置全部对不上。
2. **数据目录固定为 `%APPDATA%/umi-downloader/`**（macOS/Linux 为对应的应用数据目录），其下的 `bin/` 存放托管的外部工具、`models/` 存放 Whisper 模型。**不要重命名、不要换位置**。
3. **不污染系统 PATH**：外部工具只装到托管目录，不加系统环境变量、不写全局注册表项（检测顺序固定：用户指定路径 → 应用托管目录 → 主程序同级目录 → 系统 PATH）。
4. **不上传用户数据**：不引入遥测、统计、崩溃上报、云端解析。所有下载 / 转码 / 识别都在本机完成——这是产品的定义，不是可选项。
5. **语言包的 key 不可重命名**（中文原文即 key）：改 key 等于把所有已有翻译删掉。要改措辞就当作新增条目，四个语言包一起补。
6. **插件沙箱预算**（16 MB 内存 / 200 ms 单次执行 / 白名单 API，无 `require`·`process`·`fetch`）不得为了「让某个插件能跑」而放宽；需要更多能力请先开 issue。
7. **从 GitHub / https 直链安装的边界**：仅 https、单文件 ≤ 5 MB、请求超时 60 s、内容只落盘不执行、只写 `plugins/<id>/`；插件清单约定（`plugin.json` / `manifest.json` + `entry`）见 README「插件包与从 GitHub 安装约定」——收紧可以，放宽请先开 issue。
8. **版本号六处一致**：`package.json` / `src-tauri/Cargo.toml` / `src-tauri/tauri.conf.json` 决定产物版本；`src/components/TitleBar.vue` / `README.md` / `docs/index.html` 是界面与文档里对外显示的版本号，发版时一并更新。

---

## 许可证与贡献授权

本项目以 **AGPL-3.0-only** 发布（见 [LICENSE](LICENSE)）。你提交的贡献默认按同一许可证授权（inbound = outbound），请确保你有权提交这些代码，并且不包含与 AGPL-3.0 不兼容的第三方代码。修改运行时会下载的第三方工具（yt-dlp / FFmpeg / aria2c / ImageMagick / pandoc / poppler / whisper.cpp）不在此列——那些是各自独立许可的程序，本仓库不包含它们的源码。
