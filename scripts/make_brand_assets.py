# -*- coding: utf-8 -*-
"""从母版生成品牌位图资产：抠出猫 → 重建底板（圆形 / 圆角方形）→ 按各尺寸导出。

母版 = assets/icon-cat.png（平涂蓝底 + 猫）。脚本会按色键把蓝底（含旧底板）抠掉，
只留猫，再在同一位置贴一块新形状的底板 —— 因此形状可以来回切换而猫的位置与比例不变
（底板边长取猫的 bbox 外接方，圆形 = 该方的内切圆 = 最早的「圆底猫」）。

输出：
  assets/icon-cat.png            1024  母版级 PNG（README / 矢量化源）
  assets/logo-cat-256.png         256  PNG
  assets/logo-cat-128.png         128  PNG
  src/assets/icon-cat-512.webp    512  应用内设置页预览
  src/assets/logo-cat-256.webp    256  应用内侧栏 LOGO
  src/assets/logo-cat-128.webp    128  应用内小尺寸

用法：
  python scripts/make_brand_assets.py                    # 圆形（当前品牌）
  python scripts/make_brand_assets.py --shape rounded --radius 0.225
"""
import argparse
import os
import sys

from PIL import Image, ImageDraw

try:
    import numpy as np
except ImportError:  # 纯 Python 回退
    np = None

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "assets", "icon-cat.png")
BG = (122, 204, 253)          # 实测平涂底色
T0, T1 = 10, 42               # 色键软过渡区间（切比雪夫距离）
SS = 4                        # 形状超采样倍数

OUTS = [
    ("assets/icon-cat.png", 1024, "PNG"),
    ("assets/logo-cat-256.png", 256, "PNG"),
    ("assets/logo-cat-128.png", 128, "PNG"),
    ("src/assets/icon-cat-512.webp", 512, "WEBP"),
    ("src/assets/logo-cat-256.webp", 256, "WEBP"),
    ("src/assets/logo-cat-128.webp", 128, "WEBP"),
]


def key_out_cat(img: Image.Image) -> Image.Image:
    """把平涂蓝底抠成透明，返回只剩猫的 RGBA。"""
    img = img.convert("RGBA")
    if np is not None:
        a = np.asarray(img).astype(np.int16)
        r, g, b, al = a[..., 0], a[..., 1], a[..., 2], a[..., 3]
        dist = np.maximum(np.maximum(np.abs(r - BG[0]), np.abs(g - BG[1])), np.abs(b - BG[2]))
        t = np.clip((dist - T0) / float(T1 - T0), 0.0, 1.0)
        out = a.copy()
        out[..., 3] = (al * t).astype(np.int16)
        return Image.fromarray(out.astype(np.uint8), "RGBA")
    px = img.load()
    w, h = img.size
    out = Image.new("RGBA", (w, h))
    q = out.load()
    for y in range(h):
        for x in range(w):
            r, g, b, al = px[x, y]
            d = max(abs(r - BG[0]), abs(g - BG[1]), abs(b - BG[2]))
            t = 0.0 if d <= T0 else (1.0 if d >= T1 else (d - T0) / (T1 - T0))
            q[x, y] = (r, g, b, int(al * t))
    return out


def plate(size: int, shape: str, radius_ratio: float, color=BG) -> Image.Image:
    """同色底板（超采样后缩小，边缘平滑）。shape=circle 时半径取一半 → 正圆。"""
    big = Image.new("RGBA", (size * SS, size * SS), (0, 0, 0, 0))
    d = ImageDraw.Draw(big)
    box = [0, 0, size * SS - 1, size * SS - 1]
    if shape == "circle":
        d.ellipse(box, fill=color + (255,))
    else:
        d.rounded_rectangle(box, radius=radius_ratio * size * SS, fill=color + (255,))
    return big.resize((size, size), Image.LANCZOS)


def build(src_path: str, shape: str, radius_ratio: float) -> Image.Image:
    src = Image.open(src_path).convert("RGBA")
    w, h = src.size
    x0, y0, x1, y1 = src.getchannel("A").getbbox()
    side = max(x1 - x0, y1 - y0)
    cx, cy = (x0 + x1) / 2.0, (y0 + y1) / 2.0
    canvas = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    canvas.alpha_composite(plate(side, shape, radius_ratio),
                           (int(round(cx - side / 2.0)), int(round(cy - side / 2.0))))
    canvas.alpha_composite(key_out_cat(src))
    return canvas


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--shape", choices=["circle", "rounded"], default="circle")
    ap.add_argument("--radius", type=float, default=0.225, help="rounded 时的圆角比例")
    ap.add_argument("--src", default=SRC)
    args = ap.parse_args()

    base = build(args.src, args.shape, args.radius)
    # 预览的左侧 = 本次实际读入的母版（先取，避免和输出路径重合时读到新图）
    prev_src = Image.open(args.src).convert("RGBA").copy()
    for rel, size, fmt in OUTS:
        path = os.path.join(ROOT, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        img = base if base.size[0] == size else base.resize((size, size), Image.LANCZOS)
        if fmt == "WEBP":
            img.save(path, "WEBP", quality=90, method=6)
        else:
            img.save(path, "PNG", optimize=True)
        print(f"写出 {rel:34} {size}×{size}  {os.path.getsize(path):,} B")

    prev = Image.new("RGBA", (1024, 512), (238, 240, 246, 255))
    prev.alpha_composite(prev_src.resize((512, 512), Image.LANCZOS), (0, 0))
    prev.alpha_composite(base.resize((512, 512), Image.LANCZOS), (512, 0))
    out = os.environ.get("BRAND_PREVIEW", os.path.join(ROOT, ".tmp", "brand-preview.png"))
    os.makedirs(os.path.dirname(out), exist_ok=True)
    prev.convert("RGB").save(out, "PNG")
    print("预览（左=母版，右=新形状）：", out)


if __name__ == "__main__":
    sys.exit(main())
