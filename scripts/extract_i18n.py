"""从 src 下抽取用户可见的中文文案，生成待翻译清单（import：用于 i18n 目录）。"""

import json
import os
import re

CJK = re.compile(r"[\u4e00-\u9fa5]")
# 模板里的文本节点与属性
ATTRS = ("title", "placeholder", "content", "label", "hint", "positiveText", "negativeText", "alt")


def strip_script(src: str):
    m = re.search(r"<script[^>]*>(.*?)</script>", src, re.S)
    return m.group(1) if m else ""


def template_of(src: str):
    m = re.search(r"<template>(.*?)</template>", src, re.S)
    return m.group(1) if m else ""


def extract_vue(path):
    src = open(path, encoding="utf-8").read()
    out = []
    tpl = template_of(src)
    tpl = re.sub(r"<!--.*?-->", "", tpl, flags=re.S)
    # 属性
    for a in ATTRS:
        for m in re.finditer(rf'{a}="([^"]*)"', tpl):
            if CJK.search(m.group(1)):
                out.append(m.group(1).strip())
    # 文本节点：标签之间的中文
    for m in re.finditer(r">\s*([^<>{}]*[\u4e00-\u9fa5][^<>{}]*?)\s*<", tpl):
        v = m.group(1).strip()
        if v and not v.startswith("/*"):
            out.append(v)
    # 花括号插值里的字符串（如 {{ x ? '是' : '否' }}）
    for m in re.finditer(r"'([^']*[\u4e00-\u9fa5][^']*)'", tpl):
        out.append(m.group(1).strip())
    # script 里的字符串字面量
    for m in re.finditer(r"'([^'\\\n]*[\u4e00-\u9fa5][^'\\\n]*)'", strip_script(src)):
        out.append(m.group(1).strip())
        for m2 in re.finditer(r"\$\{([^}]*)\}", m.group(1)):
            pass
    return out


def extract_ts(path):
    src = open(path, encoding="utf-8").read()
    out = []
    for line in src.splitlines():
        s = line.strip()
        if s.startswith(("//", "*", "/*", "*/")):
            continue
        for m in re.finditer(r"'([^'\\\n]*[\u4e00-\u9fa5][^'\\\n]*)'", line):
            out.append(m.group(1).strip())
        for m in re.finditer(r'`([^`\\\n]*[\u4e00-\u9fa5][^`\\\n]*)`', line):
            out.append(m.group(1).strip())
    return out


def main():
    result = {}
    for root, _, files in os.walk("src"):
        if "test" in root or "__tests__" in root:
            continue
        for f in files:
            p = os.path.join(root, f).replace("\\", "/")
            if f.endswith(".vue"):
                items = extract_vue(p)
            elif f.endswith(".ts"):
                items = extract_ts(p)
            else:
                continue
            # 去掉注释性残留、去重保序
            seen, uniq = set(), []
            for it in items:
                it = re.sub(r"\s+", " ", it).strip()
                if not it or it in seen or len(it) < 1:
                    continue
                seen.add(it)
                uniq.append(it)
            if uniq:
                result[p] = uniq
    total = sum(len(v) for v in result.values())
    json.dump(result, open("i18n-extract.json", "w", encoding="utf-8"), ensure_ascii=False, indent=1)
    print("文件数:", len(result), "文案条数:", total)
    for p, v in sorted(result.items(), key=lambda x: -len(x[1])):
        print(f"  {len(v):3}  {p}")


main()
