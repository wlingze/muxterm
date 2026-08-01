#!/usr/bin/env bash
# 编译 TUI 前端（crossterm，无 GTK 依赖）
# 用法: ./scripts/build-tui.sh [--release]
#   或: PROFILE=release ./scripts/build-tui.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# 解析 --release 或 PROFILE=release
RELEASE=""
if [[ "${1:-}" == "--release" || "${PROFILE:-}" == "release" ]]; then
  RELEASE="--release"
fi

if [[ -n "$RELEASE" ]]; then
  echo "==> cargo build --release --no-default-features --features tui"
  cargo build --release --no-default-features --features tui
else
  echo "==> cargo build --no-default-features --features tui"
  cargo build --no-default-features --features tui
fi

echo "==> done"
