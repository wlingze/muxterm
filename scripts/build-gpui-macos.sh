#!/usr/bin/env bash
# 编译 GPUI 跨平台前端 → ./build/macos/Muxterm.app
#
# 结构：
#   Muxterm.app/Contents/Info.plist
#   Muxterm.app/Contents/MacOS/Muxterm   exec 脚本，转发到同 bundle 的二进制
#   Muxterm.app/Contents/Resources/muxterm   gpui feature 编出的单一 binary
#
# 用 exec 脚本而不是独立 Mach-O：项目规则只允许一个 frontend binary，
# 而 LaunchServices 不能给 CFBundleExecutable 传参数。
#
# 注意：Swift 前端也产出 `build/macos/Muxterm.app`（`scripts/build-macos.sh`）。
# 两者不要同时构建、不要指望在同一目录里共存：谁最后构建，谁就是当前的
# `Muxterm.app`。gpui 已是发布前端，发布构建用本脚本。
#
# 用法: ./scripts/build-gpui-macos.sh [--release]
set -euo pipefail
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.0}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source "$ROOT/scripts/build-common.sh"

RELEASE="$(parse_release "${1:-}")"
PROFILE="debug"
[[ -n "$RELEASE" ]] && PROFILE="release"

if [[ -n "${MUXTERM_BUNDLE_IDENTIFIER:-}" ]]; then
  BUNDLE_IDENTIFIER="$MUXTERM_BUNDLE_IDENTIFIER"
elif [[ -n "$RELEASE" ]]; then
  BUNDLE_IDENTIFIER="dev.muxterm.app"
else
  BUNDLE_IDENTIFIER="dev.muxterm.app.debug.$(git rev-parse --short HEAD 2>/dev/null || printf 'local')"
fi
BUNDLE_VERSION="$(git rev-parse --short HEAD 2>/dev/null || printf 'local')"

# crates.io 预取：CI 与本机都可能落到 git index，显式用 sparse 更稳。
export CARGO_REGISTRIES_CRATES_IO_PROTOCOL="${CARGO_REGISTRIES_CRATES_IO_PROTOCOL:-sparse}"

echo "==> cargo build gpui $RELEASE"
cargo build --no-default-features --features gpui $RELEASE

BIN="$(cargo_bin_path "$PROFILE")"
if [[ ! -x "$BIN" ]]; then
  echo "ERROR: $BIN not found after cargo build" >&2
  exit 1
fi
smoke_cli_help "$BIN"

OUT_DIR="$(build_os_dir)"
APP="$OUT_DIR/Muxterm.app"

echo "==> assemble $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

install_executable "$BIN" "$APP/Contents/Resources/muxterm"

# App icon：与 macOS Swift 前端共用仓库里唯一的一份 .icns。以前这里没拷图标、
# Info.plist 也没写 CFBundleIconFile，Finder / Dock 只能给通用图标。
ICON="$ROOT/src/frontend/macos/Resources/MuxtermLogo.icns"
if [[ -f "$ICON" ]]; then
  cp -f "$ICON" "$APP/Contents/Resources/MuxtermLogo.icns"
else
  echo "WARN: $ICON 缺失，app 将显示通用图标" >&2
fi

# Info.plist 里 NSHighResolutionCapable 必须为 true，否则 Retina 下按 1x 绘制。
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>Muxterm</string>
  <key>CFBundleDisplayName</key>
  <string>Muxterm</string>
  <key>CFBundleExecutable</key>
  <string>Muxterm</string>
  <key>CFBundleIconFile</key>
  <string>MuxtermLogo.icns</string>
  <key>CFBundleIdentifier</key>
  <string>$BUNDLE_IDENTIFIER</string>
  <key>CFBundleVersion</key>
  <string>$BUNDLE_VERSION</string>
  <key>CFBundleShortVersionString</key>
  <string>$BUNDLE_VERSION</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSPrincipalClass</key>
  <string>NSApplication</string>
</dict>
</plist>
PLIST

cat > "$APP/Contents/MacOS/Muxterm" <<'LAUNCHER'
#!/bin/sh
# 转发到 bundle 内的单一 binary。$0 是 Contents/MacOS/Muxterm。
# LaunchServices 会塞一个 -psn_* 参数，clap 不认，这里过滤掉再转发其余参数。
args=""
for arg in "$@"; do
  case "$arg" in
    -psn_*) ;;
    *) args="$args $arg" ;;
  esac
done
# 日志落到用户临时目录的日期文件：LaunchServices 丢弃 stderr，没有文件就
# 等于没有日志（用户报障时只能贴一个空目录）。显式 --log-file 时尊重用户。
log_file="${TMPDIR:-/tmp}/muxterm-logs/muxterm-$(date +%Y-%m-%d).log"
case " $args " in
  *" --log-file "*) ;;
  *) args="$args --log-file $log_file" ;;
esac
# 开发包开 debug：输入 / resize / attach 的现场只在 --debug 里；
# 发布包保持默认级别，避免把终端内容写进用户日志。
# shellcheck disable=SC2086  # 有意做词分割，把过滤后的参数原样交给 muxterm
exec "$(dirname "$0")/../Resources/muxterm" gpui __DEBUG_ARG__ $args
LAUNCHER
# 上面的 heredoc 必须整体引用（运行期的 $@ / $args 不能在构建期展开），
# debug 开关在写完后替换占位符。
if [[ -n "$RELEASE" ]]; then
  sed -i '' 's/__DEBUG_ARG__//' "$APP/Contents/MacOS/Muxterm"
else
  sed -i '' 's/__DEBUG_ARG__/--debug/' "$APP/Contents/MacOS/Muxterm"
fi
chmod +x "$APP/Contents/MacOS/Muxterm"

xattr -cr "$APP" 2>/dev/null || true
chmod 755 "$APP" "$APP/Contents" "$APP/Contents/MacOS" "$APP/Contents/Resources"

CODESIGN_IDENTITY="${MUXTERM_CODESIGN_IDENTITY:--}"
echo "==> codesign $CODESIGN_IDENTITY"
codesign --force --deep --sign "$CODESIGN_IDENTITY" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

echo "==> ok: $APP"
