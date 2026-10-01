#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""1.6 前端新增文案：按「排序插入」方式追加到四语言包，保持 CRLF/LF 与既有译文零改动。

- zh/ja/fr 保持 CRLF，en 保持 LF，均无 BOM；
- 只做「插入」：写回后逐条比对，任何既有 key→value 发生变化即报错退出；
- 插入后重新校验：四包条目数一致、key 集合一致、顺序与 zh 完全一致。

用法：
    python scripts/umi_v16_i18n_keys.py            # 应用（幂等）
    python scripts/umi_v16_i18n_keys.py --check    # 只检查不写入
"""
import bisect
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOCALES = os.path.join(ROOT, "src", "i18n", "locales")
LANGS = ["zh", "en", "ja", "fr"]

ENTRY = re.compile(r'^\s*"((?:\\.|[^"\\])*)"\s*:\s*"((?:\\.|[^"\\])*)"\s*,\s*$')

# 已废弃 / 改写的旧 key：脚本重跑时从四语言包里清理掉（避免留下无人使用的文案）
DROP: list[str] = [
    "每行一个链接，支持粘贴多行；空行与 # 注释行会被忽略。",
    "每行一个链接，支持粘贴多行；空行会被忽略。",
]

# ---------------------------------------------------------------------------
# 1.6 新增文案（key = 中文原文；四语言值必须占位符一致）
# ---------------------------------------------------------------------------
NEW: dict[str, dict[str, str]] = {
    # ── 字幕：导出语言多选 + 翻译成英文 ──────────────────────────────────
    "识别语言（可多选）": {
        "zh": "识别语言（可多选）",
        "en": "Language (multi-select)",
        "ja": "認識言語（複数選択）",
        "fr": "Langue (sélection multiple)",
    },
    "每个选中的语言各起一个转写任务": {
        "zh": "每个选中的语言各起一个转写任务",
        "en": "Each selected language starts its own transcription task",
        "ja": "選択した言語ごとに文字起こしタスクを 1 つ作成します",
        "fr": "Chaque langue sélectionnée lance sa propre tâche de transcription",
    },
    "翻译成英文": {
        "zh": "翻译成英文",
        "en": "Translate to English",
        "ja": "英語に翻訳",
        "fr": "Traduire en anglais",
    },
    "每个语言额外生成一份英文字幕": {
        "zh": "每个语言额外生成一份英文字幕",
        "en": "An extra English subtitle is generated for each language",
        "ja": "言語ごとに英語字幕を追加生成します",
        "fr": "Une sous-titre anglais supplémentaire est généré pour chaque langue",
    },
    "开始转写（{n} 个任务）": {
        "zh": "开始转写（{n} 个任务）",
        "en": "Start transcription ({n} tasks)",
        "ja": "文字起こしを開始（{n} 件のタスク）",
        "fr": "Lancer la transcription ({n} tâches)",
    },
    "请至少选择一个识别语言": {
        "zh": "请至少选择一个识别语言",
        "en": "Select at least one language",
        "ja": "認識言語を 1 つ以上選択してください",
        "fr": "Sélectionnez au moins une langue",
    },
    "已开始 {n} 个转写任务": {
        "zh": "已开始 {n} 个转写任务",
        "en": "Started {n} transcription tasks",
        "ja": "{n} 件の文字起こしタスクを開始しました",
        "fr": "{n} tâches de transcription lancées",
    },
    "→ 英文": {
        "zh": "→ 英文",
        "en": "→ English",
        "ja": "→ 英語",
        "fr": "→ anglais",
    },
    # ── 下载：多链接 / txt 批量导入 ──────────────────────────────────────
    "批量导入": {
        "zh": "批量导入",
        "en": "Bulk import",
        "ja": "一括インポート",
        "fr": "Import en masse",
    },
    "每行一个链接，支持粘贴多行；空行与 # / // 注释行会被忽略。": {
        "zh": "每行一个链接，支持粘贴多行；空行与 # / // 注释行会被忽略。",
        "en": "One link per line, multi-line paste supported; blank lines and # / // comment lines are ignored.",
        "ja": "1 行に 1 つのリンク。複数行の貼り付けに対応。空行と # / // コメント行は無視されます。",
        "fr": "Un lien par ligne, collage multiligne pris en charge ; les lignes vides et les commentaires # / // sont ignorés.",
    },
    "识别到 {n} 条链接，去重后 {m} 条": {
        "zh": "识别到 {n} 条链接，去重后 {m} 条",
        "en": "{n} links detected, {m} after de-duplication",
        "ja": "{n} 件のリンクを検出、重複除去後 {m} 件",
        "fr": "{n} liens détectés, {m} après déduplication",
    },
    "单次最多 {n} 条，超出的不会入队": {
        "zh": "单次最多 {n} 条，超出的不会入队",
        "en": "At most {n} per batch; the rest are not queued",
        "ja": "1 回につき最大 {n} 件。超過分はキューに入りません",
        "fr": "{n} maximums par lot ; le surplus n'est pas mis en file",
    },
    "从 txt 文件导入": {
        "zh": "从 txt 文件导入",
        "en": "Import from txt file",
        "ja": "txt ファイルからインポート",
        "fr": "Importer depuis un fichier txt",
    },
    "全部加入队列": {
        "zh": "全部加入队列",
        "en": "Add all to queue",
        "ja": "すべてキューに追加",
        "fr": "Tout ajouter à la file",
    },
    "加入中…": {
        "zh": "加入中…",
        "en": "Adding…",
        "ja": "追加中…",
        "fr": "Ajout…",
    },
    "已加入 {added} 条，拦截 {blocked} 条，跳过 {skipped} 条": {
        "zh": "已加入 {added} 条，拦截 {blocked} 条，跳过 {skipped} 条",
        "en": "Added {added}, blocked {blocked}, skipped {skipped}",
        "ja": "{added} 件を追加、{blocked} 件をブロック、{skipped} 件をスキップ",
        "fr": "{added} ajoutés, {blocked} bloqués, {skipped} ignorés",
    },
    "加入队列后自动开始": {
        "zh": "加入队列后自动开始",
        "en": "Start automatically after being queued",
        "ja": "キュー追加後に自動で開始",
        "fr": "Démarrage automatique après mise en file",
    },
    "跳过重复链接": {
        "zh": "跳过重复链接",
        "en": "Skip duplicate links",
        "ja": "重複リンクをスキップ",
        "fr": "Ignorer les liens en double",
    },
    "被过滤规则拦截的链接不会入队": {
        "zh": "被过滤规则拦截的链接不会入队",
        "en": "Links blocked by the filter rules are not queued",
        "ja": "フィルタールールでブロックされたリンクはキューに入りません",
        "fr": "Les liens bloqués par les règles de filtrage ne sont pas mis en file",
    },
    "拦截原因": {
        "zh": "拦截原因",
        "en": "Blocked reasons",
        "ja": "ブロック理由",
        "fr": "Raisons du blocage",
    },
    "请粘贴至少一个链接": {
        "zh": "请粘贴至少一个链接",
        "en": "Paste at least one link",
        "ja": "リンクを 1 つ以上貼り付けてください",
        "fr": "Collez au moins un lien",
    },
    "文本文件": {
        "zh": "文本文件",
        "en": "Text file",
        "ja": "テキストファイル",
        "fr": "Fichier texte",
    },
    "已导入 {name}": {
        "zh": "已导入 {name}",
        "en": "Imported {name}",
        "ja": "{name} をインポートしました",
        "fr": "{name} importé",
    },
    "读取文件失败": {
        "zh": "读取文件失败",
        "en": "Failed to read the file",
        "ja": "ファイルの読み込みに失敗しました",
        "fr": "Échec de la lecture du fichier",
    },
}


def read_locale(lang: str):
    """→ (text, newline, entries: list[(key, value)])"""
    path = os.path.join(LOCALES, f"{lang}.ts")
    raw = open(path, "rb").read()
    assert not raw.startswith(b"\xef\xbb\xbf"), f"{lang}.ts 不应该有 BOM"
    text = raw.decode("utf-8")
    nl = "\r\n" if "\r\n" in text else "\n"
    assert text.count("\n") == text.count(nl) or nl == "\n", f"{lang}.ts 换行混杂"
    lines = text[:-len(nl)].split(nl) if text.endswith(nl) else text.split(nl)
    assert lines[1].startswith("export default"), f"{lang}.ts 第 2 行不是 export default"
    assert lines[-1].strip() == "}", f"{lang}.ts 结尾不是 }}"
    entries = []
    for ln in lines[2:-1]:
        m = ENTRY.match(ln)
        assert m, f"{lang}.ts 无法解析的条目行：{ln!r}"
        entries.append((json.loads('"' + m.group(1) + '"'), json.loads('"' + m.group(2) + '"')))
    return text, nl, lines[0], entries


def emit(lang: str, nl: str, header: str, entries: list) -> bytes:
    out = [header, "export default {"]
    for k, v in entries:
        out.append("  " + json.dumps(k, ensure_ascii=False) + ": " + json.dumps(v, ensure_ascii=False) + ",")
    out.append("}")
    return (nl.join(out) + nl).encode("utf-8")


def main() -> int:
    check_only = "--check" in sys.argv
    data = {}
    for lang in LANGS:
        text, nl, header, entries = read_locale(lang)
        keys = [k for k, _ in entries]
        assert len(keys) == len(set(keys)), f"{lang}.ts 有重复 key"
        sorted_ok = keys == sorted(keys)
        print(f"{lang}: 条目 {len(keys)}｜换行 {'CRLF' if nl == chr(13) + chr(10) else 'LF'}｜已排序 {sorted_ok}")
        data[lang] = dict(text=text, nl=nl, header=header, entries=entries, sorted_ok=sorted_ok)

    # 插入新文案（保持全局排序：以 zh 的既有顺序为基准）；DROP 里的旧文案顺手清理
    for lang in LANGS:
        d = data[lang]
        cur = [k for k in (k for k, _ in d["entries"]) if k not in DROP]
        target = sorted(set(cur) | set(NEW))
        merged = []
        by_key = {k: v for k, v in d["entries"] if k not in DROP}
        for k in target:
            if k in by_key:
                merged.append((k, by_key[k]))
            else:
                merged.append((k, NEW[k][lang]))
        added = [k for k in target if k not in by_key]
        removed = [k for k, _ in d["entries"] if k in DROP]
        print(f"  {lang}: 新增 {len(added)} 条｜清理 {len(removed)} 条 → 共 {len(merged)} 条")
        if not check_only:
            back = dict(merged)
            for k, v in d["entries"]:
                if k in DROP:
                    continue
                assert back.get(k) == v, f"{lang} 既有条目被改动：{k}"
            open(os.path.join(LOCALES, f"{lang}.ts"), "wb").write(emit(lang, d["nl"], d["header"], merged))

    # 回读校验
    sets = {}
    orders = {}
    for lang in LANGS:
        _, _, _, entries = read_locale(lang)
        sets[lang] = set(k for k, _ in entries)
        orders[lang] = [k for k, _ in entries]
        print(f"  校验 {lang}: {len(entries)} 条｜排序 {orders[lang] == sorted(orders[lang])}")
    same = all(sets[l] == sets["zh"] for l in LANGS)
    print(f"  四语言 key 集合一致: {same}｜顺序与 zh 一致: {all(orders[l] == orders['zh'] for l in LANGS)}")
    ph = lambda s: sorted(re.findall(r"\{[a-zA-Z]+\}", s))
    bad = []
    for k, vals in NEW.items():
        for lang in LANGS:
            if ph(vals[lang]) != ph(vals["zh"]):
                bad.append((k, lang))
    print(f"  占位符一致性: {'OK' if not bad else bad}")
    return 0 if (same and not bad) else 1


if __name__ == "__main__":
    sys.exit(main())
