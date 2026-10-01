"""Umidl 下载队列 3 个 bug + 转换预览移除 —— 实机验收（可重复跑，前后对比）。

覆盖：
  1) 完成任务「文件已丢失」误报 / 打开·显示按钮消失 / 任务预览
  2) 同一条链接入队一次却在队列里出现两条一样的
  3) 格式转换队列卡片的「预览」按钮（用户要求移除）

用法：
  python scripts/verify_dqfix.py before     # 记录修复前现象（不断言）
  python scripts/verify_dqfix.py after      # 修复后验收（断言）

产物：.tmp/dqfix/<label>.json + .tmp/dqfix/<label>-*.png
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
DATA = os.path.expandvars(r"%APPDATA%\umi-downloader")
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, ".tmp", "dqfix")
TEST_URL = "https://download.samplelib.com/mp4/sample-5s.mp4"
LABEL = sys.argv[1] if len(sys.argv) > 1 else "before"
EXPECT_FIXED = LABEL == "after"

RESULT = {"label": LABEL, "url": TEST_URL, "checks": []}


def check(name, ok, detail=""):
    RESULT["checks"].append({"name": name, "ok": bool(ok), "detail": str(detail)})
    print(f"  {'✓' if ok else '✗'} {name}" + (f" ｜ {detail}" if detail else ""))
    return bool(ok)


def kill_app():
    subprocess.run(
        ["powershell", "-NoProfile", "-Command",
         "Get-Process umidl -ErrorAction SilentlyContinue | Stop-Process -Force"],
        capture_output=True,
    )
    time.sleep(1.5)


def port_open():
    try:
        requests.get(f"http://127.0.0.1:{PORT}/json/version", timeout=3)
        return True
    except Exception:
        return False


class Driver:
    def __init__(self):
        self.ws = None
        self.n = 1

    async def connect(self):
        for _ in range(60):
            try:
                d = requests.get(f"http://127.0.0.1:{PORT}/json", timeout=3).json()
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

    async def invoke(self, cmd, args=None, **kw):
        payload = json.dumps(kw or args or {})
        out = await self.js(
            "(async () => { try { const r = await window.__TAURI_INTERNALS__.invoke("
            f"{json.dumps(cmd)}, {payload}); return JSON.stringify({{ ok: true, v: r }}); }} "
            "catch (e) { return JSON.stringify({ ok: false, e: String(e) }); } })()"
        )
        data = json.loads(out) if out else {"ok": False, "e": "no result"}
        if not data.get("ok"):
            raise RuntimeError(f"{cmd} 调用失败：{data.get('e')}")
        return data.get("v")

    async def shot(self, name):
        os.makedirs(OUT, exist_ok=True)
        r = await self.cmd("Page.captureScreenshot", format="png")
        p = os.path.join(OUT, f"{LABEL}-{name}.png")
        open(p, "wb").write(base64.b64decode(r["data"]))
        print(f"    [截图] {p}")
        return p

    async def goto(self, hash_, settle=2.2):
        await self.js(f"location.hash = {json.dumps(hash_)}; 'ok'")
        await asyncio.sleep(settle)
        await self.js("window.scrollTo(0,0); 'ok'")


# ── DOM 采样 ────────────────────────────────────────────────────────────
SAMPLE_DL_JS = r"""
(() => {
  const btnTitles = (root) => Array.from(root.querySelectorAll('button'))
      .map(b => b.getAttribute('title') || (b.innerText||'').trim()).filter(Boolean);
  const rows = Array.from(document.querySelectorAll('table tbody tr')).map(tr => ({
    text: (tr.innerText||'').replace(/\s+/g,' ').trim(),
    buttons: btnTitles(tr),
  }));
  const cards = Array.from(document.querySelectorAll('.umi-card'))
      .filter(c => c.querySelector('button'))
      .map(c => ({
        text: (c.innerText||'').replace(/\s+/g,' ').trim(),
        buttons: btnTitles(c),
      }));
  const head = Array.from(document.querySelectorAll('.umi-head'))
      .map(h => (h.innerText||'').replace(/\s+/g,' ').trim())[0] || '';
  const badgeEl = document.querySelector('.umi-head span.tabular-nums');
  const storeLen = badgeEl ? Number((badgeEl.innerText||'').trim()) : null;
  return { rows, cards, head, storeLen };
})()
"""

SAMPLE_CONV_JS = r"""
(() => {
  const cards = Array.from(document.querySelectorAll('.umi-card'))
      .filter(c => c.querySelector('button'))
      .map(c => ({
        text: (c.innerText||'').replace(/\s+/g,' ').trim().slice(0, 220),
        buttons: Array.from(c.querySelectorAll('button')).map(b => b.getAttribute('title') || (b.innerText||'').trim()).filter(Boolean),
      }));
  return { cards };
})()
"""


async def open_check(d):
    """任务预览弹窗：点行内「预览」按钮 → 模态是否出现、内容是什么"""
    clicked = await d.js(
        "(() => { const rows = Array.from(document.querySelectorAll('table tbody tr')); "
        f"const row = rows.find(r => (r.innerText||'').includes('sample-5s')); if (!row) return 'NO_ROW'; "
        "const b = Array.from(row.querySelectorAll('button')).find(x => (x.getAttribute('title')||'') === '预览'); "
        "if (!b) return 'NO_BTN'; b.click(); return 'OK'; })()"
    )
    await asyncio.sleep(1.0)
    modal = await d.js(
        "(() => { const m = document.querySelector('.n-modal'); "
        "return m ? (m.innerText||'').replace(/\\s+/g,' ').trim().slice(0,400) : ''; })()"
    )
    await d.js(
        "(() => { const b = document.querySelector('.n-modal .n-card-header__close, .n-modal [class*=close]'); "
        "if (b) b.click(); return 'ok'; })()"
    )
    await asyncio.sleep(0.4)
    return clicked, modal


async def main():
    kill_app()
    if port_open():
        print("9222 仍被占用，先等待释放…")
        time.sleep(3)
    os.makedirs(OUT, exist_ok=True)

    env = dict(os.environ)
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = f"--remote-debugging-port={PORT}"
    subprocess.Popen([APP], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    d = Driver()
    ok = await d.connect()
    check("应用启动 + CDP 连接（9222）", ok)
    if not ok:
        return
    await asyncio.sleep(2.5)
    ver = await d.js("(document.body.innerText.match(/v1\\.[0-9]+\\.[0-9]+/)||[''])[0]")
    check("版本号 1.8.1", ver == "v1.8.1", str(ver))

    # ── 队列基线 ────────────────────────────────────────────────
    await d.goto("#/download")
    # 清掉上一轮遗留的同 URL 测试任务（只删记录，不动用户自己的任务与磁盘文件）
    first = await d.invoke("list_downloads")
    leftovers = [t for t in first if TEST_URL in (t.get("url") or "")]
    for t in leftovers:
        await d.invoke("remove_download", {"id": t["id"], "deleteFile": False})
    if leftovers:
        await d.js(
            "(() => { const b = Array.from(document.querySelectorAll('button'))"
            ".find(x => (x.getAttribute('title')||'') === '刷新列表'); if (b) b.click(); return 'ok'; })()"
        )
        await asyncio.sleep(2.0)
        print(f"    已清理上一轮遗留的 {len(leftovers)} 条同 URL 测试任务")

    before_db = await d.invoke("list_downloads")
    before_dom = await d.js(SAMPLE_DL_JS)
    print(f"    基线：DB {len(before_db)} 条 / store {before_dom['storeLen']} / 表格行 {len(before_dom['rows'])}")
    RESULT["baseline"] = {"db": len(before_db), "store": before_dom["storeLen"], "rows": len(before_dom["rows"])}

    # ── 入队：粘贴链接 → 解析 → 开始下载 ────────────────────────
    await d.js(
        "(() => { const el = document.querySelector('[data-test=\"link-input\"]'); "
        "const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set; "
        f"set.call(el, {json.dumps(TEST_URL)}); el.dispatchEvent(new Event('input', {{bubbles:true}})); return 'ok'; }})()"
    )
    await asyncio.sleep(0.6)
    await d.js("document.querySelector('[data-test=\"link-primary\"]').click(); 'ok'")
    probe_ok = False
    for _ in range(60):
        await asyncio.sleep(1.0)
        if await d.js("!!document.querySelector('[data-test=\"probe-result\"]')"):
            probe_ok = True
            break
    check("解析成功（出现解析结果卡）", probe_ok)
    await d.shot("1-probed")

    await d.js("document.querySelector('[data-test=\"link-start\"]').click(); 'ok'")
    await asyncio.sleep(4.0)
    after_db = await d.invoke("list_downloads")
    after_dom = await d.js(SAMPLE_DL_JS)
    new_db = [t for t in after_db if TEST_URL in (t.get("url") or "")]
    mine_rows = [r for r in after_dom["rows"] if "sample-5s" in r["text"]]
    mine_cards = [c for c in after_dom["cards"] if "sample-5s" in c["text"]]
    print(
        f"    入队后：DB 新增 {len(new_db)} 条 / 界面表格行 {len(mine_rows)} 条 / 卡片 {len(mine_cards)} 条 / "
        f"store {after_dom['storeLen']} / DB 总 {len(after_db)}"
    )
    RESULT["enqueue"] = {
        "db_new": len(new_db),
        "dom_rows": len(mine_rows),
        "dom_cards": len(mine_cards),
        "store_len": after_dom["storeLen"],
        "db_total": len(after_db),
        "row_texts": [r["text"][:160] for r in mine_rows],
        "row_buttons": [r["buttons"] for r in mine_rows],
    }
    check("后端只落 1 行（同一条链接）", len(new_db) == 1, f"db_new={len(new_db)}")
    check(
        "界面不出现两条一样的任务（入队后 4s）",
        len(mine_rows) == 1,
        f"表格行={len(mine_rows)}（修复前=2）" if not EXPECT_FIXED else f"表格行={len(mine_rows)}",
    )
    await d.shot("2-after-enqueue")

    # ── 等下载完成，复查「文件已丢失」与按钮 ────────────────────
    task_id = new_db[0]["id"] if new_db else None
    done = False
    for _ in range(180):
        await asyncio.sleep(2.0)
        rows = await d.invoke("list_downloads")
        cur = [t for t in rows if t["id"] == task_id]
        if cur and cur[0]["status"] in ("done", "error", "canceled", "handed_off"):
            done = cur[0]["status"] == "done"
            RESULT["task"] = {k: cur[0].get(k) for k in ("id", "status", "file_path", "file_exists", "title", "error")}
            break
    check("下载任务完成（status=done）", done, json.dumps(RESULT.get("task", {}), ensure_ascii=False))
    await asyncio.sleep(2.5)

    # 后端真相：磁盘上文件是否存在
    fp = (RESULT.get("task") or {}).get("file_path") or ""
    disk_exists = os.path.isfile(fp) if fp else False
    RESULT["disk_exists"] = disk_exists

    dom2 = await d.js(SAMPLE_DL_JS)
    rows2 = [r for r in dom2["rows"] if "sample-5s" in r["text"]]
    cards2 = [c for c in dom2["cards"] if "sample-5s" in c["text"]]
    RESULT["after_done"] = {"rows": rows2, "cards": cards2, "store_len": dom2["storeLen"]}
    row_buttons = rows2[0]["buttons"] if rows2 else []
    card_buttons = cards2[0]["buttons"] if cards2 else []
    lost_row = any("文件已丢失" in r["text"] for r in rows2)
    lost_card = any("文件已丢失" in c["text"] for c in cards2)
    print(f"    完成后：表格按钮 {row_buttons}")
    print(f"    完成后：卡片按钮 {card_buttons}")
    print(f"    完成后：表格行文本 {[r['text'][:120] for r in rows2]}")

    check("文件在磁盘上（后端真相）", disk_exists, fp)
    check("完成后事件里的 file_exists 与磁盘一致", bool((RESULT.get("task") or {}).get("file_exists")) == disk_exists,
          f"file_exists={(RESULT.get('task') or {}).get('file_exists')}")
    check("界面不误报「文件已丢失」", (not lost_row) and (not lost_card), f"表格={lost_row} 卡片={lost_card}")
    check("「打开文件」按钮在", "打开文件" in row_buttons, str(row_buttons))
    check("「在文件夹中显示」按钮在", "在文件夹中显示" in row_buttons, str(row_buttons))
    await d.shot("3-done-table")

    # 任务预览弹窗
    clicked, modal = await open_check(d)
    RESULT["preview_modal"] = {"clicked": clicked, "modal": modal}
    check("任务预览弹窗可用（含打开文件入口）", clicked == "OK" and "打开文件" in modal, f"{clicked} / {modal[:120]}")
    await d.shot("4-preview-modal")

    # 卡片布局复查（用户的「下载队列卡片」视角）
    await d.js(
        "(() => { const b = Array.from(document.querySelectorAll('button'))"
        ".find(x => (x.getAttribute('title')||'') === '卡片模式'); if (b) b.click(); return 'ok'; })()"
    )
    await asyncio.sleep(1.5)
    dom3 = await d.js(SAMPLE_DL_JS)
    cards3 = [c for c in dom3["cards"] if "sample-5s" in c["text"]]
    # 任务卡片（带状态词的那几张）—— 同一条任务只允许出现一张
    statusRe = ("排队中", "解析中", "下载中", "已暂停", "已完成", "失败", "已取消", "已交给引擎", "部分完成", "转换中")
    task_cards3 = [c for c in cards3 if any(w in c["text"] for w in statusRe)]
    RESULT["card_layout"] = cards3
    RESULT["card_layout_task_count"] = len(task_cards3)
    RESULT["card_layout_store_len"] = dom3["storeLen"]
    print(f"    卡片布局：store={dom3['storeLen']} / 含 sample-5s 的卡片 {len(cards3)} / 其中任务卡 {len(task_cards3)}")
    check("卡片布局：同一条任务只有一张卡", len(task_cards3) == 1, f"任务卡 {len(task_cards3)} 张")
    check("卡片布局：队列计数与列表一致", dom3["storeLen"] == len(before_db) + 1,
          f"store={dom3['storeLen']} / 基线 DB {len(before_db)} + 1")
    check("卡片布局：不误报文件已丢失", not any("文件已丢失" in c["text"] for c in cards3),
          str([c["text"][:120] for c in cards3]))
    check("卡片布局：仍有「打开文件」按钮", any("打开文件" in c["buttons"] for c in task_cards3),
          str([c["buttons"] for c in task_cards3]))
    await d.shot("5-done-card")
    # 还原用户设置（详细列表）
    await d.js(
        "(() => { const b = Array.from(document.querySelectorAll('button'))"
        ".find(x => (x.getAttribute('title')||'') === '详细列表'); if (b) b.click(); return 'ok'; })()"
    )
    await asyncio.sleep(1.2)

    # ── 转换页：队列卡「预览」按钮 ──────────────────────────────
    await d.goto("#/converter")
    conv_db = await d.invoke("list_converts")
    dom_conv = await d.js(SAMPLE_CONV_JS)
    conv_cards = [c for c in dom_conv["cards"] if "→" in c["text"]]
    RESULT["converter"] = {"db": len(conv_db), "cards": conv_cards}
    conv_buttons = [b for c in conv_cards for b in c["buttons"]]
    print(f"    转换队列：DB {len(conv_db)} 条 / 卡片 {len(conv_cards)} 张 / 按钮 {conv_buttons}")
    has_preview = "预览" in conv_buttons
    check("转换队列卡片没有「预览」按钮", not has_preview, str(conv_buttons))
    check("转换队列卡片仍有「打开文件」按钮", "打开文件" in conv_buttons, str(conv_buttons))
    # 截图前把队列卡片滚进视野
    await d.js(
        "(() => { const cards = Array.from(document.querySelectorAll('.umi-card'))"
        ".filter(c => (c.innerText||'').includes('→')); if (cards[0]) cards[0].scrollIntoView({block:'center'}); return 'ok'; })()"
    )
    await asyncio.sleep(0.8)
    await d.shot("6-converter-queue")

    # i18n：转换模块相关文案
    keys = await d.js(
        "(async () => { const r = await window.__TAURI_INTERNALS__.invoke('get_settings'); return 'ok'; })()"
    )
    del keys

    # ── 收尾 ────────────────────────────────────────────────────
    await d.js("window.close(); 'ok'")
    await asyncio.sleep(1.0)
    kill_app()
    closed = not port_open()
    check("关闭后 9222 已释放 + 进程已退出", closed)

    with open(os.path.join(OUT, f"{LABEL}.json"), "w", encoding="utf-8") as f:
        json.dump(RESULT, f, ensure_ascii=False, indent=2)
    bad = [c for c in RESULT["checks"] if not c["ok"]]
    print(f"\n══ {LABEL} 结果：{len(RESULT['checks']) - len(bad)}/{len(RESULT['checks'])} 通过")
    for c in bad:
        print(f"   ✗ {c['name']} ｜ {c['detail']}")
    print(f"   证据：{os.path.join(OUT, LABEL + '.json')}")


asyncio.run(main())
