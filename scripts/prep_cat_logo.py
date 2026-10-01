"""把用户提供的猫咪图变成 Umidl 的 LOGO 资产。

要点：
- 原图 1254×1254 RGB，四角是近白 254（不是纯白），圆外要做成透明；
- 猫脸/爪子本身有大量真正的白色 —— 所以不能按「白色→透明」全图过滤，
  必须从四个角**泛洪填充**（floodfill + thresh），只吃掉与边缘连通的背景；
- 抠完按内容包围盒裁切，居中放进正方形画布（留 2% 余量），LANCZOS 缩放。

产物：
- assets/icon-cat.png       1024×1024（给 tauri icon 生成全套图标）
- assets/logo-cat-256.png   256×256（README / Release 用）
- assets/logo-cat-128.png   128×128（落地页内联 base64 用，控制体积）
- .tmp/cat-logo/32.png      32×32（目视检查小尺寸可辨识度）
"""
from PIL import Image, ImageDraw
import os

SRC = r"C:\Users\user\AppData\Roaming\Hermes\composer-images\ChatGPT_Image_2026年9月28日_08_04_31_6669ab.png"
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, ".tmp", "cat-logo")
os.makedirs(OUT, exist_ok=True)

im = Image.open(SRC).convert("RGBA")
w, h = im.size
rgb = im.convert("RGB")

# 从四角泛洪：把与边缘连通的近白背景涂成标记色
MARK = (255, 0, 255)
for seed in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)]:
    ImageDraw.floodfill(rgb, seed, MARK, thresh=16)

px_in = im.load()
px_rgb = rgb.load()
cleared = 0
for y in range(h):
    for x in range(w):
        if px_rgb[x, y] == MARK:
            r, g, b, _ = px_in[x, y]
            px_in[x, y] = (r, g, b, 0)
            cleared += 1
print(f"泛洪清除背景像素：{cleared}（占 {cleared / (w * h) * 100:.1f}%）")

bbox = im.getbbox()
print("内容包围盒：", bbox, "→", (bbox[2] - bbox[0], bbox[3] - bbox[1]))
im = im.crop(bbox)

# 居中放进正方形，留 2% 余量
side = int(max(im.size) * 1.04)
canvas = Image.new("RGBA", (side, side), (0, 0, 0, 0))
canvas.paste(im, ((side - im.width) // 2, (side - im.height) // 2), im)

targets = [
    ("assets/icon-cat.png", 1024),
    ("assets/logo-cat-256.png", 256),
    ("assets/logo-cat-128.png", 128),
]
for rel, size in targets:
    out = canvas.resize((size, size), Image.LANCZOS)
    path = os.path.join(ROOT, rel)
    out.save(path, "PNG", optimize=True)
    print(f"写出 {rel}：{size}×{size}｜{os.path.getsize(path)} 字节")

canvas.resize((32, 32), Image.LANCZOS).save(os.path.join(OUT, "32.png"), "PNG")
canvas.resize((64, 64), Image.LANCZOS).save(os.path.join(OUT, "64.png"), "PNG")
print("小尺寸预览：", os.path.join(OUT, "32.png"))
