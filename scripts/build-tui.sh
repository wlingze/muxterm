#!/usr/bin/env bash
# 编译 TUI 前端（crossterm + ratatui，无 GTK 依赖，跨平台）
# 用法: ./scripts/build-tui.sh [--release]
#   或: PROFILE=release ./scripts/build-tui.sh
# 产物: ./build/<os>/muxterm(.exe)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source "$ROOT/scripts/build-common.sh"

RELEASE="$(parse_release "${1:-}")"
PROFILE="debug"
[[ -n "$RELEASE" ]] && PROFILE="release"
OUT_DIR="$(build_os_dir)"
mkdir -p "$OUT_DIR"

echo "==> cargo build $RELEASE --no-default-features --features tui"
cargo build $RELEASE --no-default-features --features tui

BIN="$(cargo_bin_path "$PROFILE")"
if [[ ! -f "$BIN" ]]; then
  echo "ERROR: 未找到编译产物 $BIN" >&2
  exit 1
fi
cp -f "$BIN" "$OUT_DIR/$(binary_name)"
chmod +x "$OUT_DIR/$(binary_name)"
echo "==> done"
echo "    产物: $OUT_DIR/$(binary_name)"
