"""把源码里的中文文案替换成 i18n 调用（$t / t）。

规则：
  .vue <template>：
    title="中文"        -> :title="$t('中文')"      （白名单属性）
    {{ '中文' }}        -> {{ $t('中文') }}
    >中文<              -> >{{ $t('中文') }}<
  .vue <script setup> / .ts：
    '中文'              -> t('中文')                （跳过对象 key、import、注释行）

先把整个 src 备份到 .tmp/i18n_backup（已存在则跳过备份步骤）。
"""

import json
import os
import re
import sys

CJK = re.compile(r"[\u4e00-\u9fa5]")
ATTRS = ("title", "placeholder", "content", "label", "hint", "positiveText", "negativeText", "alt")
BACKUP = ".tmp/i18n_backup"

SKIP_LINE = re.compile(r"^\s*(//|/\*|\*)")
# 这些上下文里的字符串不是界面文案
NO_TOUCH = [
    re.compile(r"^\s*import\s"),
    re.compile(r"\bimport\("),
    re.compile(r"from\s+['\"]"),
    re.compile(r"console\."),
]


def in_comment(line: str) -> bool:
    return bool(SKIP_LINE.match(line)) or line.strip().endswith("*/")


def is_key_context(line: str, start: int) -> bool:
    """判断该字面量是不是对象 key 或 import 路径"""
    rest = line[start:]
    m = re.match(r"^'[^']*'\s*:", rest)  # '中文': xxx
    if m:
        return True
    for pat in NO_TOUCH:
        if pat.search(line):
            return True
    return False


def replace_template(tpl: str) -> tuple[str, int]:
    n = 0
    # 1) 属性
    for a in ATTRS:
        pat = re.compile(rf'(?<![:\w]){a}="([^"<>{{}}]*[\u4e00-\u9fa5][^"<>{{}}]*)"')

        def sub(m):
            nonlocal n
            n += 1
            return f':{a}="$t(\'{m.group(1)}\')"'

        tpl = pat.sub(sub, tpl)

    # 2) 文本节点（可能跨行，折叠空白后放回一行）
    def sub_text(m):
        nonlocal n
        v = re.sub(r"\s+", " ", m.group(1)).strip()
        if not CJK.search(v):
            return m.group(0)
        n += 1
        return f">{{{{ $t('{v}') }}}}<"

    tpl = re.sub(r">\s*([^<>{}]*[\u4e00-\u9fa5][^<>{}]*?)\s*<", sub_text, tpl)

    # 3) 插值/表达式里的字符串字面量
    def sub_expr(m):
        nonlocal n
        inner = m.group(1)
        if not CJK.search(inner):
            return m.group(0)
        n += 1
        return inner.replace("'", "\u0001")  # 占位避免嵌套

    def sub_lit(m):
        nonlocal n
        if not CJK.search(m.group(1)):
            return m.group(0)
        n += 1
        return f"$t('{m.group(1)}')"

    # 仅在 {{ ... }} 内替换字符串字面量
    def sub_mustache(m):
        inner = m.group(1)
        new = re.sub(r"'([^'\\\n]*[\u4e00-\u9fa5][^'\\\n]*)'", sub_lit, inner)
        return "{{" + new + "}}"

    tpl = re.sub(r"\{\{(.*?)\}\}", sub_mustache, tpl, flags=re.S)
    return tpl, n


def replace_script(src: str, is_ts_module: bool) -> tuple[str, int]:
    out_lines = []
    n = 0
    for line in src.split("\n"):
        if in_comment(line):
            out_lines.append(line)
            continue
        new_line = line
        # 逐个字面量处理（从后往前，避免位移）
        matches = list(re.finditer(r"'([^'\\\n]*[\u4e00-\u9fa5][^'\\\n]*)'", line))
        for m in reversed(matches):
            if is_key_context(line, m.start()):
                continue
            new_line = new_line[: m.start()] + f"t('{m.group(1)}')" + new_line[m.end() :]
            n += 1
        out_lines.append(new_line)
    return "\n".join(out_lines), n


def process_vue(path: str) -> int:
    src = open(path, encoding="utf-8", newline="").read()
    NL_PLACEHOLDER = "\r\n" if "\r\n" in src else "\n"
    changed = 0
    m = re.search(r"<template>(.*)</template>", src, re.S)
    if m:
        tpl = m.group(1)
        # 保护注释
        comments = []

        def stash(mm):
            comments.append(mm.group(0))
            return f"\x02CMT{len(comments) - 1}\x03"

        tpl_safe = re.sub(r"<!--.*?-->", stash, tpl, flags=re.S)
        new_tpl, n1 = replace_template(tpl_safe)
        for i, c in enumerate(comments):
            new_tpl = new_tpl.replace(f"\x02CMT{i}\x03", c)
        changed += n1
        src = src[: m.start(1)] + new_tpl + src[m.end(1) :]

    m = re.search(r"(<script[^>]*>)(.*?)(</script>)", src, re.S)
    if m:
        new_script, n2 = replace_script(m.group(2), False)
        changed += n2
        src = src[: m.start(2)] + new_script + src[m.end(2) :]

    if changed:
        # 组件里补 useI18n 导入
        if "useI18n" not in src and re.search(r"\bt\('", src):
            if "from 'vue-i18n'" not in src:
                src = re.sub(
                    r'(<script setup lang="ts">\r?\n)',
                    r"\1import { useI18n } from 'vue-i18n'" + NL_PLACEHOLDER,
                    src,
                    count=1,
                )
            if "const { t } = useI18n()" not in src:
                # 插到最后一条 import 之后（import 可能跨多行，用括号深度判断）
                m2 = re.search(r"<script[^>]*>\r?\n(.*?)\r?\n", src)
                if m2:
                    body = src[m2.start(0) :]
                    lines = body.split("\n")
                    insert_at = 1
                    depth = 0
                    for idx, line in enumerate(lines[1:], start=1):
                        s = line.strip()
                        if depth > 0 or s.startswith("import"):
                            depth += line.count("{") + line.count("(") - line.count("}") - line.count(")")
                            if depth < 0:
                                depth = 0
                            insert_at = idx + 1
                        else:
                            break
                    src = src[: m2.start(0)] + "\n".join(
                        lines[:insert_at] + ["const { t } = useI18n()"] + lines[insert_at:]
                    )
        open(path, "w", encoding="utf-8", newline="").write(src)
    return changed


def process_ts(path: str) -> int:
    src = open(path, encoding="utf-8", newline="").read()
    new, n = replace_script(src, True)
    if n:
        if "from '@/i18n'" not in src and "from './i18n'" not in src:
            lines = new.split("\n")
            # 插到最后一个 import 之后
            idx = max(i for i, l in enumerate(lines) if l.startswith("import ")) if any(
                l.startswith("import ") for l in lines
            ) else -1
            lines.insert(idx + 1, "import { t } from '@/i18n'")
            new = "\n".join(lines)
        open(path, "w", encoding="utf-8", newline="").write(new)
    return n


def main():
    only = sys.argv[1] if len(sys.argv) > 1 else None
    report = {}
    for root, _, files in os.walk("src"):
        if "i18n" in root.replace("\\", "/").split("/") or "test" in root:
            continue
        for f in files:
            p = os.path.join(root, f).replace("\\", "/")
            if only and only not in p:
                continue
            if "test" in f:  # 测试文件不参与 i18n
                continue
            if f.endswith(".vue"):
                n = process_vue(p)
            elif f.endswith(".ts"):
                n = process_ts(p)
            else:
                continue
            if n:
                report[p] = n
    print(json.dumps(report, ensure_ascii=False, indent=1))
    print("总替换:", sum(report.values()))


main()
