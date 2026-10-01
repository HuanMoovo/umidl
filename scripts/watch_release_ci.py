"""盯 Release 工作流直到跑完，打印各平台结论（供后台进程 + notify 使用）。"""
import json
import time
import urllib.error
import urllib.request

tok = open(os.environ.get("UMIDL_TOKEN_FILE", r".tmp/deploy_token.txt"), encoding="utf-8").read().strip()
REPO = "HuanMoovo/umidl"


def api(path):
    req = urllib.request.Request(
        "https://api.github.com" + path,
        headers={"Authorization": f"token {tok}", "Accept": "application/vnd.github+json",
                 "User-Agent": "umidl-watch"})
    for i in range(5):
        try:
            with urllib.request.urlopen(req, timeout=45) as r:
                return json.loads(r.read() or b"{}")
        except urllib.error.HTTPError as e:
            print("HTTP", e.code, path)
            return {}
        except Exception as e:
            print("重试", i + 1, type(e).__name__)
            time.sleep(5)
    return {}


run = None
for _ in range(30):
    runs = api(f"/repos/{REPO}/actions/runs?per_page=8").get("workflow_runs", [])
    cand = [r for r in runs if r.get("name") == "Release"]
    if cand and cand[0].get("status") != "queued":
        run = cand[0]
        break
    if cand:
        run = cand[0]
    time.sleep(20)

if not run:
    print("没找到 Release 运行")
    raise SystemExit(0)

print(f"Release #{run['run_number']}｜{run['html_url']}")
print(f"初始：{run['status']} / {run.get('conclusion')}")

deadline = time.time() + 45 * 60
last = None
while time.time() < deadline:
    d = api(f"/repos/{REPO}/actions/runs/{run['id']}")
    jobs = api(f"/repos/{REPO}/actions/runs/{run['id']}/jobs").get("jobs", [])
    summary = " ｜ ".join(f"{j['name']}:{j['status']}/{j.get('conclusion')}" for j in jobs)
    if summary != last:
        print(f"[{time.strftime('%H:%M:%S')}] {d.get('status')}/{d.get('conclusion')} — {summary}", flush=True)
        last = summary
    if d.get("status") == "completed":
        print("\n══ 各平台结论")
        for j in jobs:
            print(f"  {j['name']}: {j.get('conclusion')}")
            for st in j.get("steps", []):
                if st.get("conclusion") not in ("success", "skipped", None):
                    print(f"     ✗ 步骤失败：{st.get('name')} → {st.get('conclusion')}")
        st = api(f"/repos/{REPO}/actions/runs/{run['id']}/artifacts")
        print("产物：", [f"{a['name']} ({a['size_in_bytes']} 字节)" for a in st.get("artifacts", [])])
        break
    time.sleep(45)
else:
    print("超过 45 分钟仍未结束，放弃等待（运行仍在继续）")
print(f"\n运行页：{run['html_url']}")
