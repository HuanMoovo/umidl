#!/usr/bin/env bash
# Umidl Linux 产物构建驱动（宿主机侧执行；Windows 上请在 Git-Bash 里跑）
#
#   bash scripts/docker/build-linux.sh              # 前台构建
#   DETACH=1 bash scripts/docker/build-linux.sh     # 后台构建，日志用 docker logs -f umidl-linux-build 查看
#
# 产物回写到 <repo>/src-tauri/target/linux-dist/
# 依赖：Docker（本机 29.1.3，Docker Desktop）
set -euo pipefail

REPO="${REPO:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
IMAGE="${IMAGE:-umidl-linux-builder:22.04}"
OUT_DIR="${OUT_DIR:-$REPO/src-tauri/target/linux-dist}"
VOLUME="${VOLUME:-umidl-build-workspace}"
CONTAINER="${CONTAINER:-umidl-linux-build}"
# 宿主机系统代理（Clash 等）；容器内经 host.docker.internal 访问
PROXY="${PROXY:-http://host.docker.internal:7892}"
# 国内镜像直连，不要走代理（代理反而更慢）
NO_PROXY_LIST="${NO_PROXY_LIST:-localhost,127.0.0.1,host.docker.internal,mirrors.ustc.edu.cn,mirrors.aliyun.com,registry.npmmirror.com}"

echo "REPO=$REPO"
echo "OUT_DIR=$OUT_DIR"

docker image inspect "$IMAGE" >/dev/null 2>&1 || \
  docker build -t "$IMAGE" -f "$REPO/scripts/docker/Dockerfile.linux" "$REPO/scripts/docker"

docker volume create "$VOLUME" >/dev/null
mkdir -p "$OUT_DIR"

docker rm -f "$CONTAINER" >/dev/null 2>&1 || true

RUN_ARGS=(
  --rm --name "$CONTAINER"
  -v "$REPO":/src:ro
  -v "$OUT_DIR":/out
  -v "$VOLUME":/build
  -e HTTP_PROXY="$PROXY" -e HTTPS_PROXY="$PROXY" -e http_proxy="$PROXY" -e https_proxy="$PROXY"
  -e NO_PROXY="$NO_PROXY_LIST" -e no_proxy="$NO_PROXY_LIST"
  -e CARGO_NET_RETRY=5
  -e APPIMAGE_EXTRACT_AND_RUN=1
  -e NO_STRIP=true
  "$IMAGE"
)

if [ "${DETACH:-0}" = "1" ]; then
  docker run -d "${RUN_ARGS[@]}" >/dev/null
  echo "已后台启动容器 $CONTAINER；跟踪日志： docker logs -f $CONTAINER"
else
  docker run "${RUN_ARGS[@]}"
fi
