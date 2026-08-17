#!/usr/bin/env bash
# 编译 macOS 客户端 → ./build/macos/
#   build/macos/muxterm      Rust 主程序（CLI + `muxterm gui` + `muxterm tui`）
#   build/macos/Muxterm.app  Swift 原生 GUI bundle（`muxterm gui` 用 open 启动）
#
# 用法: ./scripts/build-macos.sh [--release]
#   或: PROFILE=release ./scripts/build-macos.sh
# 环境变量: DEVELOPER_DIR (默认 /Applications/Xcode.app/Contents/Developer)
set -euo pipefail
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.0}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source "$ROOT/scripts/build-common.sh"

RELEASE="$(parse_release "${1:-}")"
PROFILE="debug"
[[ -n "$RELEASE" ]] && PROFILE="release"
MACOS_DIR="$ROOT/src/platform/macos"
OUT_DIR="$(build_os_dir)"   # -> build/macos
TARGET_DIR="$(cargo_target_dir)"
mkdir -p "$OUT_DIR" "$TARGET_DIR"

echo "==> cargo build (tui + ffi) $RELEASE"
# macOS Rust 主程序：编译 tui（含 ffi），支持 CLI 命令 / `muxterm tui` / `muxterm gui`。
cargo build --no-default-features --features tui $RELEASE

# Rust 主程序 → build/macos/muxterm
RUST_BIN="$(cargo_bin_path "$PROFILE")"
if [[ ! -f "$RUST_BIN" ]]; then
  echo "ERROR: $RUST_BIN not found after cargo build" >&2
  exit 1
fi
cp -f "$RUST_BIN" "$OUT_DIR/$(binary_name)"
chmod +x "$OUT_DIR/$(binary_name)"

echo "==> cargo build ffi $RELEASE (静态库供 Swift 链接)"
# Swift 端静态链接 libmuxterm.a；用同一 shared target 的产物。
cargo build --no-default-features --features ffi $RELEASE

# Vendor 软链（指向实际 target dir）
mkdir -p "$MACOS_DIR/Vendor"
LIBMUXTERM="$TARGET_DIR/$PROFILE/libmuxterm.a"
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
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/i18n"
cp -f "$SWIFT_BIN" "$APP/Contents/MacOS/Muxterm"
chmod +x "$APP/Contents/MacOS/Muxterm"

# SPM 资源包必须放在 Contents/Resources（.app 根目录再放文件会
# codesign: unsealed contents present in the bundle root）。
# 不要用生成的 Bundle.module：它查的是 app 根 + 编译机绝对 .build 路径。
SWIFT_BIN_DIR="$(dirname "$SWIFT_BIN")"
copy_spm_bundle() {
  local name="$1"
  local src="$SWIFT_BIN_DIR/${name}.bundle"
  if [[ ! -d "$src" ]]; then
    echo "ERROR: missing SPM resource bundle: $src" >&2
    exit 1
  fi
  rm -rf "$APP/Contents/Resources/${name}.bundle"
  cp -R "$src" "$APP/Contents/Resources/${name}.bundle"
}
copy_spm_bundle "MuxtermApp_MuxtermAppLib"
copy_spm_bundle "SwiftTerm_SwiftTerm"

# 源码 catalog：给 Bundle.main / Contents/Resources/i18n 查找（不依赖 SPM bundle 名）
cp -f "$MACOS_DIR/Resources/i18n/"*.json "$APP/Contents/Resources/i18n/"

xattr -cr "$APP" 2>/dev/null || true
cp -f "$MACOS_DIR/Info.plist" "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleExecutable Muxterm" "$APP/Contents/Info.plist" 2>/dev/null \
  || true
chmod +x "$APP/Contents/MacOS/Muxterm"
chmod 755 "$APP" "$APP/Contents" "$APP/Contents/MacOS" "$APP/Contents/Resources"

CODESIGN_IDENTITY="${MUXTERM_CODESIGN_IDENTITY:--}"
codesign --force --deep --sign "$CODESIGN_IDENTITY" "$APP"
codesign --verify --deep --strict --verbose=4 "$APP"
test -d "$APP/Contents/_CodeSignature"

echo "==> done"
echo "    rust cli/tui/gui: $OUT_DIR/$(binary_name)"
echo "    swift gui app:    $APP"
ls -la "$APP/Contents/MacOS/Muxterm"
file "$OUT_DIR/$(binary_name)"
