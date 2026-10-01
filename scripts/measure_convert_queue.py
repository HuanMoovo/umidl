"""转换队列尺寸度量（CDP 9222）—— 改造前 / 改造后同口径对比。

用法：
  python scripts/measure_convert_queue.py before --launch --shot out/queue-before.png
  python scripts/measure_convert_queue.py after  --launch --shot out/queue-after.png
  python scripts/measure_convert_queue.py download --route '#/download'          # 下载页不受影响核对
  python scripts/measure_convert_queue.py kill

度量口径（前后完全一致）：
  - 队列容器：包含「转换队列」标题的那一段 section 的最后一个子元素（滚动容器）
  - 行：容器内 [data-role="convert-row"]（新）或 .umi-card（旧卡片版）
  - 行高：getBoundingClientRect().height（min / max / 平均 / 首行）
  - 页面：main.scrollHeight / clientHeight（页面总滚动高度）
  - 其它：队列里 <img> 数量、含「预览」字样的次数、首行按钮 title 列表
输出 JSON 到 stdout，并写一份到 out/queue-<标签>.json。
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

APP = r"C:\Users\user\AppData\Local\Umidl\umidl.exe"
PORT = 9222

PROBE = r"""
(() => {
  const norm = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const secs = Array.from(document.querySelectorAll('section'));
  const q = secs.find((s) => {
    const h = s.firstElementChild;
    return h && norm(h.textContent).includes('转换队列');
  });
  if (!q) {
    return { error: '未找到「转换队列」section',
             sections: secs.map((s) => norm(s.firstElementChild && s.firstElementChild.textContent).slice(0, 24)) };
  }
  const list = q.lastElementChild;
  let rows = Array.from(list.querySelectorAll('[data-role="convert-row"]'));
  let kind = 'convert-row';
  if (!rows.length) {
    rows = Array.from(list.querySelectorAll('.umi-card'));
    kind = 'umi-card';
  }
  const hs = rows.map((r) => Math.round(r.getBoundingClientRect().height * 10) / 10);
  const main = document.querySelector('main');
  const sum = hs.reduce((a, b) => a + b, 0);
  return {
    kind,
    route: location.hash,
    rows: rows.length,
    row_heights: {
      min: hs.length ? Math.min.apply(null, hs) : null,
      max: hs.length ? Math.max.apply(null, hs) : null,
      avg: hs.length ? Math.round((sum / hs.length) * 10) / 10 : null,
      first: hs.length ? hs[0] : null,
    },
    list: {
      tag: list.tagName,
      cls: String(list.className).slice(0, 100),
      scrollHeight: list.scrollHeight,
      clientHeight: list.clientHeight,
      overflowY: getComputedStyle(list).overflowY,
      height: Math.round(list.getBoundingClientRect().height),
    },
    queue_section: {
      height: Math.round(q.getBoundingClientRect().height),
      scrollHeight: q.scrollHeight,
    },
    main: {
      scrollHeight: main.scrollHeight,
      clientHeight: main.clientHeight,
      overflowY: getComputedStyle(main).overflowY,
    },
    imgs_in_queue: list.querySelectorAll('img').length,
    preview_mentions: (norm(list.textContent).match(/预览/g) || []).length,
    row_buttons_first: Array.from(rows[0] ? rows[0].querySelectorAll('button') : []).map(
      (b) => b.getAttribute('title') || norm(b.textContent)
    ),
    rows_missing_name_or_actions: rows.filter(
      (r) => r.querySelectorAll('button').length < 3 || norm(r.textContent).length < 3
    ).length,
    row_texts: rows.slice(0, 3).map((r) => norm(r.textContent).slice(0, 160)),
  };
})()
"""

DL_PROBE = r"""
(() => {
  const norm = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const cards = Array.from(document.querySelectorAll('.umi-card'));
  const cardish = cards.filter((c) => c.querySelector('button'));
  const heights = cardish.map((c) => Math.round(c.getBoundingClientRect().height * 10) / 10);
  const sec = Array.from(document.querySelectorAll('section')).find((s) => {
    const h = s.firstElementChild;
    return h && norm(h.textContent).includes('下载队列');
  });
  const list = sec && sec.lastElementChild;
  return {
    route: location.hash,
    umi_cards: cards.length,
    cards_with_buttons: cardish.length,
    card_heights: { min: heights.length ? Math.min.apply(null, heights) : null,
                    max: heights.length ? Math.max.apply(null, heights) : null },
    thumbnails: document.querySelectorAll('img').length,
    table_rows: document.querySelectorAll('table tbody tr').length,
    layout_badge: norm((document.querySelector('.umi-seg .chip-active') || {}).textContent || ''),
    dl_queue: list ? {
      tag: list.tagName,
      cls: String(list.className).slice(0, 60),
      children: list.children.length,
      cards: list.querySelectorAll('.umi-card').length,
      table_rows: list.querySelectorAll('table tbody tr').length,
      imgs: list.querySelectorAll('img').length,
      card_heights: Array.from(list.querySelectorAll('.umi-card')).map(
        (c) => Math.round(c.getBoundingClientRect().height * 10) / 10
      ),
      first_row_text: norm((list.querySelector('.umi-card') || list.querySelector('tbody tr') || {}).textContent).slice(0, 120),
    } : null,
  };
})()
"""


def kill_app():
    subprocess.run(
        ["powershell", "-NoProfile", "-Command", "Stop-Process -Name umidl -Force -ErrorAction SilentlyContinue"],
        check=False,
    )
    time.sleep(2)


def launch():
    env = dict(os.environ)
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = f"--remote-debugging-port={PORT}"
    subprocess.Popen([APP], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def port_open():
    try:
        r = requests.get(f"http://127.0.0.1:{PORT}/json/version", timeout=3)
        return r.status_code == 200
    except Exception:
        return False


async def send(ws, state, method, **params):
    mid = state[0]
    state[0] += 1
    await ws.send(json.dumps({"id": mid, "method": method, "params": params}))
    while True:
        msg = json.loads(await ws.recv())
        if msg.get("id") == mid:
            if "error" in msg:
                raise SystemExit(f"CDP 错误：{msg['error']}")
            return msg.get("result", {})


async def ev(ws, state, expr):
    r = await send(ws, state, "Runtime.evaluate", expression=expr, returnByValue=True, awaitPromise=True)
    return r.get("result", {}).get("value")


async def run():
    label = sys.argv[1] if len(sys.argv) > 1 else "probe"
    args = sys.argv[2:]
    route, shot, do_launch, do_kill, probe_name, dl_layout = "#/converter", None, False, False, "queue", None
    for i, a in enumerate(args):
        if a == "--shot":
            shot = args[i + 1]
        elif a == "--route":
            route = args[i + 1]
        elif a == "--launch":
            do_launch = True
        elif a == "--kill":
            do_kill = True
        elif a == "--probe":
            probe_name = args[i + 1]
        elif a == "--dl-layout":
            dl_layout = args[i + 1]

    if do_kill:
        kill_app()
        print(f"[关闭] 应用已结束，9222 释放 = {not port_open()}")
        return

    if do_launch:
        kill_app()
        launch()
        for _ in range(40):
            if port_open():
                break
            time.sleep(1)
        if not port_open():
            raise SystemExit("启动后 9222 未就绪：应用没起来或没带上调试端口")
        time.sleep(4)
        print("[启动] 调试实例已就绪（9222）")

    d = requests.get(f"http://127.0.0.1:{PORT}/json", timeout=8).json()
    pages = [t for t in d if t.get("type") == "page"]
    if not pages:
        raise SystemExit("9222 上没有页面目标")
    print(f"[目标] {pages[0].get('title')} :: {pages[0].get('url')[:70]}")

    async with websockets.connect(pages[0]["webSocketDebuggerUrl"], max_size=80 * 1024 * 1024) as ws:
        state = [1]
        await send(ws, state, "Runtime.enable")
        await ev(ws, state, f"location.hash = {json.dumps(route)}; true")
        await asyncio.sleep(1.0)
        if dl_layout:
            changed = await ev(
                ws,
                state,
                "(() => { const want = %s; const all = Array.from(document.querySelectorAll('button')); "
                "const b = all.find(x => (x.getAttribute('title') || '') === want) || "
                "all.find(x => (x.textContent || '').replace(/\\s+/g, '') === want); if (!b) return false; "
                "b.click(); return true; })()" % json.dumps(dl_layout),
            )
            print(f"[下载页布局] 切换到「{dl_layout}」= {changed}")
            await asyncio.sleep(1.5)
        expr = PROBE if probe_name == "queue" else DL_PROBE
        data = None
        for _ in range(40):
            await asyncio.sleep(0.5)
            data = await ev(ws, state, expr)
            if isinstance(data, dict) and (data.get("rows") or data.get("umi_cards") is not None and probe_name == "download"):
                break
        await asyncio.sleep(1.5)
        data = await ev(ws, state, expr)
        data["window"] = await ev(
            ws, state, "({innerWidth: innerWidth, innerHeight: innerHeight, dpr: devicePixelRatio})"
        )
        data["label"] = label

        if probe_name == "queue":
            await ev(
                ws,
                state,
                "(() => { const s = Array.from(document.querySelectorAll('section')).find(x => ((x.firstElementChild && x.firstElementChild.textContent) || '').includes('转换队列')); if (s) s.scrollIntoView({block: 'start'}); return !!s; })()",
            )
            await asyncio.sleep(0.8)
        if shot:
            os.makedirs(os.path.dirname(shot) or ".", exist_ok=True)
            r = await send(ws, state, "Page.captureScreenshot", format="png")
            with open(shot, "wb") as fh:
                fh.write(base64.b64decode(r["data"]))
            print(f"[截图] {shot}")

    print(json.dumps(data, ensure_ascii=False, indent=2))
    os.makedirs("out", exist_ok=True)
    with open(os.path.join("out", f"{probe_name}-{label}.json"), "w", encoding="utf-8") as fh:
        json.dump(data, fh, ensure_ascii=False, indent=2)


asyncio.run(run())
