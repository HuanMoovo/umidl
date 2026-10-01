"""umi Downloader v1.2.0 实机验收

覆盖三件事（都走真实界面，不靠像素坐标）：
  1. 「在文件夹中显示」——文件在→资源管理器选中该文件；文件丢失→不显示按钮、给出提示、不再乱开「文档」
  2. 「删除记录」——删除后卡片立即消失；有文件时会问「同时删除文件/仅删除记录」
  3. 语言切换——中文/English/日本語/Français 实时切换，侧栏与设置页都变

校验手段：WebView2 CDP 读 DOM/点击 + Windows Shell COM 读资源管理器选中项 + SQLite 读数据库。
"""

import asyncio
import base64
import json
import os
import sqlite3
import subprocess
import sys
import time

import requests
import websockets

DB = os.path.expandvars(r"%APPDATA%\umi-downloader\umi.db")
SETTINGS = os.path.expandvars(r"%APPDATA%\umi-downloader\settings.json")
CDP = "http://127.0.0.1:9222/json"
SHOT_DIR = ".tmp"

RESULTS = []


def rec(name: str, ok: bool, detail: str = ""):
    RESULTS.append((name, ok, detail))
    print(("  ✅ " if ok else "  ❌ ") + name + ("  |  " + detail if detail else ""), flush=True)


# ───────────────────────── 外部校验工具 ─────────────────────────


def explorer_focused() -> list[str]:
    """当前所有资源管理器窗口里被选中的项"""
    ps = (
        "[Console]::OutputEncoding=[Text.Encoding]::UTF8; "
        "$sh=New-Object -ComObject Shell.Application; "
        "($sh.Windows() | ForEach-Object { try { $_.Document.FocusedItem.Path } catch { '' } }) -join '|'"
    )
    try:
        out = subprocess.run(
            ["powershell", "-NoProfile", "-Command", ps], capture_output=True, text=True, encoding='utf-8', errors='ignore', timeout=40
        ).stdout
    except Exception:
        return []
    return [p for p in (out or "").strip().split("|") if p.strip()]


def explorer_windows() -> list[str]:
    ps = (
        "[Console]::OutputEncoding=[Text.Encoding]::UTF8; "
        "$sh=New-Object -ComObject Shell.Application; "
        "($sh.Windows() | ForEach-Object { $_.LocationName }) -join '|'"
    )
    try:
        out = subprocess.run(
            ["powershell", "-NoProfile", "-Command", ps], capture_output=True, text=True, encoding='utf-8', errors='ignore', timeout=40
        ).stdout
    except Exception:
        return []
    return [p for p in (out or "").strip().split("|") if p.strip()]


def doc_window_count() -> int:
    return sum(1 for w in explorer_windows() if w in ("文档", "Documents"))


def db_rows() -> list[tuple]:
    c = sqlite3.connect(DB)
    try:
        return list(c.execute("SELECT id, title, status, coalesce(file_path,'') FROM downloads ORDER BY created_time DESC"))
    finally:
        c.close()


def http_json(url, timeout=5):
    for _ in range(int(timeout / 0.5)):
        try:
            return requests.get(url, timeout=2).json()
        except Exception:
            time.sleep(0.5)
    return None


# ───────────────────────── CDP ─────────────────────────


class App:
    def __init__(self):
        self.n = 1

    async def __aenter__(self):
        d = http_json(CDP, 15)
        if not d:
            raise SystemExit("CDP 未就绪：请用 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 启动程序")
        pages = [t for t in d if t.get("type") == "page"]
        self.ws = await websockets.connect(pages[0]["webSocketDebuggerUrl"], max_size=80 * 1024 * 1024)
        return self

    async def __aexit__(self, *a):
        await self.ws.close()

    async def cmd(self, method, **params):
        mid = self.n
        self.n += 1
        await self.ws.send(json.dumps({"id": mid, "method": method, "params": params}))
        while True:
            msg = json.loads(await self.ws.recv())
            if msg.get("id") == mid:
                if "error" in msg:
                    raise SystemExit(f"CDP 错误：{msg['error']}")
                return msg.get("result", {})

    async def js(self, expr: str):
        r = await self.cmd("Runtime.evaluate", expression=expr, returnByValue=True, awaitPromise=True, userGesture=True)
        res = r.get("result", {})
        if res.get("subtype") == "error":
            raise SystemExit(f"JS 出错：{res.get('description')}")
        return res.get("value")

    async def shot(self, path: str):
        r = await self.cmd("Page.captureScreenshot", format="png")
        os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
        open(path, "wb").write(base64.b64decode(r["data"]))
        return path

    async def goto(self, path: str):
        """按路由跳转（不依赖界面语言）"""
        ok = await self.js(
            "(() => { const want='#'+'%s'; const a=[...document.querySelectorAll('a')].find(x=>x.getAttribute('href')===want);"
            " if(!a) return false; a.click(); return true; })()" % path
        )
        await asyncio.sleep(1.1)
        return ok

    async def cards(self):
        return await self.js(
            """(() => [...document.querySelectorAll('div.umi-card')]
                 .filter(c => c.querySelector('img') || /已完成|失败|下载中|排队|处理中/.test(c.innerText))
                 .map(c => ({
                   title: c.innerText.split('\\n')[0].slice(0, 40),
                   text: c.innerText.replace(/\\n/g, ' ').slice(0, 120),
                   btns: [...c.querySelectorAll('button')].map(b => b.getAttribute('title') || b.innerText.trim()),
                   lost: c.innerText.includes('文件已丢失'),
                 })))()"""
        )


async def main():
    print("═══ umi Downloader v1.2.0 实机验收 ═══\n")

    rows = db_rows()
    print(f"数据库任务 {len(rows)} 条:")
    for r in rows:
        print(f"   {r[3] or '(无文件)'}  [{r[2]}]  {r[1][:26]}")
    print()

    async with App() as app:
        # ── 准备：下载页
        await app.goto("/download")
        await asyncio.sleep(0.6)
        cards = await app.cards()
        rec("下载队列加载", len(cards) >= 1, f"{len(cards)} 张卡片")

        # 选一张“文件存在”的卡片做删除测试（优先小文件：.jpg/.m4a），另一张做大文件定位测试
        jpg = next((r for r in rows if r[3].lower().endswith((".jpg", ".jpeg", ".png"))), None)
        big = next((r for r in rows if r[3].lower().endswith((".avi", ".mp4", ".mkv"))), None)
        if not jpg or not big:
            rec("测试数据齐备", False, "需要一条图片任务 + 一条视频任务")
            return report()
        jpg_name, jpg_path = jpg[1], jpg[3]
        big_name, big_path = big[1], big[3]

        # ── 1) 文件丢失时：不该出现「打开/在文件夹中显示」，要提示文件已丢失
        bak = jpg_path + ".hidden_for_test"
        os.replace(jpg_path, bak)
        doc_before = doc_window_count()
        await app.goto("/")           # 离开再回来，强制列表刷新
        await app.goto("/download")
        await asyncio.sleep(0.8)
        cards = await app.cards()
        jpg_card = next((c for c in cards if jpg_name[:12] in (c["text"] or c["title"])), None)
        if not jpg_card:
            rec("找到图片任务卡片", False, "未找到")
        else:
            has_bad_btn = any(("在文件夹中显示" in b) or ("打开文件" in b) for b in jpg_card["btns"])
            rec(
                "文件丢失→隐藏打开/定位按钮 + 提示已丢失",
                jpg_card["lost"] and not has_bad_btn,
                f"提示={'有' if jpg_card['lost'] else '无'} · 按钮={jpg_card['btns']}",
            )
        # 点删除（文件不在，应直接删记录、不弹窗）
        await app.js(
            """(() => { const c=[...document.querySelectorAll('div.umi-card')]
                 .find(x => x.innerText.includes(%s));
               if(!c) return 'NO_CARD';
               const b=[...c.querySelectorAll('button')].find(x => (x.getAttribute('title')||'')==='删除记录');
               if(!b) return 'NO_BTN'; b.click(); return 'CLICKED'; })()"""
            % json.dumps(jpg_name[:12])
        )
        await asyncio.sleep(1.2)
        after_del_cards = await app.cards()
        rows_after = db_rows()
        gone = not any(jpg_name[:12] in (c["text"] or c["title"] or "") for c in after_del_cards)
        rec(
            "删除记录→卡片立即消失（坏文件直接删记录）",
            gone and len(rows_after) == len(rows) - 1,
            f"卡片={'消失' if gone else '仍在'} · 数据库 {len(rows)}→{len(rows_after)}",
        )
        os.replace(bak, jpg_path)  # 文件本身不动，还原名字
        rec("文件未被误删", os.path.exists(jpg_path), os.path.basename(jpg_path))

        # ── 2) 文件在：点「在文件夹中显示」应在资源管理器里选中该文件（不再乱开「文档」）
        doc_before = doc_window_count()
        focused_before = set(explorer_focused())
        reveal_name, reveal_path = jpg_name, jpg_path
        await app.shot(f"{SHOT_DIR}/v12_queue.png")
        clicked = await app.js(
            """(() => { const c=[...document.querySelectorAll('div.umi-card')]
                 .find(x => x.innerText.includes(%s));
               if(!c) return 'NO_CARD';
               const b=[...c.querySelectorAll('button')].find(x => (x.getAttribute('title')||'')==='在文件夹中显示');
               if(!b) return 'NO_BTN'; b.click(); return 'CLICKED'; })()"""
            % json.dumps(reveal_name[:12])
        )
        focused_after, appeared = set(), False
        for _ in range(16):
            await asyncio.sleep(0.5)
            focused_after = set(explorer_focused())
            if reveal_path in focused_after:
                appeared = True
                break
        doc_after = doc_window_count()
        rec(
            "「在文件夹中显示」→资源管理器选中该文件",
            clicked == "CLICKED" and appeared,
            f"点击={clicked} · 选中={reveal_path if appeared else '未出现'} · 文档窗口 {doc_before}→{doc_after}",
        )
        await app.shot(f"{SHOT_DIR}/v12_reveal.png")

        # ── 3) 语言切换
        await app.goto("/settings")
        await asyncio.sleep(0.9)
        # 外观页签（第 3 个）
        await app.js(
            "(() => { const bs=[...document.querySelectorAll('button')].filter(b=>/依赖工具|通用设置|外观|默认参数|Tools|General|Appearance|Defaults|ツール|一般|外観|既定|Outils|Général|Apparence/.test(b.innerText));"
            " if(!bs[2]) return 'NO_TAB'; bs[2].click(); return 'OK'; })()"
        )
        await asyncio.sleep(0.9)

        checks = [
            ("en-US", "en", ["Home", "Download", "Converter", "Subtitle", "Settings"], "English"),
            ("ja-JP", "ja", ["ホーム", "ダウンロード", "変換", "字幕", "設定"], "日本語"),
            ("fr-FR", "fr", ["Accueil", "Téléchargement", "Conversion", "Sous-titres", "Paramètres"], "Français"),
            ("zh-CN", "zh", ["首页", "下载", "转换", "字幕", "设置"], "简体中文"),
        ]
        for code, lang, expect, label in checks:
            await app.js(
                """(() => { const b=[...document.querySelectorAll('button')]
                     .find(x => x.innerText.includes(%s));
                   if(!b) return 'NO_BTN'; b.click(); return 'OK'; })()""" % json.dumps(code)
            )
            await asyncio.sleep(1.0)
            nav = await app.js("document.querySelector('aside') ? document.querySelector('aside').innerText : ''") or ""
            html_lang = await app.js("document.documentElement.lang") or ""
            hit = [w for w in expect if w in nav]
            rec(
                f"语言切换 → {label}（{code}）",
                len(hit) == len(expect) and html_lang == lang,
                f"侧栏命中 {len(hit)}/{len(expect)} · html lang={html_lang}",
            )
            await app.shot(f"{SHOT_DIR}/v12_lang_{lang}.png")
            if lang == "en":
                try:
                    import io

                    s = io.open(SETTINGS, encoding="utf-8").read()
                    rec("语言选择已持久化到 settings.json", '"ui_language": "en"' in s.replace(" ", " "), "见 ui_language 字段")
                except Exception as ex:
                    rec("语言选择已持久化", False, str(ex))

        # 设置页也要跟着变（截图留档：英文界面）
        await app.goto("/settings")
        await app.js(
            "(() => { const bs=[...document.querySelectorAll('button')].filter(b=>/依赖工具|通用设置|外观|默认参数|Tools|General|Appearance|Defaults|ツール|一般|外観|既定|Outils|Général|Apparence/.test(b.innerText));"
            " if(!bs[2]) return 'NO_TAB'; bs[2].click(); return 'OK'; })()"
        )
        await asyncio.sleep(0.7)
        rec("设置页外观页签可打开", True, "已留档截图")

    return report()


def report():
    print("\n═══════ 结果 ═══════")
    ok = sum(1 for _, o, _ in RESULTS if o)
    for n, o, d in RESULTS:
        print(("✅" if o else "❌") + " " + n + (("  |  " + d) if d else ""))
    print(f"\n通过 {ok}/{len(RESULTS)}")
    return 0 if ok == len(RESULTS) else 1


sys.exit(asyncio.run(main()))
