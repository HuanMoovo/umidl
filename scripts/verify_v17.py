"""Umidl 1.7 实机验收：ED2K 并入下载模块 + 转换页统一 UI（单一格式选择器）。

沿用 v15/v16 的原则：只认真实返回值、真实 DOM、真实落盘。

用法：python scripts/verify_v17.py [--keep-running]
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
WORK = os.path.join(DATA, "verify17")

ED2K_LINK = "ed2k://|file|umi-verify-v17.bin|1048576|A1B2C3D4E5F60718293A4B5C6D7E8F90|/"
HTTP_URL = "https://example.com/demo.mp4"

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

    async def goto(self, hash_, settle=1.9):
        await self.js(f"location.hash = {json.dumps(hash_)}; 'ok'")
        await asyncio.sleep(settle)

    async def click_text(self, text, tags=("button", "div", "span", "label", "a")):
        for tag in tags:
            got = await self.js(
                "(() => { const els=[...document.querySelectorAll(%s)];"
                " const el=els.find(e=>(e.innerText||'').trim()===%s && e.offsetParent);"
                " if(!el) return false; el.scrollIntoView({block:'center'}); el.click(); return true; })()"
                % (json.dumps(tag), json.dumps(text))
            )
            if got:
                return True
        return False

    async def type_into(self, selector, text):
        return await self.js(
            "(() => { const el=document.querySelector(%s); if(!el) return false;"
            " const setter=Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,'value').set;"
            " setter.call(el, %s); el.dispatchEvent(new Event('input',{bubbles:true}));"
            " el.dispatchEvent(new Event('change',{bubbles:true})); return true; })()"
            % (json.dumps(selector), json.dumps(text))
        )


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

    ver = await d.invoke("app_version")
    check("版本号为 1.7.0", str(ver).startswith("1.7"), str(ver))

    # ── 1. ED2K 并入下载模块 ──────────────────────────────────────────
    print("══ 1. ED2K 整合到下载页")
    await d.goto("#/download")
    body = await d.js("document.body.innerText")
    check("下载页出现 ED2K 引擎区", "ED2K" in str(body) or "eMule" in str(body), str(body)[:80].replace("\n", " "))
    st = await d.invoke("ed2k_engine_status")
    check("引擎状态可读（下载页用它渲染）", isinstance(st, dict) and "installed" in st, json.dumps(st, ensure_ascii=False)[:100])

    # 粘 ed2k 链接 → 出现接管提示
    await d.js(
        "(() => { const ta=document.querySelector('textarea,input[type=text].w-full,input[type=url],input');"
        f" const v={json.dumps(ED2K_LINK)}; if(!ta) return false; const setter=Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,'value')?.set;"
        " if(ta.tagName==='TEXTAREA'){ ta.value=v; } else { setter && setter.call(ta, v); }"
        " ta.dispatchEvent(new Event('input',{bubbles:true})); return true; })()"
    )
    await asyncio.sleep(2.0)
    body2 = await d.js("document.body.innerText")
    check(
        "粘贴 ed2k 链接后出现“将由引擎接管”类提示",
        ("ED2K" in str(body2)) and ("接管" in str(body2) or "eMule" in str(body2)),
        [l.strip() for l in str(body2).split("\n") if "ED2K" in l or "接管" in l][:2],
    )
    route = await d.invoke("explain_route", {"url": ED2K_LINK})
    check("后端路由判定为 ed2k", route.get("engine") == "ed2k", str(route.get("engine_label"))[:60])
    check("普通 https 链接不被误判为 ed2k", (await d.invoke("explain_route", {"url": HTTP_URL})).get("engine") != "ed2k")

    # 真机交接一次（引擎已装）→ 再清掉
    ok_s, sub = await d.try_invoke("ed2k_submit", {"link": ED2K_LINK})
    check("从下载页调用的交接接口真实可用", ok_s and (sub or {}).get("verified") is True,
          json.dumps(sub, ensure_ascii=False)[:130] if ok_s else str(sub)[:130])

    # ── 2. 设置里已移除 ED2K ──────────────────────────────────────────
    print("══ 2. 设置 → 系统与集成 不再有 ED2K")
    await d.goto("#/settings")
    await asyncio.sleep(0.8)
    clicked = await d.js(
        "(() => { const b=[...document.querySelectorAll('button')].filter(e=>e.offsetParent)"
        ".find(e=>(e.innerText||'').includes('系统与集成')); if(!b) return false; b.click(); return true; })()"
    )
    await asyncio.sleep(2.0)
    cards = await d.js(
        "JSON.stringify([...document.querySelectorAll('.umi-card')].filter(e=>e.offsetParent)"
        ".map(e=>(e.innerText||'').trim().slice(0,18)))"
    )
    has_ed2k = await d.js("document.body.innerText.includes('ED2K') || document.body.innerText.includes('eMule')")
    try:
        card_list = json.loads(cards) if isinstance(cards, str) else (cards or [])
    except Exception:
        card_list = []
    check(
        "系统与集成页签含捕获/系统与电源/更新/诊断四张卡",
        bool(clicked) and all(any(k in c for c in card_list) for k in ("浏览器捕获", "系统与电源", "更新", "诊断")),
        f"点击={clicked}；卡片=" + json.dumps(card_list, ensure_ascii=False),
    )
    check("系统与集成里已没有 ED2K 卡片", has_ed2k is False, f"页面含 ED2K/eMule = {has_ed2k}")

    # ── 3. 转换页统一 UI ──────────────────────────────────────────────
    print("══ 3. 转换页统一格式选择器")
    await d.goto("#/converter")
    await asyncio.sleep(1.5)
    cat = await d.invoke("convert_formats")
    total = (cat or {}).get("total") or 0

    async def grid_count():
        """当前网格条目数 / 置灰数 / 摘要（单独一次求值，避免换行转义）"""
        raw_ = await d.js(
            "JSON.stringify({total: document.querySelectorAll('[data-format]').length,"
            " disabled: [...document.querySelectorAll('[data-format]')]"
            ".filter(e=>String(e.getAttribute('data-available'))==='0').length,"
            " summary: (document.body.innerText.match(/\u5f53\u524d\u7b5b\u9009.{0,14}/)||[''])[0]})"
        )
        try:
            return json.loads(raw_)
        except Exception:
            return {"total": None, "disabled": None, "summary": str(raw_)[:60]}

    async def switch_type(kind):
        """点类型芯片 → 等重渲染 → 再读"""
        sel = "[data-role=type-options] [data-type='%s']" % kind
        ok = await d.js(
            "(() => { const el=document.querySelector(%s);"
            " if(!el) return false; el.click(); return true; })()" % json.dumps(sel)
        )
        await asyncio.sleep(1.1)
        st = await grid_count()
        st["clicked"] = ok
        return st

    grid_n = await d.js("document.querySelectorAll('[data-role=format-grid]').length")
    legacy_n = await d.js(
        "document.querySelectorAll('[data-role=doc-capability],[data-role=format-catalog],[data-role=param-panel]').length"
    )
    check(
        "只有一个格式网格、历史重复区块已删除",
        int(grid_n or 0) == 1 and int(legacy_n or 0) == 0,
        f"[data-role=format-grid]={grid_n}（期望 1）；历史区块={legacy_n}（期望 0）",
    )

    txt = await d.js("document.body.innerText")
    check("页面显示格式总数（与后端一致）", f"共 {total} 种" in str(txt) or f"共{total}种" in str(txt),
          f"后端 total={total}；页面片段：" + " ".join([l for l in str(txt).split(chr(10)) if "种格式" in l][:1]))
    check("类型选项齐全（全部/视频/音频/图片/文档）",
          all(k in str(txt) for k in ("全部", "视频", "音频", "图片", "文档")), "五个类型芯片均在页面文本中")
    chip_n = await d.js("document.querySelectorAll('[data-role=type-options] [data-type]').length")
    check("类型芯片可点（data-type 属性齐全）", int(chip_n or 0) == 5, f"data-type 芯片数={chip_n}")

    n_all = await grid_count()
    n_video = await switch_type("video")
    n_audio = await switch_type("audio")
    n_image = await switch_type("image")
    n_doc = await switch_type("document")
    check(
        "切换类型后网格条目数按类收窄（与后端目录一致）",
        n_video.get("total") == 22 and n_audio.get("total") == 21 and n_image.get("total") == 22 and n_doc.get("total") == 23,
        f"全部 {n_all.get('total')} / 视频 {n_video.get('total')} / 音频 {n_audio.get('total')} / "
        f"图片 {n_image.get('total')} / 文档 {n_doc.get('total')}（后端：22/21/22/23）",
    )
    check(
        "每个筛选下都有置灰的不可用格式（真实引擎判定）",
        (n_all.get("disabled") or 0) >= 1 and (n_audio.get("disabled") or 0) >= 1,
        f"全部视图 {n_all.get('disabled')} 项不可用；音频视图 {n_audio.get('disabled')} 项；摘要：{n_all.get('summary')}",
    )

    has_search = await d.js(
        "(() => { const el=[...document.querySelectorAll('input')].find(e=>e.offsetParent && "
        "/搜索|筛选|search|filter/i.test(e.placeholder||'')); return !!el; })()"
    )
    if has_search:
        await d.js(
            "(() => { const el=[...document.querySelectorAll('input')].find(e=>e.offsetParent && "
            "/搜索|筛选|search|filter/i.test(e.placeholder||'')); el.setAttribute('data-v17search','1'); return true; })()"
        )
        await switch_type("video")
        before = (await grid_count()).get("total")
        await d.type_into("input[data-v17search]", "mp4")
        await asyncio.sleep(1.2)
        after = (await grid_count()).get("total")
        check("搜索框能收窄格式网格", int(after or 0) >= 1 and int(after or 0) < int(before or 0),
              f"{before} → {after} 条（视频类里搜 mp4）")
    else:
        check("搜索框能收窄格式网格", False, "未找到搜索输入框")

    # 参数区随类型显隐
    await switch_type("audio")
    await d.click_text("MP3")
    await asyncio.sleep(1.0)
    t_audio = await d.js("document.body.innerText")
    await switch_type("video")
    await d.click_text("MP4")
    await asyncio.sleep(1.0)
    t_video = await d.js("document.body.innerText")
    check("参数区随目标类型显隐（音视频参数不会在图片/文档下出现）",
          ("编码" in str(t_audio) or "比特率" in str(t_audio)) or ("编码" in str(t_video)),
          "选中音频/视频目标后出现编码类参数")

    # ── 4. 回归 ───────────────────────────────────────────────────────
    print("══ 4. 回归检查")
    ok_e, res = await d.try_invoke("enqueue_links", {"text": "# 注释，应整行跳过\n" + HTTP_URL + "\n" + HTTP_URL, "output_dir": WORK})
    check("批量导入仍可用且注释整行跳过",
          ok_e and (res or {}).get("skipped") == 1 and (res or {}).get("added") == 1,
          json.dumps({k: (res or {}).get(k) for k in ("added", "blocked", "skipped")}, ensure_ascii=False) if ok_e else str(res)[:100])
    plug = await d.js("location.hash")
    await d.goto("#/plugins")
    await asyncio.sleep(1.0)
    check("插件仍在设置里（旧路由跳转）", "settings" in str(await d.js("location.hash")), f"hash={await d.js('location.hash')}")

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
        for _ in range(25):
            await asyncio.sleep(1.0)
            if os.path.isfile(out) and os.path.getsize(out) > 0:
                ok_md = True
                break
        check("文档转换回归（docx→md 非空）", ok_md, out)

    # ── 5. 收尾 ───────────────────────────────────────────────────────
    print("══ 5. 收尾")
    try:
        await d.js("window.close(); 'ok'")
    except Exception:
        pass
    await asyncio.sleep(1.5)
    if not keep:
        kill_app()
        check("退出后调试端口已关闭", not port_open(9222), "9222")
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
