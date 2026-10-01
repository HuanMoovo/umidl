"""BUG-19 真机验收：在**安装版**上用真实 UI 路径跑「原文 + 英文字幕」。

覆盖：
  A. 开关打开 + 语言=自动检测 → 两份产物（原文 / 译文）各自落盘、内容不同
  B. 同设置再跑一次（同语言重跑）→ 两份文件仍各自完好、只覆盖自己
  C. 开关关闭 → 只出一份，且已有的译文产物一个字节都不动
  D. 原文语言 = en（会同名的那种组合）→ 仍是两份、名字不冲突

用法：python scripts/verify_v19.py
"""
import asyncio
import json
import os
import subprocess
import sys
import time

PORT = 9222
APP = os.path.expandvars(r"%LOCALAPPDATA%\Umidl\umidl.exe")
DATA = os.path.expandvars(r"%APPDATA%\umi-downloader")
WORK = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".tmp", "sub19"))
VIDEO = os.path.join(WORK, "Sample '샘플 영상' Official Special Clip [AbCdEfGhIjK].mp4")

import requests
import websockets


def kill_app():
    subprocess.run(["powershell", "-NoProfile", "-Command",
                    "Get-Process umidl,umi-downloader -ErrorAction SilentlyContinue | Stop-Process -Force"],
                   capture_output=True)
    time.sleep(1.5)


class Driver:
    def __init__(self, port=PORT):
        self.port = port
        self.ws = None
        self.n = 0

    async def connect(self, tries=40):
        for _ in range(tries):
            try:
                d = requests.get(f"http://127.0.0.1:{self.port}/json", timeout=3).json()
                pages = [t for t in d if t.get("type") == "page"]
                if pages:
                    self.ws = await websockets.connect(pages[0]["webSocketDebuggerUrl"], max_size=64 * 1024 * 1024)
                    return True
            except Exception:
                pass
            await asyncio.sleep(0.6)
        return False

    async def cmd(self, method, **params):
        self.n += 1
        mid = self.n
        await self.ws.send(json.dumps({"id": mid, "method": method, "params": params}))
        while True:
            msg = json.loads(await self.ws.recv())
            if msg.get("id") == mid:
                if "error" in msg:
                    raise RuntimeError(msg["error"])
                return msg.get("result", {})

    async def js(self, expr):
        r = await self.cmd("Runtime.evaluate", expression=expr, returnByValue=True, awaitPromise=True)
        if "exceptionDetails" in r:
            raise RuntimeError(r["exceptionDetails"])
        return r.get("result", {}).get("value")

    async def invoke(self, name, payload="{}"):
        out = await self.js(
            "(async () => { try { const r = await window.__TAURI_INTERNALS__.invoke("
            f"{json.dumps(name)}, {payload}); return JSON.stringify({{ok:true, v:r}}); }}"
            " catch (e) { return JSON.stringify({ok:false, e:String(e)}); } })()"
        )
        d = json.loads(out)
        return d.get("v") if d.get("ok") else {"__error": d.get("e")}


async def ui_set_video(d, path):
    return await d.js(
        "(() => {const el=document.querySelector('[data-test=subtitle-path]'); if(!el) return 'NONE';"
        "const setter=Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,'value').set;"
        f"setter.call(el, {json.dumps(path)});"
        "el.dispatchEvent(new Event('input',{bubbles:true})); el.dispatchEvent(new Event('change',{bubbles:true}));"
        "return el.value;})()"
    )


async def ui_translate_on(d):
    """把「翻译成英文」开关设成打开（幂等：已经开了就不动）。等一拍再读状态（Vue 异步渲染）"""
    await d.js(
        "(() => {const el=document.querySelector('[data-test=translate-en]'); if(!el) return 'NONE';"
        "const on = el.getAttribute('aria-checked') === 'true'; if(!on) el.click(); return 'clicked';})()"
    )
    await asyncio.sleep(0.6)
    return await d.js("document.querySelector('[data-test=translate-en]')?.getAttribute('aria-checked')")


async def ui_translate_off(d):
    await d.js(
        "(() => {const el=document.querySelector('[data-test=translate-en]'); if(!el) return 'NONE';"
        "const on = el.getAttribute('aria-checked') === 'true'; if(on) el.click(); return 'clicked';})()"
    )
    await asyncio.sleep(0.6)
    return await d.js("document.querySelector('[data-test=translate-en]')?.getAttribute('aria-checked')")


async def ui_pick_lang(d, lang="auto"):
    """只保留一个识别语言（点掉其它已选中的 chip），再等一拍读回真实状态"""
    await d.js(
        "(() => {const chips=[...document.querySelectorAll('[data-test=lang-chips] button')];"
        f"const want={json.dumps(lang)};"
        "for (const c of chips) { const v=c.getAttribute('data-lang'); const on=c.getAttribute('aria-pressed')==='true';"
        " if (v===want && !on) c.click(); else if (v!==want && on) c.click(); }"
        "return 'clicked';})()"
    )
    await asyncio.sleep(0.6)
    return await d.js(
        "[...document.querySelectorAll('[data-test=lang-chips] button')]"
        ".filter(c=>c.getAttribute('aria-pressed')==='true').map(c=>c.getAttribute('data-lang')).join(',')"
    )


async def ui_start(d):
    return await d.js(
        "(() => {const b=document.querySelector('[data-test=subtitle-start]'); if(!b) return 'NONE';"
        "if (b.disabled) return 'DISABLED'; const label=b.innerText; b.click(); return label;})()"
    )


async def rows_text(d):
    return json.loads(await d.js(
        "JSON.stringify([...document.querySelectorAll('[data-role=\"subtitle-row\"]')].map(e=>e.innerText))"))


async def all_ids(d):
    """当前库里的全部字幕任务 id（用来 diff 出「这一次点击新建了哪几条」）"""
    subs = await d.invoke("list_subtitles")
    return {t["id"] for t in (subs if isinstance(subs, list) else [])}


async def click_start_and_collect(d, before, expect):
    """点「开始转写」→ 返回本次新建的任务 id（新库行 - 点击前的 id 集合）"""
    label = await ui_start(d)
    await asyncio.sleep(1.2)
    subs = await d.invoke("list_subtitles")
    ids = [t["id"] for t in subs if t["id"] not in before]
    if len(ids) != expect:
        raise SystemExit(f"本次新建任务数 {len(ids)} ≠ 预期 {expect}（按钮文案 {label!r}）")
    return label, ids


async def snapshot_idle(d, seconds=3.0):
    """没有任务在跑时产物必须一动不动（排除「后台还有人在写」的假象）"""
    a = listing()
    await asyncio.sleep(seconds)
    b = listing()
    if a != b:
        raise SystemExit(f"空闲 {seconds}s 期间产物被改动：{a} → {b}")
    return b


async def wait_done(d, ids, timeout=420):
    """等指定任务全部脱离 pending/extracting/transcribing"""
    t0 = time.time()
    last = None
    while time.time() - t0 < timeout:
        subs = await d.invoke("list_subtitles")
        if isinstance(subs, list):
            mine = [t for t in subs if t.get("id") in ids]
            last = ",".join(f"{t['id']}:{t['status']}:{round(t['progress'])}%" for t in sorted(mine, key=lambda x: x["id"]))
            if len(mine) == len(ids) and all(
                t["status"] in ("done", "error", "canceled") for t in mine
            ):
                return mine
        await asyncio.sleep(3)
    raise SystemExit(f"等待任务结束超时：{last}")


def listing():
    import hashlib
    out = {}
    for f in sorted(os.listdir(WORK)):
        p = os.path.join(WORK, f)
        if os.path.isfile(p) and f.lower().endswith((".srt", ".vtt", ".txt", ".json")):
            b = open(p, "rb").read()
            out[f] = (len(b), hashlib.md5(b).hexdigest()[:8],
                      time.strftime("%H:%M:%S", time.localtime(os.path.getmtime(p))))
    return out


def describe(name):
    p = os.path.join(WORK, name)
    b = open(p, "rb").read()
    txt = b.decode("utf-8", errors="replace")
    lines = [l for l in txt.replace("\r\n", "\n").split("\n") if l.strip()]
    first_line = txt.replace("\r\n", "\n").split("\n")[0]
    cue = " / ".join(lines[:3])
    print(f"  · {name}")
    print(f"      字节={len(b)} · 首行={first_line!r} · 首条 cue={cue!r}")


async def main():
    os.makedirs(WORK, exist_ok=True)
    if not os.path.isfile(VIDEO):
        raise SystemExit(f"缺少测试视频：{VIDEO}")
    kill_app()
    env = dict(os.environ)
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = f"--remote-debugging-port={PORT}"
    subprocess.Popen([APP], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    d = Driver()
    if not await d.connect():
        raise SystemExit("CDP 未就绪")
    await asyncio.sleep(2.5)
    ver = await d.js("(document.body.innerText.match(/v1\\.[0-9]+\\.[0-9]+/)||[''])[0]")
    print(f"══ 安装版 {ver} · 真实 UI 路径（输入框 + 开关 + 开始按钮）")
    await d.js("location.hash = '#/subtitle'; 'ok'")
    await asyncio.sleep(1.5)

    mine = []
    print("── A. 开关打开 + 语言=自动检测")
    print("   输入框写入视频：", await ui_set_video(d, VIDEO))
    print("   语言 chips：", await ui_pick_lang(d, "auto"))
    print("   翻译开关 =", await ui_translate_on(d))
    before_ids = await all_ids(d)
    label, ids = await click_start_and_collect(d, before_ids, 2)
    print(f"   开始按钮文案：{label} · 本次两个任务 id：{ids}")
    mine += ids
    done = await wait_done(d, ids)
    for t in sorted(done, key=lambda x: x["id"]):
        print(f"   [{t['id']}] status={t['status']} translate_to={t.get('translate_to')!r} path={t['subtitle_path']}")
        print(f"        error={t.get('error')!r}")
    rows = await rows_text(d)
    print(f"   队列行（本次两条在最上）：")
    for r in rows[:2]:
        print("    ·", r.replace("\n", " | "))
    assert len({r for r in rows[:2]}) == 2, f"两条队列行内容一样（产物名撞了）：{rows[:2]}"
    assert any("-to-en" in r for r in rows[:2]) and any("detected.srt" in r and "-to-en" not in r for r in rows[:2]), \
        f"队列里看不到两份产物：{rows[:2]}"
    files = await snapshot_idle(d, 3.0)
    print("   目录产物（静置 3s 后）：", json.dumps(files, ensure_ascii=False))
    for f in files:
        describe(f)

    print("── B. 同语言重跑（同设置再来一次 → 两份都还在、只覆盖自己）")
    before_ids = await all_ids(d)
    label, ids2 = await click_start_and_collect(d, before_ids, 2)
    mine += ids2
    print(f"   开始按钮文案：{label} · 第二轮任务 id：{ids2}")
    done2 = await wait_done(d, ids2)
    for t in sorted(done2, key=lambda x: x["id"]):
        print(f"   [{t['id']}] status={t['status']} translate_to={t.get('translate_to')!r} path={t['subtitle_path']}")
    files2 = await snapshot_idle(d, 3.0)
    print("   重跑前：", json.dumps(files, ensure_ascii=False))
    print("   重跑后：", json.dumps(files2, ensure_ascii=False))
    assert set(files2) == set(files), "重跑后产物集合变了（多出或丢了文件）"
    for f in files2:
        describe(f)

    print("── C. 开关关闭 → 只出一份（译文产物内容一个字节都不动）")
    tr_files = [f for f in files2 if "-to-en" in f]
    tr_before = {f: files2[f] for f in tr_files}
    orig_files = [f for f in files2 if "-to-en" not in f]
    orig_before = {f: files2[f] for f in orig_files}
    print("   翻译开关 =", await ui_translate_off(d))
    before_ids = await all_ids(d)
    label, ids3 = await click_start_and_collect(d, before_ids, 1)
    mine += ids3
    print(f"   开始按钮文案：{label} · 第三轮任务 id：{ids3}")
    done3 = await wait_done(d, ids3)
    for t in done3:
        print(f"   [{t['id']}] status={t['status']} translate_to={t.get('translate_to')!r} path={t['subtitle_path']}")
    files3 = await snapshot_idle(d, 2.0)
    print("   目录产物：", json.dumps(files3, ensure_ascii=False))
    assert set(files3) == set(files2), f"开关关闭后产物集合变了：{set(files3) ^ set(files2)}"
    for f, v in tr_before.items():
        assert files3[f][:2] == v[:2], f"关掉开关的那次跑改了译文产物内容 {f}：{v} → {files3[f]}"
        if files3[f][2] != v[2]:
            print(f"   （提示）译文产物 {f} 的 mtime 变了：{v[2]} → {files3[f][2]}（内容 md5 未变）")
    changed = [f for f in orig_files if files3[f][:2] != orig_before[f][:2]]
    print(f"   ✓ 只更新了原文产物（内容变化：{changed}），译文产物内容一个字节都没动")

    print("── D. 原文语言 = en（会同名的组合）→ 仍是两份、名字不冲突")
    print("   语言 chips：", await ui_pick_lang(d, "en"))
    print("   翻译开关 =", await ui_translate_on(d))
    before_ids = await all_ids(d)
    label, ids4 = await click_start_and_collect(d, before_ids, 2)
    mine += ids4
    print(f"   开始按钮文案：{label} · 第四轮任务 id：{ids4}")
    done4 = await wait_done(d, ids4)
    for t in sorted(done4, key=lambda x: x["id"]):
        print(f"   [{t['id']}] status={t['status']} translate_to={t.get('translate_to')!r} path={t['subtitle_path']}")
    files4 = await snapshot_idle(d, 2.0)
    print("   目录产物：", json.dumps(files4, ensure_ascii=False))
    en_files = [f for f in files4 if f.endswith(".en.srt") or f.endswith(".en-to-en.srt")]
    assert len(en_files) == 2, f"en 组合应该有两份产物，实际 {en_files}"
    assert not any("detected-to-en" == f.split(".")[-3] for f in en_files), "命名串了"
    for f in files4:
        describe(f)

    # 收尾：只删本次自造的任务记录（video_path 指向 .tmp/sub19 的都是我造的），用户库里的记录一条都不动
    subs = await d.invoke("list_subtitles")
    mine = sorted({*(t["id"] for t in subs if "sub19" in (t.get("video_path") or "")), *mine})
    print("── 收尾：删除本次自造的字幕任务记录", mine)
    for i in mine:
        r = await d.invoke("remove_subtitle", json.dumps({"id": i}))
        print(f"   remove_subtitle({i}) →", r)
    left = await d.invoke("list_subtitles")
    print("   剩余字幕记录数：", len(left))
    print("   剩余里是否还有本次自造的：", [t["id"] for t in left if t["id"] in mine])
    await d.js("window.close(); 'ok'")
    await asyncio.sleep(0.5)
    kill_app()
    print("══ 完成")


asyncio.run(main())
