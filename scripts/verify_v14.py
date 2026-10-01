"""Umidl 1.4 实机验收：通过 WebView2 调试端口驱动真实安装的客户端。

覆盖：版本/页签/设置卡、粒子三形状、队列双布局与持久化、引擎路由与过滤器判定、
浏览器捕获端到端（真实入队 + 真实下载）、令牌桶限速的真实速率、开机自启动注册表、
日志导出、更新检查。

用法：python scripts/verify_v14.py [--keep-running]
"""
import asyncio
import json
import os
import shutil
import subprocess
import sys
import time
import urllib.request

import requests
import websockets

APP = os.path.expandvars(r"%LOCALAPPDATA%\Umidl\umidl.exe")
DATA = os.path.expandvars(r"%APPDATA%\umi-downloader")
CAPTURE_PORT = 6970
# 验收用的直链：aria2 官方发布包（2.4 MB，分段引擎可测速）
TEST_URL = "https://github.com/aria2/aria2/releases/download/release-1.37.0/aria2-1.37.0-win-64bit-build1.zip"
TEST_SIZE = 2_475_379

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

    async def click_text(self, tag, text):
        js = (
            "(() => { const els = [...document.querySelectorAll(%s)];"
            " const el = els.find(e => (e.innerText||'').includes(%s));"
            " if (!el) return 'NOT_FOUND'; el.scrollIntoView({block:'center'}); el.click(); return 'OK'; })()"
            % (json.dumps(tag), json.dumps(text))
        )
        return await self.js(js)

    async def wait_text(self, tag, text, timeout=6.0):
        end = time.time() + timeout
        while time.time() < end:
            got = await self.js(
                f"[...document.querySelectorAll({json.dumps(tag)})].some(e => (e.innerText||'').includes({json.dumps(text)}))"
            )
            if got:
                return True
            await asyncio.sleep(0.3)
        return False


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


def http_get(path, token=""):
    req = urllib.request.Request(f"http://127.0.0.1:{CAPTURE_PORT}{path}")
    if token:
        req.add_header("X-Umidl-Token", token)
    with urllib.request.urlopen(req, timeout=10) as r:
        return json.loads(r.read().decode("utf-8", "replace"))


async def main():
    keep = "--keep-running" in sys.argv
    kill_app()
    # 清掉验收用的旧产物，避免断点续传把测速糊掉
    test_dir = os.path.join(DATA, "verify14")
    if os.path.isdir(test_dir):
        shutil.rmtree(test_dir, ignore_errors=True)

    print("══ 启动被测程序")
    proc = launch_app()
    d = Driver()
    if not await d.connect():
        check("CDP 连接", False, "9222 端口未就绪")
        proc.kill()
        return 1
    check("CDP 连接 + 应用启动", True)
    await asyncio.sleep(2.5)

    api_ok = True
    try:
        await d.invoke("app_version")
    except Exception as e:
        api_ok = False
        check("IPC 通道（__TAURI_INTERNALS__）", False, str(e)[:120])

    if api_ok:
        print("══ 1. 版本与设置结构")
        ver = await d.invoke("app_version")
        check("版本号为 1.4.0", ver == "1.4.0", str(ver))

        # 页签要先进设置页才存在（应用启动落在首页）
        await d.js("location.hash = '#/settings'")
        await asyncio.sleep(2.0)
        tabs = await d.js("[...document.querySelectorAll('button.umi-btn')].map(e => e.innerText.trim())")
        tabs = [t for t in (tabs or []) if t]
        check("设置页含「下载引擎」页签", any(t == "下载引擎" for t in tabs), str(tabs))
        check("设置页含「系统与集成」页签", any(t == "系统与集成" for t in tabs))

        print("══ 2. 引擎与路由")
        info = await d.invoke("engine_info")
        check("engine_info 返回分段参数", bool(info), json.dumps(info, ensure_ascii=False)[:160])
        r1 = await d.invoke("explain_route", {"url": TEST_URL})
        route1 = json.dumps(r1, ensure_ascii=False)
        check("直链路由到 aria2 分段引擎", "aria2" in route1.lower(), route1[:160])
        r2 = await d.invoke("explain_route", {"url": "https://www.youtube.com/watch?v=aqz-KE-bpKQ"})
        route2 = json.dumps(r2, ensure_ascii=False)
        check("站点链接路由到 yt-dlp", "yt" in route2.lower() or "站点" in route2, route2[:160])

        print("══ 3. 智能过滤")
        before = await d.invoke("get_settings")
        await d.invoke("save_settings", {"settings": dict(before, filter_ext_block="exe")})
        r3 = await d.invoke("explain_route", {"url": "https://example.com/setup.exe"})
        s3 = json.dumps(r3, ensure_ascii=False)
        check("扩展名黑名单命中被拦截", ("拦截" in s3 or "block" in s3.lower() or "过滤" in s3), s3[:200])
        await d.invoke("save_settings", {"settings": dict(before, filter_ext_block=before.get("filter_ext_block", ""))})
        check("过滤规则可恢复", True)

        print("══ 4. 令牌桶限速状态")
        await d.invoke("save_settings", {"settings": dict(before, speed_limit_enabled=True, speed_limit_kb=300)})
        st = await d.invoke("get_settings")
        check("限速设置已落盘（300 KB/s）",
              st.get("speed_limit_enabled") is True and st.get("speed_limit_kb") == 300,
              f"{st.get('speed_limit_enabled')}/{st.get('speed_limit_kb')}")

        print("══ 5. 开机自启动（真实注册表写入）")
        old_auto = await d.invoke("get_autostart")
        check("get_autostart 可读", old_auto is not None, str(old_auto))
        await d.invoke("set_autostart", {"enabled": True})
        reg = subprocess.run(
            ["reg", "query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run"],
            capture_output=True, text=True,
        ).stdout
        check("注册表出现自启动项", "Umidl" in reg or "umidl" in reg.lower(), "Run 键已写入")
        await d.invoke("set_autostart", {"enabled": bool(old_auto if isinstance(old_auto, bool) else False)})
        reg2 = subprocess.run(
            ["reg", "query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run"],
            capture_output=True, text=True,
        ).stdout
        check("自启动可关闭", ("Umidl" not in reg2) or (old_auto is True))

        print("══ 6. 日志导出")
        out_dir = os.path.join(os.path.dirname(DATA), "umidl-log-verify")
        shutil.rmtree(out_dir, ignore_errors=True)
        r4 = await d.invoke("export_logs", {"dest": out_dir})
        # 命令返回的是实际落盘的日志文件路径（可能在数据目录下），按返回路径核对
        rp = r4 if isinstance(r4, str) else (r4 or {}).get("path", "")
        file_ok = bool(rp) and os.path.isfile(rp)
        text = ""
        if file_ok:
            text = open(rp, encoding="utf-8", errors="replace").read()
        check("导出日志文件真实存在", file_ok, str(rp)[:120])
        check("日志里含环境摘要（版本 / 工具 / 设置）",
              file_ok and ("1.4.0" in text and ("umidl" in text.lower() or "工具" in text)),
              f"{len(text)} 字节")

        print("══ 7. 更新检查（网络不可用也应优雅返回）")
        try:
            r5 = await d.invoke("check_update")
            check("check_update 不崩溃", isinstance(r5, (dict, str)), json.dumps(r5, ensure_ascii=False)[:160])
        except Exception as e:
            check("check_update 不崩溃", False, str(e)[:120])
    else:
        print("  （IPC 不可用，跳过后端用例）")

    print("══ 8. 界面：粒子形状 / 队列双布局")
    # 粒子形状：设置页 → 外观 chip（真实页签是 button.umi-btn 自定义按钮）
    await d.js("location.hash = '#/settings'")
    await asyncio.sleep(2.0)
    clicked_tab = await d.js(
        "(() => { const b=[...document.querySelectorAll('button.umi-btn')].find(e=>(e.innerText||'').trim()==='外观');"
        " if(!b) return 'NOT_FOUND'; b.scrollIntoView({block:'center'}); b.click(); return 'OK'; })()"
    )
    check("外观页签可点击", clicked_tab == "OK", str(clicked_tab))
    await asyncio.sleep(1.5)
    has_shape = await d.js(
        "[...document.querySelectorAll('*')].some(e => (e.innerText||'').includes('粒子形状'))"
    )
    check("外观页出现「粒子形状」选择器", bool(has_shape))
    if has_shape and api_ok:
        clicked = await d.js(
            """(() => {
                 const labels = [...document.querySelectorAll('*')].filter(e => (e.innerText||'').trim().startsWith('粒子形状'));
                 const box = labels[labels.length-1]?.parentElement;
                 if (!box) return 'NO_BOX';
                 const sel = box.querySelector('.n-base-selection');
                 if (!sel) return 'NO_SELECT';
                 sel.scrollIntoView({block:'center'}); sel.click();
                 return 'OPEN';
               })()"""
        )
        if clicked == "OPEN":
            await asyncio.sleep(0.9)
            await d.click_text(".n-base-select-option", "正方形")
            await asyncio.sleep(1.2)
            s = await d.invoke("get_settings")
            check("设置已切到 particle_shape=square", s.get("particle_shape") == "square",
                  str(s.get("particle_shape")))
            canvas = await d.js("!!document.querySelector('canvas')")
            check("粒子画布仍在渲染", bool(canvas))
        else:
            check("粒子形状下拉可交互", False, clicked)

    # 队列双布局
    await d.js("location.hash = '#/download'")
    await asyncio.sleep(1.5)
    await d.click_text("button", "详细列表")
    await asyncio.sleep(1.2)
    cols = await d.js("document.querySelectorAll('table thead th').length")
    check("切到详细表格出现 6 列", cols == 6, f"th={cols}")
    saved = await d.js("localStorage.getItem('umi.queue_layout')")
    await d.js("location.reload()")
    await asyncio.sleep(3.0)
    cols2 = await d.js("document.querySelectorAll('table thead th').length")
    check("刷新后仍是详细表格（布局持久化）", cols2 == 6, f"th={cols2}（localStorage={saved}）")
    await d.click_text("button", "卡片模式")
    await asyncio.sleep(1.0)

    print("══ 9. 浏览器捕获：端到端真实下载 + 限速实测")
    if api_ok:
        try:
            ping = http_get("/ping")
            check("捕获接口 /ping 握手", bool(ping.get("app")), json.dumps(ping, ensure_ascii=False)[:150])
        except Exception as e:
            check("捕获接口 /ping 握手", False, str(e)[:120])

        # 限速 300 KB/s 下真实下载 2.4 MB，理论 ~8s（>4s 即证明限速生效）
        try:
            # 先删掉上一次留下的同名产物与 .aria2 控制文件，否则断点续传会秒完成、测速失真
            dd = (before or {}).get("download_dir") or os.path.expanduser("~/Downloads/Umidl")
            target = os.path.join(dd, TEST_URL.split("/")[-1])
            for junk in (target, target + ".aria2"):
                try:
                    os.remove(junk)
                except OSError:
                    pass
            t0 = time.time()
            stale_ids = {t.get("id") for t in (await d.invoke("list_downloads"))}
            http_get("/capture?url=" + urllib.parse.quote(TEST_URL, safe=""))
            check("捕获接口接收链接", True)
            done, size, spent = False, 0, 0.0
            for _ in range(90):
                await asyncio.sleep(2)
                tasks = await d.invoke("list_downloads")
                # 只看本次捕获新建的任务，避免匹配到上一次运行的旧记录
                mine = [t for t in tasks if t.get("id") not in stale_ids]
                if mine:
                    t = mine[-1]
                    size = t.get("downloaded") or t.get("total") or 0
                    if t.get("status") in ("done", "completed"):
                        done = True
                        spent = time.time() - t0
                        break
                    if t.get("status") in ("error", "failed"):
                        break
            check("捕获后任务真实入队并完成", done, f"已下载≈{size} 字节，用时 {spent:.1f}s")
            if done and spent > 0:
                kbps = (TEST_SIZE / 1024) / spent
                check("限速生效（300 KB/s 上限，实测不超 450）", kbps < 450, f"实测 ≈{kbps:.0f} KB/s")
            # 产物落盘校验：用本次新建任务记录里的真实路径核对字节数
            fp = (t.get("file_path") if done else "") or ""
            check("下载产物真实落盘且字节数一致",
                  bool(fp) and os.path.isfile(fp) and os.path.getsize(fp) == TEST_SIZE,
                  f"{fp}（{os.path.getsize(fp) if fp and os.path.isfile(fp) else 0} 字节）")
        except Exception as e:
            check("捕获 → 下载链路", False, str(e)[:160])

        # 恢复原设置
        try:
            await d.invoke("save_settings", {"settings": before})
            check("设置已恢复为验收前状态", True)
        except Exception as e:
            check("设置已恢复为验收前状态", False, str(e)[:100])

    print("══ 10. 收尾")
    if not keep:
        kill_app()
        try:
            await d.js("window.close()")
        except Exception:
            pass
        time.sleep(1.0)
        closed = subprocess.run(
            ["powershell", "-NoProfile", "-Command",
             "(Get-NetTCPConnection -LocalPort 9222 -ErrorAction SilentlyContinue | Measure-Object).Count"],
            capture_output=True, text=True,
        ).stdout.strip()
        check("退出后调试端口已关闭", closed in ("0", ""), f"连接数={closed}")

    print("\n" + "=" * 56)
    print(f"通过 {len(PASS)} ｜ 失败 {len(FAIL)}")
    if FAIL:
        print("失败项：")
        for f in FAIL:
            print("  ✗", f)
    return 0 if not FAIL else 1


if __name__ == "__main__":
    import urllib.parse  # noqa: E402  (供 capture 查询串使用)

    sys.exit(asyncio.run(main()))
