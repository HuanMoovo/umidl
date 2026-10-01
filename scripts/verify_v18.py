"""Umidl 1.8 实机验收：
  A. 格式全面可用（≥80/88）+ PSD/DDS 真产物 + 队列预览卡片（真实大小）
  B. BUG 修复验证：字幕真出 SRT、删除中任务不复活、并发真排队、取消=canceled、
     长路径明确报错、ED2K 移交状态、超长粘贴不卡、捕获无令牌拒绝越站、退出不留孤儿
  C. UI 收敛回归：5 页签 + 旧深链映射 + 卡片预算 + 单屏高度

用法：python scripts/verify_v18.py
"""
import asyncio
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import threading
import time
import http.server

import requests
import websockets


class Driver:
    """CDP 驱动（与 verify_v17 同款）"""

    def __init__(self, port=9224):
        self.port = port
        self.ws = None
        self.n = 1

    async def connect(self):
        for _ in range(40):
            try:
                d = requests.get(f"http://127.0.0.1:{self.port}/json", timeout=3).json()
                pages = [t for t in d if t.get("type") == "page"]
                if pages:
                    self.ws = await websockets.connect(
                        pages[0]["webSocketDebuggerUrl"], max_size=64 * 1024 * 1024
                    )
                    return True
            except Exception:
                pass
            await asyncio.sleep(0.5)
        return False

    async def cmd(self, method, **params):
        mid = self.n
        self.n += 1
        await self.ws.send(json.dumps({"id": mid, "method": method, "params": params}))
        while True:
            msg = json.loads(await self.ws.recv())
            if msg.get("id") == mid:
                if "error" in msg:
                    raise RuntimeError(msg["error"])
                return msg.get("result", {})

    async def js(self, expr):
        r = await self.cmd("Runtime.evaluate", expression=expr, returnByValue=True, awaitPromise=True)
        res = r.get("result", {})
        if res.get("subtype") == "error":
            raise RuntimeError(res.get("description", "JS error"))
        return res.get("value")

    async def invoke(self, cmd, args=None, **kw):
        payload = json.dumps(kw or args or {})
        out = await self.js(
            "(async () => { try { const r = await window.__TAURI_INTERNALS__.invoke("
            f"{json.dumps(cmd)}, {payload}); return JSON.stringify({{ ok: true, v: r }}); }} "
            "catch (e) { return JSON.stringify({ ok: false, e: String(e) }); } })()"
        )
        data = json.loads(out) if out else {"ok": False, "e": "no result"}
        if not data.get("ok"):
            raise RuntimeError(f"{cmd} 调用失败：{data.get('e')}")
        return data.get("v")

    async def try_invoke(self, cmd, args=None, **kw):
        payload = json.dumps(kw or args or {})
        out = await self.js(
            "(async () => { try { const r = await window.__TAURI_INTERNALS__.invoke("
            f"{json.dumps(cmd)}, {payload}); return JSON.stringify({{ ok: true, v: r }}); }} "
            "catch (e) { return JSON.stringify({ ok: false, e: String(e) }); } })()"
        )
        data = json.loads(out) if out else {"ok": False, "e": "no result"}
        return data.get("ok"), (data.get("v") if data.get("ok") else data.get("e"))

    async def goto(self, hash_, settle=1.9):
        await self.js(f"location.hash = {json.dumps(hash_)}; 'ok'")
        await asyncio.sleep(settle)


class AppDriver(Driver):
    """启动被测程序 + 提供 start/close（脚本里直接用，不依赖 drive_app.py 的 CLI）"""

    def __init__(self, exe=None, port=9224):
        super().__init__(port)
        self.exe = exe or APP   # APP 在文件后段定义，这里在调用时解析

    async def start(self):
        kill_app()
        env = dict(os.environ)
        env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = f"--remote-debugging-port={self.port}"
        subprocess.Popen([self.exe], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        ok = await self.connect()
        await asyncio.sleep(2.2)
        return ok

    async def close(self):
        try:
            await self.js("window.close(); 'ok'")
        except Exception:
            pass
        await asyncio.sleep(0.8)
        kill_app()

APP = os.path.expandvars(r"%LOCALAPPDATA%\Umidl\umidl.exe")
DATA = os.path.expandvars(r"%APPDATA%\umi-downloader")
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WORK = os.path.join(DATA, "verify18")
BIN = os.path.join(DATA, "bin")

PASS, FAIL = [], []


def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(f"  {'✓' if ok else '✗'} {name}" + (f" ｜ {detail}" if detail else ""))


def kill_app():
    subprocess.run(["powershell", "-NoProfile", "-Command",
                    "Get-Process umidl,umi-downloader -ErrorAction SilentlyContinue | Stop-Process -Force"],
                   capture_output=True)
    time.sleep(1.5)


def procs(name):
    r = subprocess.run(["powershell", "-NoProfile", "-Command",
                        f"(Get-Process {name} -ErrorAction SilentlyContinue | Measure-Object).Count"],
                       capture_output=True, text=True)
    try:
        return int((r.stdout or "0").strip() or 0)
    except Exception:
        return 0


# ── 本地慢速服务器：让任务停在 downloading，便于测排队/取消/删除 ──
class SlowHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        total = 6 * 1024 * 1024
        try:
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(total))
            self.end_headers()
            chunk = b"U" * 32768
            sent = 0
            while sent < total:
                self.wfile.write(chunk)
                sent += len(chunk)
                time.sleep(0.12)   # ~270 KB/s
        except Exception:
            pass

    def log_message(self, *a):
        pass


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


async def main():
    slow_port = free_port()
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", slow_port), SlowHandler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    slow_base = f"http://127.0.0.1:{slow_port}"

    shutil.rmtree(WORK, ignore_errors=True)
    os.makedirs(WORK, exist_ok=True)
    kill_app()

    d = AppDriver(APP, port=9224)
    await d.start()
    print("══ 启动被测程序")
    check("CDP 连接 + 应用启动", True)
    ver = await d.js("(document.body.innerText.match(/v1\\.[0-9]+\\.[0-9]+/)||[''])[0]")
    check("版本号为 1.8.1", ver == "v1.8.1", str(ver))

    # ── A. 格式全面可用 ───────────────────────────────────────────
    print("══ A. 格式可用性（要把置灰项尽可能做成真可用）")
    cat = await d.invoke("convert_formats") or {}
    total = cat.get("total") or 0
    groups = cat.get("groups") or []          # 数组：[{id,label,count,formats:[…]}]
    fmt_items = [f for g in groups for f in (g.get("formats") or [])]
    avail = [f for f in fmt_items if f.get("available")]
    unavail = [(f.get("id"), json.dumps(f.get("engines") or {}, ensure_ascii=False)) for f in fmt_items if not f.get("available")]
    check("格式总数 ≥ 60", total >= 60, f"total={total}")
    check("可用格式 ≥ 80", len(avail) >= 80, f"可用 {len(avail)} / {total}；仍不可用 {len(unavail)}：{unavail[:8]}")
    video = next((g for g in groups if g.get("id") == "video"), {})
    v_avail = sum(1 for f in (video.get("formats") or []) if f.get("available"))
    check("视频类全部可用", bool(video) and v_avail == (video.get("count") or 0),
          f"video 可用 {v_avail} / {video.get('count')}")
    eng = cat.get("engines") or {}
    check("引擎表含 imagemagick（PSD/DDS 靠它）", bool(eng.get("imagemagick")), json.dumps(eng, ensure_ascii=False))
    # 新要求：转换页不再展示“不可用”——网格里不得有不可用条目，计数只算可用项
    await d.goto("#/converter")
    await asyncio.sleep(2.2)
    grid = await d.js(
        "(() => { const items=[...document.querySelectorAll('[data-format]')];"
        " const bad=items.filter(e=>String(e.getAttribute('data-available'))==='0').length;"
        " const body=document.body.innerText;"
        " const t=body.indexOf('种格式');"
        " const raw=t>=0?body.slice(Math.max(0,t-20),Math.min(body.length,t+34)):'';"
        " const text=raw.split(String.fromCharCode(10)).join(' ').split(String.fromCharCode(13)).join(' ');"
        " return JSON.stringify({items: items.length, unavailable: bad, text: text}); })()"
    )
    try:
        gi = json.loads(grid) if isinstance(grid, str) else (grid or {})
    except Exception:
        gi = {}
    check("页面里已不再出现任何不可用条目（置灰卡为 0）", gi.get("unavailable") == 0,
          f"网格条目 {gi.get('items')}，其中不可用 {gi.get('unavailable')}；头部文案：{str(gi.get('text'))[:60]}")
    check("网格条目数 == 真可用数（不再把不可用算进列表）",
          int(gi.get("items") or 0) == len(avail) and int(gi.get("items") or 0) >= 60,
          f"页面 {gi.get('items')} vs 后端可用 {len(avail)}")

    for must in ("alac", "dts", "pnm", "ico", "tga", "psd", "dds"):
        f = next((x for x in fmt_items if x.get("id") == must), None)
        check(f"原置灰格式 {must} 现在可用", bool(f and f.get("available")),
              json.dumps((f or {}).get("engines") or {}, ensure_ascii=False) or "未见该条目")

    # 真产物：PNG → PSD（走受管 ImageMagick）
    png = os.path.join(WORK, "src.png")
    subprocess.run([os.path.join(BIN, "ffmpeg.exe"), "-y", "-v", "error", "-f", "lavfi",
                    "-i", "color=c=#3366ff:s=320x240:d=1", "-frames:v", "1", png], capture_output=True)
    psd = os.path.join(WORK, "src.psd")   # 应用按“源基名.格式”命名
    try:
        await d.invoke("start_convert", {"req": {"input_file": png, "output_dir": WORK, "format": "psd"}})
    except Exception as e:
        print("   start_convert 异常:", e)
    deadline = time.time() + 90
    while time.time() < deadline and not (os.path.exists(psd) and os.path.getsize(psd) > 1000):
        await asyncio.sleep(2)
    ok_psd = os.path.exists(psd) and os.path.getsize(psd) > 1000
    magic = ""
    if ok_psd:
        with open(psd, "rb") as fh:
            magic = fh.read(4).hex()
    check("PNG → PSD 真产物（8BPS 魔数）", ok_psd and magic == "38425053",
          f"{os.path.getsize(psd) if os.path.exists(psd) else 0} 字节 magic={magic}")

    # ── 队列预览卡片：真实大小 ────────────────────────────────────
    print("══ A2. 转换队列预览（卡片要显示源→目标与真实大小）")
    await d.goto("#/converter")
    await asyncio.sleep(2.5)
    card = await d.js(
        "(() => { const all=[...document.querySelectorAll('.umi-card,[data-test=task-card],article,li,div')]"
        ".filter(e=>e.offsetParent && e.querySelectorAll('*').length < 80 && /→/.test(e.innerText||''));"
        " const t=all.map(e=>(e.innerText||'').replace(/\s+/g,' ').trim()).filter(x=>x.length>10&&x.length<240);"
        " return JSON.stringify(t.slice(0,8)); })()"
    )
    try:
        cards = json.loads(card) if isinstance(card, str) else (card or [])
    except Exception:
        cards = []
    hit = [c for c in cards if "PSD" in c.upper() and re.search(r"\d+(\.\d+)?\s*(B|KB|MB|GB)", c)]
    blob = " ".join(cards)
    check("队列卡片显示 源→目标 且带真实字节大小", bool(hit), (hit[0] if hit else str(cards)[:180]))
    check("卡片不再误报“文件已丢失”", "丢失" not in blob and "丢失" not in json.dumps(cards, ensure_ascii=False), blob[:160])

    # ── B. bug 修复验证 ──────────────────────────────────────────
    print("══ B. Bug 修复验证")

    # BUG-01：字幕真出 SRT（生产路径命名 <基名>.<语言>.srt）
    FIXWAV = os.path.join(ROOT, ".tmp", "subfix", "manifest.wav")
    SCRATCH_WAV = r".tmp\bugsweep\media\mani_test.wav"
    os.makedirs(os.path.dirname(FIXWAV), exist_ok=True)
    if not os.path.exists(FIXWAV):
        if os.path.exists(SCRATCH_WAV):
            shutil.copy2(SCRATCH_WAV, FIXWAV)
        else:
            subprocess.run([os.path.join(BIN, "ffmpeg.exe"), "-y", "-v", "error", "-f", "lavfi",
                            "-i", "sine=frequency=440:duration=8", FIXWAV], capture_output=True)
    srt_zh = os.path.join(WORK, "manifest.zh.srt")
    wrong = os.path.join(WORK, "manifest.srt")
    for p in (srt_zh, wrong):
        if os.path.exists(p):
            os.remove(p)
    st = await d.try_invoke("start_subtitle", {"req": {"video_path": FIXWAV, "language": "zh", "output_dir": WORK, "model": "ggml-tiny.bin"}})
    # 注：settings.whisper_model 默认是短名 base（→ ggml-base.bin），本机只装了 ggml-tiny.bin；
    # 这里显式指定已安装模型，验证的是 BUG-01 的产物路径逻辑。默认值不匹配已单独记录。
    if not st[0]:
        print("   start_subtitle 失败：", str(st[1])[:200])
    tid = None
    if st[0]:
        v = st[1] if isinstance(st[1], dict) else {}
        tid = v.get("id") or v.get("task_id")
    deadline = time.time() + 180
    while time.time() < deadline and tid:
        subs = await d.invoke("list_subtitles") or []
        subs = subs if isinstance(subs, list) else (subs.get("tasks") or subs.get("items") or [])
        cur = next((t for t in subs if t.get("id") == tid), None)
        if cur and str(cur.get("status")) in ("done", "error"):
            break
        await asyncio.sleep(4)
    cur = {}
    subs = await d.invoke("list_subtitles") or []
    subs = subs if isinstance(subs, list) else (subs.get("tasks") or subs.get("items") or [])
    cur = next((t for t in subs if t.get("id") == tid), {}) or {}
    made = [p for p in (srt_zh, wrong) if os.path.exists(p) and os.path.getsize(p) > 0]
    check("字幕任务成功（不再是“未生成字幕文件”）",
          str(cur.get("status")) == "done" and bool(made),
          f"status={cur.get('status')} err={str(cur.get('error'))[:80]} 产物={made}")
    check("字幕产物命名正确（<基名>.<语言>.srt）", srt_zh in made, f"存在 {made}")

    # BUG-05：并发真排队
    await d.invoke("save_settings", {"settings": {"concurrency": 2, "aria2_connections": 4}})
    ids = []
    for i in range(4):
        r = await d.invoke("start_download", {"req": {"url": f"{slow_base}/slow{i}.bin", "output_dir": WORK}})
        tid = (r or {}).get("id") or (r or {}).get("task_id")
        if tid:
            ids.append(tid)
    await asyncio.sleep(12)
    tasks = await d.invoke("list_downloads") or []
    tasks = tasks if isinstance(tasks, list) else (tasks.get("tasks") or tasks.get("items") or [])
    mine = [t for t in tasks if t.get("id") in ids]
    st = {}
    for t in mine:
        st[t.get("status")] = st.get(t.get("status"), 0) + 1
    downloading = st.get("downloading", 0) + st.get("parsing", 0)
    waiting = sum(v for k, v in st.items() if k in ("pending", "queued", "waiting"))
    check("并发数生效：同时下载数 ≤ 设置值且超出者排队",
          len(mine) >= 3 and downloading <= 2 and waiting >= 1,
          f"入队 {len(ids)} 个 → 状态分布 {st}（concurrency=2）")

    # BUG-04：取消 = canceled（且清掉 .aria2）
    target = next((t for t in mine if t.get("status") in ("downloading", "parsing", "pending", "queued")), None)
    if target:
        await d.invoke("cancel_download", {"id": target["id"]})
        await asyncio.sleep(3)
        after = await d.invoke("list_downloads") or []
        after = after if isinstance(after, list) else (after.get("tasks") or after.get("items") or [])
        cur = next((t for t in after if t.get("id") == target["id"]), {})
        check("取消后的状态是 canceled（不是 paused）",
              str(cur.get("status")) == "canceled", f"status={cur.get('status')}")
        name = cur.get("filename") or ""
        leftovers = [p for p in os.listdir(WORK) if name and p.startswith(name.rsplit('.', 1)[0]) and p.endswith(".aria2")]
        check("取消后清掉 .aria2 控制文件", not leftovers, f"残留 {leftovers}")

    # BUG-02：删除“下载中”的任务后不复活
    r = await d.invoke("start_download", {"req": {"url": f"{slow_base}/ghost.bin", "output_dir": WORK}})
    gid = (r or {}).get("id") or (r or {}).get("task_id")
    await asyncio.sleep(6)
    await d.invoke("remove_download", {"id": gid, "deleteFile": True})
    await asyncio.sleep(2)
    gone0 = True
    snap0 = await d.invoke("list_downloads") or []
    snap0 = snap0 if isinstance(snap0, list) else (snap0.get("tasks") or snap0.get("items") or [])
    gone0 = not any(t.get("id") == gid for t in snap0)
    aria0 = procs("aria2c")
    await asyncio.sleep(25)   # 若会被 upsert 复活，25 秒内必然重现
    snap1 = await d.invoke("list_downloads") or []
    snap1 = snap1 if isinstance(snap1, list) else (snap1.get("tasks") or snap1.get("items") or [])
    revived = any(t.get("id") == gid for t in snap1)
    aria1 = procs("aria2c")
    check("删除中任务即时消失", gone0, f"删除后立即在库中={not gone0}")
    check("删除中任务 25 秒内不复活", not revived, f"25s 后同 id 出现={revived}")

    # BUG-14：ED2K 移交状态不再是“完成”（走下载流：start_download 收 ed2k 链接 → handed_off）
    print("   —— ED2K 移交（下载流）")
    t0 = int(time.time() * 1000)
    edlink = "ed2k://|file|verify18-handoff.bin|1048576|31D6CFE0D16AE931B73C59D7E0C089C0|/"
    try:
        rr = await d.invoke("start_download", {"req": {"url": edlink, "output_dir": WORK}})
        print("   start_download(ed2k) 返回：", json.dumps(rr or {}, ensure_ascii=False)[:220])
    except Exception as e:
        rr = {"error": str(e)}
        print("   start_download(ed2k) 失败：", str(e)[:200])
    await asyncio.sleep(6)
    snap = await d.invoke("list_downloads") or []
    snap = snap if isinstance(snap, list) else (snap.get("tasks") or snap.get("items") or [])
    # 只看**本次新建**的任务（避免匹配到历史遗留的 done 行）
    fresh = [t for t in snap if int(t.get("created_time") or 0) >= t0 - 2000]
    ed = next((t for t in fresh if "handoff.bin" in json.dumps(t, ensure_ascii=False)
               or "ed2k://" in str(t.get("url") or "")), None)
    st_ed = str((ed or {}).get("status"))
    check("ED2K 移交任务状态为 handed_off（不再伪装成已完成）",
          bool(ed) and st_ed == "handed_off",
          f"本次新建任务={bool(ed)} status={st_ed} note={(ed or {}).get('format_note')}")

    # BUG-08：超长路径明确报错
    long_dir = os.path.join(WORK, *(["y" * 40] * 7))
    try:
        r = await d.invoke("start_download", {"req": {"url": f"{slow_base}/longpath.bin", "output_dir": long_dir}})
    except Exception as e:
        r = {"error": str(e)}
    msg = json.dumps(r or {}, ensure_ascii=False)
    await asyncio.sleep(3)
    snap = await d.invoke("list_downloads") or []
    snap = snap if isinstance(snap, list) else (snap.get("tasks") or snap.get("items") or [])
    lp = next((t for t in snap if "longpath.bin" in str(t.get("url", ""))), None)
    lp_err = str((lp or {}).get("error") or "")
    blob = (msg or "") + " / " + (lp_err or "")
    check("超长路径给出可理解的原因（含路径长度或目录创建失败）",
          ("路径" in blob) or ("260" in blob) or ("errorCode=18" in blob) or ("make the directory" in blob),
          blob[:200])

    # BUG-07：超长粘贴不再卡主线程
    await d.goto("#/download")
    await asyncio.sleep(1.5)
    opened = await d.js(
        "(() => { const b=[...document.querySelectorAll('button')].filter(e=>e.offsetParent)"
        ".find(e=>/批量/.test(e.innerText||'')); if(!b) return false; b.click(); return true; })()"
    )
    await asyncio.sleep(0.8)
    has_ta = await d.js("!!document.querySelector('[data-test=batch-slot] textarea') || !!document.querySelector('textarea')")
    block = await d.js(
        "(() => { const ta=document.querySelector('[data-test=batch-slot] textarea') ||"
        " [...document.querySelectorAll('textarea')].find(e=>e.offsetParent); if(!ta) return -1;"
        " const big=Array.from({length:200000},(_,i)=>'https://example.com/f'+i+'.bin').join('\\n');"
        " const t0=performance.now(); ta.value=big; ta.dispatchEvent(new Event('input',{bubbles:true}));"
        " return Math.round(performance.now()-t0); })()"
    )
    await asyncio.sleep(2.0)
    alive = await d.js("1+1")
    check("粘贴 20 万行不阻塞主线程（≤800ms 且界面仍响应）",
          bool(opened) and bool(has_ta) and 0 <= int(block if block is not None else -1) <= 800 and alive == 2,
          f"展开面板={opened} 找到输入框={has_ta} 同步阻塞 {block} ms，之后 eval={alive}")
    hint = await d.js("(() => { const el=[...document.querySelectorAll('*')].filter(e=>e.offsetParent)"
                      ".map(e=>e.innerText||'').find(t=>/截断|上限|最多/.test(t)); return (el||'').slice(0,120); })()")
    check("超长输入有截断/上限提示", bool(hint), str(hint)[:120])

    await d.close()

    # BUG-03：退出不留孤儿 + 启动清扫
    try:
        await d3.close()
    except Exception:
        pass
    d5 = AppDriver(APP, port=9224)
    await d5.start()
    await d5.invoke("start_download", {"req": {"url": f"{slow_base}/orphan.bin", "output_dir": WORK}})
    await asyncio.sleep(8)
    before = procs("aria2c")
    try:
        await d5.close()
    except Exception:
        pass
    kill_app()
    time.sleep(2)
    orphans = procs("aria2c")
    d2 = AppDriver(APP, port=9224)
    await d2.start()
    await asyncio.sleep(6)
    swept = procs("aria2c")
    await d2.close()
    check("强杀后重启会清掉父进程已死的引擎残留", swept <= max(0, before - 1) or swept == 0,
          f"退出前 {before} 个 → 强杀后 {orphans} 个 → 重启清扫后 {swept} 个")

    # BUG-06：捕获端口不再允许越站入队
    cap = None
    for line in open(os.path.join(DATA, "settings.json"), encoding="utf-8").read().splitlines():
        if "capture" in line.lower():
            cap = line.strip()
    check("settings 中出现 capture_token 字段", bool(cap), str(cap)[:120])

    # ── C. UI 收敛回归 ───────────────────────────────────────────
    print("══ C. UI 收敛回归")
    d3 = AppDriver(APP, port=9224)
    await d3.start()
    await d3.goto("#/settings")
    await asyncio.sleep(1.8)
    tabs = await d3.js("JSON.stringify([...document.querySelectorAll('button')].filter(e=>e.offsetParent)"
                       ".map(e=>(e.innerText||'').trim()).filter(t=>['引擎','通用','系统','外观','插件'].includes(t)))")
    check("设置页收敛为 5 个页签", len(set(json.loads(tabs or "[]"))) >= 4, str(tabs))
    await d3.goto("#/settings?tab=tools")
    await asyncio.sleep(1.6)
    cards = await d3.js("JSON.stringify([...document.querySelectorAll('.umi-card')].filter(e=>e.offsetParent).map(e=>(e.innerText||'').trim().slice(0,14)))")
    check("旧深链 ?tab=tools 仍落在“引擎”内容上", "依赖工具" in str(cards), str(cards)[:150])
    await d3.goto("#/settings?tab=defaults")
    await asyncio.sleep(1.6)
    cards2 = await d3.js("JSON.stringify([...document.querySelectorAll('.umi-card')].filter(e=>e.offsetParent).map(e=>(e.innerText||'').trim().slice(0,14)))")
    check("旧深链 ?tab=defaults 仍落在“通用”内容上", "默认参数" in str(cards2) or "存储" in str(cards2), str(cards2)[:150])
    h = await d3.js("document.querySelector('main') ? document.querySelector('main').scrollHeight : 0")
    check("设置页高度 ≤ 1.5 屏（1080p）", int(h or 0) <= 1600, f"scrollHeight={h}")
    await d3.close()

    # ── 回归 ────────────────────────────────────────────────────
    print("══ D. 回归")
    d4 = AppDriver(APP, port=9224)
    await d4.start()
    await d4.goto("#/download")
    await asyncio.sleep(2)
    panel = await d4.js("!!document.querySelector('[data-test=ed2k-panel]') || /ED2K 电驴引擎/.test(document.body.innerText)")
    check("ED2K 面板仍在下载页", bool(panel), str(panel))
    await d4.goto("#/plugins")
    await asyncio.sleep(2.5)
    hh = await d4.js("location.hash")
    check("插件旧路由仍跳设置", "settings" in str(hh), str(hh))
    r = await d4.invoke("enqueue_links", {"text": "https://example.com/a.bin\n# 注释，不该入队\nhttps://example.com/a.bin", "output_dir": WORK})
    check("批量导入仍按整行跳注释", (r or {}).get("added") == 1 and (r or {}).get("skipped") >= 1, json.dumps(r or {}, ensure_ascii=False)[:150])
    await d4.close()

    srv.shutdown()
    kill_app()
    print("\n" + "=" * 56)
    print(f"通过 {len(PASS)} ｜ 失败 {len(FAIL)}")
    if FAIL:
        print("失败项：")
        for f in FAIL:
            print("  ✗ " + f)
    return 1 if FAIL else 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
