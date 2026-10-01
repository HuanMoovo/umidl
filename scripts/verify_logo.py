"""自定义 LOGO 实机验收（WebView2 CDP）

覆盖：
  1. 侧栏显示内置矢量 LOGO 标记；首页英雄区已按要求移除大 LOGO
  2. 设置 → 外观 存在「自定义 LOGO」卡片
  3. 通过 Tauri 命令设置一张测试图 → settings.json / branding 目录 / 界面三处一致
  4. 点「恢复默认」→ 设置与目录清空、界面回到内置 LOGO

用法：python scripts/verify_logo.py
"""
import asyncio
import base64
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

CDP = "http://127.0.0.1:9222"
APP_DATA = os.path.join(os.environ["APPDATA"], "umi-downloader")
SETTINGS = os.path.join(APP_DATA, "settings.json")
BRANDING = os.path.join(APP_DATA, "branding")
SHOT = ".tmp"

results = []


def rec(name: str, ok_: bool, detail: str = "") -> None:
    results.append((ok_, name, detail))
    print(("  ✅ " if ok_ else "  ❌ ") + name + (f"  |  {detail}" if detail else ""))


class App:
    def __init__(self, ws):
        self.ws = ws
        self.id = 0

    async def send(self, method, params=None, timeout=30):
        self.id += 1
        payload = {"id": self.id, "method": method, "params": params or {}}
        await self.ws.send(json.dumps(payload))
        while True:
            msg = json.loads(await asyncio.wait_for(self.ws.recv(), timeout=timeout))
            if msg.get("id") == self.id:
                if "error" in msg:
                    raise RuntimeError(msg["error"])
                return msg.get("result", {})

    async def js(self, expr, timeout=30):
        r = await self.send(
            "Runtime.evaluate",
            {"expression": expr, "returnByValue": True, "awaitPromise": True, "userGesture": True},
            timeout,
        )
        if r.get("exceptionDetails"):
            raise RuntimeError(json.dumps(r["exceptionDetails"])[:400])
        return r.get("result", {}).get("value")

    async def shot(self, path):
        r = await self.send("Page.captureScreenshot", {"format": "png"})
        with open(path, "wb") as f:
            f.write(base64.b64decode(r["data"]))
        return path


def read_settings():
    try:
        with open(SETTINGS, encoding="utf-8") as f:
            return json.load(f)
    except Exception:
        return {}


def branding_files():
    try:
        return sorted(os.listdir(BRANDING))
    except Exception:
        return []


async def main() -> int:
    import websockets

    tabs = json.loads(await asyncio.get_event_loop().run_in_executor(None, lambda: __import__("urllib.request", fromlist=["x"]).urlopen(CDP + "/json/list").read()))
    page = next(t for t in tabs if t.get("type") == "page")
    os.makedirs(SHOT, exist_ok=True)

    async with websockets.connect(page["webSocketDebuggerUrl"], max_size=32 * 1024 * 1024) as ws:
        app = App(ws)
        await app.send("Runtime.enable")
        await app.send("Page.enable")

        # ── 1) 内置 LOGO
        await app.js("(() => { location.hash = '#/'; return 1 })()")
        await asyncio.sleep(1.5)
        info = await app.js("""(() => {
            const imgs = [...document.querySelectorAll('img')]
              .filter(i => (i.getAttribute('src') || '').includes('logo'))
              .map(i => ({ src: i.getAttribute('src'), w: i.clientWidth, h: i.clientHeight, ok: i.complete && i.naturalWidth > 0 }))
            const aside = document.querySelector('aside img')
            return JSON.stringify({ imgs, sidebar: aside ? {src: aside.getAttribute('src'), ok: aside.complete && aside.naturalWidth > 0} : null })
        })()""")
        data = json.loads(info)
        rec("首页已移除大 LOGO（回归：英雄区不再有 logo 图）",
            not any(i["w"] > 60 for i in data["imgs"]),
            f"英雄区 logo 图 {sum(1 for i in data['imgs'] if i['w'] > 60)} 张 · 全部图片 {len(data['imgs'])} 张")
        rec("侧栏显示 LOGO 标记", bool(data["sidebar"] and data["sidebar"]["ok"]),
            (data["sidebar"] or {}).get("src", "无")[:60])
        await app.shot(f"{SHOT}/logo_default.png")

        # ── 2) 设置页卡片
        await app.js("(() => { location.hash = '#/settings'; return 1 })()")
        await asyncio.sleep(1.2)
        await app.js("""(() => {
            const b = [...document.querySelectorAll('button')].find(x => /Appearance|外观/.test(x.innerText))
            if (b) b.click(); return 1
        })()""")
        await asyncio.sleep(1.0)
        card = await app.js("""(() => {
            const t = document.body.innerText
            const has = /自定义 LOGO|Custom logo|カスタムロゴ|Logo personnalisé/.test(t)
            const btns = [...document.querySelectorAll('button')].filter(x => /选择图片|Choose image|画像を選択|Choisir une image/.test(x.innerText))
            return JSON.stringify({ has, btn: btns.length, text: (t.match(/[^\\n]*(LOGO|logo)[^\\n]*/g) || []).slice(0, 4) })
        })()""")
        c = json.loads(card)
        rec("外观页存在「自定义 LOGO」卡片", c["has"] and c["btn"] >= 1, " · ".join(x[:40] for x in c["text"]))

        # ── 3) 通过命令设置自定义 LOGO（先用 Python 造一张醒目的测试图）
        test_png = os.path.abspath(f"{SHOT}/custom_logo_test.png")
        from PIL import Image, ImageDraw
        im = Image.new("RGBA", (256, 256), (26, 26, 46, 255))
        d = ImageDraw.Draw(im)
        d.ellipse((28, 28, 228, 228), fill=(255, 214, 102, 255))
        d.text((86, 118), "TEST", fill=(26, 26, 46, 255))
        im.save(test_png)
        await app.shot(f"{SHOT}/logo_before_custom.png")
        res = await app.js(
            "window.__TAURI_INTERNALS__.invoke('set_custom_logo', { path: %s })" % json.dumps(test_png)
        )
        saved = res if isinstance(res, str) else json.dumps(res)
        rec("命令 set_custom_logo 返回保存路径", "branding" in saved, saved[-48:])

        s = read_settings()
        rec("settings.json 记录 custom_logo", bool(s.get("custom_logo")) and "branding" in str(s.get("custom_logo")),
            str(s.get("custom_logo", ""))[-48:])
        rec("branding 目录已复制图片", any(f.startswith("logo.") for f in branding_files()), str(branding_files()))

        # 重载页面 → 界面应从设置读出自定义 LOGO
        await app.send("Page.reload", {"ignoreCache": True})
        await asyncio.sleep(4.0)
        applied = await app.js("""(() => {
            const imgs = [...document.querySelectorAll('img')]
              .filter(i => /branding|asset/.test(i.getAttribute('src') || ''))
              .map(i => ({ src: i.getAttribute('src'), w: i.clientWidth, h: i.clientHeight, ok: i.complete && i.naturalWidth > 0 }))
            const aside = document.querySelector('aside img')
            return JSON.stringify({ imgs, sidebar: aside ? aside.getAttribute('src') : null })
        })()""")
        a = json.loads(applied)
        rec("界面已应用自定义 LOGO（侧栏）", bool(a["sidebar"]) and "branding" in a["sidebar"], (a["sidebar"] or "无")[:70])
        rec("界面已应用自定义 LOGO（图片解码成功）", any(i["ok"] for i in a["imgs"]),
            " / ".join(f'{i["w"]}x{i["h"]} ok={i["ok"]}' for i in a["imgs"]) or "未找到")
        await app.shot(f"{SHOT}/logo_custom.png")

        # ── 4) 界面上点「恢复默认」
        await app.js("(() => { location.hash = '#/settings'; return 1 })()")
        await asyncio.sleep(1.2)
        await app.js("""(() => {
            const b = [...document.querySelectorAll('button')].find(x => /Appearance|外观/.test(x.innerText))
            if (b) b.click(); return 1
        })()""")
        await asyncio.sleep(1.0)
        clicked = await app.js("""(() => {
            const b = [...document.querySelectorAll('button')].find(x => /恢复默认|Restore default|既定に戻す|Rétablir par défaut/.test(x.innerText) && !x.disabled)
            if (!b) return 'NO_BTN'
            b.click(); return 'CLICKED'
        })()""")
        await asyncio.sleep(2.0)
        s2 = read_settings()
        rec("「恢复默认」按钮可用并已点击", clicked == "CLICKED", clicked)
        rec("设置中的 custom_logo 已清空", not s2.get("custom_logo"), repr(s2.get("custom_logo")))
        rec("branding 目录已清空", branding_files() == [], str(branding_files()))
        await app.js("(() => { location.hash = '#/'; return 1 })()")
        await asyncio.sleep(1.2)
        back = await app.js("""(() => {
            const aside = document.querySelector('aside img')
            return JSON.stringify({ sidebar: aside ? aside.getAttribute('src') : null })
        })()""")
        b2 = json.loads(back)
        rec("界面回到内置 LOGO", bool(b2["sidebar"]) and "branding" not in b2["sidebar"], (b2["sidebar"] or "无")[:70])
        await app.shot(f"{SHOT}/logo_restored.png")

    print()
    print("═══════ 结果 ═══════")
    passed = sum(1 for ok_, _, _ in results if ok_)
    for ok_, name, detail in results:
        print(("✅ " if ok_ else "❌ ") + name + (f"  |  {detail}" if detail else ""))
    print(f"\n通过 {passed}/{len(results)}")
    return 0 if passed == len(results) else 1


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
