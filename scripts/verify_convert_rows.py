"""安装版真机验收：转换队列「简约行列表」（CDP 9222）。

前置：用 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 启动安装版 umidl.exe
（可先跑 `python scripts/measure_convert_queue.py after --launch`）。

断言：
  1) 队列行数 == 后端 list_converts 记录数（真机 15 条）
  2) 每行都有文件名 + 至少一个操作按钮（删除记录），完成后在盘上的还有 打开文件/在文件夹中显示
  3) 行内没有任何缩略图 / 预览入口（无 img、无缩略图占位块、无标题为「预览」的按钮）
  4) 单行高度 ≤ 40px；队列容器自己内滚（overflow-y auto + max-height），main 高度不再随队列变长
  5) 下载页卡片不受影响（卡片布局下仍是 TaskCard：有缩略图与预览按钮）

用法： python scripts/verify_convert_rows.py [--shot 前缀]
"""
import asyncio
import base64
import json
import os
import sys

import requests
import websockets

PORT = 9222
RESULTS = []


def check(name, ok, detail=""):
    RESULTS.append((name, bool(ok), detail))
    print(f"{'✅' if ok else '❌'} {name}" + (f" — {detail}" if detail else ""))
    return ok


PROBE_QUEUE = r"""
(async () => {
  const norm = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const inv = window.__TAURI_INTERNALS__.invoke;
  let db = [];
  try { db = await inv('list_converts'); } catch (e) { db = []; }
  const box = document.querySelector('[data-role="convert-queue"]');
  const rows = box ? Array.from(box.querySelectorAll('[data-role="convert-row"]')) : [];
  const hs = rows.map((r) => Math.round(r.getBoundingClientRect().height * 10) / 10);
  const main = document.querySelector('main');
  const cs = box ? getComputedStyle(box) : null;
  // 页面（转换页）里所有「预览」相关入口（按钮标题或按钮文案）
  const isPreviewBtn = (b) => {
    const t = b.getAttribute('title') || '';
    return t === '预览' || norm(b.textContent) === '预览';
  };
  const previewButtons = Array.from(document.querySelectorAll('button')).filter(isPreviewBtn).length;
  return {
    db_count: db.length,
    rows: rows.length,
    row_heights: hs,
    row_height_max: hs.length ? Math.max.apply(null, hs) : null,
    every_row_named: rows.every((r) => norm(r.textContent).length > 3),
    row_buttons: rows.map((r) => Array.from(r.querySelectorAll('button')).map((b) => b.getAttribute('title') || norm(b.textContent))),
    rows_without_any_button: rows.filter((r) => !r.querySelector('button')).length,
    rows_without_delete: rows.filter((r) => !Array.from(r.querySelectorAll('button')).some((b) => (b.getAttribute('title') || '') === '删除记录')).length,
    imgs_in_rows: rows.reduce((n, r) => n + r.querySelectorAll('img').length, 0),
    thumb_blocks: rows.reduce((n, r) => n + r.querySelectorAll('div[class*="s-sunken"]').length, 0),
    preview_mentions_in_rows: rows.reduce((n, r) => n + ((norm(r.textContent).match(/预览/g) || []).length), 0),
    preview_buttons_on_page: previewButtons,
    card_class_in_rows: rows.filter((r) => (r.className || '').includes('umi-card')).length,
    box: box ? {
      scrollHeight: box.scrollHeight,
      clientHeight: box.clientHeight,
      overflowY: cs.overflowY,
      maxHeight: cs.maxHeight,
      height: Math.round(box.getBoundingClientRect().height),
      cls: String(box.className),
    } : null,
    main: { scrollHeight: main.scrollHeight, clientHeight: main.clientHeight },
    row_texts: rows.slice(0, 2).map((r) => norm(r.textContent).slice(0, 120)),
    umi_cards_on_converter_page: document.querySelectorAll('.umi-card').length,
  };
})()
"""

PROBE_DOWNLOAD = r"""
(() => {
  const norm = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const seg = Array.from(document.querySelectorAll('button'));
  const cardChip = seg.find((b) => (b.getAttribute('title') || '') === '卡片模式');
  const tableChip = seg.find((b) => (b.getAttribute('title') || '') === '详细列表');
  const before_clicked_card = !!cardChip;
  if (cardChip) cardChip.click();
  return new Promise((resolve) => {
    setTimeout(() => {
      const sec = Array.from(document.querySelectorAll('section')).find((s) => {
        const h = s.firstElementChild;
        return h && norm(h.textContent).includes('下载队列');
      });
      const list = sec && sec.lastElementChild;
      const cards = list ? Array.from(list.querySelectorAll('.umi-card')) : [];
      const hs = cards.map((c) => Math.round(c.getBoundingClientRect().height * 10) / 10);
      const out = {
        cards: cards.length,
        card_heights: hs,
        imgs: list ? list.querySelectorAll('img').length : 0,
        preview_buttons: 0,
        body_text: cards.length ? norm(cards[0].textContent).slice(0, 120) : '',
      };
      // 下载卡片的「预览」按钮是文案按钮（没有 title）
      out.preview_buttons = cards.reduce(
        (n, c) =>
          n +
          Array.from(c.querySelectorAll('button')).filter(
            (b) => (b.getAttribute('title') || '') === '预览' || norm(b.textContent) === '预览'
          ).length,
        0
      );
      if (tableChip) tableChip.click();
      resolve(out);
    }, 1200);
  });
})()
"""


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
    res = r.get("result", {})
    if res.get("subtype") == "error":
        raise SystemExit(f"JS 异常：{res.get('description')}")
    return res.get("value")


async def main():
    shot_prefix = None
    if "--shot" in sys.argv:
        shot_prefix = sys.argv[sys.argv.index("--shot") + 1]
    pages = [t for t in requests.get(f"http://127.0.0.1:{PORT}/json", timeout=8).json() if t.get("type") == "page"]
    if not pages:
        raise SystemExit("9222 上没有页面目标")

    async with websockets.connect(pages[0]["webSocketDebuggerUrl"], max_size=80 * 1024 * 1024) as ws:
        state = [1]
        await send(ws, state, "Runtime.enable")

        # ── 转换页 ─────────────────────────────────────────────
        await ev(ws, state, "location.hash = '#/converter'; true")
        await asyncio.sleep(5)
        q = await ev(ws, state, PROBE_QUEUE)
        print(json.dumps(q, ensure_ascii=False, indent=2))
        if shot_prefix:
            os.makedirs(os.path.dirname(shot_prefix) or ".", exist_ok=True)
            r = await send(ws, state, "Page.captureScreenshot", format="png")
            with open(f"{shot_prefix}-converter.png", "wb") as fh:
                fh.write(base64.b64decode(r["data"]))

        check("队列行数 = 后端记录数", q["rows"] == q["db_count"] and q["rows"] > 0,
              f"DOM {q['rows']} 行 / list_converts {q['db_count']} 条")
        check("单行高度 ≤ 40px", q["row_height_max"] is not None and q["row_height_max"] <= 40,
              f"最高 {q['row_height_max']}px（全行 {q['row_heights'][:3]}…）")
        check("每行都有文件名", q["every_row_named"])
        check("每行都有操作按钮", q["rows_without_any_button"] == 0 and q["rows_without_delete"] == 0,
              f"无按钮 {q['rows_without_any_button']} 行 / 无「删除记录」{q['rows_without_delete']} 行")
        check("行内无缩略图（img / 占位块）", q["imgs_in_rows"] == 0 and q["thumb_blocks"] == 0,
              f"img {q['imgs_in_rows']} / s-sunken {q['thumb_blocks']}")
        check("行内无预览入口", q["preview_mentions_in_rows"] == 0 and q["preview_buttons_on_page"] == 0,
              f"行内「预览」字样 {q['preview_mentions_in_rows']} / 页面「预览」按钮 {q['preview_buttons_on_page']}")
        check("行不再是卡片", q["card_class_in_rows"] == 0)
        box = q["box"] or {}
        check("队列容器自己内滚", box.get("overflowY") == "auto" and box.get("maxHeight") not in (None, "none"),
              f"overflow-y={box.get('overflowY')} max-height={box.get('maxHeight')} "
              f"scrollHeight={box.get('scrollHeight')} clientHeight={box.get('clientHeight')}")
        check("页面高度不再随队列变长", box.get("scrollHeight", 0) > box.get("clientHeight", 0),
              f"main scrollHeight={q['main']['scrollHeight']} clientHeight={q['main']['clientHeight']}")

        # ── 下载页（卡片不受影响） ────────────────────────────────
        await ev(ws, state, "location.hash = '#/download'; true")
        await asyncio.sleep(3)
        d = await ev(ws, state, PROBE_DOWNLOAD)
        print(json.dumps(d, ensure_ascii=False, indent=2))
        await asyncio.sleep(1.0)
        if shot_prefix:
            r = await send(ws, state, "Page.captureScreenshot", format="png")
            with open(f"{shot_prefix}-download-card.png", "wb") as fh:
                fh.write(base64.b64decode(r["data"]))
        check("下载页仍是卡片（TaskCard 未动）", d["cards"] > 0 and d["imgs"] > 0 and d["preview_buttons"] > 0,
              f"卡片 {d['cards']} 张 / 缩略图 {d['imgs']} 个 / 预览按钮 {d['preview_buttons']} 个 / 高 {d['card_heights']}")

    passed = sum(1 for _, ok, _ in RESULTS if ok)
    print(f"\n验收结果：{passed}/{len(RESULTS)} 通过")
    if passed != len(RESULTS):
        sys.exit(1)


asyncio.run(main())
