"""补「自定义 LOGO」相关文案到四个语言包（按码点顺序插入）"""
import json
import re

KEYLINE = re.compile(r'\s*"((?:[^"\\]|\\.)*)":')

NEW = {
    'zh': {
        '自定义 LOGO': '自定义 LOGO',
        '选择图片…': '选择图片…',
        '恢复默认': '恢复默认',
        '支持 PNG / JPG / WEBP / SVG / ICO，4 MB 以内。PNG、ICO 会同时替换窗口与任务栏图标。':
            '支持 PNG / JPG / WEBP / SVG / ICO，4 MB 以内。PNG、ICO 会同时替换窗口与任务栏图标。',
        '当前使用自定义 LOGO：': '当前使用自定义 LOGO：',
        '已应用自定义 LOGO': '已应用自定义 LOGO',
        '已恢复默认 LOGO': '已恢复默认 LOGO',
    },
    'en': {
        '自定义 LOGO': 'Custom logo',
        '选择图片…': 'Choose image…',
        '恢复默认': 'Restore default',
        '支持 PNG / JPG / WEBP / SVG / ICO，4 MB 以内。PNG、ICO 会同时替换窗口与任务栏图标。':
            'PNG / JPG / WEBP / SVG / ICO up to 4 MB. PNG and ICO also replace the window and taskbar icon.',
        '当前使用自定义 LOGO：': 'Using custom logo: ',
        '已应用自定义 LOGO': 'Custom logo applied',
        '已恢复默认 LOGO': 'Default logo restored',
    },
    'ja': {
        '自定义 LOGO': 'カスタムロゴ',
        '选择图片…': '画像を選択…',
        '恢复默认': '既定に戻す',
        '支持 PNG / JPG / WEBP / SVG / ICO，4 MB 以内。PNG、ICO 会同时替换窗口与任务栏图标。':
            'PNG / JPG / WEBP / SVG / ICO、4 MB まで。PNG・ICO はウィンドウとタスクバーのアイコンも置き換えます。',
        '当前使用自定义 LOGO：': '使用中のカスタムロゴ：',
        '已应用自定义 LOGO': 'カスタムロゴを適用しました',
        '已恢复默认 LOGO': '既定のロゴに戻しました',
    },
    'fr': {
        '自定义 LOGO': 'Logo personnalisé',
        '选择图片…': 'Choisir une image…',
        '恢复默认': 'Rétablir par défaut',
        '支持 PNG / JPG / WEBP / SVG / ICO，4 MB 以内。PNG、ICO 会同时替换窗口与任务栏图标。':
            "PNG / JPG / WEBP / SVG / ICO jusqu'à 4 Mo. PNG et ICO remplacent aussi l'icône de fenêtre et de la barre des tâches.",
        '当前使用自定义 LOGO：': 'Logo personnalisé utilisé : ',
        '已应用自定义 LOGO': 'Logo personnalisé appliqué',
        '已恢复默认 LOGO': 'Logo par défaut rétabli',
    },
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
    added = 0
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
        added += 1
    open(path, 'w', encoding='utf-8', newline='').write(('\r\n' if crlf else '\n').join(lines))
    print(f'{loc}.ts 新增 {added} 条')
