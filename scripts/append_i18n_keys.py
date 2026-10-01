"""把代码里新用到、语言包里还没有的 key 补进四个语言包（zh 用原文，其它语言待翻译）。"""

import json
import re
import sys

KEY_PAT = re.compile(r'"((?:[^"\\]|\\.)*)"\s*:\s*"((?:[^"\\]|\\.)*)"')


def rebuild(lang: str, additions: dict, note: str):
    path = f"src/i18n/locales/{lang}.ts"
    s = open(path, encoding="utf-8", newline="").read()
    nl = "\r\n" if "\r\n" in s else "\n"
    body = s.split("export default", 1)[1]
    pairs = KEY_PAT.findall(body)
    d = dict(pairs)
    for k, v in additions.items():
        d.setdefault(k, v)
    lines = [f'  {json.dumps(k, ensure_ascii=False)}: {json.dumps(v, ensure_ascii=False)},' for k, v in sorted(d.items())]
    out = note + nl + "export default {" + nl + nl.join(lines) + nl + "}" + nl
    open(path, "w", encoding="utf-8", newline="").write(out)
    return len(d)


def main():
    missing = json.load(open("i18n-missing.json", encoding="utf-8"))["zh"]
    added = {k: k for k in missing}
    n = rebuild("zh", added, "/* 中文源文案（原文即 key，缺失翻译时回退到这里） */")
    print(f"zh.ts → {n} 条（新增 {len(missing)}）")
    for lang in ("en", "ja", "fr"):
        s = open(f"src/i18n/locales/{lang}.ts", encoding="utf-8", newline="").read()
        have = len(KEY_PAT.findall(s.split("export default", 1)[1]))
        print(f"{lang}.ts 当前 {have} 条（待子代理翻译补齐 {len(missing)} 条）")


main()
