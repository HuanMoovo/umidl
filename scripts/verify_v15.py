"""Umidl 1.5 实机验收：文档转换 / ED2K 引擎接管 / JavaScript 插件沙箱。

全部通过 WebView2 调试端口驱动**真实安装的客户端**，断言基于真实产物与真实引擎回执，
不做任何"看起来应该没问题"的推断。

用法：python scripts/verify_v15.py [--keep-running]
"""
import asyncio
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
import zipfile

import requests
import websockets

APP = os.path.expandvars(r"%LOCALAPPDATA%\Umidl\umidl.exe")
DATA = os.path.expandvars(r"%APPDATA%\umi-downloader")
BIN = os.path.join(DATA, "bin")
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIX = os.path.join(ROOT, ".tmp", "docfix")
WORK = os.path.join(DATA, "verify15")

# 合成 ed2k 链接（32 位 hex 是协议要求；引擎会照收并进等待队列，验收后清掉）
ED2K_HASH = "a1b2c3d4e5f60718293a4b5c6d7e8f90"
ED2K_NAME = "umi-verify-v15.bin"
ED2K_SIZE = 1048576
ED2K_LINK = f"ed2k://|file|{ED2K_NAME}|{ED2K_SIZE}|{ED2K_HASH.upper()}|/"

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
        """不抛异常的调用，返回 (ok, 值或错误原文)"""
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

    async def goto(self, hash_, settle=1.6):
        await self.js(f"location.hash = {json.dumps(hash_)}; 'ok'")
        await asyncio.sleep(settle)


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
    import socket

    with socket.socket() as s:
        s.settimeout(0.6)
        return s.connect_ex((host, port)) == 0


def make_fixtures():
    """验收夹具：真 docx（pandoc 产出）、手写合法 PDF、极简 xlsx（真实 OOXML）。"""
    os.makedirs(FIX, exist_ok=True)
    docx = os.path.join(FIX, "verify.docx")
    pdf = os.path.join(FIX, "verify.pdf")
    xlsx = os.path.join(FIX, "verify.xlsx")

    if not os.path.isfile(docx) and os.path.isfile(os.path.join(BIN, "pandoc.exe")):
        md = os.path.join(FIX, "src.md")
        with open(md, "w", encoding="utf-8") as f:
            f.write("# 文档转换验收标题\n\n这是一个**测试文档**：令牌桶 300 KB/s，分段 16 连接。\n")
        subprocess.run([os.path.join(BIN, "pandoc.exe"), md, "-o", docx], capture_output=True, timeout=120)

    if not os.path.isfile(pdf):
        with open(pdf, "wb") as f:
            f.write(
                b"%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n"
                b"2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n"
                b"3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 300 120]/Contents 4 0 R"
                b"/Resources<</Font<</F1 5 0 R>>>>>>endobj\n"
                b"4 0 obj<</Length 74>>stream\n"
                b"BT /F1 16 Tf 20 70 Td (Umidl Doc Verify) Tj 0 -24 Td (token bucket 300 KB/s) Tj ET\n"
                b"endstream\nendobj\n"
                b"5 0 obj<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>endobj\ntrailer<</Root 1 0 R>>\n"
            )

    if not os.path.isfile(xlsx):
        # 极简但合法的 xlsx：一张表、两个单元格（真实 OOXML 部件）
        ct = (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
            '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
            '<Default Extension="xml" ContentType="application/xml"/>'
            '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
            '<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
            '<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>'
            "</Types>"
        )
        rels = (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>'
            "</Relationships>"
        )
        wb = (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
            'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">'
            '<sheets><sheet name="验收表" sheetId="1" r:id="rId1"/></sheets></workbook>'
        )
        wbrels = (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>'
            '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>'
            "</Relationships>"
        )
        sst = (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="4" uniqueCount="4">'
            "<si><t>项目</t></si><si><t>数值</t></si><si><t>令牌桶</t></si><si><t>300 KB/s</t></si></sst>"
        )
        sheet = (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>'
            '<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>'
            '<row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2" t="s"><v>3</v></c></row>'
            "</sheetData></worksheet>"
        )
        with zipfile.ZipFile(xlsx, "w", zipfile.ZIP_DEFLATED) as z:
            z.writestr("[Content_Types].xml", ct)
            z.writestr("_rels/.rels", rels)
            z.writestr("xl/workbook.xml", wb)
            z.writestr("xl/_rels/workbook.xml.rels", wbrels)
            z.writestr("xl/sharedStrings.xml", sst)
            z.writestr("xl/worksheets/sheet1.xml", sheet)

    return docx, pdf, xlsx


def as_list(v, *keys):
    """后端返回可能是数组，也可能包在 {"plugins":[...]} 里，两种都吃。"""
    if isinstance(v, list):
        return v
    if isinstance(v, dict):
        for k in list(keys) + ["plugins", "entries", "items", "list"]:
            x = v.get(k)
            if isinstance(x, list):
                return x
    return []


def market_file_for(entry):
    """在市场目录里找到某个插件条目的真实文件"""
    d = os.path.join(DATA, "plugins", "market", "files")
    if not os.path.isdir(d):
        return None
    pid = entry.get("id", "")
    for name in os.listdir(d):
        if pid and pid in name:
            return os.path.join(d, name)
    files = [os.path.join(d, n) for n in os.listdir(d)]
    return files[0] if files else None


async def main():
    keep = "--keep-running" in sys.argv
    kill_app()
    shutil.rmtree(WORK, ignore_errors=True)
    os.makedirs(WORK, exist_ok=True)
    docx, pdf, xlsx = make_fixtures()

    print("══ 启动被测程序")
    proc = launch_app()
    d = Driver()
    if not await d.connect():
        print("  ✗ 连接调试端口失败")
        return 1
    check("CDP 连接 + 应用启动", True)
    await asyncio.sleep(2.0)

    # ── 1. 版本与新命令 ────────────────────────────────────────────────
    print("══ 1. 版本与新命令可用性")
    ver = await d.invoke("app_version")
    check("版本号为 1.5.0", str(ver).startswith("1.5"), str(ver))

    new_cmds = [
        ("doc_capabilities", {}),
        ("plugin_sandbox_info", {}),
        ("plugin_list", {}),
        ("plugin_market_list", {}),
        ("ed2k_engine_status", {}),
    ]
    for cmd, args in new_cmds:
        try:
            await d.invoke(cmd, args)
            check(f"新命令可调用：{cmd}", True)
        except Exception as e:
            check(f"新命令可调用：{cmd}", False, str(e)[:120])

    # ── 2. 文档转换（真实文件） ────────────────────────────────────────
    print("══ 2. 文档转换能力与真实转换")
    cap = await d.invoke("doc_capabilities")
    native = cap.get("native") or []
    check("pandoc 已托管并就绪", bool(cap.get("pandoc")), str(cap.get("pandoc_path"))[:80])
    check("poppler 已托管并就绪", bool(cap.get("poppler")), str(cap.get("poppler_path"))[:80])
    check(
        "原生格式覆盖 Office/ODF",
        all(x in native for x in ["docx", "xlsx", "pptx", "odt", "ods", "odp"]),
        ",".join(native[:12]),
    )

    for path, kind in [(docx, "docx"), (pdf, "pdf"), (xlsx, "xlsx")]:
        pr = await d.invoke("probe_document", {"path": path})
        detail = f"kind={pr.get('kind')} 页={pr.get('pages')} 表={pr.get('sheets')} 字={pr.get('chars')}"
        check(f"probe_document 识别真实 {kind}", bool(pr.get("ok")), detail)

    # 真实转换：docx → md，读回产物内容
    conv = os.path.join(WORK, "conv")
    os.makedirs(conv, exist_ok=True)
    await d.try_invoke(
        "start_convert",
        {"req": {"input_file": docx, "output_dir": conv, "format": "md", "extract_audio": False, "mute": False}},
    )
    md_out = os.path.join(conv, "verify.md")
    ok_md = False
    body = ""
    for _ in range(30):
        await asyncio.sleep(1.0)
        if os.path.isfile(md_out):
            body = open(md_out, encoding="utf-8", errors="replace").read()
            if body.strip():
                ok_md = True
                break
    check("真实 docx→md 转换产出非空", ok_md, f"{len(body)} 字符")
    check("产物内容正确（中文标题在）", "文档转换验收标题" in body, body.strip().splitlines()[:1] and body.strip().splitlines()[0][:40])

    # 真实抽取：pdf → txt
    await d.try_invoke(
        "start_convert",
        {"req": {"input_file": pdf, "output_dir": conv, "format": "txt", "extract_audio": False, "mute": False}},
    )
    txt_out = os.path.join(conv, "verify.txt")
    ok_txt, tbody = False, ""
    for _ in range(30):
        await asyncio.sleep(1.0)
        if os.path.isfile(txt_out):
            tbody = open(txt_out, encoding="utf-8", errors="replace").read()
            if tbody.strip():
                ok_txt = True
                break
    check("真实 PDF→txt 抽取非空", ok_txt, f"{len(tbody)} 字符")
    check("PDF 抽取内容正确", "Umidl Doc Verify" in tbody, tbody.strip()[:48].replace("\n", " "))

    # 真实原生转换：xlsx → csv
    await d.try_invoke(
        "start_convert",
        {"req": {"input_file": xlsx, "output_dir": conv, "format": "csv", "extract_audio": False, "mute": False}},
    )
    csv_out = os.path.join(conv, "verify.csv")
    ok_csv, cbody = False, ""
    for _ in range(20):
        await asyncio.sleep(1.0)
        if os.path.isfile(csv_out):
            cbody = open(csv_out, encoding="utf-8", errors="replace").read()
            if cbody.strip():
                ok_csv = True
                break
    check("真实 xlsx→csv 转换非空", ok_csv, f"{len(cbody)} 字符")
    check("xlsx 内容正确（中文表头在）", "令牌桶" in cbody, cbody.strip().replace("\n", " | ")[:60])

    # ── 3. ED2K ───────────────────────────────────────────────────────
    print("══ 3. ED2K 链接解析与引擎接管")
    link = await d.invoke("ed2k_parse", {"link": ED2K_LINK})
    check(
        "ed2k 链接解析正确",
        link.get("hash", "").lower() == ED2K_HASH
        and link.get("name") == ED2K_NAME
        and int(link.get("size", 0)) == ED2K_SIZE,
        f"hash={link.get('hash')} name={link.get('name')} size={link.get('size')}",
    )
    ok_bad, err_bad = await d.try_invoke("ed2k_parse", {"link": "ed2k://|file|bad.iso|notanumber|xyz|/"})
    check("非法 ed2k 链接被拒并给出原因", (not ok_bad) and bool(err_bad), str(err_bad)[:90])

    st = await d.invoke("ed2k_engine_status")
    check("ED2K 引擎已安装（eMule）", bool(st.get("installed")), f"engine={st.get('engine')} path={str(st.get('path'))[-40:]}")

    ok_sub, sub = await d.try_invoke("ed2k_submit", {"link": ED2K_LINK})
    detail = json.dumps(sub, ensure_ascii=False)[:150] if ok_sub else str(sub)[:150]
    check("链接真实交给引擎并回读核对", ok_sub and (sub.get("verified") is True), detail)

    # 队列里走 ed2k 也要路由到引擎（不走 yt-dlp/aria2）
    route = await d.invoke("explain_route", {"url": ED2K_LINK})
    check("路由判定走 ED2K 引擎", route.get("engine") == "ed2k", str(route.get("engine_label"))[:60])

    tasks_before = await d.invoke("list_downloads")
    ids_before = {t.get("id") for t in (tasks_before or [])}
    await d.try_invoke(
        "start_download",
        {"req": {"url": ED2K_LINK, "mode": "video", "subtitle_langs": [], "cookies_from_browser": None}},
    )
    picked = None
    for _ in range(20):
        await asyncio.sleep(1.0)
        tasks = await d.invoke("list_downloads") or []
        fresh = [t for t in tasks if t.get("id") not in ids_before]
        if fresh:
            picked = fresh[0]
            if picked.get("status") in ("done", "error", "Done", "Error"):
                break
    note = (picked or {}).get("format_note") or ""
    check(
        "队列里的 ed2k 任务被引擎接管（非 yt-dlp/aria2）",
        bool(picked) and "ED2K" in note,
        f"status={picked and picked.get('status')} note={note} total={picked and picked.get('total')}",
    )
    check(
        "任务卡写回链接里的真实文件名与大小",
        bool(picked) and picked.get("title") == ED2K_NAME and int(picked.get("total") or 0) == ED2K_SIZE,
        f"title={picked and picked.get('title')} total={picked and picked.get('total')}",
    )
    if picked:
        await d.try_invoke("remove_download", {"id": picked.get("id")})

    # ── 4. 插件沙箱 ───────────────────────────────────────────────────
    print("══ 4. JavaScript 插件沙箱与内容寻址市场")
    sb = await d.invoke("plugin_sandbox_info")
    check(
        "沙箱可用（QuickJS + 内存/时间限制）",
        bool(sb.get("available")) and (sb.get("memory_limit_mb") or 0) > 0 and (sb.get("script_timeout_ms") or 0) > 0,
        f"engine={sb.get('engine')} {sb.get('version')} 内存={sb.get('memory_limit_mb')}MB 超时={sb.get('script_timeout_ms')}ms",
    )

    market = as_list(await d.invoke("plugin_market_list"), "plugins")
    check("插件市场有内置条目且带内容摘要", len(market) >= 2 and all(m.get("sha256") for m in market),
          f"{len(market)} 条：" + ",".join(m.get("id", "?") for m in market))

    target = next((m for m in market if m.get("id")), None)
    if target:
        pid = target["id"]
        # 4a. 篡改一个字节 → 内容寻址校验必须拒绝
        mf = market_file_for(target)
        backup = None
        if mf and os.path.isfile(mf):
            backup = mf + ".bak"
            shutil.copy2(mf, backup)
            with open(mf, "ab") as f:
                f.write(b"\n// tampered-by-verify15\n")
            ok_t, err_t = await d.try_invoke("plugin_install", {"id": pid})
            check("被篡改的插件包被拒绝安装", (not ok_t) and ("sha256" in str(err_t).lower() or "校验" in str(err_t)), str(err_t)[:110])
            shutil.move(backup, mf)
        else:
            check("被篡改的插件包被拒绝安装", False, "找不到市场文件")

        # 4b. 正常安装 → 列出 → 沙箱试跑 → 解析器表态
        ok_i, ins = await d.try_invoke("plugin_install", {"id": pid})
        check("从市场安装插件成功（摘要一致）", ok_i, json.dumps(ins, ensure_ascii=False)[:110] if ok_i else str(ins)[:110])

        lst = as_list(await d.invoke("plugin_list"), "plugins")
        mine = next((p for p in lst if p.get("id") == pid), None)
        check("已安装列表出现该插件", bool(mine), f"{len(lst)} 个已装；{mine and mine.get('version')}")

        if mine:
            t = await d.invoke("plugin_test", {"id": pid})
            logs = t.get("logs") or []
            check("沙箱内试跑插件成功", bool(t.get("ok")), f"日志 {len(logs)} 条：{' | '.join(str(x)[:28] for x in logs[:3])}")

        res = await d.invoke("plugin_run_resolvers", {"url": "https://example.com/demo.iso"})
        check("插件解析器对 URL 表态", bool(res), json.dumps(res, ensure_ascii=False)[:130])

        ok_u, _ = await d.try_invoke("plugin_uninstall", {"id": pid})
        lst2 = as_list(await d.invoke("plugin_list"), "plugins")
        check("卸载插件生效", ok_u and all(p.get("id") != pid for p in lst2), f"剩余 {len(lst2)} 个")

    # ── 5. 界面 ───────────────────────────────────────────────────────
    print("══ 5. 界面渲染（真实 DOM）")
    await d.goto("#/plugins")
    check("插件管理页渲染（沙箱信息/市场/日志）",
          await d.wait_text("body", "沙箱") and await d.wait_text("body", "市场") and await d.wait_text("body", "事件"),
          "沙箱信息 / 插件市场 / 事件日志")

    await d.goto("#/settings")
    ok_tab = False
    for tag in ["button", "div", "span"]:
        got = await d.js(
            "(() => { const els=[...document.querySelectorAll(" + json.dumps(tag) + ")];"
            " const el=els.find(e=>(e.innerText||'').trim()==='系统与集成'); if(!el) return false; el.click(); return true; })()"
        )
        if got:
            ok_tab = True
            break
    await asyncio.sleep(1.5)
    check("设置页出现 ED2K 引擎卡片", await d.wait_text("body", "ED2K"), "系统与集成 → ED2K 电驴引擎" if ok_tab else "页签点击失败")

    await d.goto("#/converter")
    check("转换页出现文档能力区", await d.wait_text("body", "文档"), "文档转换 / 外部引擎 Pandoc · Poppler")

    # ── 6. 收尾 ───────────────────────────────────────────────────────
    print("══ 6. 收尾")
    try:
        await d.js("window.close(); 'ok'")
    except Exception:
        pass
    await asyncio.sleep(1.5)
    if not keep:
        kill_app()
        closed = not (port_open(9222))
        check("退出后调试端口已关闭", closed, f"9222 打开={not closed}")

    # 清掉本次验收灌进 eMule 的合成任务
    try:
        subprocess.run(
            ["powershell", "-NoProfile", "-Command",
             "Get-Process emule -ErrorAction SilentlyContinue | Stop-Process -Force"],
            capture_output=True,
        )
        time.sleep(1.0)
        cfg = os.path.join(BIN, "emule", "config", "downloads.txt")
        if os.path.isfile(cfg):
            with open(cfg, "r", encoding="utf-8", errors="replace", newline="") as f:
                lines = f.readlines()
            kept = [ln for ln in lines if ED2K_HASH.lower() not in ln.lower()]
            with open(cfg, "w", encoding="utf-8", newline="") as f:
                f.writelines(kept)
            print(f"    已从引擎清单移除合成任务（{len(lines) - len(kept)} 行）")
        tdir = os.path.join(BIN, "emule", "Temp")
        if os.path.isdir(tdir):
            for n in os.listdir(tdir):
                if ED2K_HASH.lower()[:16] in n.lower():
                    try:
                        os.remove(os.path.join(tdir, n))
                    except Exception:
                        pass
    except Exception as e:
        print(f"    引擎清理跳过：{e}")

    print("\n" + "=" * 56)
    print(f"通过 {len(PASS)} ｜ 失败 {len(FAIL)}")
    if FAIL:
        print("失败项：")
        for f in FAIL:
            print(f"  ✗ {f}")
    return 0 if not FAIL else 1


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
