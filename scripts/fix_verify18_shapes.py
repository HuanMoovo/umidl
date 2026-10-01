"""按 Rust 真实签名校准 verify_v18.py 的调用形状（req 包裹 / input_file / settings 包裹）。"""
import sys

P = "scripts/verify_v18.py"
raw = open(P, encoding="utf-8", newline="").read()
crlf = "\r\n" in raw
s = raw.replace("\r\n", "\n")

REPL = [
    # start_convert：必须 req 包裹，字段名是 input_file
    (
        'await d.invoke("start_convert", {"input": png, "output_dir": WORK, "format": "psd"})',
        'await d.invoke("start_convert", {"req": {"input_file": png, "output_dir": WORK, "format": "psd"}})',
    ),
    # save_settings：settings 包裹
    (
        'await d.invoke("save_settings", {"concurrency": 2, "aria2_connections": 4})',
        'await d.invoke("save_settings", {"settings": {"concurrency": 2, "aria2_connections": 4}})',
    ),
    # start_download：req 包裹，去掉不确定的 filename 字段（靠 URL 基名命名）
    (
        'await d.invoke("start_download", {"url": f"{slow_base}/slow{i}.bin", "output_dir": WORK, "filename": f"slow{i}.bin"})',
        'await d.invoke("start_download", {"req": {"url": f"{slow_base}/slow{i}.bin", "output_dir": WORK}})',
    ),
    (
        'await d.invoke("start_download", {"url": f"{slow_base}/ghost.bin", "output_dir": WORK, "filename": "ghost.bin"})',
        'await d.invoke("start_download", {"req": {"url": f"{slow_base}/ghost.bin", "output_dir": WORK}})',
    ),
    (
        'await d.invoke("start_download", {"url": f"{slow_base}/longpath.bin", "output_dir": long_dir, "filename": "longpath.bin"})',
        'await d.invoke("start_download", {"req": {"url": f"{slow_base}/longpath.bin", "output_dir": long_dir}})',
    ),
]
done = []
for old, new in REPL:
    if new in s:
        done.append(f"跳过：{old[:36]}…")
        continue
    n = s.count(old)
    if n != 1:
        sys.exit(f"锚点命中 {n} 次：{old[:60]}")
    s = s.replace(old, new, 1)
    done.append(f"已改：{old[:40]}…")

open(P, "w", encoding="utf-8", newline="").write(s.replace("\n", "\r\n") if crlf else s)
print("\n".join(done))
import py_compile
try:
    py_compile.compile(P, doraise=True)
    print("语法 OK")
except py_compile.PyCompileError as e:
    sys.exit("语法错误：" + str(e))
