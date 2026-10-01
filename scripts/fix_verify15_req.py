"""修正 verify_v15.py：start_convert 的参数是命名的 req（Tauri 命令签名 `req: ConvertRequest`）。

原来把字段平铺在顶层，被后端拒为 `missing required key req`，转换自然没产出。
"""
import sys

P = "scripts/verify_v15.py"
raw = open(P, encoding="utf-8", newline="").read()
crlf = "\r\n" in raw
s = raw.replace("\r\n", "\n")

PAIRS = [
    (
        'await d.try_invoke(\n        "start_convert",\n        {"input_file": docx, "output_dir": conv, "format": "md", "extract_audio": False, "mute": False},\n    )',
        'await d.try_invoke(\n        "start_convert",\n        {"req": {"input_file": docx, "output_dir": conv, "format": "md", "extract_audio": False, "mute": False}},\n    )',
    ),
    (
        'await d.try_invoke(\n        "start_convert",\n        {"input_file": pdf, "output_dir": conv, "format": "txt", "extract_audio": False, "mute": False},\n    )',
        'await d.try_invoke(\n        "start_convert",\n        {"req": {"input_file": pdf, "output_dir": conv, "format": "txt", "extract_audio": False, "mute": False}},\n    )',
    ),
    (
        'await d.try_invoke(\n        "start_convert",\n        {"input_file": xlsx, "output_dir": conv, "format": "csv", "extract_audio": False, "mute": False},\n    )',
        'await d.try_invoke(\n        "start_convert",\n        {"req": {"input_file": xlsx, "output_dir": conv, "format": "csv", "extract_audio": False, "mute": False}},\n    )',
    ),
]

done = []
for old, new in PAIRS:
    if new in s:
        done.append("跳过（已修正）")
        continue
    n = s.count(old)
    if n != 1:
        sys.exit(f"锚点命中 {n} 次（期望 1）：{old[:70]}")
    s = s.replace(old, new, 1)
    done.append("已修正一处 start_convert 调用")

open(P, "w", encoding="utf-8", newline="").write(s.replace("\n", "\r\n") if crlf else s)
print("\n".join(done))
