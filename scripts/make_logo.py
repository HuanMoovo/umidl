"""把品牌美术（浅蓝底板 + 插画猫）生成矢量 SVG。

做法（底板用真几何、只描摹猫，边缘不走样）：
  1. 按「平涂底色」把底板和猫分开：抠出透明底的猫；同时用同色像素的 bbox 反推
     底板几何（圆形 = 外接正方形的内切圆；圆角矩形取 rx = radius × 边长）
  2. 底板用 <circle> / <rect rx> 画 —— 边缘绝对平滑，不会出现描摹晕圈
  3. 猫单独交给 vtracer（保留原始坐标与配色），叠在底板上
  4. 输出：
       assets/logo.svg        底板猫 + 「Umidl」字标（宽版）
       assets/logo-mark.svg   底板猫（正方形画布，小尺寸 / 侧栏用）
       assets/icon.svg        底板猫（应用图标源图）
     logo.svg / logo-mark.svg 同时写一份到 src/assets/（前端静态资源位置，保持同步）

历史坑：本脚本一度直接描摹整张位图 —— 底板的抗锯齿边被描成浅色晕圈；更早还描摹
最早那版「扁平蓝猫」参考图，重跑一次就把 SVG 退回旧版。现在形状由参数决定，
底板几何与猫分开处理，重跑安全。

用法：python scripts/make_logo.py [源图.png] [--shape circle|rounded] [--radius 0.225]
"""
import argparse
import io
import os
import re
import tempfile

import numpy as np
import vtracer
from PIL import Image

SRC_DEFAULT = os.path.join('assets', 'logo-cat-256.png')
OUT_DIR = 'assets'
SRC_ASSETS = os.path.join('src', 'assets')

PLATE_RGB = (122, 204, 253)   # 实测平涂底色
T0, T1 = 10, 42               # 色键软过渡区间（切比雪夫距离）

# 描摹参数（只作用于猫）：偏「简化 + 圆滑」
TRACE = dict(
    colormode='color',
    hierarchical='stacked',
    mode='spline',
    filter_speckle=8,
    color_precision=5,
    layer_difference=28,
    corner_threshold=60,
    length_threshold=4.0,
    max_iterations=10,
    splice_threshold=45,
    path_precision=1,
)

HEAD = ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="%s" role="img" aria-label="Umidl">\n'
        '  <title>Umidl</title>\n')


def split_cat_and_plate(src: str, shape: str = 'circle', radius: float = 0.225):
    """返回 (透明底的猫, (x, y, w, h))；bbox 为底板外接矩形（按 alpha 定界，
    这样抗锯齿的边沿也算进去，圆形/圆角矩形都不会偏小）。
    抠猫之后再把底板形状之外的像素清零 —— 否则底板边缘的浅色残影会被描摹成晕圈。"""
    from PIL import ImageDraw
    a = np.asarray(Image.open(src).convert('RGBA')).astype(np.int16)
    rgb, alpha = a[..., :3], a[..., 3]
    dist = np.max(np.abs(rgb - np.array(PLATE_RGB, dtype=np.int16)), axis=2)
    ys, xs = np.where(alpha >= 64)
    if len(xs) == 0:
        h, w = alpha.shape
        box = (0, 0, w, h)
    else:
        box = (int(xs.min()), int(ys.min()), int(xs.max() - xs.min() + 1), int(ys.max() - ys.min() + 1))
    t = np.clip((dist - T0) / float(T1 - T0), 0.0, 1.0)
    out = a.copy()
    out[..., 3] = (alpha * t).astype(np.int16)
    x, y, w, h = box
    mask = Image.new('L', (alpha.shape[1], alpha.shape[0]), 0)
    d = ImageDraw.Draw(mask)
    if shape == 'circle':
        d.ellipse([x, y, x + w - 1, y + h - 1], fill=255)
    else:
        d.rounded_rectangle([x, y, x + w - 1, y + h - 1], radius=radius * min(w, h), fill=255)
    out[..., 3] = np.where(np.asarray(mask) > 0, out[..., 3], 0).astype(np.int16)
    return Image.fromarray(out.astype(np.uint8), 'RGBA'), box


def plate_svg(box, shape: str, radius: float) -> str:
    x, y, w, h = box
    fill = '#%02x%02x%02x' % PLATE_RGB
    if shape == 'circle':
        return ('<circle cx="%.1f" cy="%.1f" r="%.1f" fill="%s"/>'
                % (x + w / 2.0, y + h / 2.0, min(w, h) / 2.0, fill))
    return ('<rect x="%d" y="%d" width="%d" height="%d" rx="%.1f" fill="%s"/>'
            % (x, y, w, h, radius * min(w, h), fill))


def trace_cat(cat_png: str, work_dir: str):
    """描摹透明底的猫 → (路径内容, 宽, 高)（与源图同坐标系）"""
    raw = os.path.join(work_dir, 'cat_raw.svg')
    vtracer.convert_image_to_svg_py(cat_png, raw, **TRACE)
    body = io.open(raw, encoding='utf-8').read()
    w = int(re.search(r'width="(\d+)"', body).group(1))
    h = int(re.search(r'height="(\d+)"', body).group(1))
    inner = body[body.find('>', body.find('<svg')) + 1:body.rfind('</svg>')].strip()
    return inner, w, h


def build_mark(src: str, work_dir: str, shape: str, radius: float):
    cat, box = split_cat_and_plate(src, shape, radius)
    cat_png = os.path.join(work_dir, 'cat.png')
    cat.save(cat_png)
    inner, w, h = trace_cat(cat_png, work_dir)
    body = ('  ' + plate_svg(box, shape, radius) + '\n'
            + '  <g>\n' + inner + '\n  </g>\n')
    return body, w, h


def square(body: str, w: int, h: int) -> str:
    """正方形画布（底板猫原样，补齐 viewBox）"""
    return HEAD % ('0 0 %d %d' % (w, h)) + body + '</svg>\n'


def wordmark(body: str, w: int, h: int) -> str:
    """宽版字标：左侧底板猫 + 右侧「Umidl」（1136×384）"""
    W, H = 1136, 384
    scale = 352 / w
    return (
        HEAD % ('0 0 %d %d' % (W, H))
        + '  <g transform="translate(16 %.1f) scale(%.5f)">\n' % ((H - h * scale) / 2, scale)
        + body
        + '  </g>\n'
        + '  <text x="404" y="252" font-family="Inter, Segoe UI, system-ui, sans-serif"'
          ' font-size="170" font-weight="700" fill="#22304a" letter-spacing="-2">Umidl</text>\n'
        + '</svg>\n'
    )


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument('src', nargs='?', default=SRC_DEFAULT)
    ap.add_argument('--shape', choices=['circle', 'rounded'], default='circle')
    ap.add_argument('--radius', type=float, default=0.225, help='rounded 时的圆角比例')
    args = ap.parse_args()
    if not os.path.exists(args.src):
        raise SystemExit('找不到源图：%s' % args.src)

    with tempfile.TemporaryDirectory(prefix='umi-logo-') as work:
        body, w, h = build_mark(args.src, work, args.shape, args.radius)

    mark = square(body, w, h)
    logo = wordmark(body, w, h)

    outputs = [
        (os.path.join(OUT_DIR, 'logo.svg'), logo),
        (os.path.join(OUT_DIR, 'logo-mark.svg'), mark),
        (os.path.join(OUT_DIR, 'icon.svg'), mark),
        (os.path.join(SRC_ASSETS, 'logo.svg'), logo),
        (os.path.join(SRC_ASSETS, 'logo-mark.svg'), mark),
    ]
    for path, text in outputs:
        os.makedirs(os.path.dirname(path) or '.', exist_ok=True)
        io.open(path, 'w', encoding='utf-8', newline='').write(text)
        print('写出 %-34s %6d 字节' % (path, len(text.encode('utf-8'))))
    print('源图 %s → %dx%d（%s），共 %d 个文件' % (args.src, w, h, args.shape, len(outputs)))


if __name__ == '__main__':
    main()
