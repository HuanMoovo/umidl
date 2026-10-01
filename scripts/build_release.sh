#!/usr/bin/env bash
# 构建发布产物。
#
# 关键点：Rust 会把**构建机的绝对路径**（cargo 依赖源码目录、项目目录）编进二进制，
# 直接发布等于把用户名/目录结构带出去。这里在构建时做路径重映射：
#   <cargo 家目录> → /cargo      <项目目录> → /project
# 两个前缀都在运行时推导，脚本本身不含任何个人路径。
#
# 用法：bash scripts/build_release.sh [额外的 tauri build 参数]
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

CARGO_DIR="${CARGO_HOME:-$HOME/.cargo}"
win_cargo="$(cygpath -w "$CARGO_DIR" 2>/dev/null || echo "$CARGO_DIR")"
win_proj="$(cygpath -w "$(pwd)" 2>/dev/null || pwd)"

# RUSTFLAGS 由 cargo 按空白拆分（不支持引号），所以带空格的前缀没法传 —— 
# 实测项目路径本来就不会进二进制（只有 cargo 依赖目录会），这里仅在无空格时加保护。
FLAGS="--remap-path-prefix=${win_cargo}=/cargo"
if [[ "$win_proj" != *" "* ]]; then
  FLAGS="$FLAGS --remap-path-prefix=${win_proj}=/project"
else
  echo "提示：项目路径含空格，跳过项目目录重映射（实测它不会进二进制）"
fi
export RUSTFLAGS="$FLAGS ${RUSTFLAGS:-}"
echo "RUSTFLAGS = $RUSTFLAGS"

# 构建前先结束正在运行的实例，否则 Windows 上会因 exe 被占用而失败
taskkill //F //IM umidl.exe 2>/dev/null || true
sleep 2

npx tauri build "$@"
