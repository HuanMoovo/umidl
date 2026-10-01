"""把落地页 docs/index.html 里两处内联矢量 logo 换成猫咪图（128px PNG 的 base64）。

替换判据（不靠行号，避免页面后续改动错位）：
- 一个 <svg ...>...</svg> 块，若它的属性或内容里带有 aria-label="Umidl logo"
  或者 rect 用了 url(#g)、url(#g2) 渐变填充 —— 就是那两处 logo，整体换掉。
"""
import base64
import os
import re

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PAGE = os.path.join(ROOT, "docs", "index.html")
PNG = os.path.join(ROOT, "assets", "logo-cat-128.png")

b64 = base64.b64encode(open(PNG, "rb").read()).decode()
img = (f'<img class="cat-logo" alt="Umidl" '
       f'src="data:image/png;base64,{b64}" width="64" height="64" '
       f'style="display:block;width:64px;height:64px;border-radius:50%" />')

raw = open(PAGE, encoding="utf-8", newline="").read()
crlf = "\r\n" in raw
s = raw.replace("\r\n", "\n")

hits = []
out = []
pos = 0
for m in re.finditer(r"<svg\b.*?</svg>", s, flags=re.S):
    block = m.group(0)
    if ('aria-label="Umidl logo"' in block) or ("url(#g" in block):
        hits.append(m.start())
        out.append(s[pos:m.start()])
        out.append(img)
        pos = m.end()
out.append(s[pos:])
s2 = "".join(out)

print(f"替换 logo 块数：{len(hits)}")
if len(hits) == 0:
    raise SystemExit("没有匹配到 logo 块，未写入（避免误改页面）")

# 追加一点点样式，让窄屏也不撑破
if ".cat-logo{" not in s2:
    s2 = s2.replace("</style>", "  .cat-logo{flex:0 0 auto}\n</style>", 1)

open(PAGE, "w", encoding="utf-8", newline="").write(s2.replace("\n", "\r\n") if crlf else s2)
print("docs/index.html 已更新，新大小：", os.path.getsize(PAGE), "字节")
print("仍含 <svg 的个数：", len(re.findall(r"<svg\b", s2)))
