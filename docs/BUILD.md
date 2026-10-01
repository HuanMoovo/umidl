# Umidl 多平台构建指南

> 版本：1.8.10 ｜ 适用范围：Tauri 2 + Rust + Vue 3 的 Umidl 桌面客户端
> 本文档只描述**怎么构建、产物在哪、验证到什么程度**。功能说明见 `README.md`，路线图见 `docs/ROADMAP.md`。

## 0. 平台状态总览（务必先看这张表）

| 平台 | 产物 | 状态 | 验证方式 |
|------|------|------|----------|
| **Windows x64** | `Umidl_1.8.10_x64-setup.exe`（NSIS 安装包） | ✅ **本机真编 + 真机回归**（安装、启动、捕获接口实测） | 本机 Windows 11 + MSVC 工具链，**4,844,965 B（4.62 MiB）** |
| **Linux x64** | `Umidl_1.8.10_amd64.deb` + `Umidl_1.8.10_amd64.AppImage` | ✅ **CI 真编通过并已发布**；本机 Docker 单测全绿 | GitHub Actions `ubuntu-22.04`；Docker `ubuntu:22.04` 跑 `cargo test --lib` → 230 passed / 0 failed |
| **macOS arm64** | `Umidl_1.8.10_aarch64.dmg` | ✅ **CI 真编通过并已发布**（未在真机回归） | GitHub Actions `macos-latest`（体积见 Release 资产） |
| **macOS x64 (Intel)** | `Umidl_1.8.10_x64.dmg` | ✅ **CI 交叉编译通过并已发布**（未在真机回归） | `macos-latest`（arm64）上 `--target x86_64-apple-darwin`（体积见 Release 资产）；**不再依赖稀缺的 `macos-13` runner** |
| **Android aarch64** | `.apk` | ⚠️ **实验性作业**（`continue-on-error`），本机无 JDK/SDK/NDK，且**需要移动端适配**（见 §4.4） | GitHub Actions `android` job：当前失败，不阻塞发布 |

一句话结论：**Windows 是“真的编过 + 真机跑过”；macOS（arm64 / Intel）与 Linux 是“CI 真的编出来了”，但未在对应真机回归；Android 仍是实验性。**

---

## 1. Windows x64（已验证）

```bash
npm ci
npx tauri build            # 或 npm run tauri:build
```

* 产物：`src-tauri/target/release/bundle/nsis/Umidl_1.8.1_x64-setup.exe`（仓库根目录也留了一份同名副本）。
* 安装包形态：NSIS，`installMode = currentUser`，可选简体中文 / English，配置见 `tauri.conf.json → bundle.windows.nsis`。
* 依赖：MSVC（Visual Studio Build Tools，含 Windows SDK）+ WebView2 运行时（Win10/11 自带）。
* 无需额外系统库；`rusqlite` 走 `bundled` 特性自带 SQLite 源码编译。

## 2. Linux x64（Docker 内已验证）

本机是 Windows，无法直接产出 Linux 产物，因此用 **Docker 容器**做交叉环境构建。仓库内已提供可复现的构建脚本：

| 文件 | 作用 |
|------|------|
| `scripts/docker/Dockerfile.linux` | `ubuntu:22.04` + Tauri 2 Linux 依赖 + Node 20 + Rust stable |
| `scripts/docker/linux-build-inner.sh` | 容器内步骤：拷源码 → `npm ci` → 前端构建 → `tauri build --bundles deb,appimage` → 收产物 / 打印 `dpkg-deb` 元数据 |
| `scripts/docker/build-linux.sh` | 宿主机驱动：建镜像 → 起容器（挂载 `/src`、`/build` 卷、`/out`）→ 产物回写 |

### 2.1 一键构建

```bash
# 在仓库根目录（Windows 上请在 Git-Bash 里执行）
bash scripts/docker/build-linux.sh              # 前台
DETACH=1 bash scripts/docker/build-linux.sh     # 后台，docker logs -f umidl-linux-build 看日志
```

等价的手工命令：

```bash
docker build -t umidl-linux-builder:22.04 -f scripts/docker/Dockerfile.linux scripts/docker
docker volume create umidl-build-workspace
docker run --rm --name umidl-linux-build \
  -v "D:/path/to/umi-downloader":/src:ro \
  -v "D:/path/to/umi-downloader/src-tauri/target/linux-dist":/out \
  -v umidl-build-workspace:/build \
  -e HTTP_PROXY=http://host.docker.internal:7892 -e HTTPS_PROXY=http://host.docker.internal:7892 \
  -e NO_PROXY=localhost,127.0.0.1,mirrors.ustc.edu.cn,mirrors.aliyun.com,registry.npmmirror.com \
  -e APPIMAGE_EXTRACT_AND_RUN=1 \
  umidl-linux-builder:22.04
```

产物回写到 **`src-tauri/target/linux-dist/`**（该目录在 gitignore 的 `src-tauri/target/` 之下，不会污染仓库）。

### 2.2 关键设计点（踩过的坑）

1. **不要把宿主机 `node_modules/`、`src-tauri/target/` 带进容器**：脚本用
   `tar --exclude=./node_modules --exclude=./src-tauri/target --exclude=./dist ...` 把源码复制到容器内 `/build`，
   `npm ci` 在容器内重装（原生模块与平台绑定，宿主机产物在 Linux 下不可用）。
2. **`/build` 用命名卷**（`umidl-build-workspace`）：Rust `target/` 与 npm 缓存跨次构建复用，二次构建快很多。
3. **国内网络是主要风险**：
   * `archive.ubuntu.com` 在本机网络下握手成功但 **0 字节**（实测 10s 收 0 B），必须换 `mirrors.aliyun.com`（实测 ~1.6 MB/s）；
   * `index.crates.io` 稀疏索引不可达、`static.crates.io` 仅 ~38 KB/s → cargo 走 USTC 镜像
     （`/usr/local/cargo/config.toml`：`sparse+https://mirrors.ustc.edu.cn/crates.io-index/`，实测索引 1.2 MB/s / crate ~257 KB/s）；
   * npm 用 `registry.npmmirror.com`（实测 4–8 MB/s）；
   * Node 用 npmmirror 二进制分发，Rust 用 `RUSTUP_DIST_SERVER=https://mirrors.ustc.edu.cn/rust-static`（实测 1.2 MB/s）；
   * **GitHub 只用代理**（AppImage 依赖 linuxdeploy，从 GitHub Releases 拉）：容器内 `-e HTTPS_PROXY=http://host.docker.internal:7892`，
     同时用 `NO_PROXY` 把国内镜像排除，避免代理拖慢镜像站。
4. **AppImage 打包需要 FUSE**，容器里通常没有 `/dev/fuse`，所以设置 `APPIMAGE_EXTRACT_AND_RUN=1` 让 linuxdeploy 自解压运行。
5. `tauri build` 的 `beforeBuildCommand` 已在脚本里单独执行过，所以打包时用
   `--config '{"build":{"beforeBuildCommand":""}}'` 覆盖为空，避免重复构建前端。

### 2.3 本机实测结果（产物与证据）

工具链（容器内实测）：

```
v20.19.5        # Node（npm 10.8.2）
rustc 1.98.1 (48a229cea 2026-09-01)
cargo 1.98.1 (797e8a9bc 2026-08-05)
2.50.4          # pkg-config webkit2gtk-4.1
```

关键日志（截断）：

```
[14:47:58] 3/6 前端构建（vue-tsc 类型检查 + vite build）
vite v7.3.6 building client environment for production...
✓ 4095 modules transformed.
dist/assets/index-DVgWacwW.js  675.26 kB │ gzip: 220.93 kB
✓ built in 4.70s
FRONTEND_FULL_BUILD=ok
[14:52:xx] release 编译：Finished `release` profile [optimized] target(s) in 2m 50s / Built application at: /build/src-tauri/target/release/umidl
Bundling Umidl_1.8.1_amd64.deb (/build/src-tauri/target/release/bundle/deb/Umidl_1.8.1_amd64.deb)
Bundling Umidl_1.8.1_amd64.AppImage (/build/src-tauri/target/release/bundle/appimage/Umidl_1.8.1_amd64.AppImage)
[15:11:14] BUILD_ALL_OK
```

> 注：二次构建（复用命名卷 `/build`）实测 `npm ci` 8s、`cargo` release 编译 1m55s、全流程 ~12 分钟（主要耗时是 GitHub 下载 linuxdeploy ~19.8 MB）。冷构建（清空卷）约 30–35 分钟，其中 apt 251 MB ≈ 6.5 分钟。

产物（已 `docker cp` 等价回写到宿主机 `src-tauri/target/linux-dist/`）：

| 文件 | 字节数 | sha256（本次构建） |
|------|--------|--------------------|
| `Umidl_1.8.1_amd64.deb` | 6,117,136 | `8f2ef8c2128744b111e54f610cc3973fa5ca83f72b55f4d3ed2ef24d56b68545` |
| `Umidl_1.8.1_amd64.AppImage` | 84,285,944 | `1fd1efb5ade31ce931540dd2f67b69cc9e127f895815e029aba892a3a8c4d094` |
| （容器内原始可执行文件） | 12,853,816 | `src-tauri/target/release/umidl` |

`dpkg-deb -I` 实测输出：

```
 new Debian package, version 2.0.
 size 6117136 bytes: control archive=635 bytes.
 Package: umidl
 Version: 1.8.1
 Architecture: amd64
 Installed-Size: 12634
 Maintainer: umi
 Priority: optional
 Depends: libwebkit2gtk-4.1-0, libgtk-3-0
 Description: 轻量化视频下载 · 转换 · AI 字幕
```

`dpkg-deb -c`（前几行）：`usr/bin/umidl`、`usr/share/applications/Umidl.desktop`、`usr/share/icons/hicolor/{32x32,128x128,256x256@2}/apps/umidl.png`。

**额外运行时校验**（在容器里解包后 `ldd` + 直接执行）：

```
APPIMAGE_EXTRACT_OK
APPIMAGE_BIN=squashfs-root/usr/bin/umidl   (12,894,705 B)
APPIMAGE_ALL_SHARED_LIBS_RESOLVED
DEB_LIBS_OK                                # dpkg-deb -x 解开后 ldd 无 "not found"
$ umidl --install-tools
数据目录: /root/.local/share/umi-downloader   # 无显示器环境跑 CLI 模式，二进制可正常执行
```

### 2.4 产物与校验命令

> **Node 版本注意**：容器里用的是 Node 20.19.5，`npm ci` 会对 `vue-i18n@11.4.12` / `@intlify/*` 报 `EBADENGINE`（这些包声明 `node >= 22`）——只是警告，`npm ci` 与 vite 构建均正常通过（见上方 `FRONTEND_FULL_BUILD=ok`）。
> 仓库的 CI（`release.yml`）已改用 **Node 22** 来消除该警告；如果想在容器里也用 22，覆盖构建参数即可：
> `docker build --build-arg NODE_VERSION=v22.26.0 -t umidl-linux-builder:22.04 -f scripts/docker/Dockerfile.linux scripts/docker`

```bash
ls -la src-tauri/target/linux-dist/
dpkg-deb -I src-tauri/target/linux-dist/*.deb      # 包元数据
dpkg-deb -c src-tauri/target/linux-dist/*.deb | head -30   # 文件清单
```

本次构建的**完整容器日志**已随产物留档：`src-tauri/target/linux-dist/linux-build.log`（622 行，含 `npm ci` / `vue-tsc` / `cargo` / 打包 / `dpkg-deb` 全部输出，结尾 `BUILD_ALL_OK`）。

## 3. macOS（仅 CI 就绪，本机未验证）

本机是 Windows，**没有 Xcode / clang 交叉工具链，无法本地编译** macOS 产物，因此完全依赖 CI。

`release.yml` 的 macOS 矩阵：

| runner | 架构 | `--target` | 产物名 |
|--------|------|-----------|--------|
| `macos-latest`（当前为 Apple Silicon 的 macOS 14 runner） | arm64 | `aarch64-apple-darwin` | `umidl-macos-arm64` |
| `macos-13`（最后一个 Intel runner） | x64 | `x86_64-apple-darwin` | `umidl-macos-x64` |

要点：
* `dtolnay/rust-toolchain@stable` 的 `targets:` 已显式传 `aarch64-apple-darwin` / `x86_64-apple-darwin`（矩阵字段 `rust_targets`），
  Tauri CLI 侧同步传 `--target`，两侧一致，避免“装了 A 架构编译 B 架构”。
* 产物收集路径 `src-tauri/target/**/release/bundle/**/*.dmg` 已经覆盖带 target 前缀的目录
  （`target/aarch64-apple-darwin/release/bundle/dmg/...`）。
* macOS 上通用二进制（universal）不是必须的；如需单个 universal dmg，可改用 `--target universal-apple-darwin`（需两个 target 都装上）。
* 未本机验证的部分：`icon.icns` 已存在（`src-tauri/icons/icon.icns`，由 `tauri icon` 生成）；签名 / 公证（codesign / notarize）**未配置**，
  CI 产出的是未签名 dmg，用户首次打开需要右键 → 打开。

## 4. Android aarch64（配置 + CI 就绪，本机未验证）

本机**没有 JDK、没有 Android SDK/NDK**（`java` / `adb` 均不存在），所以 Android 部分交付的是“配置到位 + CI job 就位”，
**没有本地构建产物**。下面写清楚已经改了什么、还缺什么。

### 4.1 已完成的配置

| 位置 | 内容 | 说明 |
|------|------|------|
| `src-tauri/Cargo.toml` → `[lib] crate-type` | `["staticlib", "cdylib", "rlib"]` | Tauri 2 移动端要求（Android 需要 `cdylib`，iOS 需要 `staticlib`，桌面用 `rlib`）。**已确认桌面构建不受影响**：`cargo check --lib` 通过。 |
| `src-tauri/src/lib.rs` | `run()` 上加了 `#[cfg_attr(mobile, tauri::mobile_entry_point)]` | 移动端 JNI 入口。`cfg(mobile)` 只在 android/ios 目标生效，桌面构建完全不受影响（同一次 `cargo check --lib` 验证）。 |
| `src-tauri/tauri.conf.json` | `identifier: "com.umi.downloader"`、`version: "1.8.1"`、`productName: "Umidl"` | 移动端必需，**保持不变**；桌面 `bundle` 配置（`targets: ["nsis"]`、图标列表）**未改动**。 |
| `src-tauri/icons/android/**` | mipmap 全套（17 个文件：`ic_launcher.png` / `ic_launcher_round.png` / `ic_launcher_foreground.png` + `mipmap-anydpi-v26/ic_launcher.xml` + `values/ic_launcher_background.xml`） | 官方文档：[tauri.app/develop/icons](https://tauri.app/develop/icons) —— 移动端图标由 `tauri icon` 生成后**直接放进 Xcode / Android Studio 工程**，这些文件已经就位，无需再跑 `tauri icon`。 |
| `.github/workflows/release.yml` → `android` job | JDK 17 + Android SDK + NDK 27.0.12077973 + Rust android targets + `tauri android init` + `tauri android build` | **从未在 runner 上执行过**，首次运行大概率要微调。 |

### 4.2 本地（如果将来要在这台机器上编）需要装什么

```bash
# 1) JDK 17（Temurin）+ Gradle 用 JAVA_HOME
# 2) Android SDK（cmdline-tools / platform-tools / platforms;android-34 / build-tools;34.0.0）
# 3) Android NDK 27.x，并导出：
#    export ANDROID_HOME=<sdk>  NDK_HOME=$ANDROID_HOME/ndk/27.0.12077973
# 4) Rust 目标
rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
# 5) 生成工程 + 构建
npx tauri android init
npx tauri android build --apk --target aarch64 --debug
# APK: src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
```

### 4.3 `src-tauri/gen/android` 要不要提交？

* 它是 `tauri android init` 生成的 Gradle 工程，**不是必需的提交物**；CI 里每次都会重新 `tauri android init`（幂等，工程已存在时只做增量更新）。
* 但 Tauri 官方建议**提交 `src-tauri/gen/android`**，这样你可以直接改 `AndroidManifest.xml`、权限、`build.gradle`、签名配置等原生文件，
  否则每次 `init` 都可能被模板覆盖。
* 当前仓库 **未提交** 该目录（`src-tauri/gen/` 下只有被 gitignore 的 `schemas/`）。如果后续要加权限（例如通知、后台下载、存储），建议：
  `npx tauri android init` → 提交 `src-tauri/gen/android` → 后续 `init` 改为 `--force` 需谨慎。

### 4.4 诚实的未验证清单（Android）

1. **CI job 未在 runner 上跑过**：`android-actions/setup-android@v3` 与 `sdkmanager --install "ndk;27.0.12077973"` 的组合、Gradle 首次下载、`tauri android init` 的模板版本都未验证。
2. **APK 默认是 debug 签名**（`--debug`）。release 构建需要 `keystore.properties` + `build.gradle` 签名配置（CLI 会提示），CI 里**没有配密钥**，
   所以流水线只产出 debug/未签名包。
3. **功能层面并未做移动端适配**：核心链路（yt-dlp / ffmpeg 外部可执行文件、`std::process::Command`、任意路径写文件、窗口 `dragDropEnabled` 等）
   在 Android 沙箱里不成立；`src-tauri/src/tools.rs` 的“下载外部二进制并执行”在移动端通常不可用（需要改用系统库或内置可执行）。
   也就是说：**APK 能编出来不代表功能可用**，真要做移动端需要单独一轮适配（下载器改用 Android 的系统下载/媒体 API 等）。
4. `plugins_builtin` / `selftest` 等模块含大量桌面路径假设，移动端运行前需要条件编译裁剪。
5. **图标的小瑕疵**：`src-tauri/icons/icon.png` 是 512×512，而 `tauri icon` 官方推荐源图 ≥1024×1024（并且它没被写进 `bundle.icon` 数组）。
   由于 `icons/android/**` 的 mipmap 已是完整一套，这**不阻塞** Android 构建；但如果以后要重新生成图标，建议先补一张 1024×1024 的 `icons/icon.png` 再跑 `npx tauri icon <源图>`。

## 5. CI（`.github/workflows/release.yml`）

| 触发 | 说明 |
|------|------|
| `workflow_dispatch` | 手动触发；输入 `only_windows=true` 时只跑 Windows 那个矩阵项（Android job 同样被跳过） |
| `push: tags: v*` | 打 tag 时全量构建四个平台 |

* `build` job（矩阵）：Windows / macOS arm64 / macOS x64 / Linux，步骤为 `npm ci` → `vue-tsc --noEmit` + `vitest run` → `cargo test --lib` → `tauri-action` 打包 → 上传产物。
  产物 artifact 名：`umidl-windows-x64`、`umidl-macos-arm64`、`umidl-macos-x64`、`umidl-linux-x64`。
* `android` job：见 §4，**未验证**。
* Linux runner 的系统依赖已补齐 Tauri 2 官方清单：`libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf build-essential curl wget file libssl-dev libxdo-dev pkg-config`。
* 本机（Windows）**只能验证 Windows 产物**；macOS / Android 依赖 runner，Linux 依赖 §2 的 Docker 流程（CI 与 Docker 两条路均已给出）。

### 5.1 全平台 CI 运行记录

**1.8.10（2026-10-01）**：四平台全绿 —— [run 36847765626](https://github.com/HuanMoovo/umidl/actions/runs/36847765626)（历史快照已重建）；Windows x64 / Linux x64 / macOS arm64 / macOS x64(Intel 交叉编译) 均 success，Android 实验性作业仍为 failure（`rquickjs-sys` 移动端未适配，`continue-on-error` 不阻塞）。本版把 LOGO 换回圆形；各平台产物体积见 [Release v1.8.10](https://github.com/HuanMoovo/umidl/releases/tag/v1.8.10)。

**1.8.9（2026-10-01）**：四平台全绿 —— [run 36836686644](https://github.com/HuanMoovo/umidl/actions/runs/36836686644)（历史快照已重建）；Windows x64 / Linux x64 / macOS arm64 / macOS x64(Intel 交叉编译) 均 success，Android 实验性作业仍为 failure（`rquickjs-sys` 移动端未适配，`continue-on-error` 不阻塞）。各平台的 CI 出包流程与本版一致（历史 Release 已随隐私清理一并移除）。

**1.8.8（2026-09-28）**：工作流 [run 36406666632](https://github.com/HuanMoovo/umidl/actions/runs/36406666632)（历史快照已重建）

| 作业 | 结果 | 说明 |
|------|------|------|
| `prepare` | ✅ success | 矩阵由 Python 生成成 JSON 再 `fromJSON`（matrix 不能出现在 job 级 `if` 里） |
| Windows x64 | ✅ success | 产物 `umidl-windows-x64` |
| Linux x64 | ✅ success | 产物 `umidl-linux-x64`（deb + AppImage） |
| macOS arm64 | ✅ success | 产物 `umidl-macos-arm64`（dmg） |
| macOS x64 (Intel) | ✅ success | 产物 `umidl-macos-x64`（dmg，在 arm64 runner 上交叉编译 `--target x86_64-apple-darwin`） |
| Android aarch64 | ❌ failure | 实验性作业（`continue-on-error`），不阻塞整体流程 |

首次运行时 macOS / Linux 全部倒在 `cargo test --lib` 上，暴露了两个**只在非 Windows 上出现**的缺陷（详见 §7 排查手册最后三行）；修复后 macOS（两个架构）与 Linux 的单元测试与打包全部通过。macOS x64 原计划用 `macos-13` runner，实测该 runner 在 GitHub 上长期排队（本轮其余三个平台都跑完了它仍未启动），因此改为在 `macos-latest`(arm64) 上交叉编译 x86_64 目标，产物架构与 Intel Mac 一致。

## 6. 版本号同步

一次发版需要同步 4 处，否则产物名与关于页会不一致：

1. `package.json` → `version`
2. `src-tauri/Cargo.toml` → `[package] version`
3. `src-tauri/tauri.conf.json` → `version`
4. 产物文件名里的版本（由上面自动推导）+ `README.md` 里的版本描述

```bash
# 快速自检
node -e "const p=require('./package.json'),c=require('./src-tauri/tauri.conf.json');
const t=require('fs').readFileSync('src-tauri/Cargo.toml','utf8').match(/^version = \"(.*)\"/m)[1];
console.log(p.version, c.version, t, p.version===c.version && c.version===t ? 'OK' : '不一致');"
```

## 7. 排查手册

| 现象 | 原因 | 处理 |
|------|------|------|
| Linux 构建卡在 `apt-get update` 不动 | `archive.ubuntu.com` 在国内不可达 | 换 `mirrors.aliyun.com`（Dockerfile 已处理） |
| `cargo` 卡在 `Updating crates.io index` | 稀疏索引被墙 | 用 `sparse+https://mirrors.ustc.edu.cn/crates.io-index/`（镜像内已配置） |
| AppImage 打包报 FUSE / `/dev/fuse` 错误 | 容器无 FUSE | `-e APPIMAGE_EXTRACT_AND_RUN=1` |
| AppImage 打包卡在下载 linuxdeploy | GitHub 直连慢 | 挂宿主代理 `-e HTTPS_PROXY=http://host.docker.internal:7892` |
| 容器内 `npm ci` 失败 / 原生模块报错 | 把宿主机 `node_modules` 拷进去了 | 用脚本的 tar 排除逻辑，容器内重装 |
| `tauri android init` 报找不到 NDK | `NDK_HOME` 未导出 | `export NDK_HOME=$ANDROID_HOME/ndk/<版本>` |
| macOS 产物无法打开（“已损坏”） | 未签名 / 未公证 | 右键 → 打开，或后续接入 codesign + notarize |
| **macOS 上捕获接口每个请求都回 408** | BSD 系的 `accept` 让新连接**继承监听套接字的非阻塞标志**（Linux 不继承），`read_line` 立刻拿到 `EAGAIN`，被当成“读超时” | accept 之后显式 `stream.set_nonblocking(false)`（`capture.rs`）；读超时仍由 `SO_RCVTIMEO` 兜底 |
| **Linux/macOS 上 `run_capture` 超时后拖满全程才返回**（实测 400 ms 超时、5.01 s 返回） | 只杀直接子进程时，`sh -c "sleep 5"` 的孙进程继续持有 stdout/stderr 管道写端，读线程要等它自然退出才 EOF | 子进程以 `process_group(0)` 起，超时按进程组 `kill(-pgid, SIGKILL)`，与 Windows 的 `taskkill /T` 对齐（`tools.rs::kill_tree`） |
| **macOS 上“端口没变却报端口被占用”** | 端口刚释放时立即 `bind` 可能短暂 `EADDRINUSE` | `capture::start` 做有界重试（最多 2 秒），确实被占用时仍如实报错 |
