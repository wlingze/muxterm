#!/usr/bin/env bash
# 编译 Linux GTK4 前端（默认 feature = gtk）
# 用法: ./scripts/build-linux.sh [--release]
#   或: PROFILE=release ./scripts/build-linux.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# 解析 --release 或 PROFILE=release
RELEASE=""
if [[ "${1:-}" == "--release" || "${PROFILE:-}" == "release" ]]; then
  RELEASE="--release"
fi

if [[ -n "$RELEASE" ]]; then
  echo "==> cargo build --release --features gtk"
  cargo build --release --features gtk
else
  echo "==> cargo build --features gtk"
  cargo build --features gtk
fi

echo "==> done"
