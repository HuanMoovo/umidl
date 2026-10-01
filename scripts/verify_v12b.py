"""umi Downloader v1.2.0 实机验收（走真实界面）

覆盖：
  1. 「在文件夹中显示」→ 资源管理器选中该文件（不再退回「文档」）
  2. 「删除记录」→ 卡片立即消失、数据库同步、可选是否删文件、文件不误删
  3. 语言切换 → 中文 / English / 日本語 / Français 实时生效并持久化
  4. 文件丢失 → 卡片提示「文件已丢失」并隐藏打开/定位按钮

校验手段：WebView2 CDP 驱动 DOM + Windows Shell COM 读资源管理器 + SQLite 读数据库。
"""

import asyncio
import base64
import json
import os
import re
import sqlite3
import subprocess
import sys
import time

import requests
import websockets

DB = os.path.expandvars(r"%APPDATA%\umi-downloader\umi.db")
SETTINGS = os.path.expandvars(r"%APPDATA%\umi-downloader\settings.json")
CDP = "http://127.0.0.1:9222/json"
SHOT = ".tmp"
RESULTS = []


def rec(name, ok, detail=""):
    RESULTS.append((name, bool(ok), detail))
    print(("  ✅ " if ok else "  ❌ ") + name + ("  |  " + detail if detail else ""), flush=True)


# ───────── 语言包（用于生成与界面语言无关的断言）─────────
def catalog(lang):
    s = open(f"src/i18n/locales/{lang}.ts", encoding="utf-8").read()
    b = s.split("export default", 1)[1].strip().rstrip(";")
    b = re.sub(r"/\*.*?\*/", "", b, flags=re.S)
    b = re.sub(r",(\s*[}\]])", r"\1", b)
    return json.loads(b)


CAT = {l: catalog(l) for l in ("zh", "en", "ja", "fr")}
NAV_KEYS = ["首页", "下载", "转换", "字幕", "设置"]
TAB_KEYS = ["依赖工具", "通用设置", "外观", "默认参数"]
LANG_CODES = {"zh": "zh-CN", "en": "en-US", "ja": "ja-JP", "fr": "fr-FR"}
HTML_LANG = {"zh": "zh-CN", "en": "en", "ja": "ja", "fr": "fr"}
LANG_NAMES = {"zh": "简体中文", "en": "English", "ja": "日本語", "fr": "Français"}


# ───────── 外部校验 ─────────
def _ps(cmd, timeout=40):
    try:
        r = subprocess.run(
            ["powershell", "-NoProfile", "-Command", "[Console]::OutputEncoding=[Text.Encoding]::UTF8; " + cmd],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="ignore",
            timeout=timeout,
        )
        return (r.stdout or "").strip()
    except Exception:
        return ""


def explorer_focused():
    out = _ps(
        "$sh=New-Object -ComObject Shell.Application; "
        "($sh.Windows() | ForEach-Object { try { $_.Document.FocusedItem.Path } catch { '' } }) -join '|'"
    )
    return [p for p in out.split("|") if p.strip()]


def explorer_locations():
    out = _ps(
        "$sh=New-Object -ComObject Shell.Application; "
        "($sh.Windows() | ForEach-Object { $_.LocationName }) -join '|'"
    )
    return [p for p in out.split("|") if p.strip()]


def doc_windows():
    return sum(1 for w in explorer_locations() if w in ("文档", "Documents", "Documentos", "Dokumente"))


def db_rows():
    c = sqlite3.connect(DB)
    try:
        return list(
            c.execute("SELECT id, title, status, coalesce(file_path,'') FROM downloads ORDER BY created_time DESC")
        )
    finally:
        c.close()


# ───────── CDP ─────────
class App:
    def __init__(self):
        self.n = 1

    async def __aenter__(self):
        d = None
        for _ in range(20):
            try:
                d = requests.get(CDP, timeout=2).json()
                break
            except Exception:
                time.sleep(0.5)
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

    async def js(self, expr):
        r = await self.cmd("Runtime.evaluate", expression=expr, returnByValue=True, awaitPromise=True, userGesture=True)
        res = r.get("result", {})
        if res.get("subtype") == "error":
            raise SystemExit(f"JS 出错：{res.get('description')}")
        return res.get("value")

    async def shot(self, path):
        r = await self.cmd("Page.captureScreenshot", format="png")
        open(path, "wb").write(base64.b64decode(r["data"]))
        return path

    async def goto(self, path):
        ok = await self.js(
            "(() => { const want='#'+%s; const a=[...document.querySelectorAll('a')].find(x=>x.getAttribute('href')===want);"
            " if(!a) return false; a.click(); return true; })()" % json.dumps(path)
        )
        await asyncio.sleep(1.2)
        return ok

    async def cards(self):
        return await self.js(
            """(() => [...document.querySelectorAll('div.umi-card')]
                 .filter(c => c.querySelector('img') || /已完成|失败|下载中|排队|处理中|Done|Failed|Downloading/.test(c.innerText))
                 .map(c => ({
                   text: c.innerText.replace(/\\n/g, ' ').slice(0, 160),
                   btns: [...c.querySelectorAll('button')].map(b => b.getAttribute('title') || b.innerText.trim()),
                   lost: c.innerText.includes('文件已丢失'),
                 })))()"""
        ) or []

    async def click_in_card(self, name12, title_zh):
        return await self.js(
            """(() => { const c=[...document.querySelectorAll('div.umi-card')].find(x => x.innerText.includes(%s));
               if(!c) return 'NO_CARD';
               const b=[...c.querySelectorAll('button')].find(x => (x.getAttribute('title')||'')===%s);
               if(!b) return 'NO_BTN'; b.scrollIntoView({block:'center'}); b.click(); return 'CLICKED'; })()"""
            % (json.dumps(name12), json.dumps(title_zh))
        )

    async def click_refresh(self):
        return await self.js(
            "(() => { const b=[...document.querySelectorAll('button')].find(x => (x.getAttribute('title')||'')==='刷新列表');"
            " if(!b) return 'NO_BTN'; b.click(); return 'CLICKED'; })()"
        )

    async def click_settings_tab(self, index):
        labels = [CAT[l][TAB_KEYS[index]] for l in CAT]
        return await self.js(
            "(() => { const labels=%s;"
            " const b=[...document.querySelectorAll('button')].find(x => labels.some(t => x.innerText.includes(t)));"
            " if(!b) return 'NO_TAB'; b.click(); return 'CLICKED'; })()" % json.dumps(labels, ensure_ascii=False)
        )

    async def click_language(self, lang):
        return await self.js(
            "(() => { const b=[...document.querySelectorAll('button')].find(x => x.innerText.includes(%s));"
            " if(!b) return 'NO_BTN'; b.click(); return 'CLICKED'; })()" % json.dumps(LANG_CODES[lang])
        )


def close_download_windows() -> int:
    """关闭标题含「umi Downloader」的资源管理器窗口（CabinetWClass）。

    只针对文件夹窗口类，应用自身的 Tauri 窗口不受影响；用于让「在文件夹中显示」
    的验收可重复（避免上一次运行留下的窗口已选中同一文件造成假阳性）。
    """
    import ctypes
    from ctypes import wintypes

    user32 = ctypes.windll.user32
    WM_CLOSE = 0x0010
    found = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def enum_proc(hwnd, _):
        cls = ctypes.create_unicode_buffer(256)
        user32.GetClassNameW(hwnd, cls, 256)
        if cls.value == "CabinetWClass":
            title = ctypes.create_unicode_buffer(512)
            user32.GetWindowTextW(hwnd, title, 512)
            if "umi Downloader" in title.value:
                found.append(hwnd)
        return True

    user32.EnumWindows(enum_proc, 0)
    for hwnd in found:
        user32.PostMessageW(hwnd, WM_CLOSE, 0, 0)
    return len(found)


async def main():
    print("═══ umi Downloader v1.2.0 实机验收 ═══\n")
    rows = db_rows()
    print(f"数据库任务 {len(rows)} 条")
    for r in rows:
        print(f"   [{r[2]}] {os.path.basename(r[3]) if r[3] else '(无文件)'}")
    print()

    jpg = next((r for r in rows if r[3].lower().endswith((".jpg", ".jpeg", ".png"))), None)
    if not jpg:
        rec("测试数据齐备", False, "需要一条图片任务（小文件，便于删除/定位测试）")
        return report()
    jpg_name, jpg_path = jpg[1], jpg[3]

    async with App() as app:
        # ── 0) 队列加载
        await app.goto("/download")
        cards = await app.cards()
        rec("下载队列加载", len(cards) >= 1, f"{len(cards)} 张卡片")
        await app.shot(f"{SHOT}/v12_queue.png")

        # ── 1) 「在文件夹中显示」
        # 先关掉上一次验收遗留的「下载目录」资源管理器窗口（只关 CabinetWClass，
        # 不动应用自身窗口），保证「该文件被选中」是这次点击造成的、可重复验证。
        closed = close_download_windows()
        await asyncio.sleep(1.0)
        focused_before = set(explorer_focused())
        doc_before = doc_windows()
        clicked = await app.click_in_card(jpg_name[:12], "在文件夹中显示")
        appeared = False
        for _ in range(16):
            await asyncio.sleep(0.5)
            if jpg_path in set(explorer_focused()):
                appeared = True
                break
        was_there = jpg_path in focused_before
        rec(
            "「在文件夹中显示」→ 资源管理器选中该文件",
            clicked == "CLICKED" and appeared and not was_there,
            f"点击={clicked} · 选中={os.path.basename(jpg_path) if appeared else '未出现'} · 先行清理 {closed} 个旧窗口 · 文档窗口 {doc_before}→{doc_windows()}",
        )

        # ── 2) 文件丢失 → 提示 + 隐藏按钮；恢复后按钮回来
        bak = jpg_path + ".hidden_by_test"
        os.replace(jpg_path, bak)
        await app.click_refresh()
        await asyncio.sleep(1.3)
        cards = await app.cards()
        card = next((c for c in cards if jpg_name[:12] in c["text"]), None)
        if card:
            bad = any(b in ("打开文件", "在文件夹中显示") for b in card["btns"])
            rec("文件丢失 → 提示已丢失且隐藏打开/定位按钮", card["lost"] and not bad, f"按钮={card['btns']}")
        else:
            rec("文件丢失 → 提示已丢失且隐藏打开/定位按钮", False, "未找到卡片")
        await app.shot(f"{SHOT}/v12_lost.png")
        os.replace(bak, jpg_path)
        await app.click_refresh()
        await asyncio.sleep(1.3)
        cards = await app.cards()
        card = next((c for c in cards if jpg_name[:12] in c["text"]), None)
        rec(
            "文件恢复 → 提示消失、按钮回来",
            bool(card) and not card["lost"] and "在文件夹中显示" in card["btns"],
            f"按钮={card['btns'] if card else '未找到卡片'}",
        )

        # ── 3) 删除
        before = len(db_rows())
        clicked = await app.click_in_card(jpg_name[:12], "删除记录")
        await asyncio.sleep(1.0)
        dialog_text = await app.js(
            "(() => { const d=[...document.querySelectorAll('div')].filter(x=>/同时删除文件|仅删除记录/.test(x.innerText)&&x.children.length<4);"
            " return d.length ? d[d.length-1].innerText.replace(/\\n/g,' ').slice(0,60) : ''; })()"
        )
        rec("删除 → 弹出「同时删除文件 / 仅删除记录」", clicked == "CLICKED" and "仅删除记录" in (dialog_text or ""), f"弹窗={dialog_text or '未出现'}")
        await app.js(
            "(() => { const b=[...document.querySelectorAll('button')].find(x=>x.innerText.trim()==='仅删除记录');"
            " if(!b) return 'NO_BTN'; b.click(); return 'CLICKED'; })()"
        )
        await asyncio.sleep(1.5)
        after_cards = await app.cards()
        after_rows = db_rows()
        gone = not any(jpg_name[:12] in (c["text"] or "") for c in after_cards)
        rec(
            "删除后卡片立即消失、数据库同步",
            gone and len(after_rows) == before - 1,
            f"卡片={'消失' if gone else '仍在'} · 数据库 {before}→{len(after_rows)}",
        )
        rec("选「仅删除记录」时文件仍保留", os.path.exists(jpg_path), os.path.basename(jpg_path))
        await app.shot(f"{SHOT}/v12_after_delete.png")

        # ── 4) 语言切换
        await app.goto("/settings")
        tab = await app.click_settings_tab(2)
        await asyncio.sleep(1.0)
        rec("设置页可打开「外观」页签", tab == "CLICKED", f"点击={tab}")

        for lang in ("en", "ja", "fr", "zh"):
            clicked = await app.click_language(lang)
            await asyncio.sleep(1.2)
            nav = await app.js("document.querySelector('aside').innerText") or ""
            want = [CAT[lang][k] for k in NAV_KEYS]
            hit = [w for w in want if w in nav]
            html_lang = await app.js("document.documentElement.lang") or ""
            tabs_hit = await app.js(
                "(() => { const want=%s; const t=document.body.innerText;"
                " return want.filter(w=>t.includes(w)).length; })()"
                % json.dumps([CAT[lang][k] for k in TAB_KEYS], ensure_ascii=False)
            )
            rec(
                f"语言切换 → {LANG_NAMES[lang]}：侧栏 + 设置页文案全部生效",
                clicked == "CLICKED" and len(hit) == len(want) and html_lang == HTML_LANG[lang] and tabs_hit == 4,
                f"侧栏 {len(hit)}/{len(want)} · 设置页签 {tabs_hit}/4 · html lang={html_lang}",
            )
            await app.shot(f"{SHOT}/v12_lang_{lang}.png")
            if lang == "en":
                try:
                    s = open(SETTINGS, encoding="utf-8").read()
                    rec("语言写入 settings.json 持久化", '"ui_language": "en"' in s, "ui_language=en")
                except Exception as ex:
                    rec("语言写入 settings.json 持久化", False, str(ex))

    return report()


def report():
    print("\n═══════ 结果 ═══════")
    ok = sum(1 for _, o, _ in RESULTS if o)
    for n, o, d in RESULTS:
        print(("✅" if o else "❌") + " " + n + (("  |  " + d) if d else ""))
    print(f"\n通过 {ok}/{len(RESULTS)}")
    return 0 if ok == len(RESULTS) else 1


sys.exit(asyncio.run(main()))
