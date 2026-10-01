"""按真实 DOM 校准 verify_v17.py 的三处选择器（产品侧无问题，纯脚本修正）。幂等。"""
import sys

P = "scripts/verify_v17.py"
raw = open(P, encoding="utf-8", newline="").read()
crlf = "\r\n" in raw
s = raw.replace("\r\n", "\n")
done = []

# ① 设置页卡片：用 .umi-card 标题判断，别用脆弱的 div 文本提取
old1 = """    inner = await d.js(
        "(() => { const el=[...document.querySelectorAll('div,section,main')].filter(e=>e.offsetParent && (e.innerText||'').includes('浏览器捕获'));"
        " return el.length ? el[el.length-1].innerText : ''; })()"
    )"""
new1 = """    inner = await d.js(
        "(() => { const cards=[...document.querySelectorAll('.umi-card')].filter(e=>e.offsetParent)"
        ".map(e=>(e.innerText||'').trim().split('\\\\n')[0].slice(0,20));"
        " const main=document.querySelector('main');"
        " return JSON.stringify({cards: cards, hasEd2k: document.body.innerText.includes('ED2K'),"
        " head: main ? main.innerText.slice(0,120).replace(/\\\\n/g,' / ') : ''}); })()"
    )"""
if new1.strip()[:40] in s:
    done.append("① 跳过（已校准）")
elif s.count(old1) == 1:
    s = s.replace(old1, new1, 1)
    done.append("① 设置页卡片检查已改用 .umi-card")
else:
    sys.exit(f"① 锚点命中 {s.count(old1)} 次")

old2 = """    check("系统与集成页签可打开且含捕获/系统与电源/更新/诊断",
          all(k in str(inner) for k in ("浏览器捕获", "系统与电源", "更新", "诊断")),
          str(inner).replace("\\n", " / ")[:110])
    check("系统与集成里已没有 ED2K 卡片", "ED2K" not in str(inner) and "eMule" not in str(inner), "检查的是该页签的 DOM 文本")"""
new2 = """    info = json.loads(inner) if isinstance(inner, str) and inner.startswith("{") else {}
    cards = info.get("cards") or []
    check("系统与集成页签含捕获/系统与电源/更新/诊断四张卡",
          all(any(k in c for c in cards) for k in ("浏览器捕获", "系统与电源", "更新", "诊断")),
          json.dumps(cards, ensure_ascii=False))
    check("系统与集成里已没有 ED2K 卡片", info.get("hasEd2k") is False, str(info.get("head"))[:100])"""
if 'cards = info.get("cards")' in s:
    done.append("② 跳过（已校准）")
elif s.count(old2) == 1:
    s = s.replace(old2, new2, 1)
    done.append("② 断言改为读卡片清单")
else:
    sys.exit(f"② 锚点命中 {s.count(old2)} 次")

# ③ 类型切换：点真实的 data-type 芯片；置灰统计只用合法选择器
old3 = """    await d.click_text("音频")
    await asyncio.sleep(1.0)
    n_audio = await d.js("document.querySelectorAll('[data-format][data-available]').length")
    await d.click_text("视频")
    await asyncio.sleep(1.0)
    n_video = await d.js("document.querySelectorAll('[data-format][data-available]').length")
    check("切换类型后网格条目数变化", int(n_audio or 0) > 0 and int(n_video or 0) > 0 and n_audio != n_video,
          f"音频 {n_audio} 条 / 视频 {n_video} 条（后端：音频 21 / 视频 22）")"""
new3 = """    async def grid_stats(kind):
        return await d.js(
            "(() => { const el=document.querySelector('[data-role=type-options] [data-type=\\"%s\\"]');"
            " if(el) el.click();"
            " const items=[...document.querySelectorAll('[data-format]')];"
            " return JSON.stringify({total: items.length,"
            " disabled: items.filter(e=>String(e.getAttribute('data-available'))==='0').length,"
            " summary: (document.body.innerText.match(/当前筛选[^\\\\n]*/)||[''])[0]}); })()" % kind
        )

    n_all = json.loads(await grid_stats("all"))
    await asyncio.sleep(1.0)
    n_audio = json.loads(await grid_stats("audio"))
    await asyncio.sleep(1.0)
    n_video = json.loads(await grid_stats("video"))
    await asyncio.sleep(1.0)
    n_doc = json.loads(await grid_stats("document"))
    await asyncio.sleep(1.0)
    n_img = json.loads(await grid_stats("image"))
    await asyncio.sleep(1.0)
    check(
        "切换类型后网格条目数按类收窄",
        n_all.get("total", 0) >= 60
        and n_video.get("total") == 22
        and n_audio.get("total") == 21
        and n_img.get("total") == 22
        and n_doc.get("total") == 23,
        f"全部 {n_all.get('total')} / 视频 {n_video.get('total')} / 音频 {n_audio.get('total')} / 图片 {n_img.get('total')} / 文档 {n_doc.get('total')}",
    )"""
if "async def grid_stats(kind)" in s:
    done.append("③ 跳过（已校准）")
elif s.count(old3) == 1:
    s = s.replace(old3, new3, 1)
    done.append("③ 类型切换改为点 data-type 芯片并按类断言")
else:
    sys.exit(f"③ 锚点命中 {s.count(old3)} 次")

old4 = """    n_disabled = await d.js("document.querySelectorAll('[data-format][data-available=\\"0\\"],[data-format][data-available=false]').length")
    check("存在被置灰的不可用格式（真实引擎判定）", int(n_disabled or 0) >= 1, f"{n_disabled} 项不可用")"""
new4 = """    n_disabled = await d.js("document.querySelectorAll('[data-format][data-available=\\"0\\"]').length")
    check(
        "存在被置灰的不可用格式（真实引擎判定）",
        int(n_disabled or 0) >= 1,
        f"当前筛选下 {n_disabled} 项不可用（全部视图应为 13：alac/dts/ape/heic/psd 等）",
    )"""
if 'data-available=\\"0\\"]\').length")\n    check(' in s:
    done.append("④ 跳过（已校准）")
elif s.count(old4) == 1:
    s = s.replace(old4, new4, 1)
    done.append("④ 置灰统计改用合法选择器")
else:
    sys.exit(f"④ 锚点命中 {s.count(old4)} 次")

open(P, "w", encoding="utf-8", newline="").write(s.replace("\n", "\r\n") if crlf else s)
print("\n".join(done))
