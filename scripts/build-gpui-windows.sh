#!/usr/bin/env bash
# 编译 GPUI 跨平台前端 → ./build/windows/muxterm.exe（+ 桌面快捷方式资源）
#
# 运行环境：Windows runner（Git Bash / MSYS）或任何有 bash 的 Windows 机器。
# 依赖：Rust stable + MSVC 工具链（`rustup default stable-x86_64-pc-windows-msvc`）。
# gpui 在 Windows 上走 DirectX + winit 式的原生窗口，不需要额外系统库。
#
# 用法（Git Bash）: ./scripts/build-gpui-windows.sh [--release]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source "$ROOT/scripts/build-common.sh"

RELEASE="$(parse_release "${1:-}")"
PROFILE="debug"
[[ -n "$RELEASE" ]] && PROFILE="release"

export CARGO_REGISTRIES_CRATES_IO_PROTOCOL="${CARGO_REGISTRIES_CRATES_IO_PROTOCOL:-sparse}"

echo "==> cargo build gpui $RELEASE (windows)"
cargo build --no-default-features --features gpui $RELEASE

BIN="$(cargo_bin_path "$PROFILE")"
if [[ ! -f "$BIN" ]]; then
  echo "ERROR: $BIN not found after cargo build" >&2
  exit 1
fi
smoke_cli_help "$BIN"

OUT_DIR="$(build_os_dir)"
mkdir -p "$OUT_DIR"
echo "==> install $OUT_DIR/muxterm.exe"
install_executable "$BIN" "$OUT_DIR/muxterm.exe"

# 图标 / 快捷方式资源：exe 本身不带图标（Windows 原生图标需要 winres 资源，
# 见 docs/FRONTEND-GPUI.md §8.2），包里附 PNG 供用户自建快捷方式。
ICON_SRC="$ROOT/src/frontend/macos/Resources/MuxtermLogo.png"
if [[ -f "$ICON_SRC" ]]; then
  cp -f "$ICON_SRC" "$OUT_DIR/dev.muxterm.app.png"
else
  echo "WARN: $ICON_SRC 缺失，包内将没有图标" >&2
fi

echo "==> done: $OUT_DIR/muxterm.exe"
"$OUT_DIR/muxterm.exe" --version || true