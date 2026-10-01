"""生成 umi Downloader 应用图标（渐变 + 玻璃拟态 + 光晕）"""
from PIL import Image, ImageDraw, ImageFilter
import math
import os
import sys

SIZE = 1024
OUT = sys.argv[1] if len(sys.argv) > 1 else "app-icon.png"


def lerp(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))


def radial_bg(size):
    """深色背景 + 极光渐变"""
    img = Image.new("RGB", (size, size), (10, 10, 18))
    px = img.load()
    c1 = (124, 77, 255)   # umi purple
    c2 = (34, 211, 238)   # cyan
    c3 = (244, 114, 182)  # pink
    for y in range(size):
        for x in range(size):
            u = x / size
            v = y / size
            d1 = math.hypot(u - 0.18, v - 0.12)
            d2 = math.hypot(u - 0.88, v - 0.22)
            d3 = math.hypot(u - 0.55, v - 1.05)
            base = [12.0, 12.0, 22.0]
            for d, c, k in ((d1, c1, 1.15), (d2, c2, 0.95), (d3, c3, 0.85)):
                w = max(0.0, 1.0 - d * 2.1) ** 2 * k
                base[0] += c[0] * w * 0.55
                base[1] += c[1] * w * 0.55
                base[2] += c[2] * w * 0.55
            px[x, y] = (min(255, int(base[0])), min(255, int(base[1])), min(255, int(base[2])))
    return img


def rounded_mask(size, radius, shrink=0):
    m = Image.new("L", (size, size), 0)
    d = ImageDraw.Draw(m)
    pad = shrink
    d.rounded_rectangle([pad, pad, size - 1 - pad, size - 1 - pad], radius=radius, fill=255)
    return m


def draw_glow(img, box, color, blur=70, alpha=190):
    layer = Image.new("RGBA", img.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    d.ellipse(box, fill=color + (alpha,))
    layer = layer.filter(ImageFilter.GaussianBlur(blur))
    return Image.alpha_composite(img.convert("RGBA"), layer)


def main():
    base = radial_bg(SIZE).convert("RGBA")

    # 光晕
    base = draw_glow(base, [120, 120, 620, 620], (124, 77, 255), blur=110, alpha=150)
    base = draw_glow(base, [520, 420, 940, 860], (34, 211, 238), blur=110, alpha=110)

    layer = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)

    # 玻璃面板
    d.rounded_rectangle([150, 190, 874, 834], radius=170, fill=(255, 255, 255, 30),
                        outline=(255, 255, 255, 70), width=6)

    # 下载箭头
    cx, cy = 512, 500
    arrow = (255, 255, 255, 240)
    d.rounded_rectangle([cx - 26, cy - 168, cx + 26, cy + 16], radius=26, fill=arrow)
    d.polygon([(cx - 108, cy + 4), (cx + 108, cy + 4), (cx, cy + 132)], fill=arrow)

    # 底部托盘（下载底座）
    d.rounded_rectangle([cx - 168, cy + 190, cx + 168, cy + 246], radius=28, fill=(255, 255, 255, 225))

    img = Image.alpha_composite(base, layer)

    # 内部高光
    hi = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    hd = ImageDraw.Draw(hi)
    hd.ellipse([220, 210, 804, 480], fill=(255, 255, 255, 26))
    hi = hi.filter(ImageFilter.GaussianBlur(60))
    img = Image.alpha_composite(img, hi)

    # 圆角裁切
    mask = rounded_mask(SIZE, 224)
    out = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    out.paste(img, (0, 0), mask)
    out.save(OUT)
    print("saved", OUT, os.path.getsize(OUT), "bytes")


if __name__ == "__main__":
    main()
