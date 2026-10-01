"""版本号 1.4.0 → 1.5.0（四个位置：package.json / Cargo.toml / tauri.conf.json / TitleBar.vue）。

幂等：已经是 1.5.0 就跳过。仅替换明确的版本字面量，不动其它数字。
"""
import json
import re
import sys

EDITS = [
    ("package.json", r'"version":\s*"1\.4\.0"', '"version": "1.5.0"'),
    ("src-tauri/Cargo.toml", r'^version = "1\.4\.0"', 'version = "1.5.0"'),
    ("src-tauri/tauri.conf.json", r'"version":\s*"1\.4\.0"', '"version": "1.5.0"'),
    ("src/components/TitleBar.vue", r"v1\.4\.0", "v1.5.0"),
]

done = []
for path, pat, rep in EDITS:
    try:
        raw = open(path, encoding="utf-8", newline="").read()
    except FileNotFoundError:
        done.append(f"!! 缺文件 {path}")
        continue
    crlf = "\r\n" in raw
    s = raw.replace("\r\n", "\n")
    if re.search(r"1\.5\.0", s) and not re.search(pat, s, re.M):
        done.append(f"跳过 {path}（已是 1.5.0）")
        continue
    new, n = re.subn(pat, rep, s, flags=re.M)
    if n == 0:
        done.append(f"!! {path} 未命中（模式 {pat}）")
        continue
    open(path, "w", encoding="utf-8", newline="").write(new.replace("\n", "\r\n") if crlf else new)
    done.append(f"{path}：{n} 处 → 1.5.0")

print("\n".join(done))
if any(x.startswith("!!") for x in done):
    sys.exit(1)
