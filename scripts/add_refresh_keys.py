"""补 3 条刷新相关文案到四个语言包。"""

import json
import re

KEY_PAT = re.compile(r'"((?:[^"\\]|\\.)*)"\s*:\s*"((?:[^"\\]|\\.)*)"')

NEW = {
    "刷新": {"zh": "刷新", "en": "Refresh", "ja": "更新", "fr": "Actualiser"},
    "刷新列表": {"zh": "刷新列表", "en": "Refresh list", "ja": "リストを更新", "fr": "Actualiser la liste"},
    "列表已刷新": {"zh": "列表已刷新", "en": "List refreshed", "ja": "リストを更新しました", "fr": "Liste actualisée"},
}

NOTE = {
    "zh": "/* 中文源文案（原文即 key，缺失翻译时回退到这里） */",
    "en": "/* Chinese source strings (key = source text; fallback locale) */",
    "ja": "/* 中国語の原文（key = 原文） */",
    "fr": "/* Textes sources chinois (clé = texte source) */",
}

for lang in ("zh", "en", "ja", "fr"):
    path = f"src/i18n/locales/{lang}.ts"
    s = open(path, encoding="utf-8", newline="").read()
    nl = "\r\n" if "\r\n" in s else "\n"
    d = dict(KEY_PAT.findall(s.split("export default", 1)[1]))
    for k, v in NEW.items():
        d[k] = v.get(lang, k)
    lines = [f'  {json.dumps(k, ensure_ascii=False)}: {json.dumps(v, ensure_ascii=False)},' for k, v in sorted(d.items())]
    open(path, "w", encoding="utf-8", newline="").write(NOTE[lang] + nl + "export default {" + nl + nl.join(lines) + nl + "}" + nl)
    print(f"{lang}.ts → {len(d)} 条")
