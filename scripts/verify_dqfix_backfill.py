"""补封面事件（backfill_covers → download://update）是否带正确 file_exists 的实机验证。

背景：应用启动后会给历史完成任务补封面，补成功时发一条 download://update。
修复前这条事件直接发任务对象（file_exists 恒为 false），历史任务卡片会被打成
「文件已丢失」且「打开文件 / 在文件夹中显示」按钮消失。

用法：python scripts/verify_dqfix_backfill.py
"""
import asyncio
import base64
import json
import os
import subprocess
import sys
import time

import requests
import websockets

PORT = 9222
APP = os.path.expandvars(r"%LOCALAPPDATA%\Umidl\umidl.exe")
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, ".tmp", "dqfix")
MARK = "dqfix19-probe"


def kill_app():
    subprocess.run(
        ["powershell", "-NoProfile", "-Command",
         "Get-Process umidl -ErrorAction SilentlyContinue | Stop-Process -Force"],
        capture_output=True,
    )
    time.sleep(1.5)


async def main():
    kill_app()
    env = dict(os.environ)
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = f"--remote-debugging-port={PORT}"
    subprocess.Popen([APP], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    ws = None
    for _ in range(60):
        try:
            d = requests.get(f"http://127.0.0.1:{PORT}/json", timeout=3).json()
            pages = [t for t in d if t.get("type") == "page"]
            if pages:
                ws = await websockets.connect(pages[0]["webSocketDebuggerUrl"], max_size=64 * 1024 * 1024)
                break
        except Exception:
            pass
        await asyncio.sleep(0.5)
    assert ws, "CDP 未连接"

    n = 1

    async def cmd(method, **params):
        nonlocal n
        mid = n
        n += 1
        await ws.send(json.dumps({"id": mid, "method": method, "params": params}))
        while True:
            msg = json.loads(await ws.recv())
            if msg.get("id") == mid:
                return msg.get("result", {})

    async def js(expr):
        r = await cmd("Runtime.evaluate", expression=expr, returnByValue=True, awaitPromise=True)
        return r.get("result", {}).get("value")

    async def invoke(name, args=None, **kw):
        payload = json.dumps(kw or args or {})
        out = await js(
            "(async () => { try { const r = await window.__TAURI_INTERNALS__.invoke("
            f"{json.dumps(name)}, {payload}); return JSON.stringify({{ ok: true, v: r }}); }} "
            "catch (e) { return JSON.stringify({ ok: false, e: String(e) }); } })()"
        )
        data = json.loads(out)
        return data.get("v") if data.get("ok") else {"error": data.get("e")}

    await js("location.hash = '#/download'; 'ok'")
    # 等启动后台的补封面跑完（完成后通过事件刷新卡片）
    row = None
    for i in range(40):
        await asyncio.sleep(1.0)
        rows = await invoke("list_downloads")
        row = next((t for t in rows if MARK in (t.get("title") or "")), None)
        if row:
            thumb = row.get("thumbnail") or ""
            if not thumb.startswith("http"):
                break

    dom = await js(
        "(() => { const rows = Array.from(document.querySelectorAll('table tbody tr')); "
        f"const r = rows.find(x => (x.innerText||'').includes({json.dumps(MARK)})); "
        "if (!r) return null; return { text: (r.innerText||'').replace(/\\s+/g,' ').trim(), "
        "buttons: Array.from(r.querySelectorAll('button')).map(b => b.getAttribute('title') || (b.innerText||'').trim()) }; })()"
    )
    os.makedirs(OUT, exist_ok=True)
    shot = await cmd("Page.captureScreenshot", format="png")
    open(os.path.join(OUT, "after-backfill.png"), "wb").write(base64.b64decode(shot["data"]))

    print("DB 行：", json.dumps(row, ensure_ascii=False))
    print("界面行：", json.dumps(dom, ensure_ascii=False))
    ok_thumb_local = bool(row) and not str(row.get("thumbnail") or "").startswith("http")
    ok_no_lost = bool(dom) and "文件已丢失" not in dom["text"]
    ok_open = bool(dom) and "打开文件" in dom["buttons"]
    print(f"\n补封面事件后：缩略图已本地化={ok_thumb_local} / 不误报已丢失={ok_no_lost} / 有打开文件按钮={ok_open}")

    await js("window.close(); 'ok'")
    await asyncio.sleep(1.0)
    kill_app()
    try:
        requests.get(f"http://127.0.0.1:{PORT}/json/version", timeout=2)
        print("⚠ 9222 仍在监听")
    except Exception:
        print("9222 已释放")

    if not (ok_thumb_local and ok_no_lost and ok_open):
        sys.exit(1)


asyncio.run(main())
