# Umidl 1.8 真机 Bug 排查报告（BUG_SWEEP_1.8）

- 排查对象：`D:/path/to/umi-downloader`（源码 1.8.0）+ 已安装客户端 `%LOCALAPPDATA%/Umidl/umidl.exe`
- 排查方式：只读静态审查 + 真机执行验证（**未改动任何 `src/**`、`src-tauri/**`、`scripts/**` 文件**）
- 日期：2026-09-27；执行环境：Windows 11 (10.0.26200) / WebView2 Edg 142 / cargo 1.98.1 / node v22.23.2
- 本报告是本次排查唯一写入的文件

> ⚠️ 版本注意：仓库源码 `package.json` / `src-tauri/Cargo.toml` / `tauri.conf.json` 均为 **1.8.0**，但已安装的
> `%LOCALAPPDATA%/Umidl/umidl.exe` 文件版本为 **1.7.0**（`(Get-Item umidl.exe).VersionInfo` → `FileVersion 1.7.0`，
> 应用内 UI 与日志也显示 v1.7.0）。因此：静态结论针对 1.8.0 源码，**真机结论针对 1.7.0 二进制**。
> 每个 bug 都标注了它是“源码可读出的”还是“真机复现的”。

## 0. 真机验证环境的隔离方式（避免干扰其它实例）

```bash
# 独立数据目录（含 settings.json 与 umi.db），bin/models 用 junction 指向受管目录
UMI_DATA_DIR=<scratch>/umidata  UMI_DOWNLOAD_DIR=<scratch>/umidata/downloads \
WEBVIEW2_USER_DATA_FOLDER=<scratch>/wv2profile \
WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223 \
%LOCALAPPDATA%/Umidl/umidl.exe
# settings.json 内 capture_port=6973（避开他人占用的 6970）
```
CDP 用 Node 22 内置 `WebSocket` 直连 `http://127.0.0.1:9223/json/list`；
命令调用走页面内 `window.__TAURI_INTERNALS__.invoke`（应用未开 `withGlobalTauri`，`window.__TAURI__` 不存在）。

---

## 1. 结论速览

| ID | 严重度 | 摘要 | 位置 | 验证方式 |
|----|--------|------|------|----------|
| BUG-01 | **P1** | AI 字幕**必然失败**：whisper 实际产物 `<stem>.<语言>.srt`，程序却校验 `<stem>.srt` | `subtitle.rs:419` + `subtitle.rs:528` | 真机复现 + 手工复现 + Rust 路径语义证明 |
| BUG-02 | **P1** | 删除“下载中”的任务后，任务被运行中的任务线程 `upsert` 写回（列表复活），引擎继续下载 | `lib.rs:912`（remove_download）、`lib.rs:605/610` | 真机复现（DB 快照 + 进程 + 磁盘） |
| BUG-03 | P2 | 退出应用不回收引擎子进程（无退出清理路径），aria2c/yt-dlp/ffmpeg 变孤儿进程继续跑 | `lib.rs` 全文无 `RunEvent/ExitRequested/on_exit` | 真机复现（杀掉实例后 4 个孤儿 aria2c） |
| BUG-04 | P2 | 取消下载最终显示为“已暂停”（Canceled 被覆盖），且残留 `.aria2`/半成品文件 | `lib.rs:771` 覆盖 `lib.rs:897` | 真机复现（状态 + 磁盘） |
| BUG-05 | P2 | `concurrency`（任务并发数）设置**完全不生效**，无排队、无上限 | `lib.rs:553`（空 `if` 块） | 真机复现（3 并发设置 → 6 个 aria2c 同时跑） |
| BUG-06 | P2 | 捕获接口默认无令牌 + `ACAO: *` → 任意网站可指纹识别并越站入队（drive-by 下载） | `capture.rs:208`、`capture.rs:151-160` | 真机复现（带 `Origin: https://evil.example` 的 GET 成功入队） |
| BUG-07 | P2 | 粘贴 100 万行 → 主线程阻塞 **10.0 s**（粘贴路径无长度保护，文件导入路径有 400 KB 上限） | `BatchImportPanel.vue` + `lib.rs:492` | 真机测量（rAF 计时） |
| BUG-08 | P2 | 目标路径 >260 字符直接下载失败，且 UI 只显示无关的 “Download aborted” | `downloader.rs`/`lib.rs` 错误摘取 | 真机复现（app + 手工 aria2c 对照） |
| BUG-09 | P2（条件性） | `extract_audio` 只 pipe 不读 stderr；实测长运行（t≈720 s）出现“输出冻结 + 进程存活 + 输入健康”，读 stderr 的对照组正常完成（详见 §10） | `subtitle.rs:214-283`（pipe 于 243-244，等待循环 253-266） | 真机复现（4 组对照 + 管道容量/速率实测） |
| BUG-10 | P3 | 限速器 `Mutex::lock().unwrap()` 5 处，锁中毒即 panic | `ratelimit.rs:90,103,108,117,121` | 静态审查 |
| BUG-11 | P3 | 捕获端口被占用时只写日志，UI 无提示（需用户自己进设置页看状态） | `lib.rs:1414` | 真机日志（双开实例） |
| BUG-12 | P3 | `/capture` 无论是否真的入队/是否被过滤，都回 `{"ok":true,"queued":true}` | `capture.rs:231` | 静态审查 + 真机响应 |
| BUG-13 | P3 | 字幕失败后仍把 `<stem>.<语言>.srt` 留在视频目录，任务里无任何指向 | `subtitle.rs:528` | 真机复现（磁盘） |
| BUG-14 | P3 | ED2K 任务入库即 `done` + `progress=0` + `file_path=NULL`，队列显示“完成”具误导性 | `lib.rs` ED2K 分支 | 真机 DB 快照 |

“已验证为正常”的清单见 §12；未能验证/结论受限的项见 §13。

---

## 2. BUG-01（P1）AI 字幕功能必然失败：产物路径与校验路径不一致

**位置**：`src-tauri/src/subtitle.rs:419`（校验）与 `src-tauri/src/subtitle.rs:528`（命名）；
调用链 `lib.rs:1150 start_subtitle → run_subtitle_job → transcribe_to_format → run_whisper`

```rust
// subtitle.rs:516-528  产物基名 = <视频名>.<语言>
dir.join(format!("{stem}.{}", req.language.replace("auto", "detected")))     // →  .../clip12s.zh
// subtitle.rs:419      期望文件用 with_extension("srt") 拼出来
let srt = out_base.with_extension("srt");                                    // →  .../clip12s.srt  ✗
```
`Path::with_extension("srt")` 会**替换**最后一段扩展名（把 `.zh` 换掉），而 whisper-cli 的
`-of <base>` 是**追加** `.srt`。两者永不相等 ⇒ 每个字幕任务都在“文件不存在”上失败。

**复现步骤（真机，隔离实例）**
1. 启动隔离实例，CDP 调用：
   `invoke('start_subtitle', { req: { video_path: '<...>/clip12s.mp4', language: 'zh', model: 'tiny', output_format: 'srt' } })`
2. 轮询 `list_subtitles`：任务 3 秒内变为 `error`。

**原始输出（真机）**
```
{"task_id":"b2afc0ce","states":[{"t":0,"status":"error","progress":8,
 "err":"Whisper 未生成字幕文件（C:\\...\\media\\clip12s.srt）"}]}
```
而磁盘上 whisper **确实成功产出了文件**（任务失败后仍留在视频目录）：
```
-rw-r--r-- 1 user  user 46  9月 27 20:26 clip12s.zh.srt
$ cat clip12s.zh.srt
1
00:00:00,000 --> 00:00:12,000
(不幸)
```

**机制旁证（手工复现 + 路径语义）**
```
$ <bin>/whisper/whisper-cli.exe -m .../ggml-tiny.bin -f mani_test.wav -of .../mani_test_app_flags -l zh -osrt -pp -t 8
exit: 0
files produced: ['mani_test.wav', 'mani_test_app_flags.srt']      ← 基名无语言后缀时 = 追加 .srt

# Rust 路径语义（pathtest 复现程序）
out_base                          = C:\tmp\media\clip12s.zh
out_base.with_extension("srt")    = C:\tmp\media\clip12s.srt       ← 程序去找的
whisper wrote                     = <tmp>\clip12s.zh.srt (exists=true)
app looks for                     = <tmp>\clip12s.srt    (exists=false)
```
附：受管 whisper 二进制解析为 `whisper-cli.exe`（`tools.rs:186`、`tools.rs:907`）；同目录的 `main.exe`
已废弃，手工直接跑会 `exit 1` 并提示 “The binary 'main.exe' is deprecated”。

**为什么自检没发现**：`selftest.rs:1434` 传的是 `let base = e.p("whisper_out");`（**没有**语言后缀），
绕开了 `subtitle_out_base`，所以自检用例 `subtitle.whisper` 能通过而生产路径必挂。

**修复建议**：产物路径改为“基名 + 后缀”拼接（`out_base.with_file_name(format!("{}.srt", …))` 或
`format!("{}.srt", out_base.display())`），并在找不到时回退扫描 `<base>*.srt`；同时让自检用
`subtitle_out_base` 生成基名，覆盖真实命名。

---

## 3. BUG-02（P1）删除“下载中”的任务 → 任务复活 + 引擎继续下载

**位置**：`lib.rs:912-934`（`remove_download` 不取消/不杀进程）；`lib.rs:605 update_and_emit`（写回 DB）；`db.rs upsert_download`（`INSERT … ON CONFLICT(id) DO UPDATE`）

**复现步骤（真机）**
1. 启动 6 个慢速下载（`concurrency=3`，见 BUG-05），确认全部 `downloading`。
2. `invoke('remove_download', { id: <bulk_d 的任务>, deleteFile: true })` → 立即读 `list_downloads` 确认记录消失。
3. 等 ~60 s 再读 `list_downloads`，并检查进程与磁盘。

**原始输出（真机）**
```
# 删除瞬间
{"removed_id":"8b90ef03","removed_status_after":"GONE-FROM-DB", ...}
# 约 1 分钟后同一条记录又出现（同一个 id，仍是 downloading）
{"id":"8b90ef03","url":"bulk_d.bin","status":"downloading","progress":20}
# 引擎进程仍在跑（该任务对应 -dir/URL）
11552 http://127.0.0.1:18330/bulk_d.bin
# 磁盘上文件仍在增长
-rw-r--r-- 1 user  user 4373704  9月 27 20:11 bulk_d.bin
-rw-r--r-- 1 user  user     139  9月 27 20:11 bulk_d.bin.aria2
```
`delete_file: true` 也没能删掉半成品：下载中 `file_path` 仍为 `NULL`，删除逻辑只删
`t.file_path` 与 `{file_path}.part`（`lib.rs:918-926`），而真实文件是 `--dir` 下的 URL 基名 + `.aria2` 控制文件。

**修复建议**：`remove_download` 先 `mark_cancel` + `ctx::kill_tree(pid)`（与 `cancel_download` 一致）再删库；
删除时按 `--dir` + 任务 URL 基名清理 `<name>`、`<name>.aria2`、`<name>.part`；`upsert_download` 可加
“仅在任务仍存在时更新”的保护（或让任务线程在发现行被删后自杀）。

---

## 4. BUG-03（P2）退出应用不回收引擎子进程

**位置**：全局（`lib.rs`/`main.rs` 内不存在 `RunEvent::Exit`/`ExitRequested`/`on_exit`/窗口关闭清理逻辑；
`ctx::kill_tree` 只被 `cancel_*`/`pause_*` 调用：`lib.rs:848, 900, 1132, 1327`）

**复现步骤（真机）**：让若干下载处于进行中，直接结束应用进程，然后查进程与文件。

**原始输出（真机）**
```
# 应用进程已死（对 15312 执行 Stop-Process -Force），子进程仍在：
27388 ppid=15312 ORPHAN dir=--dir
16004 ppid=15312 ORPHAN dir=--dir
15340 ppid=15312 ORPHAN dir=--dir
24676 ppid=15312 ORPHAN dir=--dir
```
（4 个孤儿 `aria2c.exe`；同机还观察到先前会话遗留的 `emule.exe` 亦为孤儿。）
结合 BUG-02，实际后果是：退出后引擎仍会写入磁盘；下次启动 `reset_stale_downloads()`（`lib.rs:1401`）
把库里任务改成“暂停”，但**孤儿进程并不会被回收**，用户看到的是“已暂停”却有后台进程在跑。

**修复建议**：在 `RunEvent::ExitRequested/Exit` 与托盘退出路径中遍历 `state.pids` 执行 `kill_tree`；
或启动时清扫受管引擎残留进程（按 exe 路径 + 父进程已死判定）。

---

## 5. BUG-04（P2）取消下载 = 显示“已暂停”，并残留临时文件

**位置**：`lib.rs:897 cancel_download`（先写 `Canceled`）被 `lib.rs:771`（`canceled || killed → Paused`）覆盖

**复现步骤（真机）**：对 `downloading` 任务调用 `invoke('cancel_download', { id })`，等 4 s 看状态。

**原始输出（真机）**
```
{"canceled_id":"15f337f8","canceled_status_after":"paused"}      # 命令本身返回 Ok，但状态是 paused
-rw-r--r-- 1 user  user    0  9月 27 20:10 bulk_c.bin          # 取消后残留
-rw-r--r-- 1 user  user   59  9月 27 20:10 bulk_c.bin.aria2    # 取消后残留（aria2 控制文件）
```
说明：取消**确实杀掉了子进程**（`aria2c .../bulk_c.bin` 之后不在进程表里，这点是好的），
但状态语义变成“可继续的暂停态”，与“取消”按钮的预期不符；`clear_downloads` 的 `done` 分支会删除
`done/error/canceled`（`db.rs`），而 `canceled` 状态在生产路径上**永远不会出现**。

**修复建议**：区分 `pause`（→Paused）与 `cancel`（→Canceled，并清理 `<name>`/`<name>.aria2`）；或在
任务结束分支里判断“是否用户主动取消”而不是统一压成 Paused。

---

## 6. BUG-05（P2）`concurrency` 设置完全不生效

**位置**：`lib.rs:552-555`
```rust
if !s.is_running(&req.url) && s.running_count() as i64 >= s.settings_snapshot().concurrency.max(1) {
    // 允许排队：不阻塞，仅记录        ← 空块：既不排队、也不“记录”
}
```
`concurrency` 在 Rust 侧仅此一处被读取（`grep -rn concurrency src-tauri/src` 印证），前端只在设置页读写数值。

**复现步骤（真机）**：设置 `concurrency=3`（默认值 3，设置文件确认），一次性入队 6 个慢速 URL。

**原始输出（真机）**
```
"concurrency_setting": 3, "tasks_for_18330": 6,
"statuses": { "parsing": 4, "downloading": 2 }        # 12s 后 6 个任务全部处于活动态
(Get-Process aria2c).Count  →  6                      # 6 个引擎进程同时运行
```
结论：设置项对“同时下载几个任务”无任何约束（`--max-concurrent-downloads=1` 只作用于单个 aria2 进程内部的队列）。

**修复建议**：实现真正的排队（超出并发的任务置 `Pending/Queued`，完成回调里唤醒下一个），
或把该设置项明确标注为“仅控制引擎分段”并改名，避免误导。

---

## 7. BUG-06（P2）捕获接口默认无令牌 → 任意网站可指纹识别并“越站入队”

**位置**：`capture.rs:208`（`if !token.is_empty() && …`：令牌为空时**不做任何校验**）；
`capture.rs:151-160`（响应固定 `Access-Control-Allow-Origin: *`）；默认 `capture_token` 为空（`settings.json` 实测）

**复现步骤（真机，隔离实例 6973）**
```bash
curl -i -H "Origin: https://evil.example" http://127.0.0.1:6973/ping
curl -i -H "Origin: https://evil.example" \
  "http://127.0.0.1:6973/capture?url=http%3A%2F%2F127.0.0.1%3A9%2Fdriveby.bin&source=web"
```
**原始输出（真机）**
```
HTTP/1.1 200 OK
Access-Control-Allow-Origin: *
{"ok":true,"app":"Umidl","version":"1.7.0","capture":true}

HTTP/1.1 200 OK
{"ok":true,"queued":true}
```
落库证据（隔离库）：
```
('99ac22e4', 'http://127.0.0.1:9/driveby.bin', 'downloading', ..., None)
app 日志：[capture] 捕获链接：http://127.0.0.1:9/driveby.bin（来源 web）
```
`GET /capture` 是 CORS “简单请求”，无需预检即可被任意站点触发；`/ping` 带 `ACAO:*`，可被任意站点读取
→ 任何网页都能探测用户是否装了 Umidl、并让客户端去下载指定链接（配合默认 `capture_auto_queue=true` 直接开始下载）。

**修复建议**：默认生成随机 `capture_token` 并在首启写入设置；令牌非空时**强制**校验；
响应去掉 `Access-Control-Allow-Origin: *`（或仅在带令牌时回 ACAO）；必要时校验 `Origin/Referer` 或要求自定义头触发预检。

---

## 8. BUG-07（P2）粘贴 100 万行冻结 UI 10 秒（粘贴路径无长度保护）

**位置**：`src/components/BatchImportPanel.vue`（`<textarea v-model="text">` → `parseLinks(text)` 全量计算）；
对照：文件导入路径受 `lib.rs:490-497 read_text_file` 的 `limit.unwrap_or(400_000)` 保护，粘贴不受保护。

**复现步骤（真机，CDP 直接驱动 v-model 的真实 input 事件）**
```js
setter.call(ta, Array.from({length:1e6},(_,i)=>`https://example.com/video_${i}.mp4`).join('\n'));
ta.dispatchEvent(new Event('input', { bubbles: true }));
// 用 requestAnimationFrame 测“下一帧到手”的延迟
```
**原始输出（真机）**
```
100,000 行 (3.6 MB):  handler_block_ms = 1092, time_to_next_frame_ms = 1051
1,000,000 行 (36.9 MB): handler_block_ms = 10034, time_to_next_frame_ms = 9433
counter 文本：识别到 100000 条链接，去重后 100000 条   （溢出提示正常显示）
```
实测：主线程被完全占用 ~10 s（期间窗口无响应），随后自行恢复；而真正能入队的上限只有 200 条（前端已提示
“单次最多 200 条”），这 10 秒纯属浪费。

**修复建议**：粘贴/输入时对文本做长度上限（如 512 KB）并给出提示；或把 `parseLinks` 放到
`requestIdleCallback`/worker 里增量计算，并对 textarea 值做截断。

---

## 9. BUG-08（P2）超长路径（>260 字符）下载失败，且错误信息无法定位原因

**位置**：错误摘取在 `lib.rs:790-796`（`let tail = o.stderr_tail.lines().last()`）与 `downloader.rs` 的 stderr 掏取逻辑

**复现步骤（真机）**：目录 = `<downloads>/xxx…(120)/yyy…(120)`（共 318 字符，目录**已存在**），
`invoke('start_download', { req: { url: 'http://127.0.0.1:18330/longpath.bin', output_dir: <该目录> } })`

**原始输出（真机）**
```
# 应用内任务错误（只有这一句，看不到原因）
"error": "09/27 20:27:09 [ERROR] CUID#7 - Download aborted. URI=http://127.0.0.1:18330/longpath.bin"
# 用同参数手工执行 aria2c 才能看到真正原因
[ERROR] CUID#7 - Download aborted. URI=...
Exception: [AbstractCommand.cc:403] errorCode=18 URI=...
  -> [util.cc:1948] errNum=2 errorCode=18 Failed to make the directory C:/.../yyy..., cause: No such file or directory
```
对照组（同为边界，全部**正常**）：目录含空格+中文+emoji → 正常下载并落盘
（`umidata/downloads/测试 目录 🎬/spaced.bin`）；目标目录**不存在** → aria2 自动创建后正常下载；
Windows 保留名 `CON.mp4` → 本机可正常创建并下载（4.37 MB 已写入）。

**修复建议**：入队前检查目标路径长度（含文件名 >259 提示“路径过长”）；或在 Windows 上给
aria2 `--dir` 传 `\\?\` 长路径前缀；同时把子进程 stderr 的“原因行”（errorCode/Exception 行）一并展示。

---

## 10. BUG-09（P2，条件性）`extract_audio` 只 pipe 不读 stderr：长时间运行时进程卡死

**位置**：`subtitle.rs:214-283`；`Stdio::piped()` 于 `subtitle.rs:243-244`，等待循环 `subtitle.rs:253-266`
（只有 `try_wait` + 120 ms sleep，**没有任何线程读 stderr**），stderr 要等子进程退出后才读（`subtitle.rs:267`）；
看门狗 3600 s。`-nostats` 只在转换路径出现（`converter.rs:275`），提取路径没有。

**真机对照实验（同一台机器、同一个 ffmpeg 8.0.1、同一套参数）**

1) 参数与应用的完全一致（`-hide_banner -nostdin -y -i <url> -vn -ac 1 -ar 16000 -c:a pcm_s16le -f wav <out>`），
   输入用节流本地 HTTP（2 小时 TS，150 KB/s，约 720 s 运行时长），**不读 stderr**（决定性实验 expG，带独立连接的输入健康探测）：
```
t=751s out=230162432B FROZEN for 20s input_health=15ms/1024
t=791s out=230162432B FROZEN for 60s input_health=2ms/1024
VERDICT=STALLED-WHILE-INPUT-HEALTHY; killing child
FINAL VERDICT=STALLED-WHILE-INPUT-HEALTHY
```
   独立复跑（bug2）：同样在 `t=720s` 冻结，`ffmpeg ALIVE`，停滞 110 s 后才被实验脚本杀掉。
2) **同样输入、同样参数，但开线程读 stderr（正确做法）**：正常退出
```
t=720s out=230400760B stderr=126165B
EXIT rc=0 elapsed=720.0s stderr_total=126165B
out_final=230400760B            # 文件完整（= 44B 头 + 230400000B 音频 + 扩展块）
```
3) 用 `-loglevel debug` 把 stderr 量放大，**输入先全部缓冲到本地**（排除输入侧干扰）：
```
A: rc=0 elapsed=15.9s stderr_bytes=22419 (22 KiB)       # 有读 → 正常结束
B: STILL ALIVE after 180s -> ffmpeg BLOCKED on the undrained stderr pipe   # 不读 → 卡死
```
**量化**：本机 Rust `Stdio::piped()` 匿名管道容量实测 = **65536 B**（`pipetest`：子进程写到 65536 字节即阻塞）；
应用参数下 ffmpeg 的 stderr 速率实测 **183.8 B/s**（45 s 采样 8273 B；另有 175-182 B/s 两组），
一次 720 s 运行累计 **126,165 B ≈ 2 × 管道容量** ⇒ 不排空时必然写满并阻塞；看门狗却是 3600 s
（`subtitle.rs:258-261`），最终只会报“音频提取失败”。**加 `-nostats` 后同样采样得 16.0 B/s（降 11×）**，
即修复成本极低。

**残留不确定性（本条结论的边界，勿当事实引用）**：① 冻结发生在“输入接近传完”的末段（输出距完整文件
仅差 238,328 B），与“stderr 写满应发生在中段 ≈356 s”的预测不完全吻合；② 收尾时管道内仍可读到 4081 B，
与“管道被 65536 B 塞满”不符，说明该次冻结可能叠加了收尾阶段的其它因素（最终 stats 写入 / 服务端关闭竞态）。
机制层面（不排空 stderr → 子进程阻塞）已由第 3 组放大实验确证；**应用代码 100% 确认不排空 stderr**（这是确定的）。
“日常小文件遇不到、大文件/慢盘会踩到”属于推断：本机 2 小时音频提取实测仅 **2.12 s**（≈3.5e3× 实时），
距阈值极远。

**修复建议**：`extract_audio` 照 `downloader.rs` 的做法起两个 reader 线程（或用 `-nostats`）；
同时把 3600 s 看门狗改为“进度停滞超时”（如 5 分钟无输出增长即判定失败）以便快速失败。

---

## 11. 其余（P3）

- **BUG-10 限速器锁 unwrap**：`ratelimit.rs:90,103,108,117,121` 使用 `self.inner.lock().unwrap()`。仅在持锁线程
  panic 后（锁中毒）才会 panic，属于潜在崩溃路径；其余生产代码的 unwrap/expect 已确认**全部**是静态正则编译
  （`downloader.rs:30-38`、`converter.rs:17-19`、`subtitle.rs:289`、`ed2k.rs:707`）或进程入口（`lib.rs:1534`）。
  `selftest.rs` 的 4 处 unwrap 仅在 `--features selftest`（开发/CI）编译，安装包不含。
- **BUG-11 捕获端口冲突仅写日志**：`lib.rs:1414`。双开实例真机日志：
  `[2026-09-27 20:01:32] [capture] 捕获服务启动失败：监听 127.0.0.1:6970 失败：通常每个套接字地址… (os error 10048)`；
  设置页确实会调 `capture_status` 显示状态（`SettingsV14.vue:333`），但主界面没有任何提示。
- **BUG-12 `/capture` 响应不反映真实结果**：`capture.rs:231` 恒返回 `{"ok":true,"queued":true}`，
  即使 `capture_auto_queue=false` 或被过滤规则拦截也不会区分。
- **BUG-13 字幕失败仍留产物**：见 §2，`clip12s.zh.srt` 留在视频目录，任务记录里没有任何指向。
- **BUG-14 ED2K 任务语义**：真机 DB 快照 `('beea9adc','umi-verify-v15.bin', 'done', progress='0.0', file_path=NULL, format_note='ED2K · 已交由引擎接管')`
  → 队列里显示“完成/100%”，实际只是把链接交给了 eMule。

---

## 12. 已验证为正常（本轮真机/测试实测通过）

1. **Rust 单测**：`cargo test --offline --lib` → `115 passed; 0 failed`；
   `cargo test --offline --features selftest` → `125 passed; 0 failed; 2 ignored`。
2. **前端单测**：`npx vitest run` → 6 个文件 / 67 个用例全绿（batch、settings、ffmpeg、ParticleBg、utils、i18n）。
3. **i18n 占位符**：自写脚本扫描 `src/**/*.{vue,ts}` 的 `$t('key', {命名参数})` 用法 × 4 个语言包
   （en/fr/ja/zh）：21 个带参 key，**0 处缺参、0 处缺 key**（`i18n_check.py`）。
4. **路由/控制台**：真机遍历 `#/ → #/download → #/converter → #/subtitle → #/settings → #/plugins`，
   无 `console.error/warn`、无 `window.onerror`、无 unhandledrejection；每页 DOM 196-884 节点。
5. **前端无定时器/监听器泄漏**：全仓 `setInterval` 为 0；`setTimeout`（Ed2kPanel 400ms、SettingsV14、Download.vue）
   均在 `clearTimeout`/`onBeforeUnmount` 里清理；`ParticleBg` 的 rAF 与 resize/visibilitychange 监听在卸载时注销；
   `theme.ts` 的 `matchMedia` 返回 disposer，`App.vue:96-99` 卸载时调用；`stores/tasks.ts` 的 6 个事件订阅
   （download/convert/subtitle/selftest/tool 进度）与 `CaptureWatcher.vue` 的 `capture://url` 订阅：
   `bootstrap()` 有 `ready` + 在飞 Promise 双重幂等保护（`tasks.ts:47-49`），各视图守卫 `if (!store.ready)`，
   真机日志只出现一次“bootstrap 明细”，**未观察到重复订阅**（订阅随应用生命周期存活，不随路由重建）。
6. **子进程回收（下载/转换/识别路径）**：`run_download`/`run_aria2`/`run_convert`/`run_whisper` 均为
   stdout+stderr 各起 reader 线程 + 取消时 `taskkill /T /F`；真机验证取消后对应 `aria2c` 进程立即消失。
7. **重启后的任务状态**：强杀实例后重启，进行中的 4 个任务全部变为 `paused`（`reset_stale_downloads`，无 error），
   符合“异常退出→可续传暂停”的设计。
8. **路径边界**：含空格+中文+emoji 目录正常；不存在的目标目录 aria2 自动创建后正常下载；
   Windows 保留名 `CON.mp4` 在本机可正常创建下载（未做名净化，但未观察到失败）。
9. **字幕临时文件清理**：失败任务结束后 `cache/subtitle-work/` 为空（`<task_id>.wav` 已被删除）。
10. **大文件导入有保护**：`read_text_file` 默认截断到 400,000 B（`lib.rs:492`），txt 导入不会把超大文件灌进 UI。
11. **批量入队 200 条上限已在前端明示**：`BatchImportPanel.vue` 渲染提示“单次最多 {n} 条，超出的不会入队”，
    后端 `enqueue_links` 的 `count >= 200` 截断与前端的展示一致 —— 属**已披露**行为，不计为 bug。
12. **锁未跨 await**：`ratelimit::acquire` 先同步取令牌再 `sleep().await`（guard 已释放）；
    `lib.rs` 中 `settings`/`running`/`pids` 的 `lock()` 均为短作用域取值，未发现跨 await 持锁。
13. **捕获服务端口/令牌机制**：默认监听 127.0.0.1；设置 `capture_token` 非空时 `/capture` 会校验
    `X-Umidl-Token`（代码路径 + 单测 `capture.rs` 的解析用例）；端口占用时不会崩溃（见 BUG-11）。
14. **ED2K 引擎状态探测**：真机下载页显示“运行中 · Web 4711 · emule 已在运行，可直接接管链接”，
    与受管 `bin/emule/emule.exe` 进程一致；未提交 ed2k 链接（避免影响其它实例共享的 eMule 引擎）。

---

## 13. 未验证 / 结论受限（不当结论使用）

1. **BUG-09 的本地文件场景**：阈值（≈6 分钟连续运行）与阻塞机制已实测，但“本地大文件/NAS 上一定触发”
   属于推断；本地 2 小时文件提取实测只需 2.12 s，未直接触发。另见 §10 末尾两条残留不确定性
   （冻结点位置与“管道残留 4081 B”不符合完全塞满的预期）。
2. **粘贴 100 万行后“长达数分钟的卡死”**：第一次观测到渲染进程 ≥52 s 无响应、随后实例被外部进程终止
   （`umidl.exe` 消失、无 WER/事件日志崩溃记录），**无法归因于粘贴**；可确定的只有干净环境下的 10.03 s 阻塞。
3. **应用“正常关闭窗口”是否回收子进程**：代码中未发现任何退出清理路径（grep 无 `RunEvent/ExitRequested/on_exit`），
   但实测用的是强制结束（BUG-03 结论对强制结束成立，正常关闭未实测）。
4. **设置 `capture_token` 后的越站防护**：未实测（需要另建配置实例）。
5. **磁盘写满 / 无写权限**：未测试（未制造磁盘满场景）；仅静态看到 `ensure_dir` 失败在 `ctx.rs:54` 被 `let _ =` 吞掉。
6. **`yt-dlp` 下载主路径 / 站点解析**：本轮未跑真实外网下载（环境的网络与用例未覆盖），
   仅验证了 aria2 分支与受管工具可用性。
7. **`docs.rs`/`plugins.rs` 的 unwrap 全部位于 `#[cfg(test)]` 之后**（首处 `#[cfg(test)]`：`docs.rs:2046`、
   `plugins.rs` 同类），未在运行路径发现。
8. **双开实例共享同一 `umi.db`**：观察到 2 个实例同时运行（20:01），但未做并发写入压力测试。

---

## 14. 复现/证据文件清单（scratch，均在本机）

| 文件 | 内容 |
|------|------|
| `bug2.log` / `control.log` / `ctrl150.log` / `expG.log` | extract_audio 管道实验（不读 stderr / 读 stderr / stderr 速率 / 带输入健康检查的对照） |
| `overfill_test.py` | 放大 stderr 的“必定溢出”对照实验（A 读、B 不读） |
| `pipetest/` | Windows 匿名管道容量实测（Rust `Stdio::piped()` → 65536 B） |
| `pathtest/` | `Path::with_extension` 语义 + “whisper 产物 vs 应用查找” 文件存在性证明 |
| `whisper_manual.py` | whisper-cli 按应用参数手工运行（产物命名旁证） |
| `cdp.mjs` / `expr_*.js` | CDP 驱动与各功能用例（并发、取消/删除、边界路径、字幕、重启状态） |
| `i18n_check.py` / `unwrap_audit.py` | i18n 占位符与生产代码 unwrap 审计脚本 |
| `throttle.go.py` | 节流本地 HTTP 源（可控 ffmpeg 运行时长） |
| `umidata/` | 隔离数据目录（独立 `settings.json` 与 `umi.db`，bin/models 为 junction） |

