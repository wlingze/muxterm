#!/usr/bin/env bash
# 编译 macOS 客户端 → build/macos/muxterm（及 Muxterm.app 供 XCUITest）
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

echo "==> cargo build ffi release"
cd "$ROOT"
cargo build --no-default-features --features ffi --release

# Vendor 软链（指向实际 target dir）
mkdir -p "$MACOS_DIR/Vendor"
# 计算从 Vendor/ 到 target_dir/release/libmuxterm.a 的相对路径
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

BIN="$(swift build -c release --show-bin-path)/MuxtermApp"
DEPS="$(otool -L "$BIN" | tail -n +2)"
printf '%s\n' "$DEPS"
if [[ "$DEPS" == *libmuxterm.dylib* ]]; then
  echo "ERROR: macOS app links libmuxterm.dylib; expected the bundled static archive" >&2
  exit 1
fi
if [[ "$DEPS" == *"/Users/runner"* || "$DEPS" == *"/home/runner"* || "$DEPS" == *muxterm-target* ]]; then
  echo "ERROR: macOS app contains a build-machine dependency path" >&2
  exit 1
fi
cp -f "$BIN" "$OUT_DIR/muxterm"
chmod +x "$OUT_DIR/muxterm"

# 组装 .app（XCUITest / 手动双击）
APP="$OUT_DIR/Muxterm.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp -f "$BIN" "$APP/Contents/MacOS/Muxterm"
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

echo "==> done"
echo "    binary: $OUT_DIR/muxterm"
echo "    app:    $APP"
ls -la "$APP/Contents/MacOS/Muxterm"
file "$OUT_DIR/muxterm"
