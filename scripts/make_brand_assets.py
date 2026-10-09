# -*- coding: utf-8 -*-
"""品牌资产管线（下载箭头）：母版 SVG → 渲染 PNG/WebP → 重生成应用图标。

设计：圆形底板 = 蓝紫渐变 + 顶部光泽，图形 = 白色下载箭头（竖杆 + 箭头 + 托盘线）。
渲染走 Edge/Chrome headless（与 WebView2 同引擎），2× 超采样后按圆形 alpha 蒙版
导出透明 PNG —— 因此不依赖 rsvg / sharp 等图形库。

输出：
  assets/icon.svg          母版（可编辑矢量源）
  assets/icon-1024.png     应用图标源图（tauri icon 输入）
  assets/logo-1024.png     1024 PNG
  assets/logo-256.png      256  PNG
  assets/logo-128.png      128  PNG
  src/assets/logo-256.webp 应用内侧栏 LOGO
  src/assets/icon-512.webp 应用内设置页预览

用法：
  python scripts/make_brand_assets.py            # 资产 + 应用图标
  python scripts/make_brand_assets.py --no-icons # 只出资产
  EDGE_BIN=<浏览器路径> 可显式指定渲染器
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# 半径 238/512 = 0.4648，圆心 256,256 —— 所有尺寸按此裁圆
R_RATIO = 238.0 / 512.0
SVG = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512" \
role="img" aria-label="Umidl">
  <title>Umidl</title>
  <defs>
    <linearGradient id="plate" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#63C8FF"/>
      <stop offset="0.55" stop-color="#6E8BFF"/>
      <stop offset="1" stop-color="#8B5CFF"/>
    </linearGradient>
  </defs>
  <circle cx="256" cy="256" r="238" fill="url(#plate)"/>
  <circle cx="256" cy="212" r="196" fill="#FFFFFF" opacity="0.10"/>
  <g fill="none" stroke="#FFFFFF" stroke-width="42" stroke-linecap="round" stroke-linejoin="round">
    <path d="M256 132 V296"/>
    <path d="M176 226 L256 306 L336 226"/>
    <path d="M158 380 H354"/>
  </g>
</svg>
"""

OUTS = [
    ("assets/logo-256.png", 256),
    ("assets/logo-128.png", 128),
    ("src/assets/logo-256.webp", 256),
    ("src/assets/icon-512.webp", 512),
]


def find_browser() -> str:
    env = os.environ.get("EDGE_BIN")
    cands = [env] if env else []
    cands += [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ]
    for c in cands:
        if c and os.path.exists(c):
            return c
    raise SystemExit("找不到 Edge/Chrome，请用 EDGE_BIN=<路径> 指定渲染器")


def render(browser: str, svg_path: str, size: int, out_png: str, tmp: str) -> None:
    """把 SVG 渲染成 size×size 的透明 PNG：2× 截图 → 圆形 alpha 蒙版 → LANCZOS 缩小。"""
    from PIL import Image, ImageDraw, ImageFilter

    ss = size * 2
    svg = open(svg_path, encoding="utf-8").read()
    svg = re.sub(r'width="512" height="512"', f'width="{ss}" height="{ss}"', svg, count=1)
    html = (f'<!doctype html><meta charset="utf-8"><style>html,body{{margin:0;padding:0;'
            f'background:#fff;overflow:hidden}}svg{{display:block}}</style>{svg}')
    hpath = os.path.join(tmp, f"_wrap_{size}.html")
    shot = os.path.join(tmp, f"_shot_{size}.png")
    open(hpath, "w", encoding="utf-8").write(html)
    subprocess.run([browser, "--headless=new", "--disable-gpu", "--hide-scrollbars",
                    f"--window-size={ss},{ss}", f"--screenshot={shot}",
                    "file:///" + hpath.replace("\\", "/")],
                   capture_output=True, text=True, timeout=180)
    im = Image.open(shot).convert("RGBA")
    mask = Image.new("L", (ss * 2, ss * 2), 0)
    d = ImageDraw.Draw(mask)
    r = int(ss * 2 * R_RATIO)
    c = ss
    d.ellipse((c - r, c - r, c + r, c + r), fill=255)
    mask = mask.resize((ss, ss), Image.LANCZOS).filter(ImageFilter.GaussianBlur(ss / 680.0))
    im.putalpha(mask)
    if im.size != (size, size):
        im = im.resize((size, size), Image.LANCZOS)
    im.save(out_png, "PNG", optimize=True)


def main() -> None:
    from PIL import Image

    ap = argparse.ArgumentParser()
    ap.add_argument("--no-icons", action="store_true", help="跳过 tauri icon 重生成")
    args = ap.parse_args()
    browser = find_browser()

    icon_svg = os.path.join(ROOT, "assets", "icon.svg")
    os.makedirs(os.path.dirname(icon_svg), exist_ok=True)
    open(icon_svg, "w", encoding="utf-8", newline="").write(SVG)
    print("写出 assets/icon.svg")

    tmp = tempfile.mkdtemp(prefix="umidl-brand-")
    try:
        render(browser, icon_svg, 1024, os.path.join(ROOT, "assets", "icon-1024.png"), tmp)
        render(browser, icon_svg, 1024, os.path.join(ROOT, "assets", "logo-1024.png"), tmp)
        for rel, size in OUTS:
            path = os.path.join(ROOT, rel)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            png = os.path.join(tmp, f"_{size}.png")
            render(browser, icon_svg, size, png, tmp)
            img = Image.open(png).convert("RGBA")
            if rel.endswith(".webp"):
                img.save(path, "WEBP", quality=85, method=6)
            else:
                img.save(path, "PNG", optimize=True)
            print(f"写出 {rel:30} {size}×{size}  {os.path.getsize(path):,} B")
        for rel in ("assets/icon-1024.png", "assets/logo-1024.png"):
            print(f"写出 {rel:30} 1024×1024  {os.path.getsize(os.path.join(ROOT, rel)):,} B")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    if not args.no_icons:
        npx = shutil.which("npx")
        if not npx:
            print("⚠ 未找到 npx（Node 未安装或不在 PATH），跳过 tauri icon；可手动跑："
                  "npx tauri icon assets/icon-1024.png")
        else:
            r = subprocess.run([npx, "tauri", "icon", "assets/icon-1024.png"], cwd=ROOT,
                               capture_output=True, text=True, encoding="utf-8",
                               errors="replace", shell=(os.name == "nt"), timeout=600)
            print("tauri icon:", "OK" if r.returncode == 0 else f"失败 exit={r.returncode}")
            if r.returncode != 0:
                print((r.stderr or r.stdout or "")[:400])


if __name__ == "__main__":
    sys.exit(main())
