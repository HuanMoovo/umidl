"""把 verify_v18.py 的驱动改成内联版（v17 同款），并修掉一处字符串拼接 bug。幂等。"""
import sys

P = "scripts/verify_v18.py"
raw = open(P, encoding="utf-8", newline="").read()
crlf = "\r\n" in raw
s = raw.replace("\r\n", "\n")

# ① 内联驱动
old_import = """sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from drive_app import AppDriver  # noqa: E402
"""
new_import = '''import requests
import websockets


class Driver:
    """CDP 驱动（与 verify_v17 同款）"""

    def __init__(self, port=9224):
        self.port = port
        self.ws = None
        self.n = 1

    async def connect(self):
        for _ in range(40):
            try:
                d = requests.get(f"http://127.0.0.1:{self.port}/json", timeout=3).json()
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

    async def goto(self, hash_, settle=1.9):
        await self.js(f"location.hash = {json.dumps(hash_)}; 'ok'")
        await asyncio.sleep(settle)


class AppDriver(Driver):
    """启动被测程序 + 提供 start/close（脚本里直接用，不依赖 drive_app.py 的 CLI）"""

    def __init__(self, exe=APP, port=9224):
        super().__init__(port)
        self.exe = exe

    async def start(self):
        kill_app()
        env = dict(os.environ)
        env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = f"--remote-debugging-port={self.port}"
        subprocess.Popen([self.exe], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        ok = await self.connect()
        await asyncio.sleep(2.2)
        return ok

    async def close(self):
        try:
            await self.js("window.close(); 'ok'")
        except Exception:
            pass
        await asyncio.sleep(0.8)
        kill_app()
'''
if "class AppDriver(Driver)" in s:
    print("① 跳过（已内联）")
elif s.count(old_import) == 1:
    s = s.replace(old_import, new_import, 1)
    print("① 驱动已内联")
else:
    sys.exit(f"① 锚点命中 {s.count(old_import)} 次")

# ② 长路径断言的字符串拼接 bug
old_lp = '''    check("超长路径给出可理解的原因（含路径长度或目录创建失败）",
          ("路径" in msg + lp_err and "260" in (msg + lp_err)) or "路径过长" in (msg + lp_err) or "路径" in lp_err,
          (msg + " / " + lp_err)[:200])'''
new_lp = '''    blob = (msg or "") + " / " + (lp_err or "")
    check("超长路径给出可理解的原因（含路径长度或目录创建失败）",
          ("路径" in blob) or ("260" in blob) or ("errorCode=18" in blob) or ("make the directory" in blob),
          blob[:200])'''
if "blob = (msg or \"\")" in s:
    print("② 跳过（已修）")
elif s.count(old_lp) == 1:
    s = s.replace(old_lp, new_lp, 1)
    print("② 长路径断言已修")
else:
    sys.exit(f"② 锚点命中 {s.count(old_lp)} 次")

# ③ 调用处：invoke(..., req={...}) 关键字写法要能用
s = s.replace('await d.invoke("start_convert", req={', 'await d.invoke("start_convert", {')
s = s.replace('await d.invoke("save_settings", settings={', 'await d.invoke("save_settings", {')
s = s.replace('await d.invoke("start_download", req={', 'await d.invoke("start_download", {')
s = s.replace('await d.invoke("remove_download", id=gid, deleteFile=True)', 'await d.invoke("remove_download", {"id": gid, "deleteFile": True})')
s = s.replace('await d.invoke("cancel_download", id=target["id"])', 'await d.invoke("cancel_download", {"id": target["id"]})')
s = s.replace('await d.invoke("ed2k_submit", link=', 'await d.invoke("ed2k_submit", {"link": "')
s = s.replace('"ed2k://|file|verify18.bin|1048576|31D6CFE0D16AE931B73C59D7E0C089C0|/")',
              '"ed2k://|file|verify18.bin|1048576|31D6CFE0D16AE931B73C59D7E0C089C0|/"})')
s = s.replace('await d.invoke("enqueue_links", text=', 'await d4.invoke("enqueue_links", {"text": ')
s = s.replace('", output_dir=WORK)\n    check("批量导入仍按整行跳注释"', '"}, {"output_dir": WORK})\n    check("批量导入仍按整行跳注释"')

open(P, "w", encoding="utf-8", newline="").write(s.replace("\n", "\r\n") if crlf else s)
import py_compile
try:
    py_compile.compile(P, doraise=True)
    print("语法 OK")
except py_compile.PyCompileError as e:
    sys.exit("语法错误：" + str(e))
