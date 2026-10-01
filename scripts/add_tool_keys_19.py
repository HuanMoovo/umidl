"""追加 1.9「按需勾选下载 + 工具安装目录」的四语言文案。

规则（沿用本仓库既有做法）：
  * 中文原文即 key，四个语言包必须 key 完全一致；
  * zh/ja/fr 是 CRLF、en 是 LF，写回时保持原样；
  * 只在正确位置插入新 key，**不重排、不改动任何已有译文**（脚本会断言这一点）。
"""
import json
import re
import sys

LANGS = ["zh", "en", "ja", "fr"]
NEW = {
    "全选": ("Select all", "すべて選択", "Tout sélectionner"),
    "只选未装": ("Missing only", "未導入のみ", "Manquants uniquement"),
    "清空": ("Clear", "クリア", "Vider"),
    "校验所选": ("Verify selected", "選択を検証", "Vérifier la sélection"),
    "下载所选（{n}）": ("Download selected ({n})", "選択をダウンロード（{n}）", "Télécharger la sélection ({n})"),
    "已选 {n} 个（已装 {have}）": (
        "{n} selected ({have} installed)",
        "選択 {n} 件（導入済み {have}）",
        "{n} sélectionné(s) ({have} installé(s))",
    ),
    "已装 · {version}": ("Installed · {version}", "導入済み · {version}", "Installé · {version}"),
    "已装": ("Installed", "導入済み", "Installé"),
    "未装 · {size}": ("Not installed · {size}", "未導入 · {size}", "Non installé · {size}"),
    "重装": ("Reinstall", "再インストール", "Réinstaller"),
    "下载": ("Download", "ダウンロード", "Télécharger"),
    "请先勾选要下载的工具": (
        "Select the tools to download first",
        "先にダウンロードするツールを選択してください",
        "Sélectionnez d'abord les outils à télécharger",
    ),
    "下载完成：成功 {ok} 个，失败 {fail} 个（{names}）": (
        "Download finished: {ok} succeeded, {fail} failed ({names})",
        "ダウンロード完了：成功 {ok} 件、失敗 {fail} 件（{names}）",
        "Téléchargement terminé : {ok} réussi(s), {fail} échoué(s) ({names})",
    ),
    "已安装 {n} 个工具": (
        "Installed {n} tool(s)",
        "{n} 個のツールをインストールしました",
        "{n} outil(s) installé(s)",
    ),
    "校验通过：{n} 个工具都可以正常运行": (
        "Verification passed: {n} tool(s) run correctly",
        "検証 OK：{n} 個のツールは正常に動作します",
        "Vérification réussie : {n} outil(s) fonctionnent correctement",
    ),
    "校验完成：{ok} 个可用，{fail} 个不可用（{names}）": (
        "Verification done: {ok} usable, {fail} unusable ({names})",
        "検証完了：利用可能 {ok} 件、利用不可 {fail} 件（{names}）",
        "Vérification terminée : {ok} utilisable(s), {fail} inutilisable(s) ({names})",
    ),
    "工具安装目录": ("Tool install directory", "ツールのインストール先", "Répertoire d'installation des outils"),
    "当前使用默认目录": ("Using the default directory", "既定のディレクトリを使用中", "Répertoire par défaut utilisé"),
    "应用目录": ("Apply directory", "ディレクトリを適用", "Appliquer le répertoire"),
    "处理中…": ("Working…", "処理中…", "Traitement…"),
    "迁移已有工具（先复制校验，成功后才删旧目录）": (
        "Migrate installed tools (copy + verify first, delete the old directory only on success)",
        "既存ツールを移行（先にコピー検証、成功後に旧ディレクトリを削除）",
        "Migrer les outils existants (copie et vérification d'abord, suppression après succès)",
    ),
    "受管工具（yt-dlp / ffmpeg / whisper 等）安装在这里；解析与调用只认这个目录，不改系统 PATH。": (
        "Managed tools (yt-dlp / ffmpeg / whisper …) are installed here; resolution and calls use only this directory and never modify the system PATH.",
        "管理対象ツール（yt-dlp / ffmpeg / whisper など）はここにインストールされます。解決と実行はこのディレクトリのみを参照し、システム PATH は変更しません。",
        "Les outils gérés (yt-dlp / ffmpeg / whisper…) sont installés ici ; la résolution et les appels utilisent uniquement ce répertoire, sans modifier le PATH système.",
    ),
    "生效目录：{dir}": ("Active directory: {dir}", "有効なディレクトリ：{dir}", "Répertoire actif : {dir}"),
    "配置里的工具目录不是绝对路径，已回退到默认目录。": (
        "The configured tool directory is not an absolute path; falling back to the default directory.",
        "設定のツールディレクトリが絶対パスではないため、既定のディレクトリに戻しました。",
        "Le répertoire d'outils configuré n'est pas un chemin absolu ; retour au répertoire par défaut.",
    ),
    "工具目录已更新": ("Tool directory updated", "ツールディレクトリを更新しました", "Répertoire d'outils mis à jour"),
    "已迁移 {n} 个文件（{size}）到新目录": (
        "Migrated {n} file(s) ({size}) to the new directory",
        "新ディレクトリへ {n} 個のファイル（{size}）を移行しました",
        "{n} fichier(s) ({size}) migré(s) vers le nouveau répertoire",
    ),
    "新目录暂无可迁移的工具": (
        "Nothing to migrate to the new directory yet",
        "新ディレクトリに移行するツールはありません",
        "Aucun outil à migrer vers le nouveau répertoire",
    ),
    "旧目录仍有 {n} 项未能删除（可手动清理）": (
        "{n} item(s) in the old directory could not be deleted (you can clean them manually)",
        "旧ディレクトリに削除できなかった項目が {n} 件あります（手動で削除できます）",
        "{n} élément(s) de l'ancien répertoire n'ont pas pu être supprimés (nettoyage manuel possible)",
    ),
    "已跳过 {n} 个未完成的下载残片": (
        "Skipped {n} unfinished download fragment(s)",
        "未完了のダウンロード残り {n} 件をスキップしました",
        "{n} fragment(s) de téléchargement inachevé(s) ignoré(s)",
    ),
    "需手动放置": ("Place manually", "手動で配置", "À placer manuellement"),
    "随 ffmpeg": ("Ships with ffmpeg", "ffmpeg に同梱", "Fourni avec ffmpeg"),
    # ---- 后端工具目录错误码（backendError.ts，tools.* 前缀） ----
    "工具目录不能为空 —— 请填写绝对路径（例如 D:\\Umidl\\tools）": (
        "The tool directory cannot be empty — enter an absolute path (e.g. D:\\Umidl\\tools)",
        "ツールディレクトリを空にできません —— 絶対パスを入力してください（例：D:\\Umidl\\tools）",
        "Le répertoire d'outils ne peut pas être vide — saisissez un chemin absolu (ex. D:\\Umidl\\tools)",
    ),
    "工具目录必须是绝对路径（{host}）—— 例如 D:\\Umidl\\tools，或点「浏览…」选择目录": (
        'The tool directory must be an absolute path ({host}) — e.g. D:\\Umidl\\tools, or click "Browse…" to pick one',
        "ツールディレクトリは絶対パスである必要があります（{host}）—— 例：D:\\Umidl\\tools、または「参照…」で選択してください",
        "Le répertoire d'outils doit être un chemin absolu ({host}) — par ex. D:\\Umidl\\tools, ou cliquez sur « Parcourir… »",
    ),
    '工具目录包含非法字符（{host}）—— 目录名不能出现 < > " | ? * 这几个字符': (
        'The tool directory contains invalid characters ({host}) — a directory name cannot contain < > " | ? *',
        'ツールディレクトリに使用できない文字が含まれています（{host}）—— ディレクトリ名に < > " | ? * は使えません',
        'Le répertoire d\'outils contient des caractères non valides ({host}) — un nom de dossier ne peut pas contenir < > " | ? *',
    ),
    "这个路径是一个文件而不是目录（{host}）—— 请选择目录": (
        "This path is a file, not a directory ({host}) — please choose a directory",
        "このパスはファイルでありディレクトリではありません（{host}）—— ディレクトリを選択してください",
        "Ce chemin est un fichier, pas un répertoire ({host}) — choisissez un répertoire",
    ),
    "无法创建工具目录（{host}）：{detail}": (
        "Cannot create the tool directory ({host}): {detail}",
        "ツールディレクトリを作成できません（{host}）：{detail}",
        "Impossible de créer le répertoire d'outils ({host}) : {detail}",
    ),
    "工具目录不可写（{host}）：{detail} —— 请换一个可写目录（避免系统盘受保护目录）": (
        "The tool directory is not writable ({host}): {detail} — choose a writable directory (avoid protected system folders)",
        "ツールディレクトリに書き込めません（{host}）：{detail} —— 書き込み可能なディレクトリを選んでください（保護されたシステムフォルダは避けてください）",
        "Le répertoire d'outils n'est pas accessible en écriture ({host}) : {detail} — choisissez un répertoire inscriptible (évitez les dossiers système protégés)",
    ),
    "新目录不能位于旧目录内部（{host}）—— 请选择另一个目录，否则复制与删除会互相覆盖": (
        "The new directory cannot be inside the old one ({host}) — pick another directory, otherwise copying and deleting would overlap",
        "新しいディレクトリは旧ディレクトリの内部に置けません（{host}）—— 別のディレクトリを選んでください。コピーと削除が競合します",
        "Le nouveau répertoire ne peut pas être à l'intérieur de l'ancien ({host}) — choisissez un autre répertoire, sinon la copie et la suppression se chevauchent",
    ),
    "迁移失败：无法复制 {host}（{detail}）—— 旧目录保持原样，没有文件被删除": (
        "Migration failed: cannot copy {host} ({detail}) — the old directory is unchanged, no file was deleted",
        "移行失敗：{host} をコピーできません（{detail}）—— 旧ディレクトリはそのまま、削除されたファイルはありません",
        "Échec de la migration : impossible de copier {host} ({detail}) — l'ancien répertoire reste intact, aucun fichier supprimé",
    ),
    "迁移失败：{host} 复制后体积不一致（{detail}）—— 旧目录保持原样，没有文件被删除": (
        "Migration failed: size mismatch after copying {host} ({detail}) — the old directory is unchanged, no file was deleted",
        "移行失敗：{host} はコピー後のサイズが一致しません（{detail}）—— 旧ディレクトリはそのまま、削除されたファイルはありません",
        "Échec de la migration : taille incohérente après copie de {host} ({detail}) — l'ancien répertoire reste intact, aucun fichier supprimé",
    ),
    "迁移到新目录失败（{host}）：{detail} —— 旧目录仍然生效，工具目录未改动": (
        "Migration to the new directory failed ({host}): {detail} — the old directory is still active, the tool directory was not changed",
        "新しいディレクトリへの移行に失敗（{host}）：{detail} —— 旧ディレクトリが引き続き有効で、ツールディレクトリは変更されていません",
        "Échec de la migration vers le nouveau répertoire ({host}) : {detail} — l'ancien répertoire reste actif, le répertoire d'outils n'a pas été modifié",
    ),
    "{host} 安装失败：{detail}": (
        "{host} installation failed: {detail}",
        "{host} のインストールに失敗しました：{detail}",
        "Échec de l'installation de {host} : {detail}",
    ),
    "{host} 未找到可用的可执行文件（{detail}）": (
        "No usable executable found for {host} ({detail})",
        "{host} の実行ファイルが見つかりません（{detail}）",
        "Aucun exécutable utilisable trouvé pour {host} ({detail})",
    ),
    "未知的工具：{host}": ("Unknown tool: {host}", "不明なツール：{host}", "Outil inconnu : {host}"),
    "操作失败：{detail}": ("Operation failed: {detail}", "操作に失敗しました：{detail}", "L'opération a échoué : {detail}"),
}

ENTRY = re.compile(r"^(  )(\".*\"): (\".*\"),$")


def load(path):
    raw = open(path, "rb").read()
    eol = "\r\n" if b"\r\n" in raw else "\n"
    text = raw.decode("utf-8")
    lines = text.split(eol)
    items = []  # [(key, value, raw_line)]
    for line in lines[2:]:
        if not line.startswith("  "):
            break
        m = ENTRY.match(line)
        if not m:
            sys.exit(f"{path}: 无法解析条目：{line!r}")
        pair = json.loads("{" + line.strip().rstrip(",") + "}")
        (k, v), = pair.items()
        items.append((k, v, line))
    return eol, lines[:2], items, lines[len(items) + 2:]


def dump_line(k, v):
    return "  " + json.dumps(k, ensure_ascii=False) + ": " + json.dumps(v, ensure_ascii=False) + ","


def main():
    files = {l: f"src/i18n/locales/{l}.ts" for l in LANGS}
    parsed = {}
    for l, p in files.items():
        eol, head, items, tail = load(p)
        keys = [k for k, _, _ in items]
        unsorted = sum(1 for a, b in zip(keys, keys[1:]) if a > b)
        if unsorted:
            print(f"{p}: 既有条目里有 {unsorted} 处非升序（保持原样，不重排）")
        parsed[l] = (eol, head, items, tail)

    zh_keys = [x for x, _, _ in parsed["zh"][2]]
    for k in list(NEW):
        if k in zh_keys:
            print(f"跳过已有文案（四语言包里已存在，沿用原译文）：{k}")
            NEW.pop(k)

    added = {}
    for idx, l in enumerate(LANGS):
        eol, head, items, tail = parsed[l]
        src = dict(parsed["zh"][2][0:0])  # 占位，仅为清晰
        new_items = []
        for k, tr in NEW.items():
            v = k if l == "zh" else tr[idx - 1]
            new_items.append((k, v))
        merged = []
        pending = sorted(new_items, key=lambda t: t[0])
        for it in items:
            while pending and pending[0][0] < it[0]:
                k, v = pending.pop(0)
                merged.append((k, v, None))
            merged.append(it)
        for k, v in pending:
            merged.append((k, v, None))
        # 已有条目必须逐行保持不变（只做插入，绝不改旧译文）
        old_lines = {k: line for k, _, line in items}
        out_lines = []
        for k, v, line in merged:
            rendered = line if line is not None else dump_line(k, v)
            if k in old_lines and rendered != old_lines[k]:
                sys.exit(f"{files[l]}: 旧条目被改动：{k}\n  旧 {old_lines[k]!r}\n  新 {rendered!r}")
            out_lines.append(rendered)
        text = eol.join(head + out_lines + tail)
        open(files[l], "wb").write(text.encode("utf-8"))
        added[l] = len(new_items)
        print(f"{files[l]}: {len(items)} → {len(out_lines)} 条（+{len(new_items)}），{eol!r}")

    # 复核：四语言 key 集合与顺序完全一致
    sets = {}
    for l in LANGS:
        _, _, items, _ = load(files[l])
        sets[l] = [k for k, _, _ in items]
    base = sets["zh"]
    for l in LANGS:
        assert sets[l] == base, f"{l} 与 zh 的 key 不一致"
    print(f"OK：四语言均为 {len(base)} 条，key 顺序一致")


if __name__ == "__main__":
    main()
