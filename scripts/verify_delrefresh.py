"""删除记录刷新缺陷 —— 实机取证（CDP 9222，安装版 / dev 版同一套口径）。

口径：一次快照同时读三组数量
  store   —— pinia tasks store 的三条队列长度（DOM 里没有的也算，直接读状态）
  dom     —— 当前页面队列真实渲染的行数（下载页 <tbody tr> / 转换·字幕页 data-role 行）
  backend —— invoke('list_downloads'|'list_converts'|'list_subtitles') 的真实长度（磁盘库）
三者一致才叫「删完就刷新」。删除动作一律点界面上的「删除记录」按钮（走真实代码路径）。

修复前 / 修复后的原始证据见 .tmp/delfix/*.json（subtitle 9/9/9 → 9/9/8 是旧版的不刷新；
修复后 → 8/8/8）。三页各点一次行内「删除记录」，每次都读 store / DOM / 后端三者数量。

用法：
  python scripts/verify_delrefresh.py before                    # 安装版（旧前端）取证
  python scripts/verify_delrefresh.py after --exe <exe 路径>     # 修复后取证（默认 = 安装版）
  python scripts/verify_delrefresh.py seed                      # 只补测试记录
  python scripts/verify_delrefresh.py clean                     # 只清测试记录（含残留）

启动方式（二选一，都要带调试端口）：
  * 安装版：export WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222
            %LOCALAPPDATA%/Umidl/umidl.exe
  * dev 实例（前端走 Vite，改完源码不重新打包也能验）：npx vite --port 1420
            src-tauri/target/debug/umidl.exe

测试记录 id 一律以 zzdel- 开头，收尾务必跑 clean；绝不碰用户原有记录。

产物：.tmp/delfix/<label>.json + .tmp/delfix/<label>-<page>.png
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

PORT = 9222
INSTALLED = os.path.expandvars(r"%LOCALAPPDATA%\Umidl\umidl.exe")
DB = os.path.expandvars(r"%APPDATA%\umi-downloader\umi.db")
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, ".tmp", "delfix")
DIR = r"D:\Media Files\Videos\Umidl"

# 自己造的测试记录（id 前缀 zzdel，方便收尾一次清干净；绝不碰用户记录）
ROWS = {
    "downloads": ("zzdel-dl", "zzdeltest-download", dict(
        title="zzdeltest-download-请删除我",
        url="https://example.com/zzdeltest-download",
        status="done", progress=100.0, file_path=DIR + r"\zzdeltest-download.mp4",
    )),
    "converts": ("zzdel-cv", "zzdeltest-convert", dict(
        input_file=DIR + r"\zzdeltest-convert.mp4", output_file=DIR + r"\zzdeltest-convert.mkv",
        format="mkv", status="done", progress=100.0,
    )),
    "subtitles": ("zzdel-st", "zzdeltest-subtitle", dict(
        video_path=DIR + r"\zzdeltest-subtitle.mp4", language="zh", model="base",
        subtitle_path=DIR + r"\zzdeltest-subtitle.zh.srt", status="done", progress=100.0,
    )),
}


def seed() -> None:
    con = sqlite3.connect(DB)
    now = int(time.time() * 1000)
    for table, (tid, _marker, fields) in ROWS.items():
        con.execute(f"DELETE FROM {table} WHERE id = ?", (tid,))
        fields = dict(fields, id=tid, created_time=now)
        cols = ", ".join(fields)
        marks = ", ".join("?" * len(fields))
        con.execute(f"INSERT INTO {table} ({cols}) VALUES ({marks})", list(fields.values()))
    con.commit()
    con.close()
    print(f"[seed] 已补 3 条测试记录（{', '.join(r[0] for r in ROWS.values())}）")


def clean() -> None:
    con = sqlite3.connect(DB)
    for table, (tid, _m, _f) in ROWS.items():
        cur = con.execute(f"DELETE FROM {table} WHERE id LIKE 'zzdel-%' OR id = ?", (tid,))
        print(f"[clean] {table}: 删掉 {cur.rowcount} 条（含残留）")
    con.commit()
    con.close()


def kill_app() -> None:
    subprocess.run(["powershell", "-NoProfile", "-Command",
                    "Get-Process umidl -ErrorAction SilentlyContinue | Stop-Process -Force"],
                   capture_output=True)
    time.sleep(1.5)


def port_ready() -> bool:
    try:
        return requests.get(f"http://127.0.0.1:{PORT}/json/version", timeout=3).status_code == 200
    except Exception:
        return False


def launch(exe: str) -> None:
    env = dict(os.environ)
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = f"--remote-debugging-port={PORT}"
    subprocess.Popen([exe], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


SNAP_JS = r"""
(async () => {
  const inv = (c) => window.__TAURI_INTERNALS__.invoke(c);
  const q = (sel) => document.querySelectorAll(sel).length;
  let store = null;
  try {
    const app = document.querySelector('#app').__vue_app__;
    const pinia = app.config.globalProperties.$pinia;
    const st = pinia._s.get('tasks');
    store = { dl: st.downloads.length, cv: st.converts.length, sub: st.subtitles.length };
  } catch (e) { store = { error: String(e) }; }
  const [d, c, s] = await Promise.all([inv('list_downloads'), inv('list_converts'), inv('list_subtitles')]);
  const dom = {
    dl: q('table tbody tr'),
    cv: q('[data-role="convert-queue"] [data-role="convert-row"]'),
    sub: q('[data-role="subtitle-queue"] [data-role="subtitle-row"]'),
  };
  const txt = document.body.innerText || '';
  return JSON.stringify({
    route: location.hash,
    url: location.href,
    store, dom,
    backend: { dl: d.length, cv: c.length, sub: s.length },
    ids: {
      dl: d.some((t) => t.id === 'zzdel-dl'),
      cv: c.some((t) => t.id === 'zzdel-cv'),
      sub: s.some((t) => t.id === 'zzdel-st'),
    },
    marker_in_dom: {
      dl: (document.body.innerHTML || '').includes('zzdeltest-download'),
      cv: (document.body.innerHTML || '').includes('zzdeltest-convert'),
      sub: (document.body.innerHTML || '').includes('zzdeltest-subtitle'),
    },
    has_marker_text: txt.includes('zzdeltest-download') || txt.includes('zzdeltest-convert') || txt.includes('zzdeltest-subtitle'),
  });
})()
"""


def click_js(page: str, marker: str) -> str:
    """点某一行最右边的「删除记录」按钮（两个行组件的删除按钮都是最后一个）。"""
    sel = {
        "download": "table tbody tr",
        "converter": '[data-role="convert-queue"] [data-role="convert-row"]',
        "subtitle": '[data-role="subtitle-queue"] [data-role="subtitle-row"]',
    }[page]
    return """
(() => {
  const rows = [...document.querySelectorAll(%s)];
  const row = rows.find((r) => (r.textContent || '').includes(%s));
  if (!row) return 'ROW_NOT_FOUND';
  const btns = [...row.querySelectorAll('button')];
  if (!btns.length) return 'NO_BUTTON';
  const del = btns[btns.length - 1];
  const title = del.getAttribute('title') || '';
  del.click();
  return 'CLICKED[' + title + ']';
})()
""" % (json.dumps(sel), json.dumps(marker))


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
                    self.ws = await websockets.connect(pages[0]["webSocketDebuggerUrl"], max_size=64 * 1024 * 1024)
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

    async def snap(self):
        return json.loads(await self.js(SNAP_JS))

    async def goto(self, route, settle=1.6):
        await self.js(f"location.hash = {json.dumps(route)}; 'ok'")
        await asyncio.sleep(settle)

    async def shot(self, name):
        os.makedirs(OUT, exist_ok=True)
        r = await self.cmd("Page.captureScreenshot", format="png")
        path = os.path.join(OUT, name + ".png")
        with open(path, "wb") as f:
            f.write(base64.b64decode(r["data"]))
        return path


async def run(label: str, exe: str, reuse: bool):
    RESULT = {"label": label, "exe": exe, "steps": []}
    d = Driver()
    if not reuse:
        kill_app()
        launch(exe)
        for _ in range(40):
            if port_ready():
                break
            time.sleep(1)
        if not port_ready():
            raise SystemExit("启动后 9222 未就绪")
        time.sleep(4)
    if not await d.connect():
        raise SystemExit("CDP 连接失败")
    await d.cmd("Runtime.enable")

    home = await d.snap()
    RESULT["app"] = {"url": home["url"], "route": home["route"]}
    print(f"[app] {home['url']}  route={home['route']}")

    for page, route, key, marker in (
        ("subtitle", "#/subtitle", "sub", "zzdeltest-subtitle"),
        ("converter", "#/converter", "cv", "zzdeltest-convert"),
        ("download", "#/download", "dl", "zzdeltest-download"),
    ):
        await d.goto(route)
        entry = {"page": page, "route": route}
        entry["row_pre"] = await d.snap()
        entry["click"] = await d.js(click_js(page, marker))
        await asyncio.sleep(1.6)
        entry["row_post"] = await d.snap()
        entry["shot"] = await d.shot(f"{label}-{page}")
        # 切首页再回来：脏状态会不会被页面重新挂载带走
        await d.goto("#/")
        await d.goto(route)
        entry["row_after_switch"] = await d.snap()
        s_pre, s_post, s_sw = entry["row_pre"], entry["row_post"], entry["row_after_switch"]
        print(f"[{page}] click={entry['click']}")
        print(f"    删除前   store={s_pre['store'][key]} dom={s_pre['dom'][key]} backend={s_pre['backend'][key]}")
        print(f"    删除后   store={s_post['store'][key]} dom={s_post['dom'][key]} backend={s_post['backend'][key]}")
        print(f"    切页回来 store={s_sw['store'][key]} dom={s_sw['dom'][key]} backend={s_sw['backend'][key]}")
        RESULT["steps"].append(entry)

    tail = await d.snap()
    RESULT["final"] = tail
    print(f"[final] store={tail['store']} dom={tail['dom']} backend={tail['backend']}  "
          f"残留id={tail['ids']}")
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, f"{label}.json"), "w", encoding="utf-8") as f:
        json.dump(RESULT, f, ensure_ascii=False, indent=2)
    print(f"[saved] {os.path.join(OUT, label + '.json')}")


def main():
    args = sys.argv[1:]
    label = args[0] if args else "probe"
    if label == "seed":
        return seed()
    if label == "clean":
        return clean()
    exe = INSTALLED
    if "--exe" in args:
        exe = os.path.abspath(args[args.index("--exe") + 1])
    reuse = "--reuse" in args
    asyncio.run(run(label, exe, reuse))


if __name__ == "__main__":
    main()
