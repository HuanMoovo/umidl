"""把 Umidl 源码树通过 GitHub Git Data API 发布到 HuanMoovo/umidl。

为什么不用 git push：本机必须走系统代理 127.0.0.1:7892，而该代理会重置 git 的 TLS 连接
（Recv failure: Connection was reset）；绕开代理又连不上 github.com:443。
GitHub REST API 走同一代理是通的（GET/POST 均 200），所以用 API 建 blob/tree/commit/ref。

- 文本文件用 tree 的 inline `content` 提交（省掉每个文件的 blob 调用）
- 二进制文件（图片等）用 /git/blobs 单独上传
- 默认推到 main 分支；已存在 main 时以其为父提交做快照提交（走 API 的 tree 全量覆盖）
- token 从文件读取，绝不打印
"""
import base64
import json
import os
import sys
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TOKEN_FILE = os.environ.get("UMIDL_TOKEN_FILE", os.path.join(ROOT, ".tmp", "deploy_token.txt"))
VERSION = json.load(open(os.path.join(ROOT, "package.json"), encoding="utf-8"))["version"]
REPO = "HuanMoovo/umidl"
BRANCH = "main"

# 不发布的目录/文件（与 .gitignore 一致）
EXCLUDE_DIRS = {
    "node_modules", "dist", "target", "target-selftest", ".tmp", ".git", ".hermes",
    "src-tauri/target", "src-tauri/gen", ".vscode", ".idea", "__pycache__", ".pytest_cache", "out",
}
EXCLUDE_EXT = {".exe", ".msi", ".7z", ".zip", ".db", ".bin", ".gguf", ".dll", ".pdb", ".rlib",
               ".pyc", ".pyo", ".log"}
EXCLUDE_FILES = {"fr_new.json", "i18n-extract.json", "i18n-missing.json", "i18n-worklist.json",
                 "package-lock.json"}  # lock 文件保留与否：保留更利于复现 → 见下方 SKIP 覆盖
KEEP_FILES = {"package-lock.json"}          # 明确保留
BINARY_EXT = {".png", ".ico", ".jpg", ".jpeg", ".gif", ".webp", ".ttf", ".woff2", ".icns", ".pdf"}
TEXT_MAX = 400_000                          # inline content 上限（保守）
CONCURRENCY = 6


def token():
    with open(TOKEN_FILE, encoding="utf-8") as fh:
        return fh.read().strip()


TOK = token()


def api(path, method="GET", body=None, tries=4):
    last = None
    for i in range(tries):
        req = urllib.request.Request(
            "https://api.github.com" + path, method=method,
            data=(json.dumps(body).encode() if body is not None else None),
            headers={"Authorization": f"token {TOK}", "Accept": "application/vnd.github+json",
                     "User-Agent": "umidl-publish", "Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                return json.loads(r.read() or b"{}")
        except urllib.error.HTTPError as e:
            detail = e.read()[:300].decode("utf-8", "replace")
            if e.code in (409, 422, 500, 502, 503) and i < tries - 1:
                time.sleep(1.5 * (i + 1))
                continue
            raise RuntimeError(f"{method} {path} → HTTP {e.code}: {detail}") from None
        except Exception as e:                     # 网络抖动
            last = e
            time.sleep(1.5 * (i + 1))
    raise RuntimeError(f"{method} {path} 连续失败：{last}")


def collect():
    files = []
    for dirpath, dirnames, filenames in os.walk(ROOT):
        rel_dir = os.path.relpath(dirpath, ROOT).replace("\\", "/")
        if rel_dir == ".":
            rel_dir = ""
        dirnames[:] = [d for d in dirnames
                       if d not in EXCLUDE_DIRS and f"{rel_dir}/{d}".lstrip("/") not in EXCLUDE_DIRS]
        if rel_dir and any(rel_dir == x or rel_dir.startswith(x + "/") for x in EXCLUDE_DIRS):
            continue
        for fn in filenames:
            rel = f"{rel_dir}/{fn}" if rel_dir else fn
            ext = os.path.splitext(fn)[1].lower()
            if fn in KEEP_FILES:
                pass
            elif fn in EXCLUDE_FILES or ext in EXCLUDE_EXT:
                continue
            p = os.path.join(dirpath, fn)
            try:
                size = os.path.getsize(p)
            except OSError:
                continue
            if size > 8_000_000:                    # 单文件 8MB 上限
                print(f"  跳过（>8MB）：{rel} {size} 字节")
                continue
            files.append((rel, p, size, ext))
    return files


def blob_sha(path):
    with open(path, "rb") as fh:
        data = fh.read()
    out = api(f"/repos/{REPO}/git/blobs", "POST", {"content": base64.b64encode(data).decode(), "encoding": "base64"})
    return out["sha"]


def main():
    print(f"══ 收集 {ROOT} 下的文件")
    files = collect()
    text = [f for f in files if f[3] not in BINARY_EXT and f[2] <= TEXT_MAX]
    binary = [f for f in files if f not in text]
    print(f"  共 {len(files)} 个：文本 {len(text)}（inline）/ 二进制或超大 {len(binary)}（blob）")

    # 空仓库引导：Git Data API 在“仓库刚建、尚无任何提交”时可能返回 404，
    # 先用 Contents API 提交一次 README（同时也让 main 分支诞生），再走全量树提交。
    parent = None
    try:
        st = api(f"/repos/{REPO}/contents/README.md?ref={BRANCH}")
        print("  仓库已有提交")
    except RuntimeError as e:
        if "404" in str(e) or "409" in str(e) or "empty" in str(e).lower():
            import base64 as _b64
            readme = os.path.join(ROOT, "README.md")
            body = {"message": "chore: 初始化仓库（Umidl 开源首发）", "branch": BRANCH,
                    "content": _b64.b64encode(open(readme, "rb").read()).decode()}
            try:
                out = api(f"/repos/{REPO}/contents/README.md", "PUT", body)
                print("  已用 Contents API 建立初始提交：", out.get("commit", {}).get("sha", "")[:8])
            except RuntimeError as e2:
                print("  引导提交失败（继续尝试全量树）：", str(e2)[:160])
            time.sleep(2)
        else:
            raise

    try:
        ref = api(f"/repos/{REPO}/git/ref/heads/{BRANCH}")
        parent = ref["object"]["sha"]
        print("  已有分支", BRANCH, "父提交", parent[:8])
    except RuntimeError as e:
        if "404" in str(e) or "409" in str(e) or "empty" in str(e).lower():
            print("  分支", BRANCH, "尚不存在（首次发布）")
        else:
            raise

    entries = []
    skipped = []
    for rel, path, size, ext in text:
        try:
            with open(path, encoding="utf-8") as fh:
                content = fh.read()
            if "\x00" in content:
                skipped.append((rel, "含 NUL 字节"))
                continue
            entries.append({"path": rel, "mode": "100644", "type": "blob", "content": content})
        except UnicodeDecodeError:
            skipped.append((rel, "非 UTF-8 文本"))
    print(f"  inline 条目 {len(entries)}；转为 blob 的：{len(skipped)} 个 {skipped[:4]}")

    with ThreadPoolExecutor(max_workers=CONCURRENCY) as ex:
        futs = {ex.submit(blob_sha, p): rel for rel, p, _s, _e in binary}
        for fut in futs:
            pass
        for fut, rel in futs.items():
            entries.append({"path": rel, "mode": "100644", "type": "blob", "sha": fut.result()})
    print(f"  blob 上传完成，总条目 {len(entries)}")

    tree = api(f"/repos/{REPO}/git/trees", "POST",
               {"tree": entries, "base_tree": None} if not parent else {"tree": entries})
    print("  tree:", tree["sha"][:8])
    commit_body = {
        # 提交信息跟着 package.json 的版本走（可用 UMIDL_COMMIT_MSG 覆盖）：写死版本号会在
        # 后续版本里留下「1.8.8 的源码顶着 1.8.1 的提交信息」这种误导性历史。
        "message": os.environ.get("UMIDL_COMMIT_MSG") or (
            f"Umidl {VERSION} — 源码同步（Windows / macOS / Linux）\n\n"
            "本地化视频下载 / 格式转换 / AI 字幕桌面客户端。\n"
            "Tauri 2 + Rust + Vue 3；三引擎（yt-dlp / aria2c / eMule）；"
            "外部工具全部托管到用户数据目录，不污染系统 PATH。"
        ),
        "tree": tree["sha"],
    }
    if parent:
        commit_body["parents"] = [parent]
    commit = api(f"/repos/{REPO}/git/commits", "POST", commit_body)
    print("  commit:", commit["sha"][:8], "→", (commit.get("message") or "").splitlines()[0])

    if parent:
        api(f"/repos/{REPO}/git/refs/heads/{BRANCH}", "PATCH", {"sha": commit["sha"], "force": True})
        print("  已更新分支", BRANCH)
    else:
        api(f"/repos/{REPO}/git/refs", "POST", {"ref": f"refs/heads/{BRANCH}", "sha": commit["sha"]})
        print("  已创建分支", BRANCH)

    print(f"\n完成：https://github.com/{REPO}/tree/{BRANCH}")
    print("文件数：", len(entries))


if __name__ == "__main__":
    sys.exit(main())
