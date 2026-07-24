#!/usr/bin/env bash
# 编译 macOS 客户端 → build/macos/muxterm（及 Muxterm.app 供 XCUITest）
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MACOS_DIR="$ROOT/src/platform/macos"
OUT_DIR="$ROOT/build/macos"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/../muxterm-target}"

mkdir -p "$OUT_DIR" "$TARGET_DIR"

echo "==> cargo build ffi release"
cd "$ROOT"
cargo build --no-default-features --features ffi --release

# Vendor 软链（若缺失）
mkdir -p "$MACOS_DIR/Vendor"
ln -sfn ../../../../../muxterm-target/release/libmuxterm.a "$MACOS_DIR/Vendor/libmuxterm.a"

echo "==> swift build -c release"
export DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}"
cd "$MACOS_DIR"
swift build -c release

BIN="$(swift build -c release --show-bin-path)/MuxtermApp"
cp -f "$BIN" "$OUT_DIR/muxterm"
chmod +x "$OUT_DIR/muxterm"

# 组装 .app（XCUITest / 手动双击）
APP="$OUT_DIR/Muxterm.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp -f "$BIN" "$APP/Contents/MacOS/Muxterm"
cp -f "$MACOS_DIR/Info.plist" "$APP/Contents/Info.plist"
# 确保可执行名与 plist 一致
/usr/libexec/PlistBuddy -c "Set :CFBundleExecutable Muxterm" "$APP/Contents/Info.plist" 2>/dev/null \
  || true

echo "==> done"
echo "    binary: $OUT_DIR/muxterm"
echo "    app:    $APP"
file "$OUT_DIR/muxterm"
