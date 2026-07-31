#!/usr/bin/env bash
# 编译 CLI 前端（无 GUI，无 TUI，仅 ffi 核心供 daemon/集成测试使用）
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "==> cargo build --no-default-features --features ffi"
cargo build --no-default-features --features ffi

echo "==> done"
