"""文档转换诊断：把 start_convert 的原始返回、任务状态、输出目录实况全部打出来。

用法：python scripts/diag_docconvert.py
"""
import asyncio
import json
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

APP = os.path.expandvars(r"%LOCALAPPDATA%\Umidl\umidl.exe")
DATA = os.path.expandvars(r"%APPDATA%\umi-downloader")
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIX = os.path.join(ROOT, ".tmp", "docfix")
OUT = os.path.join(DATA, "diagconv")


def kill():
    subprocess.run(
        ["powershell", "-NoProfile", "-Command",
         "Get-Process umidl,umi-downloader -ErrorAction SilentlyContinue | Stop-Process -Force"],
        capture_output=True,
    )
    time.sleep(1.5)


async def main():
    import requests
    import websockets

    kill()
    if os.path.isdir(OUT):
        for n in os.listdir(OUT):
            os.remove(os.path.join(OUT, n))
    os.makedirs(OUT, exist_ok=True)

    env = dict(os.environ)
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = "--remote-debugging-port=9222"
    subprocess.Popen([APP], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    ws = None
    for _ in range(40):
        try:
            d = requests.get("http://127.0.0.1:9222/json", timeout=3).json()
            pages = [t for t in d if t.get("type") == "page"]
            if pages:
                ws = await websockets.connect(pages[0]["webSocketDebuggerUrl"], max_size=64 * 1024 * 1024)
                break
        except Exception:
            pass
        await asyncio.sleep(0.5)
    if not ws:
        print("连不上调试端口")
        return 1
    await asyncio.sleep(2.0)

    n = [1]

    async def js(expr):
        mid = n[0]
        n[0] += 1
        await ws.send(json.dumps({"id": mid, "method": "Runtime.evaluate",
                                  "params": {"expression": expr, "returnByValue": True, "awaitPromise": True}}))
        while True:
            m = json.loads(await ws.recv())
            if m.get("id") == mid:
                return (m.get("result") or {}).get("result", {}).get("value")

    async def call(cmd, args=None):
        out = await js(
            "(async () => { try { const r = await window.__TAURI_INTERNALS__.invoke("
            f"{json.dumps(cmd)}, {json.dumps(args or {})}); return JSON.stringify({{ ok: true, v: r }}); }} "
            "catch (e) { return JSON.stringify({ ok: false, e: String(e) }); } })()"
        )
        try:
            return json.loads(out)
        except Exception:
            return {"ok": False, "e": f"解析失败: {out!r}"}

    # 三个夹具各试一次
    cases = [
        ("docx", os.path.join(FIX, "verify.docx"), "md"),
        ("pdf", os.path.join(FIX, "verify.pdf"), "txt"),
        ("xlsx", os.path.join(FIX, "verify.xlsx"), "csv"),
    ]
    used = []
    for kind, path, fmt in cases:
        print(f"\n══════ {kind} → {fmt}")
        if not os.path.isfile(path):
            print(f"  夹具不存在：{path}")
            continue
        r = await call("start_convert", {
            "input_file": path, "output_dir": OUT, "format": fmt,
            "extract_audio": False, "mute": False,
        })
        print("  start_convert 返回:", json.dumps(r, ensure_ascii=False)[:400])
        if r.get("ok") and isinstance(r.get("v"), dict):
            used.append(r["v"].get("id"))

    await asyncio.sleep(6)
    convs = await call("list_converts")
    print("\n══════ list_converts")
    items = convs.get("v") if convs.get("ok") else None
    if isinstance(items, dict):
        items = items.get("converts") or items.get("items") or items.get("list")
    for it in (items or []):
        print("   原始:", json.dumps(it, ensure_ascii=False)[:420])
        print("   摘要:", json.dumps({
            "id": it.get("id"), "in": os.path.basename(str(it.get("input_file", ""))),
            "out": it.get("output_file") or it.get("output_path") or it.get("file_path"),
            "fmt": it.get("format"), "status": it.get("status"), "error": it.get("error"),
            "progress": it.get("progress"),
        }, ensure_ascii=False))

    print("\n══════ 输出目录实况", OUT)
    for n_ in os.listdir(OUT):
        p = os.path.join(OUT, n_)
        size = os.path.getsize(p)
        head = ""
        if size and os.path.isfile(p):
            with open(p, "r", encoding="utf-8", errors="replace") as f:
                head = f.read(80).replace("\n", " ")
        print(f"   {n_}  {size} 字节  {head[:70]}")

    print("\n══════ 日志尾部")
    logdir = os.path.join(DATA, "logs")
    if os.path.isdir(logdir):
        files = sorted((os.path.join(logdir, f) for f in os.listdir(logdir)), key=os.path.getmtime)
        if files:
            with open(files[-1], "r", encoding="utf-8", errors="replace") as f:
                lines = f.readlines()[-14:]
            print("  " + "  ".join(lines).replace("\n", "\n  ")[:1400])

    try:
        await js("window.close(); 'ok'")
    except Exception:
        pass
    await asyncio.sleep(1)
    kill()
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
