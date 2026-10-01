#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""i18n 缺口检查：四语言包 key 集合一致性 + 代码里用到但缺翻译的文案。

用法：
    python scripts/i18n_missing.py            # 报告
    python scripts/i18n_missing.py --zh-only  # 只列出代码里用了但 zh 都没有的 key
"""
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOCALES = os.path.join(ROOT, "src", "i18n", "locales")
LANGS = ["zh", "en", "ja", "fr"]

# 代码里的取词调用：tr('x') / $t('x') / t('x') / tr("x")
CALL_RE = re.compile(r"""(?<![\w.])(?:\$?t|tr)\(\s*(['"])((?:\\.|(?!\1).)*)\1""")
# 语言包条目：  "key": "value",
ENTRY_RE = re.compile(r"""^\s*(['"])((?:\\.|(?!\1).)*)\1\s*:""", re.M)


def load_keys(lang: str) -> set:
    p = os.path.join(LOCALES, f"{lang}.ts")
    raw = open(p, encoding="utf-8").read()
    # 去掉块注释与行注释，避免注释里的示例被当成条目
    raw = re.sub(r"/\*.*?\*/", "", raw, flags=re.S)
    raw = re.sub(r"^\s*//.*$", "", raw, flags=re.M)
    return {m.group(2) for m in ENTRY_RE.finditer(raw)}


def code_keys() -> set:
    found = set()
    for dirpath, _dirs, files in os.walk(os.path.join(ROOT, "src")):
        for f in files:
            if not f.endswith((".vue", ".ts")):
                continue
            if f.endswith(".test.ts") or "/i18n/locales/" in dirpath.replace("\\", "/"):
                continue
            raw = open(os.path.join(dirpath, f), encoding="utf-8", errors="replace").read()
            for m in CALL_RE.finditer(raw):
                k = m.group(2)
                if k and not k.startswith("@") and "\n" not in k:
                    found.add(k)
    return found


def main() -> int:
    keys = {lang: load_keys(lang) for lang in LANGS}
    base = keys["zh"]
    ok = True

    print("══ 1. 四语言 key 集合一致性")
    for lang in LANGS[1:]:
        missing = base - keys[lang]
        extra = keys[lang] - base
        flag = "✓" if not missing and not extra else "✗"
        if flag == "✗":
            ok = False
        print(f"  {flag} {lang}: 条目 {len(keys[lang])} ｜ 相对 zh 缺 {len(missing)} 多 {len(extra)}")
        if missing:
            print("      缺:", sorted(missing)[:12])
        if extra:
            print("      多:", sorted(extra)[:12])

    used = code_keys()
    absent_all = sorted(k for k in used if k not in base)
    absent_some = sorted(k for k in used if k in base and any(k not in keys[l] for l in LANGS[1:]))

    print("\n══ 2. 代码里用到、但 zh 里都没有的文案（会原样显示中文，建议补进四语言包）")
    print(f"  共 {len(absent_all)} 条")
    for k in absent_all[:40]:
        print("   •", k)

    print("\n══ 3. zh 有、其它语言缺的文案（会造成英/日/法界面露中文）")
    print(f"  共 {len(absent_some)} 条")
    for k in absent_some[:40]:
        print("   •", k)

    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
