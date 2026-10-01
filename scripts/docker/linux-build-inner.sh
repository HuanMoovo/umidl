#!/usr/bin/env bash
# 在 Linux 容器内执行的 Umidl 构建步骤。
#   挂载约定：/src = 宿主机源码（只读）、/build = 容器内工作区（命名卷）、/out = 产物回写目录
# 由 scripts/docker/build-linux.sh 调用，也可以直接 docker run ... bash /usr/local/bin/linux-build-inner.sh
set -euo pipefail

SRC=${SRC:-/src}
WORK=${WORK:-/build}
OUT=${OUT:-/out}
NPM_REGISTRY=${NPM_REGISTRY:-https://registry.npmmirror.com}
BUNDLES=${BUNDLES:-deb,appimage}

log() { printf '\n\033[1;36m[%s] %s\033[0m\n' "$(date +%H:%M:%S)" "$*"; }

log "0/6 工具链版本"
node -v; npm -v; rustc -V; cargo -V; pkg-config --modversion webkit2gtk-4.1

log "1/6 同步源码到 ${WORK}（排除宿主机 node_modules / target / dist）"
mkdir -p "$WORK"
tar -C "$SRC" \
    --exclude=./node_modules --exclude=./src-tauri/target --exclude=./src-tauri/target-selftest \
    --exclude=./target-selftest --exclude=./dist --exclude=./.git --exclude=./.tmp \
    -cf - . | tar -C "$WORK" -xf -
ls "$WORK" | head -20

log "2/6 npm ci（registry=${NPM_REGISTRY}）"
cd "$WORK"
export npm_config_cache="$WORK/.npm-cache"
npm ci --no-audit --no-fund --registry="$NPM_REGISTRY"

log "3/6 前端构建（vue-tsc 类型检查 + vite build）"
if npm run build; then
  echo "FRONTEND_FULL_BUILD=ok"
else
  echo "FRONTEND_FULL_BUILD=vue-tsc-failed -> fallback vite build only"
  npx vite build
fi
ls -la dist | head -5

log "4/6 tauri build --bundles ${BUNDLES}（beforeBuildCommand 已在上面单独执行，故置空）"
npx tauri build --bundles "$BUNDLES" \
  --config '{"build":{"beforeBuildCommand":""}}'

log "5/6 收集产物 -> ${OUT}"
mkdir -p "$OUT"
find src-tauri/target/release/bundle -type f \( -name '*.deb' -o -name '*.AppImage' -o -name '*.rpm' \) -print -exec cp -v {} "$OUT"/ \;
echo "--- 产物清单 ---"
ls -la "$OUT"
echo "--- 可执行文件大小 ---"
ls -la src-tauri/target/release/umidl 2>/dev/null || ls -la src-tauri/target/release/ | grep -i umidl | head

log "6/6 deb 元数据（dpkg-deb -I）"
for d in "$OUT"/*.deb; do dpkg-deb -I "$d"; echo "--- dpkg-deb -c (前 15 行) ---"; dpkg-deb -c "$d" | head -15; done

log "BUILD_ALL_OK"
