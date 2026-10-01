"""通过 WebView2 远程调试端口驱动 umi Downloader 界面（走 DOM，不靠像素坐标）。

用法：
  python scripts/drive_app.py eval "<js>"          # 执行任意 JS 并打印结果
  python scripts/drive_app.py click "<selector>"   # 点击匹配元素
  python scripts/drive_app.py shot  out.png        # 截图（应用窗口内容）
  python scripts/drive_app.py text  "<selector>"   # 取元素文本
"""
import asyncio
import json
import sys

import requests
import websockets


def target_ws():
    d = requests.get("http://127.0.0.1:9222/json", timeout=8).json()
    pages = [t for t in d if t.get("type") == "page"]
    if not pages:
        raise SystemExit("未找到页面目标，请用 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 启动程序")
    return pages[0]["webSocketDebuggerUrl"]


async def cmd(ws, method, **params):
    mid = cmd.n
    cmd.n += 1
    await ws.send(json.dumps({"id": mid, "method": method, "params": params}))
    while True:
        msg = json.loads(await ws.recv())
        if msg.get("id") == mid:
            if "error" in msg:
                raise SystemExit(f"CDP 错误：{msg['error']}")
            return msg.get("result", {})


cmd.n = 1


async def run():
    mode = sys.argv[1] if len(sys.argv) > 1 else "eval"
    arg = sys.argv[2] if len(sys.argv) > 2 else ""
    ws_url = target_ws()
    async with websockets.connect(ws_url, max_size=80 * 1024 * 1024) as ws:
        if mode == "eval":
            r = await cmd(ws, "Runtime.evaluate", expression=arg, returnByValue=True, awaitPromise=True)
            print(json.dumps(r.get("result", {}).get("value"), ensure_ascii=False))
        elif mode == "click":
            js = (
                "(() => { const el = document.querySelector(%s); if (!el) return 'NOT_FOUND'; "
                "el.scrollIntoView({block:'center'}); el.click(); return 'OK:' + (el.innerText||'').slice(0,30); })()"
                % json.dumps(arg)
            )
            r = await cmd(ws, "Runtime.evaluate", expression=js, returnByValue=True)
            print(r.get("result", {}).get("value"))
        elif mode == "text":
            js = (
                "(() => { const el = document.querySelector(%s); return el ? el.innerText : 'NOT_FOUND'; })()"
                % json.dumps(arg)
            )
            r = await cmd(ws, "Runtime.evaluate", expression=js, returnByValue=True)
            print(r.get("result", {}).get("value"))
        elif mode == "shot":
            r = await cmd(ws, "Page.captureScreenshot", format="png")
            import base64

            open(arg, "wb").write(base64.b64decode(r["data"]))
            print(f"已保存 {arg}")


asyncio.run(run())
