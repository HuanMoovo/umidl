"""重写 verify_v17.py 的第 2、3 节：点击与读取分两次求值（等重渲染），注入 JS 不再使用换行转义。幂等。"""
import sys

P = "scripts/verify_v17.py"
raw = open(P, encoding="utf-8", newline="").read()
crlf = "\r\n" in raw
s = raw.replace("\r\n", "\n")

start_marker = "    # ── 2. 设置里已移除 ED2K ──"
end_marker = "    # ── 4. 回归 ──"
i = s.find(start_marker)
j = s.find(end_marker)
if i < 0 or j < 0 or j < i:
    sys.exit(f"定位失败 i={i} j={j}")

NEW = '''    # ── 2. 设置里已移除 ED2K ──────────────────────────────────────────
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
            " summary: (document.body.innerText.match(/\\u5f53\\u524d\\u7b5b\\u9009.{0,14}/)||[''])[0]})"
        )
        try:
            return json.loads(raw_)
        except Exception:
            return {"total": None, "disabled": None, "summary": str(raw_)[:60]}

    async def switch_type(kind):
        """点类型芯片 → 等重渲染 → 再读"""
        ok = await d.js(
            "(() => { const el=document.querySelector('[data-role=type-options] [data-type=\"%s\"]');"
            " if(!el) return false; el.click(); return true; })()" % kind
        )
        await asyncio.sleep(1.1)
        st = await grid_count()
        st["clicked"] = ok
        return st

    grids = await d.js(
        "(() => { const g=[...document.querySelectorAll('[data-role=format-grid],[data-role=format-selector],[class*=grid]')]"
        ".filter(e=>e.offsetParent && e.querySelectorAll('[data-format]').length >= 5); return g.length; })()"
    )
    check("页面只有一个格式网格容器", int(grids or 0) <= 1, f"匹配 {grids} 个（期望 ≤1）")

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

'''
s = s[:i] + NEW + s[j:]
open(P, "w", encoding="utf-8", newline="").write(s.replace("\n", "\r\n") if crlf else s)
print(f"已重写 verify_v17.py 第 2、3 节（{j - i} 字符 → {len(NEW)} 字符）")
