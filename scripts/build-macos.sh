#!/usr/bin/env bash
# 编译 macOS 客户端 → build/macos/
#   build/macos/muxterm      Rust 主程序（CLI + `muxterm gui` + `muxterm tui`）
#   build/macos/Muxterm.app  Swift 原生 GUI bundle（`muxterm gui` 用 open 启动）
#
# 用法: ./scripts/build-macos.sh
# 环境变量: CARGO_TARGET_DIR (默认 ../muxterm-target)
#           DEVELOPER_DIR (默认 /Applications/Xcode.app/Contents/Developer)
set -euo pipefail

export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.0}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MACOS_DIR="$ROOT/src/platform/macos"
OUT_DIR="$ROOT/build/macos"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/../muxterm-target}"

mkdir -p "$OUT_DIR" "$TARGET_DIR"

echo "==> cargo build (tui + ffi) release"
cd "$ROOT"
# macOS Rust 主程序：编译 tui（含 ffi），支持 CLI 命令 / `muxterm tui` / `muxterm gui`。
cargo build --no-default-features --features tui --release

# Rust 主程序 → build/macos/muxterm
RUST_BIN="$TARGET_DIR/release/muxterm"
if [[ ! -f "$RUST_BIN" ]]; then
  echo "ERROR: $RUST_BIN not found after cargo build" >&2
  exit 1
fi
cp -f "$RUST_BIN" "$OUT_DIR/muxterm"
chmod +x "$OUT_DIR/muxterm"

echo "==> cargo build ffi release (静态库供 Swift 链接)"
# Swift 端静态链接 libmuxterm.a；用同一 shared target 的 release 产物。
cargo build --no-default-features --features ffi --release

# Vendor 软链（指向实际 target dir）
mkdir -p "$MACOS_DIR/Vendor"
LIBMUXTERM="$TARGET_DIR/release/libmuxterm.a"
if [[ ! -f "$LIBMUXTERM" ]]; then
  echo "ERROR: $LIBMUXTERM not found after cargo build" >&2
  exit 1
fi
ln -sfn "$(cd "$(dirname "$LIBMUXTERM")" && pwd)/libmuxterm.a" "$MACOS_DIR/Vendor/libmuxterm.a"

echo "==> swift build -c release"
export DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}"
cd "$MACOS_DIR"
swift build -c release

SWIFT_BIN="$(swift build -c release --show-bin-path)/MuxtermApp"
DEPS="$(otool -L "$SWIFT_BIN" | tail -n +2)"
printf '%s\n' "$DEPS"
if [[ "$DEPS" == *libmuxterm.dylib* ]]; then
  echo "ERROR: macOS app links libmuxterm.dylib; expected the bundled static archive" >&2
  exit 1
fi
if [[ "$DEPS" == *"/Users/runner"* || "$DEPS" == *"/home/runner"* || "$DEPS" == *muxterm-target* ]]; then
  echo "ERROR: macOS app contains a build-machine dependency path" >&2
  exit 1
fi

# 组装 .app（GUI bundle，仅被 `muxterm gui` 用 open 启动 / 双击）
APP="$OUT_DIR/Muxterm.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp -f "$SWIFT_BIN" "$APP/Contents/MacOS/Muxterm"
chmod +x "$APP/Contents/MacOS/Muxterm"
# 清掉可能阻止启动的 quarantine / 扩展属性
xattr -cr "$APP" 2>/dev/null || true
cp -f "$MACOS_DIR/Info.plist" "$APP/Contents/Info.plist"
# 确保可执行名与 plist 一致
/usr/libexec/PlistBuddy -c "Set :CFBundleExecutable Muxterm" "$APP/Contents/Info.plist" 2>/dev/null \
  || true
# 再次确保权限（PlistBuddy / cp 之后）
chmod +x "$APP/Contents/MacOS/Muxterm"
chmod 755 "$APP" "$APP/Contents" "$APP/Contents/MacOS"

# SwiftPM only linker-signs the executable. Sign the complete bundle so
# Info.plist and sealed resources are bound into Contents/_CodeSignature.
# CI uses ad-hoc signing; a future Developer ID identity can override it.
CODESIGN_IDENTITY="${MUXTERM_CODESIGN_IDENTITY:--}"
codesign --force --deep --sign "$CODESIGN_IDENTITY" "$APP"
codesign --verify --deep --strict --verbose=4 "$APP"
test -d "$APP/Contents/_CodeSignature"

echo "==> done"
echo "    rust cli/tui/gui: $OUT_DIR/muxterm"
echo "    swift gui app:    $APP"
ls -la "$APP/Contents/MacOS/Muxterm"
file "$OUT_DIR/muxterm"
