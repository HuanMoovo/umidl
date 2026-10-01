"""探针：把转换页类型选项/网格与设置页卡片的真实 DOM 结构打出来，用于校准 verify_v17 的选择器。"""
import asyncio
import json
import os
import subprocess
import sys
import time

import requests
import websockets

APP = os.path.expandvars(r"%LOCALAPPDATA%\Umidl\umidl.exe")


def kill():
    subprocess.run(["powershell", "-NoProfile", "-Command",
                    "Get-Process umidl,umi-downloader -ErrorAction SilentlyContinue | Stop-Process -Force"],
                   capture_output=True)
    time.sleep(1.5)


async def main():
    kill()
    env = dict(os.environ)
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = "--remote-debugging-port=9222"
    subprocess.Popen([APP], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    ws = None
    for _ in range(40):
        try:
            d = requests.get("http://127.0.0.1:9222/json", timeout=3).json()
            pages = [t for t in d if t.get("type") == "page"]
            if pages:
                ws = await websockets.connect(pages[0]["webSocketDebuggerUrl"], max_size=64 * 1024 * 1024)
                break
        except Exception:
            pass
        await asyncio.sleep(0.5)
    if not ws:
        print("连不上调试端口")
        return 1
    await asyncio.sleep(2.2)

    n = [1]

    async def js(expr):
        mid = n[0]
        n[0] += 1
        await ws.send(json.dumps({"id": mid, "method": "Runtime.evaluate",
                                  "params": {"expression": expr, "returnByValue": True, "awaitPromise": True}}))
        while True:
            m = json.loads(await ws.recv())
            if m.get("id") == mid:
                res = (m.get("result") or {})
                if res.get("exceptionDetails"):
                    return f"JS_ERROR: {res['exceptionDetails'].get('text')} {(res['exceptionDetails'].get('exception') or {}).get('description','')[:200]}"
                return res.get("result", {}).get("value")

    await js("location.hash='#/converter'; 'ok'")
    await asyncio.sleep(2.5)

    print("══ 类型选项元素（data-role=type-options 内）")
    print(await js("""
(() => {
  const box=document.querySelector('[data-role=type-options]');
  if(!box) return 'NO type-options box';
  return [...box.querySelectorAll('*')].filter(e=>e.offsetParent && (e.innerText||'').trim()).slice(0,14)
    .map(e=>`${e.tagName}[${e.className.toString().slice(0,26)}] "${(e.innerText||'').trim().slice(0,14)}" data-type=${e.getAttribute('data-type')} disabled=${e.disabled===true}`).join('\\n');
})()"""))

    print("\n══ 网格里 data-format 的取样（前 6 个 + 属性清单）")
    print(await js("""
(() => {
  const items=[...document.querySelectorAll('[data-format]')];
  if(!items.length) return 'NO [data-format] items';
  const sample=items.slice(0,6).map(e=>`${e.getAttribute('data-format')} avail=${JSON.stringify(e.getAttribute('data-available'))} disabled=${e.disabled===true} tag=${e.tagName}`);
  return 'total='+items.length+'\\n'+sample.join('\\n');
})()"""))

    print("\n══ data-available 取值分布")
    print(await js("""
(() => {
  const m={};
  document.querySelectorAll('[data-format]').forEach(e=>{const v=String(e.getAttribute('data-available'));m[v]=(m[v]||0)+1;});
  return JSON.stringify(m);
})()"""))

    print("\n══ 点第一个 data-type 芯片后网格变化")
    print(await js("""
(() => { const el=document.querySelector('[data-role=type-options] [data-type]'); if(!el) return 'NO chip';
  el.click(); return 'clicked '+el.getAttribute('data-type'); })()"""))
    await asyncio.sleep(1.2)
    print(await js("""
(() => { const all=[...document.querySelectorAll('[data-format]')].length;
  const vis=[...document.querySelectorAll('[data-format]')].filter(e=>e.offsetParent).length;
  const txt=(document.body.innerText.match(/当前筛选[^\\n]*/)||[''])[0];
  return 'total='+all+' visible='+vis+' | '+txt; })()"""))

    print("\n══ 逐个类型芯片点击后的可见条目数")
    print(await js("""
(async () => {
  const out=[];
  const chips=[...document.querySelectorAll('[data-role=type-options] [data-type]')];
  for (const c of chips) {
    c.click();
    await new Promise(r=>setTimeout(r,700));
    out.push(c.getAttribute('data-type')+' -> total='+document.querySelectorAll('[data-format]').length
      +' visible='+[...document.querySelectorAll('[data-format]')].filter(e=>e.offsetParent).length
      +' disabled='+[...document.querySelectorAll('[data-format]')].filter(e=>e.disabled===true).length
      +' | '+((document.body.innerText.match(/当前筛选[^\\n]*/)||[''])[0]));
  }
  return out.join('\\n');
})()"""))

    print("\n══ 设置页卡片清单")
    await js("location.hash='#/settings'; 'ok'")
    await asyncio.sleep(2.0)
    print(await js("""
(() => { const btn=[...document.querySelectorAll('button')].find(e=>(e.innerText||'').trim()==='系统与集成');
  if(btn) btn.click(); return 'tab clicked='+!!btn; })()"""))
    await asyncio.sleep(1.8)
    print(await js("""
(() => { const cards=[...document.querySelectorAll('.umi-card')].filter(e=>e.offsetParent).map(e=>(e.innerText||'').trim().split('\\n')[0].slice(0,20));
  const main=document.querySelector('main');
  return 'umi-card 标题: '+JSON.stringify(cards)+'\\nbody 含 ED2K='+document.body.innerText.includes('ED2K')+'\\nmain 前 200 字: '+(main?main.innerText.slice(0,200).replace(/\\n/g,' | '):'no main'); })()"""))

    try:
        await js("window.close(); 'ok'")
    except Exception:
        pass
    await asyncio.sleep(1)
    kill()
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
