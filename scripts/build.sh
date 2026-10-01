#!/usr/bin/env bash
# umi Downloader 一键构建流水线：前端测试 → 后端自检 → 打包桌面客户端
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
export PATH="$HOME/.cargo/bin:$PATH"

echo "=============================================="
echo " umi Downloader 构建流水线"
echo " 工作目录: $ROOT"
echo "=============================================="

echo
echo "▶ [1/5] 前端单元测试 (vitest)"
npm run test

echo
echo "▶ [2/5] 前端类型检查 (vue-tsc)"
npx vue-tsc --noEmit

echo
echo "▶ [3/5] 后端依赖检测与安装 (yt-dlp / FFmpeg / Whisper)"
(cd src-tauri && cargo run --release -- --install-tools)

echo
echo "▶ [4/5] 后端功能自检（真实下载 / 转码 / AI 识别，需 --features selftest）"
(cd src-tauri && cargo run --release --features selftest -- --selftest)

echo
echo "▶ [5/5] 打包桌面客户端 (Tauri bundle，正式版不含自检模块)"
npm run tauri:build

echo
echo "=============================================="
echo " 构建完成，产物位于："
echo "   src-tauri/target/release/umi-downloader.exe"
ls -la src-tauri/target/release/bundle/nsis/ 2>/dev/null || true
echo "=============================================="
