#!/usr/bin/env bash
# 在 Linux 容器内跑 Umidl 的 Rust 单测（与 CI 一致：cargo test --lib --features selftest）
#
#   bash scripts/docker/test-linux.sh              # 全量
#   bash scripts/docker/test-linux.sh capture::    # 只跑名字匹配的用例（附加 filter）
#   NOCAPTURE=1 bash scripts/docker/test-linux.sh sandbox_enforces_memory_limit   # 带 --nocapture
#
# 说明：
#   * 容器复用（名字 ${CONTAINER}）：第一次创建，之后 docker exec —— 避免每次重建容器；
#   * 源码每次从 /src（只读挂载）同步到 /build（命名卷），cargo 增量编译；
#   * 退出码 = cargo test 的退出码（0 = 全绿）。
set -euo pipefail

REPO="${REPO:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
IMAGE="${IMAGE:-umidl-linux-builder:22.04}"
VOLUME="${VOLUME:-umidl-build-workspace}"
CONTAINER="${CONTAINER:-umidl-linux-test}"
FILTER="${1:-}"
EXTRA=""
[ "${NOCAPTURE:-0}" = "1" ] && EXTRA="--nocapture"

echo "REPO=$REPO  IMAGE=$IMAGE  CONTAINER=$CONTAINER  FILTER=${FILTER:-<全部>}  NOCAPTURE=${NOCAPTURE:-0}"

if ! docker inspect "$CONTAINER" >/dev/null 2>&1; then
  docker volume create "$VOLUME" >/dev/null
  docker run -d --name "$CONTAINER" \
    -v "$REPO":/src:ro -v "$VOLUME":/build \
    -w /build "$IMAGE" sleep infinity >/dev/null
fi

docker exec "$CONTAINER" bash -c '
set -euo pipefail
mkdir -p /build
tar -C /src \
    --exclude=./node_modules --exclude=./src-tauri/target --exclude=./src-tauri/target-selftest \
    --exclude=./target-selftest --exclude=./dist --exclude=./.git --exclude=./.tmp \
    -cf - . | tar -C /build -xf -
cd /build/src-tauri
rustc -V; cargo -V
if [ -n "'"$FILTER"'" ]; then
  cargo test --lib --features selftest -- "'"$FILTER"'" '"$EXTRA"'
else
  cargo test --lib --features selftest '"$EXTRA"'
fi
'
