"""脚本内翻译调用统一改名为 tr()，避免与局部变量 t 冲突（如 const t = task）。"""

import os
import re

p = "src/i18n/index.ts"
s = open(p, encoding="utf-8", newline="").read()
s = s.replace(
    "/** 非组件模块里使用的 t（组件内请用 useI18n() 以获得响应式） */\nexport function t(",
    "/** 非组件模块里使用的翻译函数（组件内请用 useI18n() 取 tr 以获得响应式） */\nexport function tr(",
)
open(p, "w", encoding="utf-8", newline="").write(s)

files = []
for root, _, fs in os.walk("src"):
    parts = root.replace(os.sep, "/").split("/")
    if "i18n" in parts:
        continue
    for f in fs:
        if f.endswith((".vue", ".ts")):
            files.append(os.path.join(root, f).replace(os.sep, "/"))

changed = {}
for path in files:
    src = open(path, encoding="utf-8", newline="").read()
    orig = src
    src = src.replace("const { t } = useI18n()", "const { t: tr } = useI18n()")
    src = src.replace("import { t } from '@/i18n'", "import { tr } from '@/i18n'")
    # 调用改名 t('...') -> tr('...')，跳过 $t( 与 foo_t(
    src = re.sub(r"(?<![\w$])t\('", "tr('", src)
    # 日志/控制台文案还原为中文原文（开发者诊断信息不翻译）
    src = re.sub(r"flog\(tr\('([^']*)'\)\)", r"flog('\1')", src)
    src = re.sub(r"console\.(log|warn|error)\(tr\('([^']*)'\)\)", r"console.\1('\2')", src)
    if src != orig:
        open(path, "w", encoding="utf-8", newline="").write(src)
        changed[path] = len(re.findall(r"\btr\('", src))

print("改名文件数:", len(changed))
for k, v in sorted(changed.items(), key=lambda x: -x[1]):
    print(f"  {v:3}  {k}")
