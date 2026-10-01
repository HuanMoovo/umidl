"""给四个语言包补 3 条 format_note 段用的键：最佳 / 封面图片 / 全部语言"""
import json
import re

KEYLINE = re.compile(r'\s*"((?:[^"\\]|\\.)*)":')

NEW = {
    'zh': {'最佳': '最佳', '封面图片': '封面图片', '全部语言': '全部语言'},
    'en': {'最佳': 'Best', '封面图片': 'Cover image', '全部语言': 'All languages'},
    'ja': {'最佳': '最適', '封面图片': 'カバー画像', '全部语言': 'すべての言語'},
    'fr': {'最佳': 'Optimal', '封面图片': 'Image de couverture', '全部语言': 'Toutes les langues'},
}


def entries(lines):
    out = []
    for i, l in enumerate(lines):
        m = KEYLINE.match(l)
        if m:
            out.append((i, m.group(1)))
    return out


for loc, kv in NEW.items():
    path = f'src/i18n/locales/{loc}.ts'
    raw = open(path, encoding='utf-8', newline='').read()
    crlf = '\r\n' in raw
    lines = raw.replace('\r\n', '\n').split('\n')
    added = []
    for k, v in kv.items():
        ent = entries(lines)
        if any(key == k for _, key in ent):
            continue
        line = '  %s: %s,' % (json.dumps(k, ensure_ascii=False), json.dumps(v, ensure_ascii=False))
        pos = next((i for i, key in ent if key > k), None)
        if pos is None:
            last = ent[-1][0]
            lines[last] = lines[last].rstrip().rstrip(',')
            lines.insert(last + 1, line)
        else:
            lines.insert(pos, line)
        added.append(k)
    out = ('\r\n' if crlf else '\n').join(lines)
    open(path, 'w', encoding='utf-8', newline='').write(out)
    print(f'{loc}.ts 新增 {len(added)} 条 {added}')
