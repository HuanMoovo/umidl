"""给四个语言包补「统一转换流程」用到的 13 条键（转换页重构 1.7）。

保持：zh/ja/fr 的 CRLF、en 的 LF、2 空格缩进、按 key 码点排序插入，
只插入新增行，不重写整个文件。
"""
import json
import re

LOCALES = ['zh', 'en', 'ja', 'fr']
KEYLINE = re.compile(r'\s*"((?:[^"\\]|\\.)*)":')

NEW = {
    '可用 {n} 种': {
        'zh': '可用 {n} 种',
        'en': '{n} available',
        'ja': '利用可能 {n} 種',
        'fr': '{n} disponibles',
    },
    '当前筛选：{name}（{n}）': {
        'zh': '当前筛选：{name}（{n}）',
        'en': 'Filter: {name} ({n})',
        'ja': '絞り込み：{name}（{n}）',
        'fr': 'Filtre : {name} ({n})',
    },
    '搜索格式（名称 / 说明）': {
        'zh': '搜索格式（名称 / 说明）',
        'en': 'Search formats (name / description)',
        'ja': '形式を検索（名前 / 説明）',
        'fr': 'Rechercher un format (nom / description)',
    },
    '清除': {
        'zh': '清除',
        'en': 'Clear',
        'ja': 'クリア',
        'fr': 'Effacer',
    },
    '转换参数': {
        'zh': '转换参数',
        'en': 'Conversion options',
        'ja': '変換オプション',
        'fr': 'Options de conversion',
    },
    '没有匹配的格式（换个类型或清空搜索）': {
        'zh': '没有匹配的格式（换个类型或清空搜索）',
        'en': 'No matching formats (switch type or clear the search)',
        'ja': '一致する形式がありません（種類を変えるか検索をクリア）',
        'fr': 'Aucun format correspondant (changez de type ou effacez la recherche)',
    },
    '图片格式直接输出，无需编码参数。': {
        'zh': '图片格式直接输出，无需编码参数。',
        'en': 'Image formats are written as-is, no codec options needed.',
        'ja': '画像形式はそのまま出力され、コーデック設定は不要です。',
        'fr': "Les formats d'image sont écrits tels quels, sans options de codec.",
    },
    '文档格式在上方网格选择；转换按扩展名自动路由到文档引擎，不再走 ffmpeg。': {
        'zh': '文档格式在上方网格选择；转换按扩展名自动路由到文档引擎，不再走 ffmpeg。',
        'en': 'Pick the document target in the grid above; conversion is routed to the document engine by extension, not ffmpeg.',
        'ja': 'ドキュメントの出力形式は上のグリッドで選択します。変換は拡張子に応じてドキュメントエンジンへ振り分けられ、ffmpeg は使用しません。',
        'fr': "Choisissez le format cible dans la grille ci-dessus ; la conversion est routée vers le moteur document selon l'extension, sans ffmpeg.",
    },
    '（来自后端能力表）': {
        'zh': '（来自后端能力表）',
        'en': '(from the backend capability table)',
        'ja': '（バックエンドの対応表から）',
        'fr': '(d’après la table du backend)',
    },
    '（后端未提供能力表，使用内置列表）': {
        'zh': '（后端未提供能力表，使用内置列表）',
        'en': '(no capability table from the backend — using the built-in list)',
        'ja': '（バックエンドが対応表を提供していないため、内蔵リストを使用）',
        'fr': '(table indisponible côté backend — liste intégrée utilisée)',
    },
    '（浏览器预览，使用内置列表）': {
        'zh': '（浏览器预览，使用内置列表）',
        'en': '(browser preview — using the built-in list)',
        'ja': '（ブラウザプレビュー：内蔵リストを使用）',
        'fr': '(aperçu navigateur — liste intégrée utilisée)',
    },
    '已开始转换（图片）': {
        'zh': '已开始转换（图片）',
        'en': 'Conversion started (image)',
        'ja': '変換を開始しました（画像）',
        'fr': 'Conversion démarrée (image)',
    },
    '已重新检测转换引擎与格式目录': {
        'zh': '已重新检测转换引擎与格式目录',
        'en': 'Conversion engines and format catalog re-detected',
        'ja': '変換エンジンと形式カタログを再検出しました',
        'fr': 'Moteurs de conversion et catalogue de formats redétectés',
    },
    '{label}（CRF {crf}）': {
        'zh': '{label}（CRF {crf}）',
        'en': '{label} (CRF {crf})',
        'ja': '{label}（CRF {crf}）',
        'fr': '{label} (CRF {crf})',
    },
}


def entries(lines):
    return [(i, m.group(1)) for i, l in enumerate(lines) if (m := KEYLINE.match(l))]


for loc in LOCALES:
    path = f'src/i18n/locales/{loc}.ts'
    raw = open(path, encoding='utf-8', newline='').read()
    crlf = '\r\n' in raw
    lines = raw.replace('\r\n', '\n').split('\n')
    close = max(i for i, l in enumerate(lines) if l.strip() == '}')
    added = []
    for key, kv in NEW.items():
        ent = entries(lines)
        if any(k == key for _, k in ent):
            continue
        line = '  %s: %s,' % (json.dumps(key, ensure_ascii=False), json.dumps(kv[loc], ensure_ascii=False))
        pos = next((i for i, k in ent if k > key), None)
        if pos is None or pos > close:
            pos = close
        lines.insert(pos, line)
        close += 1
        added.append(key)
    out = ('\r\n' if crlf else '\n').join(lines)
    assert '\r\n' in out if crlf else '\r\n' not in out, f'{loc}: 行尾被破坏'
    open(path, 'w', encoding='utf-8', newline='').write(out)
    print(f'{loc}.ts 新增 {len(added)} 条 → 共 {len(entries(out.replace(chr(13)+chr(10), chr(10)).split(chr(10))))} 条')
