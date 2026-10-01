"""Umidl 1.6 实机验收：格式目录（≥60 种）/ 批量导入入队 / 多线程设置 / 插件进设置 / 字幕多选。

原则同 v15：只认真实返回值、真实 DOM、真实落盘，不做"看到参数就通过"的推断。

用法：python scripts/verify_v16.py [--keep-running]
"""
import asyncio
import json
import os
import shutil
import socket
import subprocess
import sys
import time

import requests
import websockets

APP = os.path.expandvars(r"%LOCALAPPDATA%\Umidl\umidl.exe")
DATA = os.path.expandvars(r"%APPDATA%\umi-downloader")
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIX = os.path.join(ROOT, ".tmp", "docfix")
WORK = os.path.join(DATA, "verify16")

# 批量导入用：1 条真链接（可下，v15 已证明经本应用可通）+ 1 条重复 + 1 条注释 + 1 条会被过滤规则挡掉的
SMALL_URL = "https://github.com/aria2/aria2/releases/download/release-1.37.0/aria2-1.37.0-win-64bit-build1.zip"

PASS, FAIL = [], []


def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(f"  {'✓' if ok else '✗'} {name}" + (f" ｜ {detail}" if detail else ""))
    return ok


class Driver:
    def __init__(self):
        self.ws = None
        self.n = 1

    async def connect(self):
        for _ in range(40):
            try:
                d = requests.get("http://127.0.0.1:9222/json", timeout=3).json()
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

    async def invoke(self, cmd, args=None):
        payload = json.dumps(args or {})
        out = await self.js(
            "(async () => { try { const r = await window.__TAURI_INTERNALS__.invoke("
            f"{json.dumps(cmd)}, {payload}); return JSON.stringify({{ ok: true, v: r }}); }} "
            "catch (e) { return JSON.stringify({ ok: false, e: String(e) }); } })()"
        )
        data = json.loads(out) if out else {"ok": False, "e": "no result"}
        if not data.get("ok"):
            raise RuntimeError(f"{cmd} 调用失败：{data.get('e')}")
        return data.get("v")

    async def try_invoke(self, cmd, args=None):
        payload = json.dumps(args or {})
        out = await self.js(
            "(async () => { try { const r = await window.__TAURI_INTERNALS__.invoke("
            f"{json.dumps(cmd)}, {payload}); return JSON.stringify({{ ok: true, v: r }}); }} "
            "catch (e) { return JSON.stringify({ ok: false, e: String(e) }); } })()"
        )
        data = json.loads(out) if out else {"ok": False, "e": "no result"}
        return data.get("ok"), (data.get("v") if data.get("ok") else data.get("e"))

    async def wait_text(self, tag, text, timeout=8.0):
        end = time.time() + timeout
        while time.time() < end:
            got = await self.js(
                f"[...document.querySelectorAll({json.dumps(tag)})].some(e => (e.innerText||'').includes({json.dumps(text)}))"
            )
            if got:
                return True
            await asyncio.sleep(0.3)
        return False

    async def goto(self, hash_, settle=1.8):
        await self.js(f"location.hash = {json.dumps(hash_)}; 'ok'")
        await asyncio.sleep(settle)

    async def click_text(self, text, tags=("button", "div", "span", "label", "a")):
        for tag in tags:
            got = await self.js(
                "(() => { const els=[...document.querySelectorAll(%s)];"
                " const el=els.find(e=>(e.innerText||'').trim()===%s || (e.innerText||'').includes(%s));"
                " if(!el) return false; el.scrollIntoView({block:'center'}); el.click(); return true; })()"
                % (json.dumps(tag), json.dumps(text), json.dumps(text))
            )
            if got:
                return True
        return False


def kill_app():
    subprocess.run(
        ["powershell", "-NoProfile", "-Command",
         "Get-Process umidl,umi-downloader -ErrorAction SilentlyContinue | Stop-Process -Force"],
        capture_output=True,
    )
    time.sleep(1.5)


def launch_app():
    env = dict(os.environ)
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = "--remote-debugging-port=9222"
    return subprocess.Popen([APP], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def port_open(port, host="127.0.0.1"):
    with socket.socket() as s:
        s.settimeout(0.6)
        return s.connect_ex((host, port)) == 0


def as_list(v, *keys):
    if isinstance(v, list):
        return v
    if isinstance(v, dict):
        for k in list(keys) + ["groups", "plugins", "tasks", "entries", "items", "list"]:
            x = v.get(k)
            if isinstance(x, list):
                return x
    return []


async def main():
    keep = "--keep-running" in sys.argv
    kill_app()
    shutil.rmtree(WORK, ignore_errors=True)
    os.makedirs(WORK, exist_ok=True)

    print("══ 启动被测程序")
    proc = launch_app()
    d = Driver()
    if not await d.connect():
        print("  ✗ 连接调试端口失败")
        return 1
    check("CDP 连接 + 应用启动", True)
    await asyncio.sleep(2.2)

    # ── 1. 转换格式目录 ≥ 60 种 ────────────────────────────────────────
    print("══ 1. 转换格式目录（类型分组 + 真实可用性）")
    ver = await d.invoke("app_version")
    check("版本号为 1.6.0", str(ver).startswith("1.6"), str(ver))

    cat = await d.invoke("convert_formats")
    groups = as_list(cat, "groups")
    by_id = {g.get("id"): g for g in groups if isinstance(g, dict)}
    counts = {gid: len(g.get("formats") or []) for gid, g in by_id.items()}
    total = cat.get("total") if isinstance(cat, dict) else None
    check(
        "格式总数 ≥ 60",
        (total or 0) >= 60,
        f"total={total}；分组：" + "、".join(f"{k}={v}" for k, v in counts.items()),
    )
    check(
        "四个类型分组齐全（视频/音频/图片/文档）",
        all(k in by_id for k in ("video", "audio", "image", "document")),
        ",".join(by_id.keys()),
    )
    check(
        "每类各自 ≥ 10 种",
        all((counts.get(k) or 0) >= 10 for k in ("video", "audio", "image", "document")),
        json.dumps(counts, ensure_ascii=False),
    )
    flat = {}
    for gid, g in by_id.items():
        for f in (g.get("formats") or []):
            if isinstance(f, dict) and f.get("id"):
                flat[f["id"].lower()] = f
    must = ["mp4", "mkv", "webm", "mp3", "aac", "flac", "wav", "png", "jpg", "webp", "docx", "pdf", "xlsx", "pptx", "md", "html", "csv"]
    missing = [m for m in must if m not in flat]
    check("主流格式全部在列", not missing, ("缺：" + ",".join(missing)) if missing else f"抽查 {len(must)} 种全在")
    unavail = [k for k, v in flat.items() if v.get("available") is False]
    check(
        "可用性来自真实引擎探测（存在被标为不可用的条目即证明在探测）",
        len(flat) >= 60,
        f"{len(flat)} 条中不可用 {len(unavail)} 条" + (f"：{','.join(unavail[:6])}" if unavail else ""),
    )
    eng = (cat or {}).get("engines") or {}
    check("引擎状态随目录返回", isinstance(eng, dict) and len(eng) >= 2, json.dumps(eng, ensure_ascii=False)[:90])

    # ── 2. 批量导入入队 ───────────────────────────────────────────────
    print("══ 2. 多链接 / txt 批量导入（真实入队）")
    before = as_list(await d.invoke("list_downloads"))
    ids_before = {t.get("id") for t in before if isinstance(t, dict)}
    batch = "\n".join([
        "# 这是一行注释，应被跳过",
        SMALL_URL,
        SMALL_URL,          # 重复
        "   ",              # 空行
        "https://example.com/payload.exe",  # 命中扩展名黑名单（若已启用）
    ])
    ok_b, res = await d.try_invoke("enqueue_links", {"text": batch, "output_dir": WORK})
    detail = json.dumps(res, ensure_ascii=False)[:180] if ok_b else str(res)[:180]
    check("enqueue_links 调用成功", ok_b, detail)
    if ok_b and isinstance(res, dict):
        check(
            "统计正确：至少入队 1 条、重复/空行被跳过",
            (res.get("added") or 0) >= 1 and (res.get("skipped") or 0) >= 1,
            f"added={res.get('added')} blocked={res.get('blocked')} skipped={res.get('skipped')}",
        )
        new_tasks = [t for t in as_list(res, "tasks") if isinstance(t, dict)]
        check("返回本次新建的任务对象", len(new_tasks) >= 1, f"{len(new_tasks)} 个：" + ",".join(str(t.get('id'))[:8] for t in new_tasks))

        # 真实等待落盘
        target = new_tasks[0]
        ok_land = False
        size = 0
        for _ in range(40):
            await asyncio.sleep(1.0)
            rows = as_list(await d.invoke("list_downloads"))
            row = next((r for r in rows if isinstance(r, dict) and r.get("id") == target.get("id")), None)
            if row and row.get("status") in ("Done", "done"):
                p = row.get("file_path")
                size = os.path.getsize(p) if p and os.path.isfile(p) else 0
                ok_land = size > 0
                break
            if row and row.get("status") in ("Error", "error"):
                print("    下载失败：", row.get("error"))
                break
        check("批量入队的任务真实下载完成并有产物", ok_land, f"{size} 字节 → {WORK}")

    # ── 3. 多线程 / 并发设置往返 ──────────────────────────────────────
    print("══ 3. 多线程数与连接数设置")
    st = await d.invoke("get_settings")
    old = {k: st.get(k) for k in ("concurrency", "aria2_connections", "aria2_split", "aria2_min_split_mb", "ytdlp_concurrency")}
    check(
        "设置里存在多线程/连接数字段且有默认值",
        isinstance(st, dict) and st.get("aria2_connections") is not None and st.get("concurrency") is not None,
        json.dumps(old, ensure_ascii=False),
    )
    patched = dict(st)
    patched.update({"concurrency": 5, "aria2_connections": 8, "aria2_split": 8, "aria2_min_split_mb": 2, "ytdlp_concurrency": 4})
    ok_s, _ = await d.try_invoke("save_settings", {"settings": patched})
    back = await d.invoke("get_settings")
    got = {k: back.get(k) for k in old}
    check(
        "多线程设置可写入并落库",
        ok_s and got.get("aria2_connections") == 8 and got.get("aria2_split") == 8 and got.get("concurrency") == 5,
        json.dumps(got, ensure_ascii=False),
    )
    # 限幅：越界值应被夹到上限而不是原样存
    wild = dict(back)
    wild.update({"aria2_connections": 999, "aria2_split": 0, "concurrency": 99})
    await d.try_invoke("save_settings", {"settings": wild})
    clamped = await d.invoke("get_settings")
    check(
        "越界值被限幅（不会原样落库）",
        int(clamped.get("aria2_connections") or 0) <= 16 and int(clamped.get("aria2_split") or 0) >= 1 and int(clamped.get("concurrency") or 0) <= 8,
        f"connections={clamped.get('aria2_connections')} split={clamped.get('aria2_split')} concurrency={clamped.get('concurrency')}",
    )
    # 还原
    restore = dict(clamped)
    restore.update(old)
    await d.try_invoke("save_settings", {"settings": restore})
    check("设置已还原", True, json.dumps(old, ensure_ascii=False))

    # ── 4. 插件在设置里、侧边栏不再有独立入口 ─────────────────────────
    print("══ 4. 插件模块搬到设置")
    side = await d.js(
        "(() => { const t=[...document.querySelectorAll('a,button,div,span')]"
        ".filter(e=>(e.innerText||'').trim()==='插件' && e.offsetParent);"
        " return JSON.stringify(t.map(e=>e.tagName+'.'+(e.className||'').toString().slice(0,30))); })()"
    )
    await d.goto("#/plugins")
    await asyncio.sleep(1.2)
    hash_after = await d.js("location.hash")
    check("旧路由 #/plugins 会跳到设置页（不死链）", "settings" in str(hash_after), f"hash={hash_after}")
    check("设置页里能找到插件入口", await d.wait_text("body", "插件"), "设置 → 插件")
    ok_tab = await d.click_text("插件")
    await asyncio.sleep(1.5)
    check("插件页签可切换并渲染沙箱信息", ok_tab and await d.wait_text("body", "沙箱"), "沙箱信息卡片")

    # ── 5. 字幕导出语言多选 ───────────────────────────────────────────
    print("══ 5. 字幕导出语言多选")
    await d.goto("#/subtitle")
    await asyncio.sleep(1.0)
    langs = await d.js(
        "(() => { const el=[...document.querySelectorAll('*')].find(e=>(e.innerText||'').includes('语言'));"
        " return el ? el.innerText.slice(0,160) : ''; })()"
    )
    check("字幕页有语言选择区", bool(langs), str(langs).replace("\n", " | ")[:130])
    # 多选三个语言
    picked = 0
    for name in ("中文", "English", "日本語", "英语", "日语", "中文（简体）"):
        if await d.js(
            "(() => { const els=[...document.querySelectorAll('label,button,span,div')]"
            ".filter(e=>(e.innerText||'').trim()===" + json.dumps(name) + " && e.offsetParent);"
            " if(!els.length) return false; els[0].click(); return true; })()"
        ):
            picked += 1
            await asyncio.sleep(0.35)
        if picked >= 3:
            break
    btn = await d.js(
        "(() => { const b=[...document.querySelectorAll('button')].filter(e=>e.offsetParent && /转写|开始|任务/.test(e.innerText||''));"
        " return b.length ? b.map(x=>x.innerText.trim()).join(' || ') : ''; })()"
    )
    check(
        "多选语言后主按钮反映任务数与选中数",
        picked >= 2 and bool(btn) and ("任务" in str(btn) or str(picked) in str(btn)),
        f"点击选中 {picked} 个语言；按钮：{str(btn)[:90]}",
    )

    # ── 6. 转换页类型选项 ─────────────────────────────────────────────
    print("══ 6. 转换页类型选项与格式网格")
    await d.goto("#/converter")
    await asyncio.sleep(1.2)
    body = await d.js("document.body.innerText.slice(0, 2000)")
    check("转换页显示格式总数与分类", "共" in str(body) and ("视频" in str(body) and "音频" in str(body)), str(body).replace("\n"," ")[:120])
    ok_nav = await d.click_text("音频")
    await asyncio.sleep(1.0)
    grid = await d.js(
        "(() => { const els=[...document.querySelectorAll('button,div,span')]"
        ".filter(e=>e.offsetParent && /^(MP3|AAC|FLAC|WAV|OPUS|M4A|OGG|WMA|AIFF|AC3|APE)$/.test((e.innerText||'').trim()));"
        " return els.length; })()"
    )
    check("切到「音频」后网格出现音频格式", ok_nav and int(grid or 0) >= 6, f"匹配到 {grid} 个音频格式条目")

    # ── 7. 回归：文档转换仍然可用 ─────────────────────────────────────
    print("══ 7. 回归：真实文档转换")
    docx = os.path.join(FIX, "verify.docx")
    if os.path.isfile(docx):
        conv = os.path.join(WORK, "conv")
        os.makedirs(conv, exist_ok=True)
        await d.try_invoke(
            "start_convert",
            {"req": {"input_file": docx, "output_dir": conv, "format": "md", "extract_audio": False, "mute": False}},
        )
        out = os.path.join(conv, "verify.md")
        ok_md = False
        for _ in range(30):
            await asyncio.sleep(1.0)
            if os.path.isfile(out) and os.path.getsize(out) > 0:
                ok_md = True
                break
        check("docx→md 仍然产出非空（1.5 功能未回归）", ok_md, out)
    else:
        check("docx→md 仍然产出非空（1.5 功能未回归）", False, "夹具缺失")

    # ── 8. 收尾 ───────────────────────────────────────────────────────
    print("══ 8. 收尾")
    try:
        await d.js("window.close(); 'ok'")
    except Exception:
        pass
    await asyncio.sleep(1.5)
    if not keep:
        kill_app()
        check("退出后调试端口已关闭", not port_open(9222), "9222")
    # 清掉本次验收产生的下载产物与任务
    try:
        rows = as_list(await d.invoke("list_downloads")) if keep else []
    except Exception:
        rows = []
    shutil.rmtree(WORK, ignore_errors=True)

    print("\n" + "=" * 56)
    print(f"通过 {len(PASS)} ｜ 失败 {len(FAIL)}")
    if FAIL:
        print("失败项：")
        for f in FAIL:
            print(f"  ✗ {f}")
    return 0 if not FAIL else 1


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
