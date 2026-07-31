#!/usr/bin/env bash
# 编译 Linux GTK4 前端（默认 feature = gtk）
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "==> cargo build --features gtk"
cargo build --features gtk

echo "==> done"
