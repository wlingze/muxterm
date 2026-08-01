#!/usr/bin/env bash
# 编译 CLI 前端（ffi 核心，无 GUI，无 TUI）
# 用法: ./scripts/build-cli.sh [--release]
#   或: PROFILE=release ./scripts/build-cli.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# 解析 --release 或 PROFILE=release
RELEASE=""
if [[ "${1:-}" == "--release" || "${PROFILE:-}" == "release" ]]; then
  RELEASE="--release"
fi

if [[ -n "$RELEASE" ]]; then
  echo "==> cargo build --release --no-default-features --features ffi"
  cargo build --release --no-default-features --features ffi
else
  echo "==> cargo build --no-default-features --features ffi"
  cargo build --no-default-features --features ffi
fi

echo "==> done"
