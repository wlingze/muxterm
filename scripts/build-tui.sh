#!/usr/bin/env bash
# 编译 TUI 前端（crossterm，无 GTK 依赖）
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "==> cargo build --no-default-features --features tui"
cargo build --no-default-features --features tui

echo "==> done"
