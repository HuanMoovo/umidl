import sys
"""审计：模板/脚本里还有哪些中文没被 i18n 包住。"""

import os
import re

CJK = re.compile(r"[\u4e00-\u9fa5]")
SKIP_LINE = re.compile(r"^\s*(//|/\*|\*)")


def strip_wrapped(text: str) -> str:
    """把 $t('...') / tr('...') / {{ $t('...') }} 里的内容挖掉"""
    prev = None
    while prev != text:
        prev = text
        text = re.sub(r"\$t\('([^']*)'\)", "", text)
        text = re.sub(r"\btr\('([^']*)'\)", "", text)
    return text


report = []
for root, _, files in os.walk("src"):
    parts = root.replace(os.sep, "/").split("/")
    if "i18n" in parts:
        continue
    for f in files:
        if not f.endswith((".vue", ".ts")) or "test" in f:
            continue
        path = os.path.join(root, f).replace("\\", "/")
        src = open(path, encoding="utf-8").read()
        for i, line in enumerate(src.splitlines(), 1):
            if not CJK.search(line):
                continue
            s = line.strip()
            if s.startswith(("//", "/*", "*", "<!--")):
                continue
            left = strip_wrapped(line)
            if CJK.search(left):
                report.append((path, i, s[:150]))

print(f"残留未国际化行数: {len(report)}")
from collections import Counter

c = Counter(p for p, _, _ in report)
for p, n in c.most_common():
    print(f"  {n:3}  {p}")
print()
only = sys.argv[1] if len(sys.argv) > 1 else None
shown = [r for r in report if not only or only in r[0]]
for p, i, s in shown:
    print(f"{p}:{i}  {s}")
