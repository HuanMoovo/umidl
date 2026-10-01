"""校对语言包：从代码里抽出所有 $t('..')/tr('..') 的 key，与四个语言包对比。"""

import json
import os
import re
import sys

KEY_RE = re.compile(r"(?:\$t|\btr|\bt)\(\s*'((?:[^'\\]|\\.)*)'")
LANGS = ["zh", "en", "ja", "fr"]


def load_catalog(lang: str) -> dict:
    p = f"src/i18n/locales/{lang}.ts"
    s = open(p, encoding="utf-8").read()
    body = s.split("export default", 1)[1].strip()
    body = body.rstrip().rstrip(";").rstrip()
    # 去掉 JS 注释与尾随逗号，使其可被 JSON 解析
    body = re.sub(r"/\*.*?\*/", "", body, flags=re.S)
    body = re.sub(r"(?m)^\s*//.*$", "", body)
    body = re.sub(r",(\s*[}\]])", r"\1", body)
    return json.loads(body)


def code_keys() -> set:
    keys = set()
    for root, _, files in os.walk("src"):
        parts = root.replace(os.sep, "/").split("/")
        if "i18n" in parts:
            continue
        for f in files:
            if not f.endswith((".vue", ".ts")) or "test" in f:
                continue
            src = open(os.path.join(root, f), encoding="utf-8").read()
            # 模板部分 + 脚本部分统一扫
            for m in KEY_RE.finditer(src):
                # 源码里的 JS 转义要还原成运行时字符串再比对：`'… D:\\Umidl\\tools …'` 运行时是
                # 反斜杠单写，语言包 key 也是单写 —— 不还原就会把这两条报成「四语都缺」的假故障。
                keys.add(m.group(1).replace("\\\\", "\\"))
    return keys


def main():
    keys = code_keys()
    print(f"代码中使用的 key 数: {len(keys)}")
    cats = {l: load_catalog(l) for l in LANGS}
    for l in LANGS:
        print(f"  {l}.ts 条目: {len(cats[l])}")
    missing = {l: sorted(k for k in keys if k not in cats[l]) for l in LANGS}
    for l in LANGS:
        ms = missing[l]
        print(f"\n缺少 {len(ms)} 条 —— {l}.ts:")
        for k in ms:
            print(f"   {k}")
    # 语言包里有但代码不再使用的（信息性）
    for l in ["en", "ja", "fr"]:
        extra = sorted(k for k in cats[l] if k not in keys)
        if extra:
            print(f"\n{l}.ts 中代码未使用: {len(extra)} 条（示例 {extra[:5]}）")
    # key 一致性
    zh_keys = set(cats["zh"])
    for l in ["en", "ja", "fr"]:
        diff = set(cats[l]) ^ zh_keys
        print(f"key 一致性 {l} vs zh: {'一致' if not diff else f'差异 {len(diff)} 条'}")
    json.dump({l: missing[l] for l in LANGS}, open("i18n-missing.json", "w", encoding="utf-8"),
              ensure_ascii=False, indent=1)


main()
