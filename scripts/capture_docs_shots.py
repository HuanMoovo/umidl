"""抓取 Umidl 界面截图（用于 docs/index.html 展示；CDP 走 WebView2 调试端口）。

用法：
  # 先带调试端口启动应用：
  #   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 %LOCALAPPDATA%/Umidl/umidl.exe
  python scripts/capture_docs_shots.py [输出目录，默认 docs/img]

抓取：首页 / 下载 / 转换 / 字幕 / 设置-引擎 / 设置-外观 / 设置-插件
"""
import asyncio
import base64
import json
import os
import sys

import requests
import websockets

ROUTES = [
    ("#/", "home"),
    ("#/download", "download"),
    ("#/converter", "convert"),
    ("#/subtitle", "subtitle"),
    ("#/settings?tab=engine", "settings-engine"),
    ("#/settings?tab=appearance", "settings-appearance"),
    ("#/settings?tab=plugins", "settings-plugins"),
]


def target_ws():
    d = requests.get("http://127.0.0.1:9222/json", timeout=8).json()
    pages = [t for t in d if t.get("type") == "page"]
    if not pages:
        raise SystemExit("未找到页面目标：请带 --remote-debugging-port=9222 启动应用")
    return pages[0]["webSocketDebuggerUrl"]


async def cmd(ws, n, method, **params):
    await ws.send(json.dumps({"id": n, "method": method, "params": params}))
    while True:
        msg = json.loads(await ws.recv())
        if msg.get("id") == n:
            if "error" in msg:
                raise SystemExit(f"CDP 错误：{msg['error']}")
            return msg.get("result", {})


async def run(out_dir: str):
    os.makedirs(out_dir, exist_ok=True)
    ws_url = target_ws()
    n = 0
    async with websockets.connect(ws_url, max_size=80 * 1024 * 1024) as ws:
        # 固定视口，保证每张图尺寸一致、页面比例正常
        n += 1
        await cmd(ws, n, "Emulation.setDeviceMetricsOverride",
                  width=1280, height=860, deviceScaleFactor=2, mobile=False)
        for route, name in ROUTES:
            n += 1
            await cmd(ws, n, "Runtime.evaluate",
                      expression=f"location.hash = {json.dumps(route)}; 'ok'",
                      returnByValue=True)
            await asyncio.sleep(2.6)
            n += 1
            r = await cmd(ws, n, "Page.captureScreenshot", format="png")
            p = os.path.join(out_dir, f"{name}.png")
            with open(p, "wb") as fh:
                fh.write(base64.b64decode(r["data"]))
            print(f"{name:20s} -> {p} ({os.path.getsize(p)} B)")


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "docs/img"
    asyncio.run(run(out))
